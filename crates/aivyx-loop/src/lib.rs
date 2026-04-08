//! aivyx-loop — Proactive agent loop for the personal assistant.
//!
//! The loop is what makes this an *assistant* instead of a chatbot.
//! It periodically wakes up, checks registered sources (email, calendar,
//! reminders), reads active goals from the Brain, and either takes
//! autonomous action or queues items for user approval.

pub mod briefing;
pub mod heartbeat;
pub mod notify_dispatch;
pub mod pacing;
pub mod priority;
pub mod schedule;
pub mod trigger;
pub mod triage;

pub use notify_dispatch::{DispatchContext, NotificationDispatchConfig};
// ApprovalResponse is defined in this module (lib.rs) directly.

use aivyx_actions::email::{EmailConfig, EmailSummary, ReadInbox};
use aivyx_actions::reminders;
use aivyx_actions::web::FetchPage;
use aivyx_actions::Action;
use aivyx_brain::{BrainStore, Goal, GoalStatus};
use aivyx_config::ScheduleEntry;
use aivyx_core::Result;
use aivyx_crypto::{EncryptedStore, MasterKey};
use aivyx_llm::{ChatMessage, ChatRequest, LlmProvider};
use chrono::{DateTime, Timelike, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Emit an audit event to the context's audit log (if configured).
/// Silently does nothing if no audit log is set.
fn emit_audit(ctx: &LoopContext, event: aivyx_audit::AuditEvent) {
    if let Some(ref log) = ctx.audit_log
        && let Err(e) = log.append(event) {
        tracing::warn!("Failed to write audit event: {e}");
    }
}

/// Send a notification, logging a warning if the channel is full or closed.
pub(crate) fn send_notification(tx: &mpsc::Sender<Notification>, notif: Notification) {
    let title = notif.title.clone();
    if let Err(e) = tx.try_send(notif) {
        match e {
            mpsc::error::TrySendError::Full(_) => {
                tracing::warn!(
                    title = %title,
                    "Notification channel full — dropping notification",
                );
            }
            mpsc::error::TrySendError::Closed(_) => {
                tracing::warn!(
                    title = %title,
                    "Notification channel closed — dropping notification",
                );
            }
        }
    }
}

/// An item the loop wants to tell the user about or get approval for.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub id: String,
    pub kind: NotificationKind,
    pub title: String,
    pub body: String,
    pub source: String,
    pub timestamp: DateTime<Utc>,
    pub requires_approval: bool,
    /// If this notification was triggered by a goal, its ID.
    #[serde(default)]
    pub goal_id: Option<String>,
}

/// The decision a user makes on a pending `ApprovalNeeded` notification.
///
/// Sent from the TUI (or API) back to the agent loop when the user
/// presses `[A]` Approve or `[D]` Deny in the Approvals view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalResponse {
    /// Matches `Notification.id` of the pending item.
    pub notification_id: String,
    /// `true` = approved, `false` = denied.
    pub approved: bool,
    /// Optional reason or comment from the user.
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NotificationKind {
    /// New information (email arrived, reminder due, calendar event soon)
    Info,
    /// The assistant did something autonomously (filed a receipt, drafted a reply)
    ActionTaken,
    /// The assistant wants permission to do something
    ApprovalNeeded,
    /// Something urgent that needs attention
    Urgent,
}

/// Configuration for the agent loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopConfig {
    /// How often to run the full check cycle (minutes).
    pub check_interval_minutes: u32,
    /// Whether to run a morning briefing.
    pub morning_briefing: bool,
    /// Morning briefing hour (0-23, local time).
    pub briefing_hour: u8,
    /// Generate a briefing immediately on first tick (TUI launch).
    /// Only fires if no briefing has been generated today yet.
    #[serde(default = "default_true")]
    pub briefing_on_launch: bool,
    /// Heartbeat configuration — LLM-driven autonomous reasoning.
    #[serde(default)]
    pub heartbeat: HeartbeatConfig,
}

impl Default for LoopConfig {
    fn default() -> Self {
        Self {
            check_interval_minutes: 15,
            morning_briefing: true,
            briefing_hour: 8,
            briefing_on_launch: true,
            heartbeat: HeartbeatConfig::default(),
        }
    }
}

/// Configuration for the heartbeat — periodic LLM-driven introspection.
///
/// The heartbeat gathers context from available sources, presents it to
/// the LLM, and lets it decide what autonomous actions to take. When
/// nothing has changed since the last beat, the LLM call is skipped
/// entirely (zero token cost on quiet periods).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatConfig {
    /// Whether the heartbeat is active.
    #[serde(default = "default_heartbeat_enabled")]
    pub enabled: bool,
    /// Minutes between heartbeat ticks. Should be >= check_interval.
    #[serde(default = "default_heartbeat_interval")]
    pub interval_minutes: u32,
    /// Include active goals and their progress in context.
    #[serde(default = "default_true")]
    pub check_goals: bool,
    /// Include due/pending reminders in context.
    #[serde(default = "default_true")]
    pub check_reminders: bool,
    /// Include recent email subjects in context.
    #[serde(default = "default_true")]
    pub check_email: bool,
    /// Include self-model summary in context.
    #[serde(default = "default_true")]
    pub check_self_model: bool,
    /// Include recent schedule results in context.
    #[serde(default = "default_true")]
    pub check_schedules: bool,
    /// Include today's calendar events and conflicts in context.
    #[serde(default = "default_true")]
    pub check_calendar: bool,
    /// Include upcoming bills and budget alerts in context.
    #[serde(default = "default_true")]
    pub check_finance: bool,
    /// Include recent contact activity in context.
    #[serde(default = "default_true")]
    pub check_contacts: bool,
    /// Allow the heartbeat to create/update goals.
    #[serde(default = "default_true")]
    pub can_manage_goals: bool,
    /// Allow the heartbeat to store notifications for the user.
    #[serde(default = "default_true")]
    pub can_notify: bool,
    /// Allow the heartbeat to generate proactive suggestions.
    #[serde(default = "default_true")]
    pub can_suggest: bool,
    /// Allow the heartbeat to update the self-model (reflection).
    #[serde(default)]
    pub can_reflect: bool,
    /// Allow the heartbeat to trigger memory consolidation.
    #[serde(default)]
    pub can_consolidate_memory: bool,
    /// Allow the heartbeat to analyze failures and store post-mortem reflections.
    #[serde(default)]
    pub can_analyze_failures: bool,
    /// Allow the heartbeat to extract knowledge triples from context
    /// (emails, calendar, contacts, etc.) and store them in the knowledge graph.
    #[serde(default)]
    pub can_extract_knowledge: bool,
    /// Allow the heartbeat to prune old audit log entries.
    /// Default: false — the audit log is a tamper-evident chain and pruning
    /// should be an explicit opt-in to protect forensic integrity.
    #[serde(default)]
    pub can_prune_audit: bool,
    /// Audit log retention in days. Entries older than this are pruned
    /// when the heartbeat fires a `prune_audit` action.
    #[serde(default = "default_audit_retention_days")]
    pub audit_retention_days: u64,
    /// Allow the heartbeat to create encrypted backups of the data directory.
    #[serde(default)]
    pub can_backup: bool,

    // ── Phase 6: Smarter Agent ─────────────────────────────────
    /// Allow the heartbeat to organize goals into time horizons.
    #[serde(default)]
    pub can_plan_review: bool,
    /// Allow the heartbeat to run weekly strategy reviews.
    #[serde(default)]
    pub can_strategy_review: bool,
    /// Allow the heartbeat to track user mood signals.
    #[serde(default)]
    pub can_track_mood: bool,
    /// Allow the heartbeat to generate encouragement notifications.
    #[serde(default)]
    pub can_encourage: bool,
    /// Allow the heartbeat to detect and surface milestones.
    #[serde(default)]
    pub can_track_milestones: bool,
    /// Enable notification pacing (throttling based on mood, time, rate).
    #[serde(default)]
    pub notification_pacing: bool,
    /// Max notifications per hour when pacing is enabled.
    #[serde(default = "default_max_notifications_per_hour")]
    pub max_notifications_per_hour: u8,
}

fn default_audit_retention_days() -> u64 { 90 }
fn default_max_notifications_per_hour() -> u8 { 5 }

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self {
            enabled: default_heartbeat_enabled(),
            interval_minutes: default_heartbeat_interval(),
            check_goals: true,
            check_reminders: true,
            check_email: true,
            check_self_model: true,
            check_schedules: true,
            check_calendar: true,
            check_finance: true,
            check_contacts: true,
            can_manage_goals: true,
            can_notify: true,
            can_suggest: true,
            can_reflect: false,
            can_consolidate_memory: false,
            can_analyze_failures: false,
            can_extract_knowledge: false,
            can_prune_audit: false,
            audit_retention_days: default_audit_retention_days(),
            can_backup: false,
            can_plan_review: false,
            can_strategy_review: false,
            can_track_mood: false,
            can_encourage: false,
            can_track_milestones: false,
            notification_pacing: false,
            max_notifications_per_hour: default_max_notifications_per_hour(),
        }
    }
}

fn default_heartbeat_enabled() -> bool { true }
fn default_heartbeat_interval() -> u32 { 30 }
fn default_true() -> bool { true }

/// Handle to the running agent loop.
pub struct AgentLoop {
    config: LoopConfig,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    trigger_tx: mpsc::Sender<()>,
    /// Send approval decisions (Approve/Deny) from the UI back into the loop.
    ///
    /// Clone this sender and give it to the TUI/API at startup so the user's
    /// decisions flow back to the heartbeat without polling a shared mutex.
    pub approval_tx: mpsc::Sender<ApprovalResponse>,
}

// ── LLM JSON extraction utilities ────────────────────────────

/// Find the first top-level JSON object `{...}` in text that may contain
/// markdown fences or surrounding prose from an LLM response.
pub fn extract_json_object(text: &str) -> Option<&str> {
    extract_json_block(text, '{', '}')
}

/// Find the first top-level JSON array `[...]` in text that may contain
/// markdown fences or surrounding prose from an LLM response.
pub fn extract_json_array(text: &str) -> Option<&str> {
    extract_json_block(text, '[', ']')
}

/// Generic balanced-delimiter extraction for JSON blocks.
fn extract_json_block(text: &str, open: char, close: char) -> Option<&str> {
    let start = text.find(open)?;
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape_next = false;

    for (i, ch) in text[start..].char_indices() {
        if escape_next {
            escape_next = false;
            continue;
        }
        match ch {
            '\\' if in_string => escape_next = true,
            '"' => in_string = !in_string,
            c if c == open && !in_string => depth += 1,
            c if c == close && !in_string => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[start..start + i + 1]);
                }
            }
            _ => {}
        }
    }
    None
}

