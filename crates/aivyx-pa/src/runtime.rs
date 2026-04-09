//! Shared runtime infrastructure for agent lifecycle management.
//!
//! Extracts the common setup code used by the API server and CLI commands.
//! This prevents duplication and ensures new features (keys, loop fields)
//! are wired consistently.

use std::sync::Arc;

use crate::agent::BuiltAgent;
use crate::config::PaConfig;
use aivyx_actions::email::EmailConfig;
use aivyx_audit::AuditLog;
use aivyx_config::{AivyxDirs, ScheduleEntry};
use aivyx_core::ToolRegistry;
use aivyx_crypto::MasterKey;
use aivyx_llm::LlmProvider;
use aivyx_loop::LoopContext;

// ── Derived Keys ───────────────────────────────────────────────

/// All keys derived from the master key at startup.
///
/// Centralizes key derivation so every consumer (TUI, API, CLI) gets
/// the same set without risk of omitting one. Keys are derived
/// deterministically from the master key via HKDF domain separation,
/// so deriving them multiple times from the same master key produces
/// identical bytes.
pub struct DerivedKeys {
    // ── Agent keys (consumed by build_agent / audit log) ────────
    /// HMAC key for the agent's audit log.
    pub audit_key: Vec<u8>,

    // ── Loop keys (consumed by LoopContext) ─────────────────────
    /// Brain encryption key for the loop's goal reads.
    pub loop_brain_key: MasterKey,
    /// HMAC key for the loop's audit log instance.
    pub loop_audit_key: Vec<u8>,
    /// Domain key for reminder decryption.
    pub loop_reminder_key: MasterKey,
    /// Domain key for vault encryption (None if vault unconfigured).
    pub loop_vault_key: Option<MasterKey>,
    /// Domain key for finance encryption (None if finance disabled).
    pub loop_finance_key: Option<MasterKey>,
    /// Domain key for contacts encryption (None if contacts unconfigured).
    pub loop_contacts_key: Option<MasterKey>,
    /// Domain key for triage encryption (None if triage unconfigured).
    pub loop_triage_key: Option<MasterKey>,

    // ── UI keys (consumed by TUI views / API handlers) ──────────
    /// Brain key for the UI's read-only goal queries.
    pub ui_brain_key: MasterKey,
    /// Audit HMAC key for the UI's read-only audit view.
    pub ui_audit_key: Vec<u8>,
    /// Conversation encryption key for chat session persistence.
    pub conversation_key: MasterKey,

    // ── Optional keys ──────────────────────────────────────────
    /// Webhook HMAC key (None if webhooks disabled).
    pub webhook_key: Option<MasterKey>,
}

/// Derive all keys from the master key in a single call.
///
/// Must be called *before* the master key is moved into `build_agent()`,
/// since `MasterKey` is not `Clone`.
pub fn derive_all_keys(
    master_key: &MasterKey,
    pa_config: &PaConfig,
    has_vault: bool,
    has_contacts: bool,
) -> DerivedKeys {
    let finance_enabled = pa_config.finance.as_ref().is_some_and(|f| f.enabled);
    let triage_enabled = pa_config.triage.is_some();
    let webhooks_enabled = pa_config.webhook.as_ref().is_some_and(|w| w.enabled);

    DerivedKeys {
        audit_key: aivyx_crypto::derive_audit_key(master_key),

        loop_brain_key: aivyx_crypto::derive_brain_key(master_key),
        loop_audit_key: aivyx_crypto::derive_audit_key(master_key),
        loop_reminder_key: aivyx_crypto::derive_domain_key(master_key, b"reminders"),
        loop_vault_key: if has_vault {
            Some(aivyx_crypto::derive_domain_key(master_key, b"vault"))
        } else {
            None
        },
        loop_finance_key: if finance_enabled {
            Some(aivyx_crypto::derive_domain_key(master_key, b"finance"))
        } else {
            None
        },
        loop_contacts_key: if has_contacts {
            Some(aivyx_crypto::derive_domain_key(master_key, b"contacts"))
        } else {
            None
        },
        loop_triage_key: if triage_enabled {
            Some(aivyx_crypto::derive_domain_key(master_key, b"triage"))
        } else {
            None
        },

        ui_brain_key: aivyx_crypto::derive_brain_key(master_key),
        ui_audit_key: aivyx_crypto::derive_audit_key(master_key),
        conversation_key: aivyx_crypto::derive_domain_key(master_key, b"conversation"),

        webhook_key: if webhooks_enabled {
            Some(aivyx_crypto::derive_domain_key(master_key, b"webhook"))
        } else {
            None
        },
    }
}

// ── Resolved Loop Configs ──────────────────────────────────────