// ── Integration sub-contexts ─────────────────────────────────

/// Document vault integration context.
pub struct VaultCtx {
    pub config: aivyx_actions::documents::VaultConfig,
    pub key: MasterKey,
}

/// Finance tracking integration context.
pub struct FinanceCtx {
    pub key: MasterKey,
}

/// Contacts integration context (CardDAV + local store).
pub struct ContactsCtx {
    pub config: aivyx_actions::contacts::ContactsConfig,
    pub key: MasterKey,
}

/// Email triage integration context.
pub struct TriageCtx {
    pub config: triage::TriageConfig,
    pub key: MasterKey,
}

// ── Phase 6: Smarter Agent types ──────────────────────────────

/// Ephemeral per-session tracking of user interaction patterns.
///
/// Used by mood awareness (heuristic stress detection) and communication
/// pacing (notification throttling). Resets each session — mood is
/// transient, not persistent.
#[derive(Debug, Clone)]
pub struct InteractionSignals {
    /// When the user last sent a chat message.
    pub last_user_message_at: Option<DateTime<Utc>>,
    /// Total messages received this session.
    pub message_count_session: u32,
    /// Running total of message lengths (for computing average).
    total_message_length: u64,
    /// Consecutive short messages (<20 chars) — frustration signal.
    pub short_message_streak: u32,
    /// Count of negative keywords in recent messages.
    pub negative_keyword_count: u32,
    /// Notifications sent in the current clock hour.
    pub notifications_sent_this_hour: u32,
    /// Notifications sent today.
    pub notifications_sent_today: u32,
    /// When a notification was last dispatched.
    pub last_notification_at: Option<DateTime<Utc>>,
    /// The clock hour when `notifications_sent_this_hour` was last reset.
    hour_of_last_reset: u32,
    /// The date when `notifications_sent_today` was last reset.
    date_of_last_reset: chrono::NaiveDate,
}

/// Keywords that suggest user frustration or stress.
const NEGATIVE_KEYWORDS: &[&str] = &[
    "frustrated", "frustrating", "annoyed", "annoying", "broken",
    "ugh", "wtf", "damn", "useless", "wrong", "terrible", "horrible",
    "stupid", "hate", "sucks", "awful",
];

impl InteractionSignals {
    pub fn new() -> Self {
        let now = Utc::now();
        Self {
            last_user_message_at: None,
            message_count_session: 0,
            total_message_length: 0,
            short_message_streak: 0,
            negative_keyword_count: 0,
            notifications_sent_this_hour: 0,
            notifications_sent_today: 0,
            last_notification_at: None,
            hour_of_last_reset: now.timestamp() as u32 / 3600,
            date_of_last_reset: now.date_naive(),
        }
    }

    /// Record a user message and update all signal counters.
    pub fn record_message(&mut self, text: &str) {
        let now = Utc::now();
        self.last_user_message_at = Some(now);
        self.message_count_session += 1;
        self.total_message_length += text.len() as u64;

        // Short message streak
        let trimmed_len = text.trim().len();
        if trimmed_len < 20 {
            self.short_message_streak += 1;
        } else {
            self.short_message_streak = 0;
        }

        // Negative keyword scan (case-insensitive)
        let lower = text.to_lowercase();
        for kw in NEGATIVE_KEYWORDS {
            if lower.contains(kw) {
                self.negative_keyword_count += 1;
                break; // count one hit per message, not per keyword
            }
        }
    }

    /// Record that a notification was dispatched to the user.
    pub fn record_notification_sent(&mut self) {
        let now = Utc::now();
        self.last_notification_at = Some(now);

        // Reset hourly counter if the clock hour changed
        let current_hour = now.timestamp() as u32 / 3600;
        if current_hour != self.hour_of_last_reset {
            self.notifications_sent_this_hour = 0;
            self.hour_of_last_reset = current_hour;
        }
        self.notifications_sent_this_hour += 1;

        // Reset daily counter if the date changed
        let today = now.date_naive();
        if today != self.date_of_last_reset {
            self.notifications_sent_today = 0;
            self.date_of_last_reset = today;
        }
        self.notifications_sent_today += 1;
    }

    /// Average message length across all messages this session.
    pub fn avg_message_length(&self) -> f32 {
        if self.message_count_session == 0 {
            0.0
        } else {
            self.total_message_length as f32 / self.message_count_session as f32
        }
    }

    /// Minutes since the user last sent a message. `None` if no messages yet.
    pub fn idle_minutes(&self) -> Option<u64> {
        self.last_user_message_at.map(|t| {
            let secs = (Utc::now() - t).num_seconds().max(0) as u64;
            secs / 60
        })
    }
}

impl Default for InteractionSignals {
    fn default() -> Self {
        Self::new()
    }
}

/// Estimated user mood derived from interaction signals.
///
/// Used to gate notification delivery and provide context to the LLM
/// for tone adaptation. Detected heuristically — no extra LLM call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MoodSignal {
    /// Insufficient data or no clear signal.
    Neutral,
    /// Long, detailed messages at a steady pace — user is deep in work.
    Focused,
    /// Short messages, negative keywords, rapid pace — user is stressed.
    Frustrated,
    /// Long gap since last message — user has stepped away.
    Disengaged,
}

impl MoodSignal {
    /// Estimate mood from current interaction signals.
    ///
    /// Requires at least 3 messages in the session to produce a non-Neutral
    /// signal (avoids false positives on sparse data).
    pub fn estimate(signals: &InteractionSignals) -> Self {
        // Not enough data
        if signals.message_count_session < 3 {
            return Self::Neutral;
        }

        // Frustration: short message streak or negative keywords
        if signals.short_message_streak > 3 || signals.negative_keyword_count > 0 {
            return Self::Frustrated;
        }

        // Disengaged: idle for 30+ minutes
        if let Some(idle) = signals.idle_minutes() {
            if idle >= 30 {
                return Self::Disengaged;
            }
        }

        // Focused: long messages, steady pace
        if signals.avg_message_length() > 100.0 {
            return Self::Focused;
        }

        Self::Neutral
    }

    /// Human-readable label for heartbeat context injection.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Neutral => "neutral",
            Self::Focused => "focused (deep work, long messages)",
            Self::Frustrated => "frustrated (short messages, negative tone)",
            Self::Disengaged => "disengaged (idle, stepped away)",
        }
    }
}

impl std::fmt::Display for MoodSignal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Token and time budget tracking for resource-aware planning.
///
/// The heartbeat surfaces this as context when budget is significantly
/// consumed or when approaching quiet hours, so the LLM avoids starting
/// expensive operations at the wrong time.
#[derive(Debug, Clone)]
pub struct ResourceBudget {
    /// Tokens consumed by LLM calls today (heartbeat + scheduled prompts).
    pub tokens_used_today: u64,
    /// Tokens consumed specifically by heartbeat calls today.
    pub heartbeat_tokens_today: u64,
    /// Date when daily counters were last reset.
    pub last_reset_date: chrono::NaiveDate,
    /// Configurable daily token cap (0 = unlimited).
    pub token_budget_daily: u64,
    /// User's quiet hours start (0-23, local time). None = not configured.
    pub user_quiet_hours_start: Option<u8>,
    /// User's quiet hours end (0-23, local time). None = not configured.
    pub user_quiet_hours_end: Option<u8>,
}

impl ResourceBudget {
    pub fn new(token_budget_daily: u64) -> Self {
        Self {
            tokens_used_today: 0,
            heartbeat_tokens_today: 0,
            last_reset_date: Utc::now().date_naive(),
            token_budget_daily,
            user_quiet_hours_start: None,
            user_quiet_hours_end: None,
        }
    }

    /// Record tokens consumed by an LLM call. Resets if the day changed.
    pub fn record_tokens(&mut self, tokens: u64, is_heartbeat: bool) {
        let today = Utc::now().date_naive();
        if today != self.last_reset_date {
            self.tokens_used_today = 0;
            self.heartbeat_tokens_today = 0;
            self.last_reset_date = today;
        }
        self.tokens_used_today += tokens;
        if is_heartbeat {
            self.heartbeat_tokens_today += tokens;
        }
    }

    /// Fraction of daily budget consumed (0.0–1.0). Returns 0.0 if unlimited.
    pub fn budget_fraction(&self) -> f32 {
        if self.token_budget_daily == 0 {
            0.0
        } else {
            (self.tokens_used_today as f32 / self.token_budget_daily as f32).min(1.0)
        }
    }

    /// Whether we're currently in the user's quiet hours.
    pub fn in_quiet_hours(&self) -> bool {
        let (Some(start), Some(end)) = (self.user_quiet_hours_start, self.user_quiet_hours_end) else {
            return false;
        };
        let hour = chrono::Local::now().hour() as u8;
        if start <= end {
            // e.g., 22-8 wraps; 9-17 does not
            // This branch: no wrap (e.g., 9-17)
            hour >= start && hour < end
        } else {
            // Wraps midnight (e.g., 22-8)
            hour >= start || hour < end
        }
    }

    /// Format a context section for the heartbeat prompt (only when noteworthy).
    pub fn format_for_prompt(&self) -> Option<String> {
        let mut lines = Vec::new();

        // Show budget if > 50% consumed
        if self.token_budget_daily > 0 && self.budget_fraction() > 0.5 {
            lines.push(format!(
                "Token budget: {}/{} ({:.0}% used today)",
                self.tokens_used_today,
                self.token_budget_daily,
                self.budget_fraction() * 100.0,
            ));
        }

        // Show quiet hours warning if within 2 hours of start
        if let Some(start) = self.user_quiet_hours_start {
            let hour = chrono::Local::now().hour() as u8;
            let hours_until = if start > hour { start - hour } else { start + 24 - hour };
            if hours_until <= 2 && hours_until > 0 {
                lines.push(format!(
                    "Quiet hours begin in ~{hours_until}h (at {start}:00) — avoid starting long tasks"
                ));
            } else if self.in_quiet_hours() {
                lines.push("Currently in quiet hours — only urgent actions".into());
            }
        }

        if lines.is_empty() { None } else { Some(lines.join("\n")) }
    }
}

impl Default for ResourceBudget {
    fn default() -> Self {
        Self::new(0)
    }
}

/// Messaging integration context (Telegram + Matrix + Signal + SMS).
pub struct MessagingCtx {
    pub telegram: Option<aivyx_actions::messaging::TelegramConfig>,
    pub matrix: Option<aivyx_actions::messaging::MatrixConfig>,
    pub signal: Option<aivyx_actions::messaging::SignalConfig>,
    pub sms: Option<aivyx_actions::messaging::SmsConfig>,
}

pub struct LoopContext {
    /// Shared brain store — reads active goals each tick.
    pub brain_store: Arc<BrainStore>,
    /// Derived brain key for decrypting goals.
    pub brain_key: MasterKey,
    /// Email config for inbox checks (None if email not configured).
    pub email_config: Option<EmailConfig>,
    /// LLM provider for executing schedule prompts as agent turns.
    pub provider: Box<dyn LlmProvider>,
    /// System prompt used for schedule execution.
    pub system_prompt: String,
    /// User-defined cron schedules.
    pub schedules: Vec<ScheduleEntry>,
    /// Tracks when each schedule last fired (by name).
    pub schedule_last_run: HashMap<String, DateTime<Utc>>,
    /// Whether the morning briefing has already fired today.
    pub briefing_fired_today: Option<chrono::NaiveDate>,
    /// Encrypted store for reading due reminders.
    pub reminder_store: Arc<EncryptedStore>,
    /// Domain-separated key for reminder encryption.
    pub reminder_key: MasterKey,
    /// When the last heartbeat tick ran (None = never).
    pub last_heartbeat_at: Option<DateTime<Utc>>,
    /// Memory manager for heartbeat consolidation (optional).
    pub memory_manager: Option<Arc<tokio::sync::Mutex<aivyx_memory::MemoryManager>>>,
    /// Tool registry for scheduled prompts (optional, read-only subset).
    pub schedule_tools: Option<aivyx_core::ToolRegistry>,
    /// Calendar config for briefing context (optional).
    pub calendar_config: Option<aivyx_actions::calendar::CalendarConfig>,
    /// Domain-separated key for workflow template encryption (optional).
    pub workflow_key: Option<MasterKey>,
    /// Persistent state for the workflow trigger engine.
    pub trigger_state: trigger::TriggerState,

    // ── Grouped integration contexts ───────────────────────────
    /// Document vault (None if not configured).
    pub vault: Option<VaultCtx>,
    /// Finance tracking (None if not enabled).
    pub finance: Option<FinanceCtx>,
    /// Contacts integration (None if not configured).
    pub contacts: Option<ContactsCtx>,
    /// Email triage (None if not configured).
    pub triage: Option<TriageCtx>,
    /// Messaging integrations (Telegram + Matrix).
    pub messaging: Option<MessagingCtx>,

    // ── Phase 4.5: Resilient Agent additions ─────────────────────
    /// Audit log for heartbeat-driven pruning (optional).
    pub audit_log: Option<aivyx_audit::AuditLog>,
    /// MCP server pool for heartbeat health checks (optional).
    pub mcp_pool: Option<Arc<aivyx_mcp::pool::McpServerPool>>,
    /// Custom consolidation settings (None = use upstream defaults).
    pub consolidation_config: Option<aivyx_memory::ConsolidationConfig>,

    // ── Phase 5B: Deep Agent additions ──────────────────────────
    /// IMAP connection pool for reusing connections across ticks.
    pub imap_pool: Option<Arc<aivyx_actions::email::ImapPool>>,

    /// Backup destination directory (None = backups disabled at context level).
    pub backup_destination: Option<std::path::PathBuf>,
    /// Days to keep old backup archives before pruning.
    pub backup_retention_days: u64,
    /// Root data directory (source for backup archive).
    pub data_dir: Option<std::path::PathBuf>,

    // ── Phase 6: Smarter Agent additions ─────────────────────────
    /// Ephemeral interaction signals for mood/pacing (shared with TUI/API).
    pub interaction_signals: Arc<tokio::sync::Mutex<InteractionSignals>>,
    /// Token and time budget tracking.
    pub resource_budget: ResourceBudget,
    /// Set to `true` by the strategy-review workflow trigger; consumed by
    /// the next heartbeat tick to gather extended context.
    pub strategy_review_pending: bool,

    /// Proactive notification dispatch configuration.
    /// When `Some`, notifications are forwarded to desktop/Telegram/Signal.
    /// When `None`, notifications remain in-process only (TUI display).
    pub dispatch_config: Option<NotificationDispatchConfig>,

    // ── Error tracking (for graceful degradation) ───────────────
    /// Consecutive IMAP login failures (reset on success).
    pub imap_consecutive_failures: u32,
    /// Whether the credential-expiry notification has been sent (avoids spam).
    pub imap_expiry_notified: bool,
    /// Consecutive heartbeat LLM call failures (reset on success).
    pub heartbeat_consecutive_failures: u32,

    // ── Per-tick cache (cleared at the start of each tick) ──────
    /// Cached unread email summaries for the current tick.
    pub tick_email_cache: Option<Vec<EmailSummary>>,
    /// Cached calendar events for the current tick.
    pub tick_calendar_cache: Option<Vec<aivyx_actions::calendar::CalendarEvent>>,

    // ── Bidirectional approval channel ──────────────────────────
    /// Receives `ApprovalResponse` decisions from the TUI or API.
    /// The heartbeat drains this each tick and acts on pending decisions.
    /// `None` until `AgentLoop::start()` threads the receiver in.
    pub approval_rx: Option<mpsc::Receiver<ApprovalResponse>>,
    /// In-memory buffer for approval responses that arrived before
    /// the heartbeat was looking for a specific notification_id.
    pub pending_approval_responses: Vec<ApprovalResponse>,
}

impl AgentLoop {
    /// Start the agent loop in a background task.
    /// Returns the loop handle and a receiver for notifications.
    pub fn start(
        config: LoopConfig,
        context: Option<LoopContext>,
    ) -> (Self, mpsc::Receiver<Notification>) {
        let (notification_tx, notification_rx) = mpsc::channel(500);
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        let (trigger_tx, trigger_rx) = mpsc::channel(50);
        let (approval_tx, approval_rx) = mpsc::channel::<ApprovalResponse>(64);

        // Thread the approval receiver into the loop context so the
        // heartbeat can await decisions without polling a shared mutex.
        let context_with_approval = context.map(|mut ctx| {
            ctx.approval_rx = Some(approval_rx);
            ctx
        });

        let tx = notification_tx.clone();
        let cfg = config.clone();

        tokio::spawn(async move {
            run_loop(cfg, tx, shutdown_rx, trigger_rx, context_with_approval).await;
        });

        let handle = Self {
            config,
            shutdown_tx: Some(shutdown_tx),
            trigger_tx,
            approval_tx,
        };

        (handle, notification_rx)
    }

    /// Stop the agent loop.
    pub fn stop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }

    /// Get the loop config.
    pub fn config(&self) -> &LoopConfig {
        &self.config
    }

    /// Manually trigger an immediate check cycle (e.g. on app launch).
    pub fn trigger_check(&self) {
        let _ = self.trigger_tx.try_send(());
    }
}

impl Drop for AgentLoop {
    fn drop(&mut self) {
        self.stop();
    }
}

// ── Approval Helpers ─────────────────────────────────────────────

/// Drain all pending `ApprovalResponse` items for a specific notification ID
/// from the context buffer. Consumes the matching entries.
///
/// Used by `heartbeat.rs` after emitting an `ApprovalNeeded` notification
/// to check if the user has responded yet.
///
/// Returns `Some(response)` if a decision was found, `None` if still pending.
pub fn poll_approval(ctx: &mut LoopContext, notification_id: &str) -> Option<ApprovalResponse> {
    // Drain the live channel first
    if let Some(ref mut rx) = ctx.approval_rx {
        while let Ok(resp) = rx.try_recv() {
            tracing::debug!(
                notification_id = %resp.notification_id,
                approved = resp.approved,
                "Buffering approval response"
            );
            ctx.pending_approval_responses.push(resp);
        }
    }

    // Check the buffer for a matching decision
    if let Some(pos) = ctx.pending_approval_responses
        .iter()
        .position(|r| r.notification_id == notification_id)
    {
        return Some(ctx.pending_approval_responses.remove(pos));
    }

    None
}