/// All resolved service configs and derived data needed to build the
/// agent loop. Collected before `build_agent()` consumes the master key.
pub struct LoopInputs {
    pub email_config: Option<EmailConfig>,
    pub calendar_config: Option<aivyx_actions::calendar::CalendarConfig>,
    pub contacts_config: Option<aivyx_actions::contacts::ContactsConfig>,
    pub telegram_config: Option<aivyx_actions::messaging::TelegramConfig>,
    pub matrix_config: Option<aivyx_actions::messaging::MatrixConfig>,
    pub signal_config: Option<aivyx_actions::messaging::SignalConfig>,
    pub vault_config: Option<aivyx_actions::documents::VaultConfig>,
    pub triage_config: Option<aivyx_loop::triage::TriageConfig>,
    pub schedules: Vec<ScheduleEntry>,
    pub system_prompt: String,
    pub finance_enabled: bool,
}

/// Resolve loop inputs from `PaConfig` and `ServiceConfigs`.
///
/// Clones the service configs that the loop needs before they're consumed
/// by `build_agent()`. In TUI mode, configs come from the already-resolved
/// `ServiceConfigs`; in API mode they can be re-resolved from `PaConfig`.
impl LoopInputs {
    /// Build from already-resolved service configs (used by TUI path).
    pub fn from_services(services: &crate::agent::ServiceConfigs, pa_config: &PaConfig) -> Self {
        Self {
            email_config: services.email.clone(),
            calendar_config: services.calendar.clone(),
            contacts_config: services.contacts.clone(),
            telegram_config: services.telegram.clone(),
            matrix_config: services.matrix.clone(),
            signal_config: services.signal.clone(),
            vault_config: services.vault.clone(),
            triage_config: pa_config.triage.as_ref().map(|t| t.to_triage_config()),
            schedules: pa_config.schedules.clone(),
            system_prompt: pa_config.effective_system_prompt(),
            finance_enabled: pa_config.finance.as_ref().is_some_and(|f| f.enabled),
        }
    }
}

// ── Schedule Tools ─────────────────────────────────────────────

/// Build the read-only tool registry used for scheduled prompt execution.
///
/// This is a restricted subset of tools — only safe, read-only actions
/// that the autonomous loop can invoke during scheduled prompts.
pub fn build_schedule_tools(
    inputs: &LoopInputs,
    imap_pool: Option<Arc<aivyx_actions::email::ImapPool>>,
) -> ToolRegistry {
    let mut tools = ToolRegistry::new();
    aivyx_actions::bridge::register_schedule_actions(
        &mut tools,
        inputs.email_config.clone(),
        imap_pool,
        inputs.telegram_config.clone(),
        inputs.matrix_config.clone(),
        inputs.signal_config.clone(),
    );
    if let Some(ref cc) = inputs.calendar_config {
        tools.register(Box::new(aivyx_actions::bridge::ActionTool::new(Box::new(
            aivyx_actions::calendar::TodayAgenda { config: cc.clone() },
        ))));
        tools.register(Box::new(aivyx_actions::bridge::ActionTool::new(Box::new(
            aivyx_actions::calendar::FetchCalendarEvents { config: cc.clone() },
        ))));
        tools.register(Box::new(aivyx_actions::bridge::ActionTool::new(Box::new(
            aivyx_actions::calendar::CheckConflicts { config: cc.clone() },
        ))));
    }
    tools
}

// ── Loop Context ───────────────────────────────────────────────