/// Async-wait for a user approval decision on a specific notification ID.
///
/// Polls every 500ms up to `timeout`. Returns `Some(response)` when the
/// user approves or denies, or `None` if the timeout elapses.
///
/// **Example (heartbeat):**
/// ```rust
/// let notif_id = uuid::Uuid::new_v4().to_string();
/// send_notification(&tx, Notification {
///     id: notif_id.clone(),
///     kind: NotificationKind::ApprovalNeeded,
///     requires_approval: true,
///     title: "Delete these 200 files?".into(),
///     // ...
/// });
/// if let Some(resp) = await_approval(ctx, &notif_id, Duration::from_secs(300)).await {
///     if resp.approved {
///         // proceed with deletion
///     }
/// }
/// ```
pub async fn await_approval(
    ctx: &mut LoopContext,
    notification_id: &str,
    timeout: std::time::Duration,
) -> Option<ApprovalResponse> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Some(resp) = poll_approval(ctx, notification_id) {
            return Some(resp);
        }
        if tokio::time::Instant::now() >= deadline {
            tracing::warn!(
                notification_id = %notification_id,
                "Approval wait timed out — proceeding without user decision"
            );
            return None;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

/// Main loop — checks sources and evaluates goals on a schedule.
async fn run_loop(
    config: LoopConfig,
    tx: mpsc::Sender<Notification>,
    mut shutdown: tokio::sync::oneshot::Receiver<()>,
    mut trigger_rx: mpsc::Receiver<()>,
    mut context: Option<LoopContext>,
) {
    // Floor interval at 1 minute to prevent spin loop
    let interval_secs = (config.check_interval_minutes as u64 * 60).max(60);
    let interval = std::time::Duration::from_secs(interval_secs);

    // Initial delay so the TUI has time to render before the first tick
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    // On-launch briefing: generate immediately if enabled and not already fired today
    if config.briefing_on_launch && config.morning_briefing {
        let today = chrono::Local::now().date_naive();
        let already_fired = context
            .as_ref()
            .and_then(|c| c.briefing_fired_today)
            .is_some_and(|d| d == today);

        if !already_fired
            && let Some(ref mut ctx) = context {
                tracing::info!("Generating launch briefing");
                let briefing = generate_morning_briefing(ctx).await;

                send_notification(&tx, Notification {
                    id: uuid::Uuid::new_v4().to_string(),
                    kind: NotificationKind::Info,
                    title: "Your briefing is ready".into(),
                    body: briefing.summary,
                    source: "briefing".into(),
                    timestamp: Utc::now(),
                    requires_approval: false,
                    goal_id: None,
                });

                ctx.briefing_fired_today = Some(today);
            }
    }

    run_tick(&config, &tx, context.as_mut()).await;

    loop {
        tokio::select! {
            _ = tokio::time::sleep(interval) => {
                run_tick(&config, &tx, context.as_mut()).await;
            }
            Some(()) = trigger_rx.recv() => {
                // Manual trigger — run immediately
                tracing::debug!("Manual check cycle triggered");
                run_tick(&config, &tx, context.as_mut()).await;
            }
            _ = &mut shutdown => {
                tracing::info!("agent loop shutting down");
                break;
            }
        }
    }
}

/// A single tick of the loop — check sources, evaluate goals, run heartbeat.
async fn run_tick(
    config: &LoopConfig,
    tx: &mpsc::Sender<Notification>,
    mut context: Option<&mut LoopContext>,
) {
    // Clear per-tick caches so each tick starts fresh.
    if let Some(ref mut ctx) = context {
        ctx.tick_email_cache = None;
        ctx.tick_calendar_cache = None;

        // Drain any pending approval responses from the TUI/API into the
        // context buffer so the heartbeat can check them.
        if let Some(ref mut rx) = ctx.approval_rx {
            while let Ok(resp) = rx.try_recv() {
                tracing::info!(
                    notification_id = %resp.notification_id,
                    approved = resp.approved,
                    "Approval response received from user"
                );
                ctx.pending_approval_responses.push(resp);
            }
        }
    }

    // Check if it's morning briefing time (only fire once per day)
    if config.morning_briefing && schedule::is_briefing_time(config.briefing_hour) {
        let today = chrono::Local::now().date_naive();
        let already_fired = context
            .as_ref()
            .and_then(|c| c.briefing_fired_today)
            .is_some_and(|d| d == today);

        if !already_fired
            && let Some(ref mut ctx) = context {
                let briefing = generate_morning_briefing(ctx).await;

                // Audit: briefing generated
                emit_audit(ctx, aivyx_audit::AuditEvent::BriefingGenerated {
                    item_count: briefing.items.len(),
                    summary: crate::truncate(&briefing.summary, 200).to_string(),
                });

                send_notification(tx, Notification {
                    id: uuid::Uuid::new_v4().to_string(),
                    kind: NotificationKind::Info,
                    title: "Good morning — your briefing is ready".into(),
                    body: briefing.summary,
                    source: "briefing".into(),
                    timestamp: Utc::now(),
                    requires_approval: false,
                    goal_id: None,
                });

                ctx.briefing_fired_today = Some(today);
            }
    }

    // Evaluate active goals, schedules, and reminders
    if let Some(ref mut ctx) = context {
        if let Err(e) = evaluate_goals(ctx, tx).await {
            tracing::warn!("Goal evaluation failed: {e}");
        }

        evaluate_schedules(ctx, tx).await;
        create_calendar_auto_reminders(ctx).await;
        check_due_reminders(ctx, tx);
        reindex_vault_if_configured(ctx).await;
        triage_inbox_if_configured(ctx, tx).await;
        evaluate_workflow_triggers(ctx, tx);

        // Credential expiry detection: after 3 consecutive IMAP failures,
        // emit a one-time persistent notification so the user knows to fix it.
        if ctx.imap_consecutive_failures >= 3 && !ctx.imap_expiry_notified {
            ctx.imap_expiry_notified = true;
            tracing::error!(
                failures = ctx.imap_consecutive_failures,
                "Email credentials may have expired — sending notification",
            );
            send_notification(tx, Notification {
                id: uuid::Uuid::new_v4().to_string(),
                kind: NotificationKind::Urgent,
                title: "Email authentication failing".into(),
                body: format!(
                    "IMAP login has failed {} consecutive times. Your email password may \
                     have expired or changed. Run `aivyx init` to update your credentials, \
                     or check Settings → Integrations → Email.",
                    ctx.imap_consecutive_failures,
                ),
                source: "system:credential-check".into(),
                timestamp: Utc::now(),
                requires_approval: false,
                goal_id: None,
            });
        }
    }

    // Heartbeat: LLM-driven autonomous reasoning (runs on its own interval).
    // The heartbeat checks internally whether enough time has elapsed,
    // so calling it every tick is safe and cheap.
    if let Some(ctx) = context {
        let fired = heartbeat::run_heartbeat_tick(&config.heartbeat, ctx, tx).await;
        if fired {
            tracing::info!("Heartbeat tick completed");
        }
    }
}

// ── Morning briefing ──────────────────────────────────────────

/// Gather context from all sources and generate an LLM-powered briefing.
///
/// This is public so the TUI can call it at launch for an immediate briefing,
/// independent of the scheduled morning briefing time window.
pub async fn generate_morning_briefing(ctx: &mut LoopContext) -> briefing::Briefing {
    use aivyx_brain::GoalFilter;

    // Gather active goals
    let goals = ctx.brain_store
        .list_goals(
            &GoalFilter { status: Some(GoalStatus::Active), ..Default::default() },
            &ctx.brain_key,
        )
        .unwrap_or_default();

    // Gather recent email subjects if email is configured
    let email_subjects = if ctx.email_config.is_some() {
        fetch_email_subjects(ctx).await
    } else {
        vec![]
    };

    // Gather names of schedules that ran since midnight
    let today_start = chrono::Local::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|dt| dt.and_local_timezone(chrono::Local).earliest())
        .map(|dt| dt.with_timezone(&Utc));

    let recent_schedules: Vec<String> = ctx.schedule_last_run
        .iter()
        .filter(|(_, fired_at)| {
            today_start.is_some_and(|start| **fired_at >= start)
        })
        .map(|(name, _)| name.clone())
        .collect();

    let (calendar_events, calendar_conflicts) = fetch_calendar_for_briefing(ctx).await;
    let (upcoming_bills, over_budget) = gather_finance_briefing(ctx);

    let briefing_ctx = briefing::BriefingContext {
        goals,
        email_subjects,
        recent_schedules,
        calendar_events,
        calendar_conflicts,
        upcoming_bills,
        over_budget,
    };

    briefing::build_briefing(ctx.provider.as_ref(), &ctx.system_prompt, briefing_ctx).await
}

/// Fetch today's calendar events and detect conflicts for the briefing context.
async fn fetch_calendar_for_briefing(
    ctx: &LoopContext,
) -> (Vec<briefing::CalendarEventSummary>, Vec<briefing::CalendarConflictSummary>) {
    let Some(ref cal_config) = ctx.calendar_config else {
        return (vec![], vec![]);
    };

    let today = chrono::Local::now().date_naive();
    let Some(from) = today.and_hms_opt(0, 0, 0).map(|dt| dt.and_utc()) else {
        tracing::warn!("Failed to construct start-of-day timestamp");
        return (vec![], vec![]);
    };
    let Some(to) = today
        .succ_opt()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|dt| dt.and_utc())
    else {
        tracing::warn!("Failed to construct end-of-day timestamp");
        return (vec![], vec![]);
    };

    match aivyx_actions::calendar::fetch_events(cal_config, from, to).await {
        Ok(events) => {
            // Detect conflicts before converting to summaries.
            let conflicts = aivyx_actions::calendar::detect_conflicts(&events);
            let conflict_summaries: Vec<briefing::CalendarConflictSummary> = conflicts
                .into_iter()
                .map(|c| {
                    let overlap_start = c.overlap_start.with_timezone(&chrono::Local).format("%H:%M");
                    let overlap_end = c.overlap_end.with_timezone(&chrono::Local).format("%H:%M");
                    briefing::CalendarConflictSummary {
                        event_a: c.event_a,
                        event_b: c.event_b,
                        overlap: format!("{overlap_start}–{overlap_end}"),
                    }
                })
                .collect();

            let event_summaries = events.into_iter().map(|e| {
                let time = if e.all_day {
                    "All day".to_string()
                } else {
                    e.start.with_timezone(&chrono::Local).format("%H:%M").to_string()
                };
                briefing::CalendarEventSummary {
                    time,
                    summary: e.summary,
                    location: e.location,
                }
            }).collect();

            (event_summaries, conflict_summaries)
        }
        Err(e) => {
            tracing::warn!("Failed to fetch calendar events for briefing: {e}");
            (vec![], vec![])
        }
    }
}

/// Fetch unread email summaries, using the per-tick cache if available.
async fn fetch_email_cached(ctx: &mut LoopContext) -> Vec<EmailSummary> {
    if let Some(ref cached) = ctx.tick_email_cache {
        return cached.clone();
    }

    let Some(ref email_config) = ctx.email_config else {
        return vec![];
    };

    let reader = ReadInbox {
        config: email_config.clone(),
        pool: ctx.imap_pool.clone(),
    };
    let input = serde_json::json!({ "limit": 20, "unread_only": true });
    let summaries = match reader.execute(input).await {
        Ok(result) => {
            if ctx.imap_expiry_notified && ctx.imap_consecutive_failures >= 3 {
                tracing::info!(
                    "IMAP recovered after {} consecutive failures — credential expiry notification cleared",
                    ctx.imap_consecutive_failures,
                );
            }
            ctx.imap_consecutive_failures = 0;
            ctx.imap_expiry_notified = false;
            serde_json::from_value::<Vec<EmailSummary>>(result).unwrap_or_default()
        }
        Err(e) => {
            ctx.imap_consecutive_failures += 1;
            tracing::warn!(
                failures = ctx.imap_consecutive_failures,
                "Email fetch failed: {e}",
            );
            vec![]
        }
    };

    ctx.tick_email_cache = Some(summaries.clone());
    summaries
}

/// Fetch unread email subjects for the briefing context (uses cache).
async fn fetch_email_subjects(ctx: &mut LoopContext) -> Vec<String> {
    fetch_email_cached(ctx).await
        .into_iter()
        .map(|s| format!("{}: {}", s.from, s.subject))
        .collect()
}

// ── Finance briefing context ─────────────────────────────────

/// Gather upcoming bills and over-budget categories for the briefing.
fn gather_finance_briefing(ctx: &LoopContext) -> (Vec<String>, Vec<String>) {
    let Some(ref finance) = ctx.finance else {
        return (vec![], vec![]);
    };

    let bills = match aivyx_actions::finance::upcoming_bills(
        &ctx.reminder_store, &finance.key, 7,
    ) {
        Ok(bills) => bills
            .iter()
            .map(|b| {
                let amount = aivyx_actions::finance::format_dollars(b.amount_cents);
                match &b.due_date {
                    Some(d) => format!("{} — {} due {}", b.description, amount, d.format("%b %d")),
                    None => format!("{} — {}", b.description, amount),
                }
            })
            .collect(),
        Err(e) => {
            tracing::warn!("Failed to load upcoming bills for briefing: {e}");
            vec![]
        }
    };

    let over = match aivyx_actions::finance::over_budget_categories(
        &ctx.reminder_store, &finance.key,
    ) {
        Ok(cats) => cats
            .iter()
            .map(|(cat, spent, limit)| {
                format!(
                    "{}: {} spent (limit: {})",
                    cat,
                    aivyx_actions::finance::format_dollars(*spent),
                    aivyx_actions::finance::format_dollars(*limit),
                )
            })
            .collect(),
        Err(e) => {
            tracing::warn!("Failed to check budget for briefing: {e}");
            vec![]
        }
    };

    (bills, over)
}

// ── Calendar auto-reminders ───────────────────────────────────

/// Default lead time for calendar auto-reminders (minutes before event start).
const CALENDAR_REMINDER_LEAD_MINUTES: i64 = 15;

/// Automatically create reminders for upcoming calendar events.
///
/// Each tick, fetches events starting within `CALENDAR_REMINDER_LEAD_MINUTES`
/// and creates a reminder for any that don't already have one. Reminders are
/// keyed by `cal-remind:{uid}:{date}` so recurring events get one per day
/// and the same event is never reminded twice.
async fn create_calendar_auto_reminders(ctx: &LoopContext) {
    let Some(ref cal_config) = ctx.calendar_config else {
        return;
    };

    let now = Utc::now();
    // Fetch events for the next hour (wider than lead time to ensure we
    // catch events even if the tick interval is long).
    let horizon = now + chrono::Duration::hours(1);

    let events = match aivyx_actions::calendar::fetch_events(cal_config, now, horizon).await {
        Ok(events) => events,
        Err(e) => {
            tracing::debug!("Calendar auto-reminder fetch failed: {e}");
            return;
        }
    };

    let upcoming = aivyx_actions::calendar::events_needing_reminder(
        &events,
        now,
        CALENDAR_REMINDER_LEAD_MINUTES,
    );

    for event in upcoming {
        let event_date = event.start.with_timezone(&chrono::Local).date_naive();
        let marker_key = aivyx_actions::calendar::auto_reminder_key(&event.uid, &event_date);

        // Check if we already created a reminder for this event today.
        match ctx.reminder_store.get(&marker_key, &ctx.reminder_key) {
            Ok(Some(_)) => continue, // already exists
            Ok(None) => {}
            Err(e) => {
                tracing::debug!("Failed to check auto-reminder marker: {e}");
                continue;
            }
        }

        // Create a reminder that fires at the event's start time.
        // The user will see it as a normal reminder notification.
        let local_time = event.start.with_timezone(&chrono::Local).format("%H:%M");
        let message = if let Some(ref loc) = event.location {
            format!("Calendar: {} at {} ({})", event.summary, local_time, loc)
        } else {
            format!("Calendar: {} at {}", event.summary, local_time)
        };

        let reminder = reminders::Reminder {
            id: uuid::Uuid::new_v4().to_string(),
            message,
            due_at: event.start - chrono::Duration::minutes(CALENDAR_REMINDER_LEAD_MINUTES),
            completed: false,
            created_at: now,
        };

        let json = match serde_json::to_vec(&reminder) {
            Ok(j) => j,
            Err(e) => {
                tracing::warn!("Failed to serialize auto-reminder: {e}");
                continue;
            }
        };

        // Store the reminder itself.
        if let Err(e) = ctx.reminder_store.put(
            &format!("reminder:{}", reminder.id),
            &json,
            &ctx.reminder_key,
        ) {
            tracing::warn!("Failed to save auto-reminder: {e}");
            continue;
        }

        // Store a marker so we don't create another reminder for this event today.
        if let Err(e) = ctx.reminder_store.put(
            &marker_key,
            reminder.id.as_bytes(),
            &ctx.reminder_key,
        ) {
            tracing::warn!("Failed to save auto-reminder marker: {e}");
        }

        tracing::info!(
            "Auto-reminder set for calendar event '{}' at {}",
            event.summary,
            event.start.to_rfc3339()
        );
    }

    // Prune stale reminder markers older than 7 days — at most once per day
    // to avoid iterating all store keys on every tick.
    let today = chrono::Local::now().date_naive();
    let prune_marker = format!("cal-prune:{today}");
    let already_pruned = ctx.reminder_store
        .get(&prune_marker, &ctx.reminder_key)
        .ok()
        .flatten()
        .is_some();
    if !already_pruned {
        prune_calendar_markers(&ctx.reminder_store, 7);
        let _ = ctx.reminder_store.put(&prune_marker, b"1", &ctx.reminder_key);
    }
}

/// Remove `cal-remind:*` markers whose embedded date is older than `max_age_days`.
fn prune_calendar_markers(store: &EncryptedStore, max_age_days: i64) {
    let cutoff = chrono::Local::now().date_naive() - chrono::Duration::days(max_age_days);
    let Ok(keys) = store.list_keys() else { return };

    for key in &keys {
        if let Some(date_str) = key.strip_prefix("cal-remind:").and_then(|rest| rest.rsplit(':').next())
            && let Ok(date) = chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d")
                && date < cutoff {
                    let _ = store.delete(key);
                }
    }
}

// ── Vault re-indexing ────────────────────────────────────────

/// Re-index the document vault if configured.
///
/// This runs on every tick but `index_vault` is content-hash idempotent —
/// unchanged files are skipped in O(1) (hash comparison). Only files that
/// have been added or modified since the last index are re-processed.
/// This gives us "watch mode" without needing a filesystem watcher.
async fn reindex_vault_if_configured(ctx: &LoopContext) {
    let (Some(vault), Some(memory)) =
        (&ctx.vault, &ctx.memory_manager) else {
        return;
    };

    match aivyx_actions::documents::index_vault(
        &vault.config,
        memory,
        &ctx.reminder_store, // reuse same EncryptedStore for vault index
        &vault.key,
    ).await {
        Ok(result) if result.indexed > 0 => {
            tracing::info!(
                "Vault re-index: {} new/updated, {} skipped, {} errors",
                result.indexed, result.skipped, result.errors
            );
        }
        Ok(_) => {} // all files unchanged — silent
        Err(e) => {
            tracing::warn!("Vault re-index failed: {e}");
        }
    }
}

// ── Email triage ─────────────────────────────────────────────

/// Run autonomous email triage if configured.
///
/// Processes new emails since the last triage cursor, applies rules
/// and LLM classification, and emits notifications for the user.
async fn triage_inbox_if_configured(
    ctx: &LoopContext,
    tx: &mpsc::Sender<Notification>,
) {
    let (Some(triage), Some(email_config)) =
        (&ctx.triage, &ctx.email_config) else {
        return;
    };

    if !triage.config.enabled {
        return;
    }

    let summary = triage::triage_inbox(
        &triage.config,
        email_config,
        ctx.imap_pool.as_ref(),
        ctx.provider.as_ref(),
        &ctx.reminder_store,
        &triage.key,
    ).await;

    if summary.processed == 0 {
        return;
    }

    // Audit: triage completed
    emit_audit(ctx, aivyx_audit::AuditEvent::TriageCompleted {
        processed: summary.processed,
        classified: summary.classified,
        auto_replied: summary.auto_replied,
        forwarded: summary.forwarded,
        errors: summary.errors,
    });

    // Emit a notification summarizing what happened
    let mut parts = Vec::new();
    if summary.classified > 0 {
        parts.push(format!("{} classified", summary.classified));
    }
    if summary.auto_replied > 0 {
        parts.push(format!("{} auto-replied", summary.auto_replied));
    }
    if summary.forwarded > 0 {
        parts.push(format!("{} forwarded", summary.forwarded));
    }
    if summary.ignored > 0 {
        parts.push(format!("{} ignored", summary.ignored));
    }
    if summary.errors > 0 {
        parts.push(format!("{} errors", summary.errors));
    }

    let body = format!("Processed {} email(s): {}", summary.processed, parts.join(", "));

    send_notification(tx, Notification {
        id: uuid::Uuid::new_v4().to_string(),
        kind: if summary.auto_replied > 0 || summary.forwarded > 0 {
            NotificationKind::ActionTaken
        } else {
            NotificationKind::Info
        },
        title: "Email triage complete".into(),
        body,
        source: "triage".into(),
        timestamp: Utc::now(),
        requires_approval: false,
        goal_id: None,
    });
}

// ── Reminder checks ───────────────────────────────────────────

/// Check for due reminders and emit notifications for each.
/// Marks fired reminders as completed so they don't fire again.
fn check_due_reminders(
    ctx: &LoopContext,
    tx: &mpsc::Sender<Notification>,
) {
    let due = match reminders::load_due_reminders(&ctx.reminder_store, &ctx.reminder_key) {
        Ok(due) => due,
        Err(e) => {
            tracing::warn!("Failed to check due reminders: {e}");
            return;
        }
    };

    for reminder in &due {
        send_notification(tx, Notification {
            id: reminder.id.clone(),
            kind: NotificationKind::Info,
            title: format!("Reminder: {}", truncate(&reminder.message, 60)),
            body: reminder.message.clone(),
            source: "reminder".into(),
            timestamp: Utc::now(),
            requires_approval: false,
            goal_id: None,
        });

        // Mark as completed so it doesn't fire again
        let mut completed = reminder.clone();
        completed.completed = true;
        let json = match serde_json::to_vec(&completed) {
            Ok(j) => j,
            Err(e) => {
                tracing::warn!("Failed to serialize reminder: {e}");
                continue;
            }
        };
        if let Err(e) = ctx.reminder_store.put(
            &format!("reminder:{}", reminder.id),
            &json,
            &ctx.reminder_key,
        ) {
            tracing::warn!("Failed to mark reminder as completed: {e}");
        }

        tracing::info!("Reminder fired: '{}'", reminder.message);
    }
}

// ── Schedule evaluation ────────────────────────────────────────