/// Build the `LoopContext` from a `BuiltAgent`, derived keys, and resolved inputs.
///
/// Returns `Some(LoopContext)` if the brain is available (brain_store present),
/// `None` otherwise. The agent loop requires a brain to function.
pub fn build_loop_context(
    built: &mut BuiltAgent,
    keys: &mut DerivedKeys,
    inputs: LoopInputs,
    loop_provider: Box<dyn LlmProvider>,
    schedule_tools: ToolRegistry,
    store: Arc<aivyx_crypto::EncryptedStore>,
    dirs: &AivyxDirs,
    pa_config: &PaConfig,
) -> Option<LoopContext> {
    let brain_store = built.brain_store.take()?;

    let messaging = if inputs.telegram_config.is_some()
        || inputs.matrix_config.is_some()
        || inputs.signal_config.is_some()
    {
        Some(aivyx_loop::MessagingCtx {
            telegram: inputs.telegram_config,
            matrix: inputs.matrix_config,
            signal: inputs.signal_config,
            sms: None,
        })
    } else {
        None
    };

    Some(LoopContext {
        brain_store,
        // Safety: take() the key out of DerivedKeys. The loop owns it from here.
        brain_key: take_key(&mut keys.loop_brain_key),
        email_config: inputs.email_config,
        provider: loop_provider,
        system_prompt: inputs.system_prompt,
        schedules: inputs.schedules,
        schedule_last_run: std::collections::HashMap::new(),
        briefing_fired_today: None,
        reminder_store: store,
        reminder_key: take_key(&mut keys.loop_reminder_key),
        last_heartbeat_at: None,
        memory_manager: built.memory_manager.clone(),
        vault: inputs.vault_config.and_then(|config| {
            let key = keys.loop_vault_key.take()?;
            Some(aivyx_loop::VaultCtx { config, key })
        }),
        finance: if inputs.finance_enabled {
            keys.loop_finance_key
                .take()
                .map(|key| aivyx_loop::FinanceCtx { key })
        } else {
            None
        },
        schedule_tools: Some(schedule_tools),
        calendar_config: inputs.calendar_config,
        contacts: inputs.contacts_config.and_then(|config| {
            let key = keys.loop_contacts_key.take()?;
            Some(aivyx_loop::ContactsCtx { config, key })
        }),
        triage: inputs.triage_config.and_then(|config| {
            let key = keys.loop_triage_key.take()?;
            Some(aivyx_loop::TriageCtx { config, key })
        }),
        messaging,
        workflow_key: Some(take_key(&mut built.workflow_key)),
        trigger_state: Default::default(),
        imap_pool: built.imap_pool.clone(),
        audit_log: Some(AuditLog::new(dirs.audit_path(), &keys.loop_audit_key)),
        mcp_pool: built.mcp_pool.clone(),
        consolidation_config: pa_config
            .consolidation
            .as_ref()
            .map(|c| c.to_consolidation_config()),
        backup_destination: pa_config.backup.as_ref().and_then(|b| {
            if b.enabled {
                Some(
                    b.destination
                        .as_deref()
                        .map(std::path::PathBuf::from)
                        .unwrap_or_else(|| dirs.root().join("backups")),
                )
            } else {
                None
            }
        }),
        backup_retention_days: pa_config
            .backup
            .as_ref()
            .map(|b| b.retention_days)
            .unwrap_or(30),
        data_dir: Some(dirs.root().to_path_buf()),
        interaction_signals: std::sync::Arc::new(tokio::sync::Mutex::new(
            aivyx_loop::InteractionSignals::new(),
        )),
        resource_budget: aivyx_loop::ResourceBudget::new(
            pa_config.heartbeat_config().token_budget_daily.unwrap_or(0),
        ),
        strategy_review_pending: false,
        imap_consecutive_failures: 0,
        imap_expiry_notified: false,
        heartbeat_consecutive_failures: 0,
        tick_email_cache: None,
        tick_calendar_cache: None,
        dispatch_config: pa_config.notifications.as_ref().map(|n| {
            aivyx_loop::NotificationDispatchConfig {
                desktop: n.desktop,
                urgency_level: n.urgency_level.clone(),
                telegram: n.telegram,
                signal: n.signal,
                quiet_hours_start: n.quiet_hours_start,
                quiet_hours_end: n.quiet_hours_end,
                min_kind: n.min_kind.clone(),
            }
        }),
        // Injected by AgentLoop::start() — starts as None here.
        approval_rx: None,
        pending_approval_responses: Vec::new(),
    })
}

/// Build a `LoopConfig` from PA config.
pub fn build_loop_config(pa_config: &PaConfig) -> aivyx_loop::LoopConfig {
    let pa_loop = pa_config.loop_config();
    let pa_hb = pa_config.heartbeat_config();
    aivyx_loop::LoopConfig {
        check_interval_minutes: pa_loop.check_interval_minutes,
        morning_briefing: pa_loop.morning_briefing,
        briefing_hour: pa_loop.briefing_hour,
        briefing_on_launch: true,
        heartbeat: aivyx_loop::HeartbeatConfig {
            enabled: pa_hb.enabled,
            interval_minutes: pa_hb.interval_minutes,
            can_reflect: pa_hb.can_reflect,
            can_consolidate_memory: pa_hb.can_consolidate_memory,
            can_analyze_failures: pa_hb.can_analyze_failures,
            can_extract_knowledge: pa_hb.can_extract_knowledge,
            can_prune_audit: pa_hb.can_prune_audit,
            audit_retention_days: pa_hb.audit_retention_days,
            can_backup: pa_config
                .backup
                .as_ref()
                .map(|b| b.enabled)
                .unwrap_or(false),
            can_plan_review: pa_hb.can_plan_review,
            can_strategy_review: pa_hb.can_strategy_review,
            can_track_mood: pa_hb.can_track_mood,
            can_encourage: pa_hb.can_encourage,
            can_track_milestones: pa_hb.can_track_milestones,
            notification_pacing: pa_hb.notification_pacing,
            max_notifications_per_hour: pa_hb.max_notifications_per_hour,
            ..Default::default()
        },
    }
}

/// Replace a `MasterKey` with a zero-filled placeholder, returning the original.
///
/// `MasterKey` doesn't implement `Clone` (by design — it wraps `SecretBox`),
/// so we swap it out when transferring ownership to the `LoopContext`.
fn take_key(key: &mut MasterKey) -> MasterKey {
    std::mem::replace(key, MasterKey::from_bytes([0u8; 32]))
}