/// Evaluate workflow triggers and emit notifications for any that fire.
fn evaluate_workflow_triggers(
    ctx: &mut LoopContext,
    tx: &mpsc::Sender<Notification>,
) {
    let Some(ref workflow_key) = ctx.workflow_key else { return };

    // Gather recent email data from triage log for email triggers
    let recent_emails: Vec<(String, String)> = if let Some(ref triage) = ctx.triage {
        triage::load_triage_log(&ctx.reminder_store, &triage.key, 50)
            .into_iter()
            .map(|r| (r.from, r.subject))
            .collect()
    } else {
        vec![]
    };

    // Gather active goals for goal-progress triggers
    let active_goals: Vec<(String, f32)> = {
        let filter = aivyx_brain::GoalFilter {
            status: Some(GoalStatus::Active),
            ..Default::default()
        };
        ctx.brain_store.list_goals(&filter, &ctx.brain_key)
            .unwrap_or_default()
            .into_iter()
            .map(|g| (g.description, g.progress))
            .collect()
    };

    let trigger_ctx = trigger::TriggerContext {
        recent_emails: &recent_emails,
        active_goals: &active_goals,
        last_tick_at: ctx.trigger_state.last_evaluated_at,
        ..Default::default()
    };

    let result = trigger::evaluate_triggers(
        &ctx.reminder_store, // workflows share the same EncryptedStore
        workflow_key,
        &mut ctx.trigger_state,
        &trigger_ctx,
    );
    ctx.trigger_state.last_evaluated_at = Some(Utc::now());

    if !result.fired.is_empty() {
        tracing::info!(
            "Workflow triggers: {} fired out of {} evaluated",
            result.fired.len(),
            result.triggers_evaluated,
        );
    }

    for fired in result.fired {
        send_notification(tx, Notification {
            id: uuid::Uuid::new_v4().to_string(),
            kind: NotificationKind::ActionTaken,
            title: format!("Workflow triggered: {}", fired.template_name),
            body: fired.reason.clone(),
            source: "trigger".into(),
            timestamp: Utc::now(),
            requires_approval: false,
            goal_id: None,
        });
        tracing::info!(
            "Trigger fired: template='{}' reason='{}'",
            fired.template_name,
            fired.reason,
        );
    }
}

/// Check if a cron expression is due.
///
/// A schedule is due if at least one cron match occurred between
/// `last_run_at` (or 61s ago if never run) and `now`.
fn is_due(
    cron_expr: &str,
    last_run_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> bool {
    let cron = match croner::Cron::new(cron_expr).parse() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("Invalid cron expression '{cron_expr}': {e}");
            return false;
        }
    };

    let from = last_run_at.unwrap_or_else(|| now - chrono::Duration::seconds(61));
    cron.find_next_occurrence(&from, false)
        .is_ok_and(|next| next <= now)
}

/// Evaluate cron-based schedules: run the prompt through the LLM and emit
/// the response as a notification.
///
/// When `schedule_tools` is present in the context, tool definitions are
/// included in the request and tool calls are executed in a loop (max 5
/// iterations). This allows schedules to read email, search the web, etc.
async fn evaluate_schedules(
    ctx: &mut LoopContext,
    tx: &mpsc::Sender<Notification>,
) {
    let now = Utc::now();

    // Collect due schedules first to avoid borrowing ctx across await points.
    let due: Vec<(String, String, bool)> = ctx
        .schedules
        .iter()
        .filter(|s| s.enabled)
        .filter(|s| {
            let last_run = ctx.schedule_last_run.get(&s.name).copied()
                .or(s.last_run_at);
            is_due(&s.cron, last_run, now)
        })
        .map(|s| (s.name.clone(), s.prompt.clone(), s.notify))
        .collect();

    // Generate tool definitions once if we have a schedule registry.
    let tool_defs = ctx.schedule_tools.as_ref()
        .map(|r| r.tool_definitions())
        .unwrap_or_default();

    for (name, prompt, notify) in &due {
        tracing::info!("Schedule '{name}' is due — executing prompt");

        // Audit: schedule fired
        emit_audit(ctx, aivyx_audit::AuditEvent::ScheduleFired {
            schedule_name: name.clone(),
            agent_name: "pa".into(),
            timestamp: now,
        });

        let body = execute_schedule_turn(
            ctx.provider.as_ref(),
            &ctx.system_prompt,
            prompt,
            &tool_defs,
            ctx.schedule_tools.as_ref(),
            name,
        ).await;

        // Audit: schedule completed
        emit_audit(ctx, aivyx_audit::AuditEvent::ScheduleCompleted {
            schedule_name: name.clone(),
            success: true,
            result_summary: crate::truncate(&body, 200).to_string(),
        });

        if *notify {
            send_notification(tx, Notification {
                id: uuid::Uuid::new_v4().to_string(),
                kind: NotificationKind::ActionTaken,
                title: format!("Scheduled: {name}"),
                body,
                source: "schedule".into(),
                timestamp: now,
                requires_approval: false,
                goal_id: None,
            });
        }

        // Record that this schedule fired so it doesn't fire again until
        // the next cron match.
        ctx.schedule_last_run.insert(name.clone(), now);
    }
}

/// Execute a single schedule turn with optional tool-call loop.
///
/// If tools are provided, the LLM can invoke them and the results are fed
/// back for up to `MAX_TOOL_ROUNDS` iterations. The final text response
/// becomes the notification body.
const MAX_TOOL_ROUNDS: usize = 5;

async fn execute_schedule_turn(
    provider: &dyn LlmProvider,
    system_prompt: &str,
    prompt: &str,
    tool_defs: &[serde_json::Value],
    tools: Option<&aivyx_core::ToolRegistry>,
    schedule_name: &str,
) -> String {
    let mut messages = vec![ChatMessage::user(prompt.to_string())];

    for round in 0..=MAX_TOOL_ROUNDS {
        let request = ChatRequest {
            system_prompt: Some(system_prompt.to_string()),
            messages: messages.clone(),
            tools: tool_defs.to_vec(),
            model: None,
            max_tokens: 1024,
        };

        let response = match aivyx_actions::retry::retry(
            &aivyx_actions::retry::RetryConfig::llm(),
            || async {
                tokio::time::timeout(
                    std::time::Duration::from_secs(60),
                    provider.chat(&request),
                ).await
                .map_err(|_| aivyx_core::AivyxError::LlmProvider("timeout after 60s".into()))?
            },
            aivyx_actions::retry::is_transient,
        ).await {
            Ok(response) => response,
            Err(e) => {
                tracing::warn!("Schedule '{schedule_name}' LLM call failed: {e}");
                return format!("(Schedule ran but LLM failed: {e})");
            }
        };

        // If no tool calls or no tool registry, return the text response.
        if response.message.tool_calls.is_empty() || tools.is_none() {
            let text = response.message.content.to_text();
            tracing::info!(
                "Schedule '{schedule_name}' completed ({} tokens, {round} tool rounds)",
                response.usage.output_tokens
            );
            return text;
        }

        // Execute tool calls and build result messages.
        // Safety: tools.is_none() is checked above and returns early.
        let Some(registry) = tools else {
            return response.message.content.to_text();
        };
        let assistant_msg = ChatMessage::assistant_with_tool_calls(
            response.message.content.to_text(),
            response.message.tool_calls.clone(),
        );
        messages.push(assistant_msg);

        for tc in &response.message.tool_calls {
            let result = if let Some(tool) = registry.get_by_name(&tc.name) {
                match tool.execute(tc.arguments.clone()).await {
                    Ok(output) => {
                        tracing::info!(
                            "Schedule '{schedule_name}' tool '{}'  OK",
                            tc.name
                        );
                        aivyx_llm::ToolResult {
                            tool_call_id: tc.id.clone(),
                            content: output,
                            is_error: false,
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Schedule '{schedule_name}' tool '{}' failed: {e}",
                            tc.name
                        );
                        aivyx_llm::ToolResult {
                            tool_call_id: tc.id.clone(),
                            content: serde_json::json!({"error": e.to_string()}),
                            is_error: true,
                        }
                    }
                }
            } else {
                tracing::warn!(
                    "Schedule '{schedule_name}' requested unknown tool '{}'",
                    tc.name
                );
                aivyx_llm::ToolResult {
                    tool_call_id: tc.id.clone(),
                    content: serde_json::json!({"error": "tool not available"}),
                    is_error: true,
                }
            };
            messages.push(ChatMessage::tool(result));
        }
    }

    // Safety: if we exhaust rounds, return whatever we have.
    tracing::warn!(
        "Schedule '{schedule_name}' hit max tool rounds ({MAX_TOOL_ROUNDS}), returning partial"
    );
    "(Schedule completed but hit tool call limit)".to_string()
}

// ── Goal evaluation ────────────────────────────────────────────

/// Outcome of evaluating a single goal check.
enum CheckOutcome {
    /// Found something relevant (e.g. matching emails, web content).
    Success,
    /// Check ran but found nothing relevant.
    NoMatch,
    /// Check failed (network error, parse error, etc.).
    Failure,
    /// No automated check applies to this goal.
    Skipped,
}

/// Read active goals from the Brain and evaluate each one.
///
/// For each goal, determines the right source to check (email, web, etc.),
/// executes the check, emits notifications if something relevant is found,
/// and records success/failure outcomes back to the goal for cooldown/backoff.
async fn evaluate_goals(
    ctx: &LoopContext,
    tx: &mpsc::Sender<Notification>,
) -> Result<()> {
    use aivyx_brain::GoalFilter;

    let goals = ctx.brain_store.list_goals(
        &GoalFilter {
            status: Some(GoalStatus::Active),
            ..Default::default()
        },
        &ctx.brain_key,
    )?;

    if goals.is_empty() {
        return Ok(());
    }

    tracing::debug!("Evaluating {} active goals", goals.len());

    for goal in goals {
        // Skip goals that are in cooldown (recently evaluated, circuit breaker)
        if goal.is_in_cooldown() {
            tracing::trace!("Goal '{}': in cooldown, skipping", goal.description);
            continue;
        }

        let action = match_goal_to_action(&goal);

        let outcome = match action {
            GoalAction::CheckEmail { query } => {
                check_email_for_goal(ctx, tx, &goal, &query).await
            }
            GoalAction::CheckWeb { url } => {
                check_web_for_goal(tx, &goal, &url).await
            }
            GoalAction::CheckReminders => {
                check_reminders_for_goal(ctx, tx, &goal)
            }
            GoalAction::NoAction => {
                tracing::trace!("Goal '{}': no automated action", goal.description);
                CheckOutcome::Skipped
            }
        };

        // Record outcome back to the goal for cooldown/backoff
        match outcome {
            CheckOutcome::Success => {
                let mut updated = goal.clone();
                updated.record_success();
                if let Err(e) = ctx.brain_store.upsert_goal(&updated, &ctx.brain_key) {
                    tracing::warn!("Failed to record goal success: {e}");
                } else {
                    tracing::debug!(
                        "Goal '{}': recorded success (failures reset)",
                        updated.description
                    );
                }
            }
            CheckOutcome::Failure => {
                let mut updated = goal.clone();
                updated.record_failure();
                let was_abandoned = updated.status == GoalStatus::Abandoned;
                if let Err(e) = ctx.brain_store.upsert_goal(&updated, &ctx.brain_key) {
                    tracing::warn!("Failed to record goal failure: {e}");
                } else if was_abandoned {
                    tracing::warn!(
                        "Goal '{}': auto-abandoned after {} consecutive failures",
                        updated.description,
                        updated.consecutive_failures
                    );
                    send_notification(tx, Notification {
                        id: uuid::Uuid::new_v4().to_string(),
                        kind: NotificationKind::Info,
                        title: format!("Goal abandoned: {}", truncate(&updated.description, 50)),
                        body: format!(
                            "Automatically abandoned after {} consecutive failures. \
                             Use brain_update_goal to reactivate if needed.",
                            updated.consecutive_failures
                        ),
                        source: "goal".into(),
                        timestamp: Utc::now(),
                        requires_approval: false,
                        goal_id: Some(updated.id.to_string()),
                    });
                } else {
                    tracing::debug!(
                        "Goal '{}': recorded failure #{} (cooldown until {:?})",
                        updated.description,
                        updated.consecutive_failures,
                        updated.cooldown_until
                    );
                }
            }
            CheckOutcome::NoMatch | CheckOutcome::Skipped => {
                // No state change needed — goal remains active, no backoff
            }
        }
    }

    Ok(())
}

/// Check email inbox and emit notifications for messages matching a goal.
async fn check_email_for_goal(
    ctx: &LoopContext,
    tx: &mpsc::Sender<Notification>,
    goal: &Goal,
    query: &str,
) -> CheckOutcome {
    let Some(ref email_config) = ctx.email_config else {
        tracing::debug!("Goal '{}': email not configured, skipping", goal.description);
        return CheckOutcome::Skipped;
    };

    let reader = ReadInbox {
        config: email_config.clone(),
        pool: ctx.imap_pool.clone(),
    };

    // Fetch recent unread emails
    let input = serde_json::json!({ "limit": 10, "unread_only": true });
    match reader.execute(input).await {
        Ok(result) => {
            // Parse the email summaries
            let summaries: Vec<EmailSummary> = serde_json::from_value(result).unwrap_or_default();

            if summaries.is_empty() {
                return CheckOutcome::NoMatch;
            }

            // Filter: find emails that match the goal's query
            let query_lower = query.to_lowercase();
            let matches: Vec<&EmailSummary> = summaries
                .iter()
                .filter(|s| {
                    s.from.to_lowercase().contains(&query_lower)
                        || s.subject.to_lowercase().contains(&query_lower)
                        || s.preview.to_lowercase().contains(&query_lower)
                })
                .collect();

            if matches.is_empty() {
                return CheckOutcome::NoMatch;
            }

            for email in &matches {
                send_notification(tx, Notification {
                    id: uuid::Uuid::new_v4().to_string(),
                    kind: NotificationKind::Info,
                    title: format!("Email from {}: {}", email.from, email.subject),
                    body: email.preview.clone(),
                    source: "email".into(),
                    timestamp: Utc::now(),
                    requires_approval: false,
                    goal_id: Some(goal.id.to_string()),
                });

                tracing::info!(
                    "Goal '{}': found matching email from {}",
                    goal.description,
                    email.from
                );
            }

            CheckOutcome::Success
        }
        Err(e) => {
            tracing::warn!(
                "Goal '{}': email check failed: {e}",
                goal.description
            );
            CheckOutcome::Failure
        }
    }
}

/// Fetch a web page and emit a notification if content seems relevant.
async fn check_web_for_goal(
    tx: &mpsc::Sender<Notification>,
    goal: &Goal,
    url: &str,
) -> CheckOutcome {
    // Only fetch if the goal contains something that looks like a URL
    if !url.starts_with("http://") && !url.starts_with("https://") {
        tracing::debug!(
            "Goal '{}': no URL found in description, skipping web check",
            goal.description
        );
        return CheckOutcome::Skipped;
    }

    let fetcher = FetchPage;
    let input = serde_json::json!({ "url": url });

    match fetcher.execute(input).await {
        Ok(result) => {
            let content = result["content"].as_str().unwrap_or("");
            // Take first 500 chars as a preview
            let preview: String = content.chars().take(500).collect();

            if preview.is_empty() {
                return CheckOutcome::NoMatch;
            }

            send_notification(tx, Notification {
                id: uuid::Uuid::new_v4().to_string(),
                kind: NotificationKind::Info,
                title: format!("Web update for: {}", truncate(&goal.description, 50)),
                body: preview,
                source: "web".into(),
                timestamp: Utc::now(),
                requires_approval: false,
                goal_id: Some(goal.id.to_string()),
            });

            CheckOutcome::Success
        }
        Err(e) => {
            tracing::warn!(
                "Goal '{}': web check failed: {e}",
                goal.description
            );
            CheckOutcome::Failure
        }
    }
}

/// Check pending reminders that might be related to a goal.
fn check_reminders_for_goal(
    ctx: &LoopContext,
    tx: &mpsc::Sender<Notification>,
    goal: &Goal,
) -> CheckOutcome {
    let pending = match reminders::load_all_reminders(&ctx.reminder_store, &ctx.reminder_key) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("Goal '{}': reminder check failed: {e}", goal.description);
            return CheckOutcome::Failure;
        }
    };

    let pending: Vec<_> = pending.into_iter().filter(|r| !r.completed).collect();
    if pending.is_empty() {
        return CheckOutcome::NoMatch;
    }

    let desc_lower = goal.description.to_lowercase();
    let mut found = false;

    for reminder in &pending {
        let msg_lower = reminder.message.to_lowercase();
        // Simple keyword overlap check — does the reminder relate to this goal?
        if desc_lower.split_whitespace().any(|w| w.len() > 3 && msg_lower.contains(w)) {
            found = true;
            let due_label = if reminder.due_at <= Utc::now() {
                "DUE NOW".to_string()
            } else {
                format!("due {}", reminder.due_at.format("%b %d %H:%M"))
            };

            send_notification(tx, Notification {
                id: uuid::Uuid::new_v4().to_string(),
                kind: NotificationKind::Info,
                title: format!("Reminder ({due_label}): {}", truncate(&reminder.message, 50)),
                body: reminder.message.clone(),
                source: "reminder".into(),
                timestamp: Utc::now(),
                requires_approval: false,
                goal_id: Some(goal.id.to_string()),
            });
        }
    }

    if found { CheckOutcome::Success } else { CheckOutcome::NoMatch }
}

// ── Goal → Action classification ───────────────────────────────

/// What automated action a goal maps to.
enum GoalAction {
    /// Check email inbox for messages matching the query.
    CheckEmail { query: String },
    /// Fetch a web page and check for updates.
    CheckWeb { url: String },
    /// Check if any reminders are due.
    CheckReminders,
    /// No automated action — goal is evaluated during conversation.
    NoAction,
}

/// Heuristic: match a goal's description to an automated action.
///
/// Uses keyword matching for now. In the future, this will use the LLM
/// to classify goals and select appropriate source checks.
fn match_goal_to_action(goal: &Goal) -> GoalAction {
    let desc = goal.description.to_lowercase();
    let criteria = goal.success_criteria.to_lowercase();

    // Email-related goals
    if desc.contains("email") || desc.contains("inbox") || desc.contains("mail") {
        // Use success criteria as the search query if available, otherwise the description
        let query = if criteria.is_empty() {
            goal.description.clone()
        } else {
            goal.success_criteria.clone()
        };
        return GoalAction::CheckEmail { query };
    }

    // Web monitoring — look for URLs in description or criteria
    if let Some(url) = extract_url(&goal.description).or_else(|| extract_url(&goal.success_criteria))
    {
        return GoalAction::CheckWeb { url };
    }

    // Price/stock monitoring without a URL — would need a search engine
    if desc.contains("price") || desc.contains("stock") || desc.contains("monitor") {
        // No URL to check — this is a NoAction until we add web search
        return GoalAction::NoAction;
    }

    // Reminder-related goals
    if desc.contains("remind") || desc.contains("schedule") || desc.contains("every") {
        return GoalAction::CheckReminders;
    }

    GoalAction::NoAction
}

/// Extract the first URL from a string.
fn extract_url(text: &str) -> Option<String> {
    for word in text.split_whitespace() {
        if word.starts_with("http://") || word.starts_with("https://") {
            // Clean trailing punctuation
            let clean = word.trim_end_matches([',', '.', ')', ']']);
            return Some(clean.to_string());
        }
    }
    None
}

/// Truncate a string to max_len characters.
fn truncate(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        s
    } else {
        &s[..s.floor_char_boundary(max_len)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── is_due tests ──────────────────────────────────────────

    #[test]
    fn is_due_fires_when_cron_matches() {
        // "every minute" cron — should always be due if last_run was > 60s ago
        let now = Utc::now();
        let last = now - chrono::Duration::seconds(120);
        assert!(is_due("* * * * *", Some(last), now));
    }

    #[test]
    fn is_due_does_not_fire_when_recently_run() {
        // Hourly cron — should not fire if last run was 30s ago
        let now = Utc::now();
        let last = now - chrono::Duration::seconds(30);
        assert!(!is_due("0 * * * *", Some(last), now));
    }

    #[test]
    fn is_due_fires_on_first_run() {
        // Never run before (None) — should fire for "every minute"
        let now = Utc::now();
        assert!(is_due("* * * * *", None, now));
    }

    #[test]
    fn is_due_rejects_invalid_cron() {
        let now = Utc::now();
        assert!(!is_due("not a cron", None, now));
    }

    #[test]
    fn is_due_hourly_not_due_within_hour() {
        // Hourly at minute 0. If we last ran 30 min ago, next occurrence
        // is still in the future (30 min from now).
        let now = Utc::now();
        let _last = now - chrono::Duration::minutes(30);
        // This may or may not be due depending on current minute — test with fixed time
        // Instead test: if last_run == now, definitely not due
        assert!(!is_due("0 * * * *", Some(now), now));
    }

    // ── schedule dedup tests ──────────────────────────────────

    #[test]
    fn schedule_last_run_prevents_refire() {
        // Simulate: schedule fired, then is_due should return false
        let now = Utc::now();
        // "every minute" — just fired
        assert!(!is_due("* * * * *", Some(now), now));
        // But 61 seconds later, it should be due again
        let later = now + chrono::Duration::seconds(61);
        assert!(is_due("* * * * *", Some(now), later));
    }

    // ── briefing guard tests ──────────────────────────────────

    #[test]
    fn briefing_time_check() {
        use crate::schedule::is_briefing_time;
        // We can't control the clock, but we can verify it returns bool
        // and doesn't panic for edge values
        let _ = is_briefing_time(0);
        let _ = is_briefing_time(23);
        let _ = is_briefing_time(12);
    }

    #[test]
    fn seconds_until_hour_returns_positive() {
        use crate::schedule::seconds_until_hour;
        // Should always be > 0 (either today or tomorrow)
        let secs = seconds_until_hour(3);
        assert!(secs > 0);
        assert!(secs <= 86400); // at most 24 hours
    }


    // ── interval floor test ───────────────────────────────────

    #[test]
    fn interval_floor_prevents_spin() {
        // The floor of .max(60) is applied in run_loop
        let interval_minutes = 0u32;
        let interval_secs = (interval_minutes as u64 * 60).max(60);
        assert_eq!(interval_secs, 60);

        let interval_minutes = 1u32;
        let interval_secs = (interval_minutes as u64 * 60).max(60);
        assert_eq!(interval_secs, 60);

        let interval_minutes = 15u32;
        let interval_secs = (interval_minutes as u64 * 60).max(60);
        assert_eq!(interval_secs, 900);
    }

    // ── notification kind tests ───────────────────────────────

    #[test]
    fn notification_serialization_round_trip() {
        let notif = Notification {
            id: "test-1".into(),
            kind: NotificationKind::ActionTaken,
            title: "Schedule done".into(),
            body: "Result of the schedule".into(),
            source: "schedule".into(),
            timestamp: Utc::now(),
            requires_approval: false,
            goal_id: Some("goal-123".into()),
        };

        let json = serde_json::to_string(&notif).unwrap();
        let deserialized: Notification = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, "test-1");
        assert_eq!(deserialized.title, "Schedule done");
        assert_eq!(deserialized.goal_id, Some("goal-123".into()));
    }

    // ── truncate tests ────────────────────────────────────────

    #[test]
    fn truncate_short_string() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_string() {
        assert_eq!(truncate("hello world", 5), "hello");
    }

    #[test]
    fn truncate_unicode_safe() {
        // Should not panic on multi-byte chars
        let s = "café résumé";
        let t = truncate(s, 5);
        assert!(t.len() <= 5);
        assert!(t.is_char_boundary(t.len()));
    }

    // ── extract_url tests ─────────────────────────────────────

    #[test]
    fn extract_url_from_text() {
        assert_eq!(
            extract_url("Check https://example.com/status for updates"),
            Some("https://example.com/status".into())
        );
    }

    #[test]
    fn extract_url_strips_trailing_punctuation() {
        assert_eq!(
            extract_url("Visit https://example.com."),
            Some("https://example.com".into())
        );
        assert_eq!(
            extract_url("(see https://example.com)"),
            Some("https://example.com".into())
        );
    }

    #[test]
    fn extract_url_returns_none_for_no_url() {
        assert_eq!(extract_url("no url here"), None);
    }

    // ── goal action classification tests ──────────────────────

    #[test]
    fn goal_classifies_email() {
        use aivyx_brain::{Goal, Priority};
        use aivyx_core::GoalId;
        let goal = Goal {
            id: GoalId::new(),
            description: "Monitor my email for invoices".into(),
            priority: Priority::Medium,
            status: GoalStatus::Active,
            parent: None,
            success_criteria: "from:accounting".into(),
            progress: 0.0,
            tags: vec![],
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deadline: None,
            failure_count: 0,
            consecutive_failures: 0,
            cooldown_until: None,
        };
        match match_goal_to_action(&goal) {
            GoalAction::CheckEmail { query } => {
                assert_eq!(query, "from:accounting");
            }
            other => panic!("Expected CheckEmail, got {:?}", std::mem::discriminant(&other)),
        }
    }

    #[test]
    fn goal_classifies_web_url() {
        use aivyx_brain::{Goal, Priority};
        use aivyx_core::GoalId;
        let goal = Goal {
            id: GoalId::new(),
            description: "Track price at https://shop.example.com/widget".into(),
            priority: Priority::Low,
            status: GoalStatus::Active,
            parent: None,
            success_criteria: String::new(),
            progress: 0.0,
            tags: vec![],
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deadline: None,
            failure_count: 0,
            consecutive_failures: 0,
            cooldown_until: None,
        };
        match match_goal_to_action(&goal) {
            GoalAction::CheckWeb { url } => {
                assert_eq!(url, "https://shop.example.com/widget");
            }
            other => panic!("Expected CheckWeb, got {:?}", std::mem::discriminant(&other)),
        }
    }

    #[test]
    fn goal_classifies_reminder() {
        use aivyx_brain::{Goal, Priority};
        use aivyx_core::GoalId;
        let goal = Goal {
            id: GoalId::new(),
            description: "Remind me to call the dentist every week".into(),
            priority: Priority::Low,
            status: GoalStatus::Active,
            parent: None,
            success_criteria: String::new(),
            progress: 0.0,
            tags: vec![],
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deadline: None,
            failure_count: 0,
            consecutive_failures: 0,
            cooldown_until: None,
        };
        assert!(matches!(match_goal_to_action(&goal), GoalAction::CheckReminders));
    }

    // ── InteractionSignals tests ──────────────────────────────

    #[test]
    fn interaction_signals_record_message_updates_counters() {
        let mut s = InteractionSignals::new();
        assert_eq!(s.message_count_session, 0);
        assert_eq!(s.avg_message_length(), 0.0);

        s.record_message("Hello, world!");
        assert_eq!(s.message_count_session, 1);
        assert!(s.last_user_message_at.is_some());
        assert!((s.avg_message_length() - 13.0).abs() < 0.1);
    }

    #[test]
    fn interaction_signals_short_message_streak() {
        let mut s = InteractionSignals::new();
        s.record_message("ok");
        assert_eq!(s.short_message_streak, 1);
        s.record_message("yes");
        assert_eq!(s.short_message_streak, 2);
        s.record_message("This is a much longer message that should reset the streak");
        assert_eq!(s.short_message_streak, 0);
    }

    #[test]
    fn interaction_signals_negative_keywords() {
        let mut s = InteractionSignals::new();
        s.record_message("This is fine");
        assert_eq!(s.negative_keyword_count, 0);
        s.record_message("I'm so frustrated with this broken thing");
        assert_eq!(s.negative_keyword_count, 1); // one hit per message
        s.record_message("ugh");
        assert_eq!(s.negative_keyword_count, 2);
    }

    #[test]
    fn interaction_signals_notification_tracking() {
        let mut s = InteractionSignals::new();
        s.record_notification_sent();
        s.record_notification_sent();
        assert_eq!(s.notifications_sent_this_hour, 2);
        assert_eq!(s.notifications_sent_today, 2);
        assert!(s.last_notification_at.is_some());
    }

    #[test]
    fn interaction_signals_idle_minutes() {
        let s = InteractionSignals::new();
        assert!(s.idle_minutes().is_none()); // no messages yet

        let mut s2 = InteractionSignals::new();
        s2.record_message("hello");
        // Just sent a message, idle should be 0
        assert_eq!(s2.idle_minutes(), Some(0));
    }

    // ── MoodSignal tests ──────────────────────────────────────

    #[test]
    fn mood_neutral_with_few_messages() {
        let mut s = InteractionSignals::new();
        s.record_message("hi");
        s.record_message("ok");
        // Only 2 messages — not enough for non-Neutral
        assert_eq!(MoodSignal::estimate(&s), MoodSignal::Neutral);
    }

    #[test]
    fn mood_frustrated_on_short_streak() {
        let mut s = InteractionSignals::new();
        for msg in &["ok", "no", "why", "fix it", "ugh"] {
            s.record_message(msg);
        }
        // 5 messages, short streak of 5, negative keyword "ugh"
        assert_eq!(MoodSignal::estimate(&s), MoodSignal::Frustrated);
    }

    #[test]
    fn mood_frustrated_on_negative_keywords() {
        let mut s = InteractionSignals::new();
        s.record_message("This is working perfectly fine for me");
        s.record_message("I appreciate the detailed explanation");
        s.record_message("This is broken and I'm frustrated");
        assert_eq!(MoodSignal::estimate(&s), MoodSignal::Frustrated);
    }

    #[test]
    fn mood_focused_on_long_messages() {
        let mut s = InteractionSignals::new();
        let long_msg = "a".repeat(150);
        s.record_message(&long_msg);
        s.record_message(&long_msg);
        s.record_message(&long_msg);
        assert_eq!(MoodSignal::estimate(&s), MoodSignal::Focused);
    }

    // ── ResourceBudget tests ──────────────────────────────────

    #[test]
    fn resource_budget_tracks_tokens() {
        let mut rb = ResourceBudget::new(100_000);
        rb.record_tokens(5000, true);
        assert_eq!(rb.tokens_used_today, 5000);
        assert_eq!(rb.heartbeat_tokens_today, 5000);
        rb.record_tokens(3000, false);
        assert_eq!(rb.tokens_used_today, 8000);
        assert_eq!(rb.heartbeat_tokens_today, 5000);
    }

    #[test]
    fn resource_budget_fraction() {
        let mut rb = ResourceBudget::new(100_000);
        rb.record_tokens(60_000, false);
        assert!((rb.budget_fraction() - 0.6).abs() < 0.01);
    }

    #[test]
    fn resource_budget_unlimited_returns_zero() {
        let rb = ResourceBudget::new(0);
        assert_eq!(rb.budget_fraction(), 0.0);
    }

    #[test]
    fn resource_budget_context_hidden_when_low_usage() {
        let mut rb = ResourceBudget::new(100_000);
        rb.record_tokens(1000, false); // 1% — should not surface
        assert!(rb.format_for_prompt().is_none());
    }

    #[test]
    fn resource_budget_context_shows_when_high_usage() {
        let mut rb = ResourceBudget::new(100_000);
        rb.record_tokens(60_000, false); // 60% — should surface
        let text = rb.format_for_prompt();
        assert!(text.is_some());
        assert!(text.unwrap().contains("60%"));
    }
}
