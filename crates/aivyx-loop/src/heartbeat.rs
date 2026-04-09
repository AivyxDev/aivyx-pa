//! Heartbeat — periodic LLM-driven autonomous reasoning.
//!
//! The heartbeat is the cognitive core of the agent loop. On each tick it:
//! 1. Gathers context from available sources (goals, reminders, email, self-model)
//! 2. Checks if anything has changed since the last beat (context-aware skip)
//! 3. Presents context to the LLM and asks it to decide what actions to take
//! 4. Dispatches actions (notifications, goal updates, reflection, consolidation)
//!
//! When nothing has changed, the LLM call is skipped entirely — zero token
//! cost on quiet periods. This makes the heartbeat cheap to run frequently.

use std::sync::Arc;

use aivyx_brain::{BrainStore, Goal, GoalFilter, GoalStatus, SelfModel};
use aivyx_crypto::MasterKey;
use aivyx_llm::{ChatMessage, ChatRequest};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::briefing::sanitize_for_prompt;
use crate::{HeartbeatConfig, LoopContext, Notification, NotificationKind};

// ── Context gathering ──────────────────────────────────────────

/// A named section of context gathered for the heartbeat prompt.
#[derive(Debug, Clone)]
struct ContextSection {
    label: String,
    body: String,
}

/// Gathered context from all configured sources.
#[derive(Debug, Default)]
pub struct HeartbeatContext {
    sections: Vec<ContextSection>,
}

/// Raw data fetched during context gathering, reused by priority scoring
/// to avoid duplicate store reads.
#[derive(Debug, Default)]
pub struct GatheredData {
    pub goals: Vec<Goal>,
    pub reminders: Vec<aivyx_actions::reminders::Reminder>,
    pub bills: Vec<aivyx_actions::finance::Transaction>,
    pub over_budget: Vec<(String, i64, i64)>,
}

impl HeartbeatContext {
    fn add(&mut self, label: impl Into<String>, body: impl Into<String>) {
        let body = body.into();
        if !body.is_empty() {
            self.sections.push(ContextSection {
                label: label.into(),
                body,
            });
        }
    }

    /// True when no sources contributed any context — LLM call can be skipped.
    pub fn is_empty(&self) -> bool {
        self.sections.is_empty()
    }

    /// Number of context sections gathered.
    pub fn section_count(&self) -> usize {
        self.sections.len()
    }

    /// Format all sections into a single block for the prompt.
    fn format_for_prompt(&self) -> String {
        self.sections
            .iter()
            .map(|s| format!("## {}\n{}", s.label, s.body))
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

/// Gather context from all configured sources, off the async runtime.
///
/// The redb store reads are synchronous I/O — running them on
/// `spawn_blocking` keeps the async runtime thread free.
pub async fn gather_context(
    config: &HeartbeatConfig,
    ctx: &LoopContext,
) -> (HeartbeatContext, GatheredData) {
    let config = config.clone();
    let brain_store = Arc::clone(&ctx.brain_store);
    let brain_key_bytes = ctx.brain_key.expose_secret().to_vec();
    let reminder_store = Arc::clone(&ctx.reminder_store);
    let reminder_key_bytes = ctx.reminder_key.expose_secret().to_vec();
    let finance_key_bytes = ctx.finance.as_ref().map(|f| f.key.expose_secret().to_vec());
    let contacts_key_bytes = ctx
        .contacts
        .as_ref()
        .map(|c| c.key.expose_secret().to_vec());
    let schedule_last_run = ctx.schedule_last_run.clone();
    let check_contacts = ctx.contacts.is_some();
    let has_email = ctx.email_config.is_some();

    tokio::task::spawn_blocking(move || {
        let brain_key = MasterKey::from_bytes(
            brain_key_bytes
                .try_into()
                .expect("brain key must be 32 bytes"),
        );
        let reminder_key = MasterKey::from_bytes(
            reminder_key_bytes
                .try_into()
                .expect("reminder key must be 32 bytes"),
        );
        let finance_key = finance_key_bytes
            .map(|b| MasterKey::from_bytes(b.try_into().expect("finance key must be 32 bytes")));
        let contacts_key = contacts_key_bytes
            .map(|b| MasterKey::from_bytes(b.try_into().expect("contacts key must be 32 bytes")));

        gather_context_sync(
            &config,
            &brain_store,
            &brain_key,
            &reminder_store,
            &reminder_key,
            finance_key.as_ref(),
            contacts_key.as_ref(),
            &schedule_last_run,
            check_contacts,
            has_email,
        )
    })
    .await
    .expect("gather_context_sync panicked")
}

/// Synchronous inner implementation of context gathering.
#[allow(clippy::too_many_arguments)]
fn gather_context_sync(
    config: &HeartbeatConfig,
    brain_store: &BrainStore,
    brain_key: &MasterKey,
    reminder_store: &aivyx_crypto::EncryptedStore,
    reminder_key: &MasterKey,
    finance_key: Option<&MasterKey>,
    contacts_key: Option<&MasterKey>,
    schedule_last_run: &std::collections::HashMap<String, chrono::DateTime<Utc>>,
    check_contacts: bool,
    has_email: bool,
) -> (HeartbeatContext, GatheredData) {
    let mut hb_ctx = HeartbeatContext::default();
    let mut data = GatheredData::default();

    // Active goals with progress
    if config.check_goals
        && let Ok(goals) = brain_store.list_goals(
            &GoalFilter {
                status: Some(GoalStatus::Active),
                ..Default::default()
            },
            brain_key,
        )
    {
        if !goals.is_empty() {
            hb_ctx.add("Active Goals", format_goals(&goals));
        }
        data.goals = goals;
    }

    // Due/pending reminders
    if config.check_reminders {
        match aivyx_actions::reminders::load_all_reminders(reminder_store, reminder_key) {
            Ok(reminders) => {
                let pending: Vec<_> = reminders.into_iter().filter(|r| !r.completed).collect();
                if !pending.is_empty() {
                    let now = Utc::now();
                    let lines: Vec<String> = pending
                        .iter()
                        .map(|r| {
                            let status = if r.due_at <= now {
                                "OVERDUE"
                            } else {
                                "pending"
                            };
                            format!(
                                "- [{}] {} (due {})",
                                status,
                                sanitize_for_prompt(&r.message),
                                r.due_at.format("%b %d %H:%M")
                            )
                        })
                        .collect();
                    hb_ctx.add("Reminders", lines.join("\n"));
                }
                data.reminders = pending;
            }
            Err(e) => tracing::debug!("Heartbeat: reminder check failed: {e}"),
        }
    }

    // Recent email subjects — async, handled separately in run_heartbeat_tick.
    if config.check_email && has_email {
        // Intentionally empty for sync gather.
    }

    // Recent schedule results
    if config.check_schedules && !schedule_last_run.is_empty() {
        let now = Utc::now();
        let recent: Vec<String> = schedule_last_run
            .iter()
            .filter(|(_, fired_at)| now.signed_duration_since(**fired_at).num_minutes() < 60)
            .map(|(name, fired_at)| format!("- '{}' ran at {}", name, fired_at.format("%H:%M")))
            .collect();

        if !recent.is_empty() {
            hb_ctx.add("Recent Schedule Activity", recent.join("\n"));
        }
    }

    // Self-model summary
    if config.check_self_model
        && let Ok(Some(model)) = brain_store.load_self_model(brain_key)
    {
        hb_ctx.add("Self-Model", format_self_model(&model));
    }

    // Upcoming bills and budget alerts
    if config.check_finance
        && let Some(fkey) = finance_key
    {
        if let Ok(bills) = aivyx_actions::finance::upcoming_bills(reminder_store, fkey, 7) {
            if !bills.is_empty() {
                let lines: Vec<String> = bills
                    .iter()
                    .map(|b| {
                        let amount = aivyx_actions::finance::format_dollars(b.amount_cents);
                        let desc = sanitize_for_prompt(&b.description);
                        match &b.due_date {
                            Some(d) => format!("- {} — {} due {}", desc, amount, d.format("%b %d")),
                            None => format!("- {} — {}", desc, amount),
                        }
                    })
                    .collect();
                hb_ctx.add("Upcoming Bills", lines.join("\n"));
            }
            data.bills = bills;
        }
        if let Ok(cats) = aivyx_actions::finance::over_budget_categories(reminder_store, fkey) {
            if !cats.is_empty() {
                let lines: Vec<String> = cats
                    .iter()
                    .map(|(cat, spent, limit)| {
                        format!(
                            "- {}: {} spent (limit: {})",
                            cat,
                            aivyx_actions::finance::format_dollars(*spent),
                            aivyx_actions::finance::format_dollars(*limit),
                        )
                    })
                    .collect();
                hb_ctx.add("Budget Alerts", lines.join("\n"));
            }
            data.over_budget = cats;
        }
    }

    // Contact count (lightweight — just the count for awareness)
    if check_contacts
        && let Some(ckey) = contacts_key
        && let Ok(all) = aivyx_actions::contacts::load_all_contacts(reminder_store, ckey)
        && !all.is_empty()
    {
        hb_ctx.add(
            "Contacts",
            format!("{} contacts in address book", all.len()),
        );
    }

    (hb_ctx, data)
}

/// Format goals for the heartbeat prompt.
fn format_goals(goals: &[Goal]) -> String {
    goals
        .iter()
        .map(|g| {
            let cooldown = if g.is_in_cooldown() {
                " [COOLDOWN]"
            } else {
                ""
            };
            let failures = if g.consecutive_failures > 0 {
                format!(" ({}x failed)", g.consecutive_failures)
            } else {
                String::new()
            };
            format!(
                "- [{:.0}%] {}{}{}\n  Criteria: {}",
                g.progress * 100.0,
                g.description,
                cooldown,
                failures,
                g.success_criteria,
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Format self-model for the heartbeat prompt.
fn format_self_model(model: &SelfModel) -> String {
    let mut lines = Vec::new();

    if !model.strengths.is_empty() {
        lines.push(format!("Strengths: {}", model.strengths.join(", ")));
    }
    if !model.weaknesses.is_empty() {
        lines.push(format!("Weaknesses: {}", model.weaknesses.join(", ")));
    }
    if !model.domain_confidence.is_empty() {
        let domains: Vec<String> = model
            .domain_confidence
            .iter()
            .map(|(d, c)| format!("{d}: {:.0}%", c * 100.0))
            .collect();
        lines.push(format!("Domain confidence: {}", domains.join(", ")));
    }
    if !model.tool_proficiency.is_empty() {
        let tools: Vec<String> = model
            .tool_proficiency
            .iter()
            .map(|(t, p)| format!("{t}: {:.0}%", p * 100.0))
            .collect();
        lines.push(format!("Tool proficiency: {}", tools.join(", ")));
    }

    lines.push(format!(
        "Last updated: {}",
        model.updated_at.format("%Y-%m-%d %H:%M")
    ));

    lines.join("\n")
}

// ── Heartbeat actions ──────────────────────────────────────────

/// Actions the LLM can request during a heartbeat tick.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action")]
pub enum HeartbeatAction {
    /// Store a notification for the user to see next session.
    #[serde(rename = "notify")]
    Notify {
        title: String,
        body: String,
        #[serde(default)]
        urgent: bool,
    },
    /// Create a new brain goal.
    #[serde(rename = "set_goal")]
    SetGoal {
        description: String,
        success_criteria: String,
    },
    /// Update an existing goal's progress or status.
    #[serde(rename = "update_goal")]
    UpdateGoal {
        /// Substring match against goal descriptions.
        goal_match: String,
        #[serde(default)]
        progress: Option<f32>,
        #[serde(default)]
        status: Option<String>,
    },
    /// Reflect on performance and update self-model.
    #[serde(rename = "reflect")]
    Reflect {
        #[serde(default)]
        add_strengths: Vec<String>,
        #[serde(default)]
        add_weaknesses: Vec<String>,
        #[serde(default)]
        remove_strengths: Vec<String>,
        #[serde(default)]
        remove_weaknesses: Vec<String>,
        #[serde(default)]
        domain_confidence: std::collections::HashMap<String, f32>,
    },
    /// Trigger memory consolidation.
    #[serde(rename = "consolidate_memory")]
    ConsolidateMemory,
    /// Proactive suggestion based on cross-source correlation.
    #[serde(rename = "suggest")]
    Suggest {
        title: String,
        body: String,
        /// Sources that contributed to this suggestion (e.g., ["email", "calendar"]).
        #[serde(default)]
        sources: Vec<String>,
        /// Priority: low, normal, high, urgent.
        #[serde(default = "default_priority")]
        priority: String,
    },
    /// Analyze a failure and store a post-mortem reflection.
    #[serde(rename = "analyze_failure")]
    AnalyzeFailure {
        /// What failed (goal description, task summary, or tool name).
        subject: String,
        /// Root cause analysis from the LLM.
        root_cause: String,
        /// What should be done differently next time.
        remediation: String,
        /// Domain affected (scope discriminant for confidence adjustment).
        #[serde(default)]
        domain: Option<String>,
    },
    /// Extract structured knowledge triples from the current context.
    #[serde(rename = "extract_knowledge")]
    ExtractKnowledge {
        /// Triples extracted from the heartbeat context.
        triples: Vec<ExtractedTriple>,
    },
    /// Prune audit log entries older than the configured retention period.
    #[serde(rename = "prune_audit")]
    PruneAudit,
    /// Create an encrypted backup of the data directory.
    #[serde(rename = "backup")]
    Backup,
    // ── Phase 6: Smarter Agent actions ─────────────────────────
    /// Organize goals into time horizons and suggest adjustments.
    #[serde(rename = "plan_review")]
    PlanReview {
        /// Goals organized by horizon: "today", "week", "month", "quarter".
        #[serde(default)]
        horizons: std::collections::HashMap<String, Vec<String>>,
        /// Identified planning gaps (e.g., "no goals for this quarter").
        #[serde(default)]
        gaps: Vec<String>,
        /// Suggested tag/deadline changes to existing goals.
        #[serde(default)]
        adjustments: Vec<PlanAdjustment>,
    },
    /// Weekly strategy review — deeper reflection on patterns and approach.
    #[serde(rename = "strategy_review")]
    StrategyReview {
        /// Summary of the review period.
        period_summary: String,
        /// Number of goals completed in the period.
        #[serde(default)]
        goals_completed: u32,
        /// Goal descriptions that have stalled.
        #[serde(default)]
        goals_stalled: Vec<String>,
        /// Patterns observed (e.g., "consistently failing at X").
        #[serde(default)]
        patterns: Vec<String>,
        /// Strategic adjustments recommended.
        #[serde(default)]
        strategic_adjustments: Vec<String>,
        /// Self-model confidence updates to apply.
        #[serde(default)]
        domain_confidence_updates: std::collections::HashMap<String, f32>,
    },
    /// Acknowledge observed user mood (informational).
    #[serde(rename = "track_mood")]
    TrackMood {
        /// The mood observed (e.g., "frustrated", "focused").
        observed_mood: String,
        /// How the agent is adjusting (e.g., "reducing notifications").
        adjustment: String,
    },
    /// Celebrate a completed goal or achievement streak.
    #[serde(rename = "encourage")]
    Encourage {
        /// What was achieved.
        achievement: String,
        /// The congratulatory message.
        message: String,
        /// Current streak count, if applicable.
        #[serde(default)]
        streak: Option<u32>,
    },
    /// No action needed — everything is fine.
    #[serde(rename = "no_action")]
    NoAction {
        #[serde(default)]
        reason: String,
    },
}

/// A suggested adjustment to an existing goal's tags or deadline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanAdjustment {
    /// Substring match against goal descriptions.
    pub goal_match: String,
    /// Tags to set on the matched goal (replaces existing horizon tags).
    #[serde(default)]
    pub set_tags: Option<Vec<String>>,
    /// Deadline to set (ISO 8601 date or datetime string).
    #[serde(default)]
    pub set_deadline: Option<String>,
    /// Why this adjustment is recommended.
    #[serde(default)]
    pub reasoning: String,
}

/// A knowledge triple extracted by the LLM during heartbeat or on demand.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedTriple {
    /// The entity this fact is about (e.g., "Alice", "Project Orion").
    pub subject: String,
    /// The relationship or attribute (e.g., "works_at", "deadline_is", "prefers").
    pub predicate: String,
    /// The value or target entity (e.g., "Acme Corp", "2026-04-15", "dark mode").
    pub object: String,
    /// Confidence in this extraction (0.0–1.0). Default: 0.8.
    #[serde(default = "default_extraction_confidence")]
    pub confidence: f32,
}

fn default_extraction_confidence() -> f32 {
    0.8
}

fn default_priority() -> String {
    "normal".into()
}

/// Parsed response from the LLM's heartbeat reasoning.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatResponse {
    /// Brief reasoning about what the agent observed.
    #[serde(default)]
    pub reasoning: String,
    /// Actions to execute.
    #[serde(default)]
    pub actions: Vec<HeartbeatAction>,
}

// ── Prompt building ────────────────────────────────────────────

/// Build the heartbeat prompt from gathered context.
pub fn build_heartbeat_prompt(
    config: &HeartbeatConfig,
    context: &HeartbeatContext,
    priority_summary: Option<&str>,
) -> String {
    let mut prompt = String::from(
        "You are running a periodic heartbeat — a moment to reflect on your current state \
         and decide if any autonomous action is needed.\n\n\
         Review the context below and decide what to do. You may take zero or more actions.\n\n\
         CROSS-SOURCE REASONING: Look for connections across different sources. For example:\n\
         - If someone emailed about a topic AND there's a meeting with them soon, connect those facts.\n\
         - If a bill is due AND the user has a budget alert for that category, highlight both.\n\
         - If a goal has stalled AND there are related calendar events or emails, suggest action.\n\
         - If an email has gone unanswered for days, suggest a follow-up.\n\
         Correlate information from email, calendar, contacts, finance, goals, and reminders \
         to generate proactive suggestions that the user wouldn't get from any single source alone.\n\n",
    );

    prompt.push_str(&context.format_for_prompt());

    if let Some(priorities) = priority_summary
        && !priorities.is_empty()
    {
        prompt.push_str("\n\n## Priority Summary (ranked by urgency)\n");
        prompt.push_str(priorities);
        prompt.push('\n');
    }

    prompt.push_str("\n\n## Available Actions\n");
    prompt.push_str(
        "Respond with a JSON object containing `reasoning` (brief) and `actions` (array).\n\n",
    );

    if config.can_notify {
        prompt.push_str("- `{\"action\": \"notify\", \"title\": \"...\", \"body\": \"...\", \"urgent\": false}` — Store a notification for the user\n");
    }
    if config.can_manage_goals {
        prompt.push_str("- `{\"action\": \"set_goal\", \"description\": \"...\", \"success_criteria\": \"...\"}` — Create a new goal\n");
        prompt.push_str("- `{\"action\": \"update_goal\", \"goal_match\": \"...\", \"progress\": 0.5, \"status\": \"completed\"}` — Update a goal (status: active/dormant/completed/abandoned)\n");
    }
    if config.can_reflect {
        prompt.push_str("- `{\"action\": \"reflect\", \"add_strengths\": [...], \"domain_confidence\": {\"domain\": 0.8}}` — Update self-model\n");
    }
    if config.can_consolidate_memory {
        prompt
            .push_str("- `{\"action\": \"consolidate_memory\"}` — Trigger memory consolidation\n");
    }
    if config.can_suggest {
        prompt.push_str("- `{\"action\": \"suggest\", \"title\": \"...\", \"body\": \"...\", \"sources\": [\"email\", \"calendar\"], \"priority\": \"normal\"}` — Proactive suggestion from cross-source correlation\n");
    }
    if config.can_analyze_failures {
        prompt.push_str("- `{\"action\": \"analyze_failure\", \"subject\": \"what failed\", \"root_cause\": \"why it failed\", \"remediation\": \"what to change\", \"domain\": \"email\"}` — Post-mortem on a failed goal/task (domain is optional scope discriminant)\n");
    }
    if config.can_extract_knowledge {
        prompt.push_str(
            "- `{\"action\": \"extract_knowledge\", \"triples\": [{\"subject\": \"entity\", \"predicate\": \"relationship\", \"object\": \"value\", \"confidence\": 0.9}, ...]}` — Extract structured facts from context into the knowledge graph. Use clear entity names and consistent predicates (e.g., works_at, has_meeting_with, deadline_is, prefers, located_in, email_is). Only extract facts you are confident about.\n"
        );
    }
    if config.can_backup {
        prompt.push_str("- `{\"action\": \"backup\"}` — Create an encrypted backup of the data directory (use sparingly, e.g., daily or weekly)\n");
    }
    if config.can_prune_audit {
        prompt.push_str("- `{\"action\": \"prune_audit\"}` — Remove audit entries older than the retention period (use sparingly, e.g., weekly)\n");
    }
    if config.can_plan_review {
        prompt.push_str(
            "- `{\"action\": \"plan_review\", \"horizons\": {\"today\": [...], \"week\": [...], \"month\": [...], \"quarter\": [...]}, \"gaps\": [...], \"adjustments\": [{\"goal_match\": \"...\", \"set_tags\": [\"horizon:week\"], \"set_deadline\": \"2026-04-11\", \"reasoning\": \"...\"}]}` — Organize goals into time horizons and suggest tag/deadline adjustments. Use horizon tags: horizon:today, horizon:week, horizon:month, horizon:quarter.\n"
        );
    }
    if config.can_strategy_review {
        prompt.push_str(
            "- `{\"action\": \"strategy_review\", \"period_summary\": \"...\", \"goals_completed\": 3, \"goals_stalled\": [...], \"patterns\": [...], \"strategic_adjustments\": [...], \"domain_confidence_updates\": {\"email\": 0.8}}` — Weekly strategy review: analyze patterns, identify stalled goals, recommend strategic changes and update domain confidence.\n"
        );
    }
    if config.can_track_mood {
        prompt.push_str(
            "- `{\"action\": \"track_mood\", \"observed_mood\": \"frustrated\", \"adjustment\": \"reducing notification frequency\"}` — Acknowledge observed user mood and how you are adapting\n"
        );
    }
    if config.can_encourage {
        prompt.push_str(
            "- `{\"action\": \"encourage\", \"achievement\": \"Completed 3 goals this week\", \"message\": \"...\", \"streak\": 3}` — Celebrate a completed goal or streak. Calibrate warmth to persona: coach/companion=enthusiastic, ops/analyst=brief metrics, coder/researcher=note and suggest next steps.\n"
        );
    }
    prompt.push_str(
        "- `{\"action\": \"no_action\", \"reason\": \"...\"}` — Nothing to do right now\n",
    );

    // Calibrate proactivity based on how many autonomous capabilities are enabled.
    // More `can_*` flags = user wants a more active heartbeat.
    let enabled_caps = [
        config.can_notify,
        config.can_manage_goals,
        config.can_reflect,
        config.can_consolidate_memory,
        config.can_suggest,
        config.can_analyze_failures,
        config.can_extract_knowledge,
        config.can_backup,
        config.can_prune_audit,
        config.can_plan_review,
        config.can_strategy_review,
        config.can_track_mood,
        config.can_encourage,
        config.can_track_milestones,
    ]
    .iter()
    .filter(|&&x| x)
    .count();

    if enabled_caps >= 10 {
        prompt.push_str(
            "\nYou have broad autonomous capabilities enabled. Be PROACTIVE — look for \
             opportunities to act, optimize, suggest, and maintain. Take initiative. \
             The user has given you many capabilities because they want you to use them. \
             Only use `no_action` when genuinely nothing needs attention.\n",
        );
    } else if enabled_caps >= 5 {
        prompt.push_str(
            "\nYou have moderate autonomy. Act when there's a clear reason — surface insights, \
             track goals, consolidate knowledge. Don't be passive, but focus on high-value actions. \
             Use `no_action` when nothing meaningful has changed.\n"
        );
    } else {
        prompt.push_str(
            "\nBe conservative — only act when there's a clear and important reason. \
             Prefer observing and noting over taking action. Use `no_action` when in doubt.\n",
        );
    }

    prompt.push_str("Prefer `suggest` over `notify` when the insight comes from correlating multiple sources.\n");
    prompt.push_str("\nRespond with ONLY valid JSON, no markdown fences.");

    prompt
}

// ── Action dispatch ────────────────────────────────────────────

/// Parse the LLM response into a HeartbeatResponse.
pub fn parse_response(text: &str) -> HeartbeatResponse {
    // Use the shared JSON extractor which handles markdown fences, surrounding
    // prose, and balanced braces — more robust than simple trim.
    let json_str = crate::extract_json_object(text).unwrap_or_else(|| text.trim());

    match serde_json::from_str::<HeartbeatResponse>(json_str) {
        Ok(resp) => resp,
        Err(e) => {
            tracing::warn!("Heartbeat: failed to parse LLM response: {e}");
            tracing::debug!("Heartbeat: raw response: {json_str}");
            HeartbeatResponse {
                reasoning: "Failed to parse response".into(),
                actions: vec![HeartbeatAction::NoAction {
                    reason: format!("Parse error: {e}"),
                }],
            }
        }
    }
}

/// Execute a list of heartbeat actions.
pub async fn dispatch_actions(
    actions: &[HeartbeatAction],
    config: &HeartbeatConfig,
    ctx: &mut LoopContext,
    tx: &mpsc::Sender<Notification>,
) {
    for action in actions {
        match action {
            HeartbeatAction::Notify {
                title,
                body,
                urgent,
            } if config.can_notify => {
                let kind = if *urgent {
                    NotificationKind::Urgent
                } else {
                    NotificationKind::Info
                };
                crate::send_notification(
                    tx,
                    Notification {
                        id: uuid::Uuid::new_v4().to_string(),
                        kind,
                        title: title.clone(),
                        body: body.clone(),
                        source: "heartbeat".into(),
                        timestamp: Utc::now(),
                        requires_approval: false,
                        goal_id: None,
                    },
                );
                tracing::info!("Heartbeat: stored notification '{title}'");

                // Forward urgent notifications to messaging channels (fire-and-forget)
                if *urgent && let Some(ref msg) = ctx.messaging {
                    if let Some(ref tc) = msg.telegram {
                        let tc = tc.clone();
                        let t = title.clone();
                        let b = body.clone();
                        tokio::spawn(async move {
                            if let Err(e) =
                                aivyx_actions::messaging::telegram::forward_notification(
                                    &tc, &t, &b,
                                )
                                .await
                            {
                                tracing::warn!("Telegram notification forward failed: {e}");
                            }
                        });
                    }
                    if let Some(ref mc) = msg.matrix {
                        let mc = mc.clone();
                        let t = title.clone();
                        let b = body.clone();
                        tokio::spawn(async move {
                            if let Err(e) =
                                aivyx_actions::messaging::matrix::forward_notification(&mc, &t, &b)
                                    .await
                            {
                                tracing::warn!("Matrix notification forward failed: {e}");
                            }
                        });
                    }
                    if let Some(ref sc) = msg.signal {
                        let sc = sc.clone();
                        let t = title.clone();
                        let b = body.clone();
                        tokio::spawn(async move {
                            if let Err(e) =
                                aivyx_actions::messaging::signal::forward_notification(&sc, &t, &b)
                                    .await
                            {
                                tracing::warn!("Signal notification forward failed: {e}");
                            }
                        });
                    }
                    if let Some(ref sc) = msg.sms {
                        let sc = sc.clone();
                        let t = title.clone();
                        let b = body.clone();
                        tokio::spawn(async move {
                            if let Err(e) =
                                aivyx_actions::messaging::sms::forward_notification(&sc, &t, &b)
                                    .await
                            {
                                tracing::warn!("SMS notification forward failed: {e}");
                            }
                        });
                    }
                }
            }

            HeartbeatAction::SetGoal {
                description,
                success_criteria,
            } if config.can_manage_goals => {
                use aivyx_brain::Priority;
                let goal = Goal::new(description.clone(), success_criteria.clone())
                    .with_priority(Priority::Medium);
                if let Err(e) = ctx.brain_store.upsert_goal(&goal, &ctx.brain_key) {
                    tracing::warn!("Heartbeat: failed to set goal: {e}");
                } else {
                    tracing::info!("Heartbeat: created goal '{description}'");
                }
            }

            HeartbeatAction::UpdateGoal {
                goal_match,
                progress,
                status,
            } if config.can_manage_goals => {
                dispatch_update_goal(goal_match, progress, status, ctx);
            }

            HeartbeatAction::Reflect {
                add_strengths,
                add_weaknesses,
                remove_strengths,
                remove_weaknesses,
                domain_confidence,
            } if config.can_reflect => {
                dispatch_reflect(
                    add_strengths,
                    add_weaknesses,
                    remove_strengths,
                    remove_weaknesses,
                    domain_confidence,
                    ctx,
                );
            }

            HeartbeatAction::ConsolidateMemory if config.can_consolidate_memory => {
                let notif_id = uuid::Uuid::new_v4().to_string();
                crate::send_notification(tx, crate::Notification {
                    id: notif_id.clone(),
                    kind: crate::NotificationKind::ApprovalNeeded,
                    title: "Action requires approval".into(),
                    body: "The agent wants to consolidate memory clusters (deletes stale memories).".into(),
                    source: "heartbeat(consolidate_memory)".into(),
                    timestamp: Utc::now(),
                    requires_approval: true,
                    goal_id: None,
                });

                match crate::await_approval(ctx, &notif_id, std::time::Duration::from_secs(120))
                    .await
                {
                    Some(resp) if resp.approved => {
                        crate::emit_audit(
                            ctx,
                            aivyx_audit::AuditEvent::ConsolidationTriggered {
                                source: "heartbeat".into(),
                                timestamp: Utc::now(),
                            },
                        );

                        if let Some(ref mm) = ctx.memory_manager {
                            let consolidation_config =
                                ctx.consolidation_config.clone().unwrap_or_default();
                            // Use try_lock to avoid blocking the heartbeat dispatch loop.
                            // Consolidation makes LLM calls and can take minutes — holding
                            // the lock across that would starve all other memory operations.
                            let guard = mm.try_lock();
                            match guard {
                                Ok(mut mgr) => {
                                    match mgr
                                        .consolidate(ctx.provider.as_ref(), &consolidation_config)
                                        .await
                                    {
                                        Ok(report) => {
                                            // Drop the lock before doing non-memory work.
                                            drop(mgr);
                                            let summary = format!(
                                                "Merged {} clusters, pruned {} memories, strengthened {}",
                                                report.clusters_merged,
                                                report.memories_pruned,
                                                report.memories_strengthened,
                                            );
                                            tracing::info!(
                                                "Heartbeat: memory consolidation complete — {summary}"
                                            );

                                            crate::emit_audit(
                                                ctx,
                                                aivyx_audit::AuditEvent::ConsolidationCompleted {
                                                    agent_id: aivyx_core::AgentId::new(),
                                                    clusters_merged: report.clusters_merged,
                                                    memories_pruned: report.memories_pruned,
                                                    triples_decayed: 0,
                                                    patterns_mined: 0,
                                                    skills_crystallized: report.skills_crystallized,
                                                },
                                            );

                                            crate::send_notification(
                                                tx,
                                                Notification {
                                                    id: uuid::Uuid::new_v4().to_string(),
                                                    kind: NotificationKind::ActionTaken,
                                                    title: "Memory consolidated".into(),
                                                    body: summary,
                                                    source: "heartbeat".into(),
                                                    timestamp: Utc::now(),
                                                    requires_approval: false,
                                                    goal_id: None,
                                                },
                                            );
                                        }
                                        Err(e) => {
                                            tracing::warn!(
                                                "Heartbeat: memory consolidation failed: {e}"
                                            );
                                        }
                                    }
                                }
                                Err(_) => {
                                    tracing::info!(
                                        "Heartbeat: memory consolidation skipped — manager lock held by another operation"
                                    );
                                }
                            }
                        } else {
                            tracing::debug!(
                                "Heartbeat: consolidation requested but no memory manager"
                            );
                        }
                    }
                    Some(_resp) => {
                        tracing::info!("Heartbeat: memory consolidation denied by user");
                        crate::send_notification(
                            tx,
                            crate::Notification {
                                id: uuid::Uuid::new_v4().to_string(),
                                kind: crate::NotificationKind::Info,
                                title: "Consolidation skipped".into(),
                                body: "Memory consolidation was denied.".into(),
                                source: "heartbeat(consolidate_memory)".into(),
                                timestamp: Utc::now(),
                                requires_approval: false,
                                goal_id: None,
                            },
                        );
                    }
                    None => {
                        tracing::warn!("Heartbeat: memory consolidation approval timed out");
                    }
                }
            }

            HeartbeatAction::Suggest {
                title,
                body,
                sources,
                priority,
            } if config.can_suggest => {
                let kind = match priority.to_ascii_lowercase().as_str() {
                    "urgent" => NotificationKind::Urgent,
                    "high" => NotificationKind::Urgent,
                    _ => NotificationKind::Info,
                };
                let source_tag = if sources.is_empty() {
                    "heartbeat".to_string()
                } else {
                    format!("heartbeat:{}", sources.join("+"))
                };
                crate::send_notification(
                    tx,
                    Notification {
                        id: uuid::Uuid::new_v4().to_string(),
                        kind,
                        title: title.clone(),
                        body: body.clone(),
                        source: source_tag,
                        timestamp: Utc::now(),
                        requires_approval: false,
                        goal_id: None,
                    },
                );
                tracing::info!(
                    "Heartbeat: suggestion '{title}' (sources: {})",
                    sources.join(", ")
                );
            }

            HeartbeatAction::AnalyzeFailure {
                subject,
                root_cause,
                remediation,
                domain,
            } if config.can_analyze_failures => {
                let notif_id = uuid::Uuid::new_v4().to_string();
                let conf_str = domain
                    .as_ref()
                    .map(|d| format!(" (will decrease confidence in '{d}')"))
                    .unwrap_or_default();
                crate::send_notification(
                    tx,
                    crate::Notification {
                        id: notif_id.clone(),
                        kind: crate::NotificationKind::ApprovalNeeded,
                        title: "Action requires approval".into(),
                        body: format!(
                            "The agent wants to record a failure analysis for '{subject}'{conf_str}."
                        ),
                        source: "heartbeat(analyze_failure)".into(),
                        timestamp: Utc::now(),
                        requires_approval: true,
                        goal_id: None,
                    },
                );

                match crate::await_approval(ctx, &notif_id, std::time::Duration::from_secs(120))
                    .await
                {
                    Some(resp) if resp.approved => {
                        dispatch_analyze_failure(subject, root_cause, remediation, domain, ctx, tx)
                            .await;
                    }
                    Some(_resp) => {
                        tracing::info!("Heartbeat: failure analysis denied by user");
                        crate::send_notification(
                            tx,
                            crate::Notification {
                                id: uuid::Uuid::new_v4().to_string(),
                                kind: crate::NotificationKind::Info,
                                title: "Analysis skipped".into(),
                                body: "Failure analysis was denied.".into(),
                                source: "heartbeat(analyze_failure)".into(),
                                timestamp: Utc::now(),
                                requires_approval: false,
                                goal_id: None,
                            },
                        );
                    }
                    None => {
                        tracing::warn!("Heartbeat: failure analysis approval timed out");
                    }
                }
            }

            HeartbeatAction::ExtractKnowledge { triples } if config.can_extract_knowledge => {
                dispatch_extract_knowledge(triples, ctx, tx).await;
            }

            HeartbeatAction::Backup if config.can_backup => {
                // Gate: ask the user before writing a potentially large backup archive.
                let notif_id = uuid::Uuid::new_v4().to_string();
                crate::send_notification(
                    tx,
                    crate::Notification {
                        id: notif_id.clone(),
                        kind: crate::NotificationKind::ApprovalNeeded,
                        title: "Scheduled data backup — approve?".into(),
                        body: format!(
                            "The agent wants to create an encrypted backup of your data directory.\n\
                         Destination: {}\n\n\
                         [A] to approve, [D] to deny (2-minute window).",
                            ctx.backup_destination
                                .as_ref()
                                .map(|p| p.display().to_string())
                                .unwrap_or_else(|| "<not configured>".into()),
                        ),
                        source: "heartbeat:backup".into(),
                        timestamp: Utc::now(),
                        requires_approval: true,
                        goal_id: None,
                    },
                );
                tracing::info!("Heartbeat: backup approval requested (notif_id={notif_id})");

                match crate::await_approval(ctx, &notif_id, std::time::Duration::from_secs(120))
                    .await
                {
                    Some(resp) if resp.approved => {
                        tracing::info!("Heartbeat: backup approved by user — proceeding");
                        dispatch_backup(ctx, tx).await;
                    }
                    Some(_) => {
                        tracing::info!("Heartbeat: backup denied by user — skipping");
                        crate::send_notification(
                            tx,
                            crate::Notification {
                                id: uuid::Uuid::new_v4().to_string(),
                                kind: crate::NotificationKind::Info,
                                title: "Backup skipped".into(),
                                body: "You denied the scheduled backup request.".into(),
                                source: "heartbeat:backup".into(),
                                timestamp: Utc::now(),
                                requires_approval: false,
                                goal_id: None,
                            },
                        );
                    }
                    None => {
                        tracing::warn!("Heartbeat: backup approval timed out — skipping");
                    }
                }
            }

            HeartbeatAction::PruneAudit if config.can_prune_audit => {
                // Gate: ask before permanently removing audit entries.
                let notif_id = uuid::Uuid::new_v4().to_string();
                let retain_days = config.audit_retention_days;
                crate::send_notification(
                    tx,
                    crate::Notification {
                        id: notif_id.clone(),
                        kind: crate::NotificationKind::ApprovalNeeded,
                        title: "Audit log prune — approve?".into(),
                        body: format!(
                            "The agent wants to permanently remove audit entries older than {retain_days} days.\n\
                         This action cannot be undone.\n\n\
                         [A] to approve, [D] to deny (2-minute window)."
                        ),
                        source: "heartbeat:audit".into(),
                        timestamp: Utc::now(),
                        requires_approval: true,
                        goal_id: None,
                    },
                );
                tracing::info!("Heartbeat: audit prune approval requested (notif_id={notif_id})");

                match crate::await_approval(ctx, &notif_id, std::time::Duration::from_secs(120))
                    .await
                {
                    Some(resp) if resp.approved => {
                        tracing::info!("Heartbeat: audit prune approved by user — proceeding");
                        if let Some(ref audit_log) = ctx.audit_log {
                            let keep_after =
                                Utc::now() - chrono::Duration::days(retain_days as i64);
                            match aivyx_audit::prune(audit_log, keep_after) {
                                Ok(result) => {
                                    tracing::info!(
                                        removed = result.entries_removed,
                                        remaining = result.entries_remaining,
                                        "Heartbeat: audit log pruned"
                                    );
                                }
                                Err(e) => {
                                    tracing::warn!("Heartbeat: audit prune failed: {e}");
                                }
                            }
                        } else {
                            tracing::debug!("Heartbeat: audit prune requested but no audit log");
                        }
                    }
                    Some(_) => {
                        tracing::info!("Heartbeat: audit prune denied by user — skipping");
                    }
                    None => {
                        tracing::warn!("Heartbeat: audit prune approval timed out — skipping");
                    }
                }
            }

            // ── Phase 6: Smarter Agent actions ─────────────────────
            HeartbeatAction::PlanReview {
                horizons,
                gaps,
                adjustments,
            } if config.can_plan_review => {
                let notif_id = uuid::Uuid::new_v4().to_string();
                crate::send_notification(
                    tx,
                    crate::Notification {
                        id: notif_id.clone(),
                        kind: crate::NotificationKind::ApprovalNeeded,
                        title: "Plan Review Approval Needed".into(),
                        body: "The agent wants to update goal horizons, tags, and deadlines."
                            .into(),
                        source: "heartbeat(plan_review)".into(),
                        timestamp: Utc::now(),
                        requires_approval: true,
                        goal_id: None,
                    },
                );

                match crate::await_approval(ctx, &notif_id, std::time::Duration::from_secs(120))
                    .await
                {
                    Some(resp) if resp.approved => {
                        dispatch_plan_review(horizons, gaps, adjustments, ctx, tx);
                    }
                    Some(_) => {
                        tracing::info!("Heartbeat: plan review denied by user");
                        crate::send_notification(
                            tx,
                            crate::Notification {
                                id: uuid::Uuid::new_v4().to_string(),
                                kind: crate::NotificationKind::Info,
                                title: "Plan review skipped".into(),
                                body: "Plan review was denied.".into(),
                                source: "heartbeat(plan_review)".into(),
                                timestamp: Utc::now(),
                                requires_approval: false,
                                goal_id: None,
                            },
                        );
                    }
                    None => {
                        tracing::warn!("Heartbeat: plan review approval timed out");
                    }
                }
            }

            HeartbeatAction::StrategyReview {
                period_summary,
                goals_completed,
                goals_stalled,
                patterns,
                strategic_adjustments,
                domain_confidence_updates,
            } if config.can_strategy_review => {
                let notif_id = uuid::Uuid::new_v4().to_string();
                crate::send_notification(tx, crate::Notification {
                    id: notif_id.clone(),
                    kind: crate::NotificationKind::ApprovalNeeded,
                    title: "Strategy Review Approval Needed".into(),
                    body: "The agent wants to update domain confidence and record strategy adjustments.".into(),
                    source: "heartbeat(strategy_review)".into(),
                    timestamp: Utc::now(),
                    requires_approval: true,
                    goal_id: None,
                });

                match crate::await_approval(ctx, &notif_id, std::time::Duration::from_secs(120))
                    .await
                {
                    Some(resp) if resp.approved => {
                        dispatch_strategy_review(
                            period_summary,
                            *goals_completed,
                            goals_stalled,
                            patterns,
                            strategic_adjustments,
                            domain_confidence_updates,
                            ctx,
                            tx,
                        )
                        .await;
                    }
                    Some(_) => {
                        tracing::info!("Heartbeat: strategy review denied by user");
                        crate::send_notification(
                            tx,
                            crate::Notification {
                                id: uuid::Uuid::new_v4().to_string(),
                                kind: crate::NotificationKind::Info,
                                title: "Strategy review skipped".into(),
                                body: "Strategy review was denied.".into(),
                                source: "heartbeat(strategy_review)".into(),
                                timestamp: Utc::now(),
                                requires_approval: false,
                                goal_id: None,
                            },
                        );
                    }
                    None => {
                        tracing::warn!("Heartbeat: strategy review approval timed out");
                    }
                }
            }

            HeartbeatAction::TrackMood {
                observed_mood,
                adjustment,
            } if config.can_track_mood => {
                tracing::info!("Heartbeat: mood tracked — {observed_mood} → {adjustment}");
                crate::send_notification(
                    tx,
                    Notification {
                        id: uuid::Uuid::new_v4().to_string(),
                        kind: NotificationKind::Info,
                        title: format!("Mood: {observed_mood}"),
                        body: adjustment.clone(),
                        source: "heartbeat:mood".into(),
                        timestamp: Utc::now(),
                        requires_approval: false,
                        goal_id: None,
                    },
                );
            }

            HeartbeatAction::Encourage {
                achievement,
                message,
                streak,
            } if config.can_encourage => {
                dispatch_encourage(achievement, message, streak, ctx, tx);
            }

            HeartbeatAction::NoAction { reason } => {
                tracing::debug!("Heartbeat: no action — {reason}");
            }

            // Action was requested but not permitted by config
            _ => {
                tracing::debug!("Heartbeat: action not permitted by config: {action:?}");
            }
        }
    }
}

/// Find and update a goal by description match.
fn dispatch_update_goal(
    goal_match: &str,
    progress: &Option<f32>,
    status: &Option<String>,
    ctx: &LoopContext,
) {
    let goals = match ctx.brain_store.list_goals(
        &GoalFilter {
            status: Some(GoalStatus::Active),
            ..Default::default()
        },
        &ctx.brain_key,
    ) {
        Ok(g) => g,
        Err(e) => {
            tracing::warn!("Heartbeat: failed to list goals for update: {e}");
            return;
        }
    };

    let match_lower = goal_match.to_lowercase();
    let matched = goals
        .iter()
        .find(|g| g.description.to_lowercase().contains(&match_lower));

    let Some(goal) = matched else {
        tracing::debug!("Heartbeat: no goal matched '{goal_match}'");
        return;
    };

    let mut updated = goal.clone();

    if let Some(p) = progress {
        updated.set_progress(p.clamp(0.0, 1.0));
    }

    if let Some(s) = status {
        match s.as_str() {
            "completed" => updated.set_status(GoalStatus::Completed),
            "abandoned" => updated.set_status(GoalStatus::Abandoned),
            "dormant" => updated.set_status(GoalStatus::Dormant),
            "active" => updated.set_status(GoalStatus::Active),
            other => {
                tracing::warn!("Heartbeat: unknown goal status '{other}', ignoring");
            }
        }
    }

    if let Err(e) = ctx.brain_store.upsert_goal(&updated, &ctx.brain_key) {
        tracing::warn!("Heartbeat: failed to update goal: {e}");
    } else {
        tracing::info!(
            "Heartbeat: updated goal '{}' (progress: {:.0}%)",
            updated.description,
            updated.progress * 100.0,
        );
    }
}

/// Update the self-model based on reflection.
fn dispatch_reflect(
    add_strengths: &[String],
    add_weaknesses: &[String],
    remove_strengths: &[String],
    remove_weaknesses: &[String],
    domain_confidence: &std::collections::HashMap<String, f32>,
    ctx: &LoopContext,
) {
    let mut model = match ctx.brain_store.load_self_model(&ctx.brain_key) {
        Ok(Some(m)) => m,
        Ok(None) => SelfModel::default(),
        Err(e) => {
            tracing::warn!("Heartbeat: failed to load self-model: {e}");
            return;
        }
    };

    let mut changes = 0u32;

    for s in add_strengths {
        if !model.strengths.contains(s) {
            model.strengths.push(s.clone());
            changes += 1;
        }
    }
    for s in remove_strengths {
        let before = model.strengths.len();
        model.strengths.retain(|x| x != s);
        if model.strengths.len() < before {
            changes += 1;
        }
    }
    for w in add_weaknesses {
        if !model.weaknesses.contains(w) {
            model.weaknesses.push(w.clone());
            changes += 1;
        }
    }
    for w in remove_weaknesses {
        let before = model.weaknesses.len();
        model.weaknesses.retain(|x| x != w);
        if model.weaknesses.len() < before {
            changes += 1;
        }
    }
    for (domain, conf) in domain_confidence {
        model
            .domain_confidence
            .insert(domain.clone(), conf.clamp(0.0, 1.0));
        changes += 1;
    }

    if changes > 0 {
        model.updated_at = Utc::now();
        if let Err(e) = ctx.brain_store.save_self_model(&model, &ctx.brain_key) {
            tracing::warn!("Heartbeat: failed to save self-model: {e}");
        } else {
            tracing::info!("Heartbeat: self-model updated ({changes} changes)");
        }
    }
}

/// Analyze a failure and store a post-mortem as a Reflection memory.
///
/// Also decreases domain_confidence for the affected domain (if specified)
/// and notifies the user of the analysis.
async fn dispatch_analyze_failure(
    subject: &str,
    root_cause: &str,
    remediation: &str,
    domain: &Option<String>,
    ctx: &LoopContext,
    tx: &mpsc::Sender<Notification>,
) {
    // Store as Reflection memory if memory manager is available
    let reflection_content = format!(
        "FAILURE ANALYSIS: {subject}\n\
         Root cause: {root_cause}\n\
         Remediation: {remediation}"
    );

    if let Some(ref mm) = ctx.memory_manager {
        let mut mgr = mm.lock().await;
        if let Err(e) = mgr
            .remember(
                reflection_content,
                aivyx_memory::MemoryKind::Custom("reflection".into()),
                None,
                vec!["failure-analysis".to_string()],
            )
            .await
        {
            tracing::warn!("Heartbeat: failed to store failure analysis memory: {e}");
        }
    }

    // Decrease domain confidence if a domain is specified
    if let Some(domain_name) = domain {
        let mut model = match ctx.brain_store.load_self_model(&ctx.brain_key) {
            Ok(Some(m)) => m,
            Ok(None) => SelfModel::default(),
            Err(e) => {
                tracing::warn!("Heartbeat: failed to load self-model for failure analysis: {e}");
                SelfModel::default()
            }
        };

        let current = model
            .domain_confidence
            .get(domain_name)
            .copied()
            .unwrap_or(0.5);
        let new_conf = (current - 0.1).max(0.0); // decrease by 10%
        model
            .domain_confidence
            .insert(domain_name.clone(), new_conf);

        // Add weakness if not already present
        let weakness = format!("failure in {domain_name}: {subject}");
        if !model.weaknesses.iter().any(|w| w.contains(domain_name)) {
            model.weaknesses.push(weakness);
        }

        model.updated_at = Utc::now();
        if let Err(e) = ctx.brain_store.save_self_model(&model, &ctx.brain_key) {
            tracing::warn!("Heartbeat: failed to save self-model after failure analysis: {e}");
        } else {
            tracing::info!(
                "Heartbeat: domain confidence for '{domain_name}' decreased to {new_conf:.2}"
            );
        }
    }

    // Notify the user
    crate::send_notification(
        tx,
        Notification {
            id: uuid::Uuid::new_v4().to_string(),
            kind: NotificationKind::Info,
            title: format!("Failure analysis: {subject}"),
            body: format!("Root cause: {root_cause}\nRemediation: {remediation}"),
            source: "heartbeat:failure-analysis".into(),
            timestamp: Utc::now(),
            requires_approval: false,
            goal_id: None,
        },
    );

    tracing::info!("Heartbeat: analyzed failure '{subject}'");
}

/// Dispatch extracted knowledge triples into the memory manager's knowledge graph.
async fn dispatch_extract_knowledge(
    triples: &[ExtractedTriple],
    ctx: &LoopContext,
    tx: &mpsc::Sender<Notification>,
) {
    let mm = match ctx.memory_manager {
        Some(ref mm) => mm,
        None => {
            tracing::debug!("Heartbeat: knowledge extraction skipped — no memory manager");
            return;
        }
    };

    let mut mgr = mm.lock().await;
    let mut added = 0u32;
    let mut reinforced = 0u32;
    let mut superseded = 0u32;
    let mut errors = 0u32;

    for triple in triples {
        // Skip triples with empty fields
        if triple.subject.trim().is_empty()
            || triple.predicate.trim().is_empty()
            || triple.object.trim().is_empty()
        {
            tracing::debug!("Heartbeat: skipping empty triple");
            continue;
        }

        let confidence = triple.confidence.clamp(0.0, 1.0);

        match mgr.add_or_reinforce_triple(
            triple.subject.trim().to_string(),
            triple.predicate.trim().to_string(),
            triple.object.trim().to_string(),
            None, // global scope
            confidence,
            "heartbeat".to_string(),
            0.1, // reinforce boost
        ) {
            Ok((_id, action)) => match action {
                aivyx_memory::TripleAction::Created => added += 1,
                aivyx_memory::TripleAction::Reinforced => reinforced += 1,
                aivyx_memory::TripleAction::Superseded { .. } => superseded += 1,
            },
            Err(e) => {
                tracing::warn!(
                    "Heartbeat: failed to store triple ({}, {}, {}): {e}",
                    triple.subject,
                    triple.predicate,
                    triple.object,
                );
                errors += 1;
            }
        }
    }

    if added + reinforced + superseded > 0 {
        let body = format!(
            "Extracted {} triples: {added} new, {reinforced} reinforced, {superseded} superseded",
            added + reinforced + superseded,
        );
        tracing::info!("Heartbeat: {body}");
        crate::send_notification(
            tx,
            Notification {
                id: uuid::Uuid::new_v4().to_string(),
                kind: NotificationKind::Info,
                title: "Knowledge extracted".into(),
                body,
                source: "heartbeat:knowledge".into(),
                timestamp: Utc::now(),
                requires_approval: false,
                goal_id: None,
            },
        );
    }

    if errors > 0 {
        tracing::warn!("Heartbeat: {errors} triple extraction errors");
    }
}

/// Create a tar.gz backup of the data directory and prune old archives.
async fn dispatch_backup(ctx: &LoopContext, tx: &mpsc::Sender<Notification>) {
    let data_dir = match ctx.data_dir {
        Some(ref d) if d.exists() => d,
        _ => {
            tracing::debug!("Heartbeat: backup skipped — no data directory");
            return;
        }
    };

    let dest = match ctx.backup_destination {
        Some(ref d) => d.clone(),
        None => {
            tracing::debug!("Heartbeat: backup skipped — no destination configured");
            return;
        }
    };

    // Ensure destination directory exists
    if let Err(e) = std::fs::create_dir_all(&dest) {
        tracing::warn!("Heartbeat: backup failed — cannot create destination: {e}");
        return;
    }

    let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
    let archive_name = format!("pa_backup_{timestamp}.tar.gz");
    let archive_path = dest.join(&archive_name);

    // Create tar.gz archive off the async runtime to avoid blocking.
    let src = data_dir.to_path_buf();
    let dst = archive_path.clone();
    let tar_result = tokio::task::spawn_blocking(move || create_tar_gz(&src, &dst)).await;
    match tar_result {
        Ok(Ok(bytes)) => {
            let size_mb = bytes as f64 / (1024.0 * 1024.0);
            tracing::info!("Heartbeat: backup created — {archive_name} ({size_mb:.1} MB)");

            // Audit: backup completed
            crate::emit_audit(
                ctx,
                aivyx_audit::AuditEvent::BackupCompleted {
                    size_bytes: bytes,
                    destination: archive_path.display().to_string(),
                },
            );

            // Prune old backups beyond retention period
            let pruned = prune_old_backups(&dest, ctx.backup_retention_days);

            let body = format!(
                "Backup `{archive_name}` created ({size_mb:.1} MB). {pruned} old archive(s) pruned."
            );
            crate::send_notification(
                tx,
                Notification {
                    id: uuid::Uuid::new_v4().to_string(),
                    kind: NotificationKind::Info,
                    title: "Data backup complete".into(),
                    body,
                    source: "heartbeat:backup".into(),
                    timestamp: Utc::now(),
                    requires_approval: false,
                    goal_id: None,
                },
            );
        }
        Ok(Err(e)) => {
            tracing::warn!("Heartbeat: backup failed — {e}");
            crate::emit_audit(
                ctx,
                aivyx_audit::AuditEvent::BackupFailed {
                    reason: e.to_string(),
                },
            );
        }
        Err(e) => {
            tracing::warn!("Heartbeat: backup task panicked — {e}");
            crate::emit_audit(
                ctx,
                aivyx_audit::AuditEvent::BackupFailed {
                    reason: format!("task panicked: {e}"),
                },
            );
        }
    }
}

/// Create a gzipped tar archive of `source_dir` at `dest_path`.
/// Returns the archive size in bytes on success.
fn create_tar_gz(
    source_dir: &std::path::Path,
    dest_path: &std::path::Path,
) -> std::io::Result<u64> {
    let parent = source_dir.parent().unwrap_or(std::path::Path::new("/"));
    let dir_name = source_dir.file_name().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "no directory name")
    })?;

    let output = std::process::Command::new("tar")
        .arg("czf")
        .arg(dest_path)
        .arg("-C")
        .arg(parent)
        .arg(dir_name)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(std::io::Error::other(format!("tar failed: {stderr}")));
    }

    let meta = std::fs::metadata(dest_path)?;
    Ok(meta.len())
}

/// Remove backup archives older than `retention_days`. Returns the count removed.
fn prune_old_backups(backup_dir: &std::path::Path, retention_days: u64) -> usize {
    let cutoff = Utc::now() - chrono::Duration::days(retention_days as i64);
    let mut pruned = 0;

    let entries = match std::fs::read_dir(backup_dir) {
        Ok(e) => e,
        Err(_) => return 0,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };

        // Only prune files matching our naming pattern
        if !name.starts_with("pa_backup_") || !name.ends_with(".tar.gz") {
            continue;
        }

        // Parse timestamp from filename: pa_backup_YYYYMMDD_HHMMSS.tar.gz
        let ts_part = name
            .strip_prefix("pa_backup_")
            .and_then(|s| s.strip_suffix(".tar.gz"));
        let created =
            ts_part.and_then(|ts| chrono::NaiveDateTime::parse_from_str(ts, "%Y%m%d_%H%M%S").ok());

        if let Some(created) = created {
            let created_utc = created.and_utc();
            if created_utc < cutoff && std::fs::remove_file(&path).is_ok() {
                tracing::debug!("Heartbeat: pruned old backup {name}");
                pruned += 1;
            }
        }
    }

    pruned
}

// ── Phase 6: Smarter Agent dispatch functions ────────────────

/// Dispatch plan review — apply horizon tags and deadlines to goals.
fn dispatch_plan_review(
    horizons: &std::collections::HashMap<String, Vec<String>>,
    gaps: &[String],
    adjustments: &[PlanAdjustment],
    ctx: &LoopContext,
    tx: &mpsc::Sender<Notification>,
) {
    let goals = match ctx.brain_store.list_goals(
        &GoalFilter {
            status: Some(GoalStatus::Active),
            ..Default::default()
        },
        &ctx.brain_key,
    ) {
        Ok(g) => g,
        Err(e) => {
            tracing::warn!("Heartbeat: plan review failed to list goals: {e}");
            return;
        }
    };

    let mut changes = 0u32;

    for adj in adjustments {
        let match_lower = adj.goal_match.to_lowercase();
        let matched = goals
            .iter()
            .find(|g| g.description.to_lowercase().contains(&match_lower));

        let Some(goal) = matched else {
            tracing::debug!(
                "Heartbeat: plan review — no goal matched '{}'",
                adj.goal_match
            );
            continue;
        };

        let mut updated = goal.clone();

        // Apply horizon tags (replace any existing horizon: tags)
        if let Some(ref new_tags) = adj.set_tags {
            updated.tags.retain(|t| !t.starts_with("horizon:"));
            for tag in new_tags {
                if !updated.tags.contains(tag) {
                    updated.tags.push(tag.clone());
                }
            }
        }

        // Apply deadline (parse ISO 8601)
        if let Some(ref deadline_str) = adj.set_deadline {
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(deadline_str) {
                updated.deadline = Some(dt.with_timezone(&Utc));
            } else if let Ok(d) = chrono::NaiveDate::parse_from_str(deadline_str, "%Y-%m-%d")
                && let Some(dt) = d.and_hms_opt(23, 59, 59)
            {
                updated.deadline = Some(dt.and_utc());
            }
        }

        if let Err(e) = ctx.brain_store.upsert_goal(&updated, &ctx.brain_key) {
            tracing::warn!("Heartbeat: plan review failed to update goal: {e}");
        } else {
            changes += 1;
        }
    }

    // Build summary
    let horizon_summary: Vec<String> = horizons
        .iter()
        .filter(|(_, goals)| !goals.is_empty())
        .map(|(h, goals)| format!("{h}: {} goals", goals.len()))
        .collect();

    let summary = if horizon_summary.is_empty() {
        "Plan reviewed — no horizon assignments made".into()
    } else {
        horizon_summary.join(", ")
    };

    let gap_note = if gaps.is_empty() {
        String::new()
    } else {
        format!("\nGaps: {}", gaps.join("; "))
    };

    crate::send_notification(
        tx,
        Notification {
            id: uuid::Uuid::new_v4().to_string(),
            kind: NotificationKind::Info,
            title: "Plan review complete".into(),
            body: format!("{summary}{gap_note}\n{changes} goal(s) updated."),
            source: "heartbeat:planning".into(),
            timestamp: Utc::now(),
            requires_approval: false,
            goal_id: None,
        },
    );

    tracing::info!("Heartbeat: plan review — {summary}, {changes} goals updated");
}

/// Dispatch strategy review — store as memory, update self-model, notify.
#[allow(clippy::too_many_arguments)]
async fn dispatch_strategy_review(
    period_summary: &str,
    goals_completed: u32,
    goals_stalled: &[String],
    patterns: &[String],
    strategic_adjustments: &[String],
    domain_confidence_updates: &std::collections::HashMap<String, f32>,
    ctx: &LoopContext,
    tx: &mpsc::Sender<Notification>,
) {
    // Store the review as a persistent memory
    let review_content = format!(
        "STRATEGY REVIEW\n\
         Period: {period_summary}\n\
         Goals completed: {goals_completed}\n\
         Stalled: {}\n\
         Patterns: {}\n\
         Adjustments: {}",
        if goals_stalled.is_empty() {
            "none".into()
        } else {
            goals_stalled.join(", ")
        },
        if patterns.is_empty() {
            "none observed".into()
        } else {
            patterns.join("; ")
        },
        if strategic_adjustments.is_empty() {
            "none".into()
        } else {
            strategic_adjustments.join("; ")
        },
    );

    if let Some(ref mm) = ctx.memory_manager {
        let mut mgr = mm.lock().await;
        if let Err(e) = mgr
            .remember(
                review_content,
                aivyx_memory::MemoryKind::Custom("review".into()),
                None,
                vec!["strategy-review".to_string()],
            )
            .await
        {
            tracing::warn!("Heartbeat: failed to store strategy review memory: {e}");
        }
    }

    // Apply domain confidence updates to self-model
    if !domain_confidence_updates.is_empty() {
        let mut model = match ctx.brain_store.load_self_model(&ctx.brain_key) {
            Ok(Some(m)) => m,
            Ok(None) => SelfModel::default(),
            Err(e) => {
                tracing::warn!("Heartbeat: failed to load self-model for strategy review: {e}");
                SelfModel::default()
            }
        };

        for (domain, conf) in domain_confidence_updates {
            model
                .domain_confidence
                .insert(domain.clone(), conf.clamp(0.0, 1.0));
        }
        model.updated_at = Utc::now();

        if let Err(e) = ctx.brain_store.save_self_model(&model, &ctx.brain_key) {
            tracing::warn!("Heartbeat: failed to save self-model after strategy review: {e}");
        }
    }

    // Notify
    let body = format!(
        "{period_summary}\n\
         Completed: {goals_completed} | Stalled: {}{}",
        goals_stalled.len(),
        if !patterns.is_empty() {
            format!("\nPatterns: {}", patterns.join("; "))
        } else {
            String::new()
        },
    );

    crate::send_notification(
        tx,
        Notification {
            id: uuid::Uuid::new_v4().to_string(),
            kind: NotificationKind::Info,
            title: "Weekly strategy review".into(),
            body,
            source: "heartbeat:strategy-review".into(),
            timestamp: Utc::now(),
            requires_approval: false,
            goal_id: None,
        },
    );

    tracing::info!(
        "Heartbeat: strategy review — {goals_completed} completed, {} stalled",
        goals_stalled.len(),
    );
}

/// Dispatch encouragement notification.
fn dispatch_encourage(
    achievement: &str,
    message: &str,
    streak: &Option<u32>,
    _ctx: &LoopContext,
    tx: &mpsc::Sender<Notification>,
) {
    let streak_note = streak
        .map(|s| format!(" (streak: {s})"))
        .unwrap_or_default();

    crate::send_notification(
        tx,
        Notification {
            id: uuid::Uuid::new_v4().to_string(),
            kind: NotificationKind::Info,
            title: format!("{achievement}{streak_note}"),
            body: message.to_string(),
            source: "heartbeat:encouragement".into(),
            timestamp: Utc::now(),
            requires_approval: false,
            goal_id: None,
        },
    );

    tracing::info!("Heartbeat: encouragement — {achievement}{streak_note}");
}

/// Check for goal milestones (anniversaries of creation/completion).
///
/// Returns human-readable milestone descriptions for heartbeat context injection.
pub fn check_milestones(goals: &[Goal], now: chrono::DateTime<Utc>) -> Vec<String> {
    let mut milestones = Vec::new();

    // Milestone thresholds: 1 week, 1 month, 3 months, 6 months, 1 year
    let thresholds: &[(i64, &str)] = &[
        (7, "1 week"),
        (30, "1 month"),
        (90, "3 months"),
        (180, "6 months"),
        (365, "1 year"),
    ];

    for goal in goals {
        let days_since_created = (now - goal.created_at).num_days();

        for &(threshold_days, label) in thresholds {
            // Match if within ±1 day of the threshold
            if (days_since_created - threshold_days).abs() <= 1 && days_since_created > 0 {
                let status_label = if goal.status == GoalStatus::Completed {
                    "completed"
                } else if goal.status == GoalStatus::Active {
                    "active"
                } else {
                    continue; // skip dormant/abandoned for milestones
                };
                milestones.push(format!(
                    "It's been {label} since you started '{}' ({})",
                    goal.description, status_label,
                ));
                break; // one milestone per goal
            }
        }
    }

    milestones
}

/// Gather recent achievements for encouragement context.
///
/// Returns a formatted section if there are recently completed goals.
pub fn gather_achievements(
    goals: &[Goal],
    last_heartbeat_at: Option<chrono::DateTime<Utc>>,
) -> Option<String> {
    let since = last_heartbeat_at.unwrap_or_else(|| Utc::now() - chrono::Duration::hours(1));

    let recently_completed: Vec<&Goal> = goals
        .iter()
        .filter(|g| g.status == GoalStatus::Completed && g.updated_at >= since)
        .collect();

    if recently_completed.is_empty() {
        return None;
    }

    // Count weekly completions
    let week_ago = Utc::now() - chrono::Duration::days(7);
    let weekly_count = goals
        .iter()
        .filter(|g| g.status == GoalStatus::Completed && g.updated_at >= week_ago)
        .count();

    let mut lines = vec![format!(
        "Recently completed ({}):",
        recently_completed.len()
    )];
    for g in &recently_completed {
        lines.push(format!("- {}", g.description));
    }
    if weekly_count > recently_completed.len() {
        lines.push(format!("Total completed this week: {weekly_count}"));
    }

    Some(lines.join("\n"))
}

// ── Priority scoring integration ──────────────────────────────

/// Build scored items from pre-fetched data for priority ranking.
///
/// Uses data already loaded by `gather_context` to avoid redundant store reads.
fn build_priority_items(data: &GatheredData) -> Vec<crate::priority::ScoredItem> {
    use crate::priority::{self, Priority, ScoredItem};

    let now = Utc::now();
    let mut items = Vec::new();

    // Score due/overdue reminders
    for r in &data.reminders {
        let score = priority::score_reminder(r.due_at, now);
        items.push(ScoredItem {
            summary: r.message.clone(),
            source: "reminder".into(),
            score,
            priority: Priority::from_score(score),
        });
    }

    // Score upcoming bills
    for b in &data.bills {
        let days = b
            .due_date
            .map(|d: chrono::DateTime<Utc>| d.signed_duration_since(now).num_days())
            .unwrap_or(7);
        let score = priority::score_upcoming_bill(days);
        let amount = aivyx_actions::finance::format_dollars(b.amount_cents);
        items.push(ScoredItem {
            summary: format!("{} — {}", b.description, amount),
            source: "finance".into(),
            score,
            priority: Priority::from_score(score),
        });
    }

    // Budget alerts
    for (cat, spent, limit) in &data.over_budget {
        let score = priority::score_over_budget();
        items.push(ScoredItem {
            summary: format!(
                "{} over budget: {} / {}",
                cat,
                aivyx_actions::finance::format_dollars(*spent),
                aivyx_actions::finance::format_dollars(*limit),
            ),
            source: "finance".into(),
            score,
            priority: Priority::from_score(score),
        });
    }

    // Score stale goals
    for g in &data.goals {
        let days_since = now.signed_duration_since(g.updated_at).num_days();
        let score = priority::score_stale_goal(days_since, g.progress);
        if score >= 0.3 {
            items.push(ScoredItem {
                summary: format!(
                    "{} ({:.0}% done, updated {}d ago)",
                    g.description,
                    g.progress * 100.0,
                    days_since
                ),
                source: "goal".into(),
                score,
                priority: Priority::from_score(score),
            });
        }
    }

    items
}

// ── Heartbeat tick ─────────────────────────────────────────────

/// Run a single heartbeat tick. Returns true if the LLM was called.
pub async fn run_heartbeat_tick(
    config: &HeartbeatConfig,
    ctx: &mut LoopContext,
    tx: &mpsc::Sender<Notification>,
) -> bool {
    if !config.enabled {
        return false;
    }

    // Check if enough time has elapsed since last beat.
    // When the LLM is down (consecutive failures), exponentially back off:
    // 0 failures → normal interval, 1 → 2x, 2 → 4x, 3+ → 8x (capped).
    let now = Utc::now();
    if let Some(last) = ctx.last_heartbeat_at {
        let elapsed_minutes = now.signed_duration_since(last).num_minutes();
        let base_interval = (config.interval_minutes as i64).max(5);
        let backoff_multiplier = match ctx.heartbeat_consecutive_failures {
            0 => 1,
            1 => 2,
            2 => 4,
            _ => 8, // Cap at 8x (e.g., 30min interval → 4 hours)
        };
        let effective_interval = base_interval * backoff_multiplier;
        if elapsed_minutes < effective_interval {
            return false;
        }
        if ctx.heartbeat_consecutive_failures > 0 {
            tracing::info!(
                failures = ctx.heartbeat_consecutive_failures,
                interval = effective_interval,
                "Heartbeat: retrying after backoff ({backoff_multiplier}x interval)",
            );
        }
    }

    tracing::debug!("Heartbeat: gathering context");

    // Gather context from all configured sources.
    let (mut hb_ctx, gathered_data) = gather_context(config, ctx).await;

    // Async: fetch email subjects if configured.
    // Sanitize external data to prevent prompt injection via crafted email subjects.
    if config.check_email && ctx.email_config.is_some() {
        let subjects = super::fetch_email_subjects(ctx).await;
        if !subjects.is_empty() {
            let sanitized: Vec<String> = subjects.iter().map(|s| sanitize_for_prompt(s)).collect();
            hb_ctx.add("Recent Emails", sanitized.join("\n"));
        }
    }

    // Async: fetch today's calendar events and conflicts if configured.
    // Sanitize summaries/locations — they originate from external CalDAV servers.
    if config.check_calendar && ctx.calendar_config.is_some() {
        let (events, conflicts) = super::fetch_calendar_for_briefing(ctx).await;
        if !events.is_empty() {
            let lines: Vec<String> = events
                .iter()
                .map(|e| {
                    let summary = sanitize_for_prompt(&e.summary);
                    if let Some(ref loc) = e.location {
                        format!("- {} {} ({})", e.time, summary, sanitize_for_prompt(loc))
                    } else {
                        format!("- {} {}", e.time, summary)
                    }
                })
                .collect();
            hb_ctx.add("Today's Calendar", lines.join("\n"));
        }
        if !conflicts.is_empty() {
            let lines: Vec<String> = conflicts
                .iter()
                .map(|c| {
                    format!(
                        "- CONFLICT: \"{}\" and \"{}\" overlap ({})",
                        sanitize_for_prompt(&c.event_a),
                        sanitize_for_prompt(&c.event_b),
                        c.overlap
                    )
                })
                .collect();
            hb_ctx.add("Scheduling Conflicts", lines.join("\n"));
        }
    }

    // ── Phase 6: Smarter Agent context sections ──────────────────

    // Mood awareness: estimate and inject user mood signal
    if config.can_track_mood {
        let signals = ctx.interaction_signals.lock().await;
        let mood = crate::MoodSignal::estimate(&signals);
        if mood != crate::MoodSignal::Neutral {
            hb_ctx.add("User Mood Estimate", format!(
                "Current mood signal: {}. Adapt your tone and notification frequency accordingly.",
                mood.label(),
            ));
        }
    }

    // Resource awareness: token budget and quiet hours
    if let Some(text) = ctx.resource_budget.format_for_prompt() {
        hb_ctx.add("Resource Budget", text);
    }

    // Milestone tracking: goal anniversaries
    if config.can_track_milestones && !gathered_data.goals.is_empty() {
        let milestones = check_milestones(&gathered_data.goals, Utc::now());
        if !milestones.is_empty() {
            hb_ctx.add("Milestones", milestones.join("\n"));
        }
    }

    // Achievement tracking: recently completed goals
    if config.can_encourage {
        // Load all goals (including completed) for achievement tracking
        let all_goals = ctx
            .brain_store
            .list_goals(&GoalFilter::default(), &ctx.brain_key)
            .unwrap_or_default();

        if let Some(text) = gather_achievements(&all_goals, ctx.last_heartbeat_at) {
            hb_ctx.add("Achievements", text);
        }
    }

    // Strategy review: extended context when cron trigger has fired
    if config.can_strategy_review && ctx.strategy_review_pending {
        let all_goals = ctx
            .brain_store
            .list_goals(&GoalFilter::default(), &ctx.brain_key)
            .unwrap_or_default();

        let week_ago = Utc::now() - chrono::Duration::days(7);
        let completed_this_week: Vec<&Goal> = all_goals
            .iter()
            .filter(|g| g.status == GoalStatus::Completed && g.updated_at >= week_ago)
            .collect();
        let abandoned_this_week: Vec<&Goal> = all_goals
            .iter()
            .filter(|g| g.status == GoalStatus::Abandoned && g.updated_at >= week_ago)
            .collect();

        let mut review_lines = vec!["--- STRATEGY REVIEW CONTEXT ---".to_string()];
        review_lines.push(format!(
            "Goals completed this week: {}",
            completed_this_week.len()
        ));
        for g in &completed_this_week {
            review_lines.push(format!("  ✓ {}", g.description));
        }
        if !abandoned_this_week.is_empty() {
            review_lines.push(format!(
                "Goals abandoned this week: {}",
                abandoned_this_week.len()
            ));
            for g in &abandoned_this_week {
                review_lines.push(format!("  ✗ {}", g.description));
            }
        }
        let stalled: Vec<&Goal> = all_goals
            .iter()
            .filter(|g| {
                g.status == GoalStatus::Active
                    && g.progress < 0.5
                    && (Utc::now() - g.updated_at).num_days() >= 3
            })
            .collect();
        if !stalled.is_empty() {
            review_lines.push(format!("Stalled goals (>3 days, <50%): {}", stalled.len()));
            for g in &stalled {
                review_lines.push(format!(
                    "  ⚠ {} ({:.0}%)",
                    g.description,
                    g.progress * 100.0
                ));
            }
        }
        review_lines.push("Use `strategy_review` to summarize findings.".into());
        hb_ctx.add("Strategy Review", review_lines.join("\n"));

        ctx.strategy_review_pending = false;
    }

    // Context-aware skip: if nothing to review, skip the LLM call entirely.
    if hb_ctx.is_empty() {
        tracing::debug!("Heartbeat: no context to review, skipping LLM call");
        crate::emit_audit(
            ctx,
            aivyx_audit::AuditEvent::HeartbeatSkipped {
                reason: "no context to review".into(),
            },
        );
        ctx.last_heartbeat_at = Some(now);
        return false;
    }

    let section_count = hb_ctx.section_count();
    tracing::info!("Heartbeat: {} context sections, calling LLM", section_count);

    // Audit: heartbeat fired
    crate::emit_audit(
        ctx,
        aivyx_audit::AuditEvent::HeartbeatFired {
            agent_name: "pa".into(),
            context_sections: section_count,
            timestamp: now,
        },
    );

    // MCP health check — quick ping of connected servers.
    if let Some(ref pool) = ctx.mcp_pool {
        for name in pool.server_names().await {
            if let Some(client) = pool.get(&name).await
                && client.list_tools().await.is_err()
            {
                tracing::warn!(server = %name, "MCP server health check failed");
                hb_ctx.add("MCP Status", format!("Server '{name}' is unreachable"));
            }
        }
    }

    // Build priority scores from gathered context.
    let mut scored_items = build_priority_items(&gathered_data);
    crate::priority::rank(&mut scored_items);
    let priority_text = crate::priority::format_priority_summary(&scored_items, 10, 0.3);
    let priority_ref = if priority_text.is_empty() {
        None
    } else {
        Some(priority_text.as_str())
    };

    // Build prompt and call LLM.
    let user_prompt = build_heartbeat_prompt(config, &hb_ctx, priority_ref);
    let request = ChatRequest {
        system_prompt: Some(ctx.system_prompt.clone()),
        messages: vec![ChatMessage::user(user_prompt)],
        tools: vec![],
        model: None,
        max_tokens: 1024,
    };

    let response_text = match aivyx_actions::retry::retry(
        &aivyx_actions::retry::RetryConfig::llm(),
        || async {
            tokio::time::timeout(
                std::time::Duration::from_secs(120),
                ctx.provider.chat(&request),
            )
            .await
            .map_err(|_| aivyx_core::AivyxError::LlmProvider("timeout after 120s".into()))?
        },
        aivyx_actions::retry::is_transient,
    )
    .await
    {
        Ok(response) => {
            // Reset failure counter on success
            if ctx.heartbeat_consecutive_failures > 0 {
                tracing::info!(
                    "Heartbeat: LLM recovered after {} consecutive failures",
                    ctx.heartbeat_consecutive_failures,
                );
            }
            ctx.heartbeat_consecutive_failures = 0;
            tracing::info!(
                "Heartbeat: LLM responded ({} tokens)",
                response.usage.output_tokens
            );
            response.message.content.to_text()
        }
        Err(e) => {
            ctx.heartbeat_consecutive_failures += 1;
            tracing::warn!(
                failures = ctx.heartbeat_consecutive_failures,
                "Heartbeat: LLM call failed after retries: {e}",
            );
            crate::emit_audit(
                ctx,
                aivyx_audit::AuditEvent::HeartbeatSkipped {
                    reason: format!(
                        "LLM call failed ({} consecutive): {e}",
                        ctx.heartbeat_consecutive_failures
                    ),
                },
            );
            ctx.last_heartbeat_at = Some(now);
            return false;
        }
    };

    // Parse response and dispatch actions.
    let parsed = parse_response(&response_text);

    tracing::info!(
        "Heartbeat: reasoning='{}', {} actions",
        crate::truncate(&parsed.reasoning, 100),
        parsed.actions.len(),
    );

    dispatch_actions(&parsed.actions, config, ctx, tx).await;

    // Audit: heartbeat completed
    crate::emit_audit(
        ctx,
        aivyx_audit::AuditEvent::HeartbeatCompleted {
            agent_name: "pa".into(),
            acted: !parsed.actions.is_empty(),
            actions_dispatched: parsed.actions.len(),
            actions_completed: parsed.actions.len(), // all dispatched synchronously
            summary: crate::truncate(&parsed.reasoning, 200).to_string(),
        },
    );

    ctx.last_heartbeat_at = Some(now);
    true
}

// ── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_context_is_empty() {
        let ctx = HeartbeatContext::default();
        assert!(ctx.is_empty());
        assert_eq!(ctx.section_count(), 0);
    }

    #[test]
    fn context_with_section_is_not_empty() {
        let mut ctx = HeartbeatContext::default();
        ctx.add("Goals", "- Learn user preferences");
        assert!(!ctx.is_empty());
        assert_eq!(ctx.section_count(), 1);
    }

    #[test]
    fn empty_body_is_not_added() {
        let mut ctx = HeartbeatContext::default();
        ctx.add("Goals", "");
        assert!(ctx.is_empty());
    }

    #[test]
    fn format_for_prompt_includes_labels() {
        let mut ctx = HeartbeatContext::default();
        ctx.add("Active Goals", "- Goal A\n- Goal B");
        ctx.add("Reminders", "- Reminder X");
        let formatted = ctx.format_for_prompt();
        assert!(formatted.contains("## Active Goals"));
        assert!(formatted.contains("## Reminders"));
        assert!(formatted.contains("- Goal A"));
    }

    #[test]
    fn parse_valid_response() {
        let json = r#"{
            "reasoning": "All goals on track",
            "actions": [
                {"action": "no_action", "reason": "nothing to do"}
            ]
        }"#;
        let resp = parse_response(json);
        assert_eq!(resp.reasoning, "All goals on track");
        assert_eq!(resp.actions.len(), 1);
        assert!(matches!(resp.actions[0], HeartbeatAction::NoAction { .. }));
    }

    #[test]
    fn parse_response_with_fences() {
        let text = "```json\n{\"reasoning\": \"ok\", \"actions\": []}\n```";
        let resp = parse_response(text);
        assert_eq!(resp.reasoning, "ok");
        assert!(resp.actions.is_empty());
    }

    #[test]
    fn parse_invalid_response_falls_back() {
        let resp = parse_response("this is not json at all");
        assert_eq!(resp.actions.len(), 1);
        assert!(matches!(resp.actions[0], HeartbeatAction::NoAction { .. }));
    }

    #[test]
    fn parse_multi_action_response() {
        let json = r#"{
            "reasoning": "Need to update goals and notify",
            "actions": [
                {"action": "notify", "title": "Alert", "body": "Something happened"},
                {"action": "set_goal", "description": "Track project X", "success_criteria": "Know status"},
                {"action": "update_goal", "goal_match": "tech stack", "progress": 0.5},
                {"action": "reflect", "add_strengths": ["email-triage"]}
            ]
        }"#;
        let resp = parse_response(json);
        assert_eq!(resp.actions.len(), 4);
        assert!(matches!(resp.actions[0], HeartbeatAction::Notify { .. }));
        assert!(matches!(resp.actions[1], HeartbeatAction::SetGoal { .. }));
        assert!(matches!(
            resp.actions[2],
            HeartbeatAction::UpdateGoal { .. }
        ));
        assert!(matches!(resp.actions[3], HeartbeatAction::Reflect { .. }));
    }

    #[test]
    fn heartbeat_config_defaults() {
        let config = HeartbeatConfig::default();
        assert!(config.enabled);
        assert_eq!(config.interval_minutes, 30);
        assert!(config.check_goals);
        assert!(config.check_reminders);
        assert!(config.check_calendar);
        assert!(config.check_finance);
        assert!(config.check_contacts);
        assert!(config.can_notify);
        assert!(config.can_suggest);
        assert!(config.can_manage_goals);
        assert!(!config.can_reflect); // conservative default
        assert!(!config.can_consolidate_memory); // conservative default
    }

    #[test]
    fn parse_suggest_action() {
        let json = r#"{
            "reasoning": "Sarah emailed about the proposal and you have a meeting with her",
            "actions": [
                {"action": "suggest", "title": "Prepare for Sarah meeting", "body": "Sarah emailed about the Q2 proposal. You meet her at 2pm.", "sources": ["email", "calendar"], "priority": "high"}
            ]
        }"#;
        let resp = parse_response(json);
        assert_eq!(resp.actions.len(), 1);
        match &resp.actions[0] {
            HeartbeatAction::Suggest {
                title,
                sources,
                priority,
                ..
            } => {
                assert_eq!(title, "Prepare for Sarah meeting");
                assert_eq!(sources, &["email", "calendar"]);
                assert_eq!(priority, "high");
            }
            other => panic!("Expected Suggest, got {other:?}"),
        }
    }

    #[test]
    fn build_prompt_includes_priority_summary() {
        let config = HeartbeatConfig::default();
        let mut ctx = HeartbeatContext::default();
        ctx.add("Goals", "- Test goal");

        let prompt = build_heartbeat_prompt(
            &config,
            &ctx,
            Some("- [URGENT] (email) Old email needs reply"),
        );
        assert!(prompt.contains("Priority Summary"));
        assert!(prompt.contains("URGENT"));
    }

    #[test]
    fn build_prompt_includes_available_actions() {
        let config = HeartbeatConfig {
            can_reflect: true,
            can_consolidate_memory: true,
            ..Default::default()
        };
        let mut ctx = HeartbeatContext::default();
        ctx.add("Goals", "- Test goal");

        let prompt = build_heartbeat_prompt(&config, &ctx, None);
        assert!(prompt.contains("notify"));
        assert!(prompt.contains("set_goal"));
        assert!(prompt.contains("reflect"));
        assert!(prompt.contains("consolidate_memory"));
        assert!(prompt.contains("suggest"));
        assert!(prompt.contains("no_action"));
        assert!(prompt.contains("CROSS-SOURCE REASONING"));
    }

    #[test]
    fn build_prompt_omits_disabled_actions() {
        let config = HeartbeatConfig {
            can_reflect: false,
            can_consolidate_memory: false,
            can_manage_goals: false,
            can_suggest: false,
            ..Default::default()
        };
        let mut ctx = HeartbeatContext::default();
        ctx.add("Reminders", "- Due now");

        let prompt = build_heartbeat_prompt(&config, &ctx, None);
        assert!(prompt.contains("notify"));
        // These action names should not appear in the available actions list
        assert!(!prompt.contains("\"set_goal\""));
        assert!(!prompt.contains("\"reflect\""));
        assert!(!prompt.contains("\"consolidate_memory\""));
        assert!(!prompt.contains("\"suggest\""));
    }

    #[test]
    fn parse_extract_knowledge_action() {
        let json = r#"{
            "reasoning": "Found facts in email context",
            "actions": [
                {"action": "extract_knowledge", "triples": [
                    {"subject": "Alice", "predicate": "works_at", "object": "Acme Corp", "confidence": 0.9},
                    {"subject": "Project Orion", "predicate": "deadline_is", "object": "2026-04-15"}
                ]}
            ]
        }"#;
        let resp = parse_response(json);
        assert_eq!(resp.actions.len(), 1);
        match &resp.actions[0] {
            HeartbeatAction::ExtractKnowledge { triples } => {
                assert_eq!(triples.len(), 2);
                assert_eq!(triples[0].subject, "Alice");
                assert_eq!(triples[0].predicate, "works_at");
                assert_eq!(triples[0].object, "Acme Corp");
                assert!((triples[0].confidence - 0.9).abs() < 0.01);
                // Second triple uses default confidence
                assert!((triples[1].confidence - 0.8).abs() < 0.01);
            }
            other => panic!("Expected ExtractKnowledge, got {other:?}"),
        }
    }

    #[test]
    fn build_prompt_includes_extract_knowledge_when_enabled() {
        let config = HeartbeatConfig {
            can_extract_knowledge: true,
            ..Default::default()
        };
        let mut ctx = HeartbeatContext::default();
        ctx.add("Email", "- From: alice@acme.com Subject: Q2 Review");

        let prompt = build_heartbeat_prompt(&config, &ctx, None);
        assert!(prompt.contains("extract_knowledge"));
        assert!(prompt.contains("triples"));
    }

    #[test]
    fn build_prompt_omits_extract_knowledge_when_disabled() {
        let config = HeartbeatConfig::default(); // can_extract_knowledge defaults to false
        let mut ctx = HeartbeatContext::default();
        ctx.add("Email", "- From: alice@acme.com");

        let prompt = build_heartbeat_prompt(&config, &ctx, None);
        assert!(!prompt.contains("extract_knowledge"));
    }

    #[test]
    fn can_extract_knowledge_defaults_false() {
        let config = HeartbeatConfig::default();
        assert!(!config.can_extract_knowledge);
    }

    // ── Phase 6: Smarter Agent tests ──────────────────────────

    #[test]
    fn parse_plan_review_action() {
        let json = r#"{
            "reasoning": "Organizing goals into horizons",
            "actions": [{
                "action": "plan_review",
                "horizons": {"today": ["finish report"], "week": ["review PRs"]},
                "gaps": ["no quarterly goals"],
                "adjustments": [{
                    "goal_match": "finish report",
                    "set_tags": ["horizon:today"],
                    "set_deadline": "2026-04-05",
                    "reasoning": "due tomorrow"
                }]
            }]
        }"#;
        let resp = parse_response(json);
        assert_eq!(resp.actions.len(), 1);
        match &resp.actions[0] {
            HeartbeatAction::PlanReview {
                horizons,
                gaps,
                adjustments,
            } => {
                assert_eq!(horizons.get("today").map(|v| v.len()), Some(1));
                assert_eq!(gaps.len(), 1);
                assert_eq!(adjustments.len(), 1);
                assert_eq!(adjustments[0].goal_match, "finish report");
            }
            other => panic!("Expected PlanReview, got {other:?}"),
        }
    }

    #[test]
    fn parse_strategy_review_action() {
        let json = r#"{
            "reasoning": "Weekly review",
            "actions": [{
                "action": "strategy_review",
                "period_summary": "Good week overall",
                "goals_completed": 3,
                "goals_stalled": ["learn rust"],
                "patterns": ["strong at email tasks"],
                "strategic_adjustments": ["focus on learning goals"],
                "domain_confidence_updates": {"email": 0.85}
            }]
        }"#;
        let resp = parse_response(json);
        assert_eq!(resp.actions.len(), 1);
        match &resp.actions[0] {
            HeartbeatAction::StrategyReview {
                goals_completed,
                goals_stalled,
                domain_confidence_updates,
                ..
            } => {
                assert_eq!(*goals_completed, 3);
                assert_eq!(goals_stalled.len(), 1);
                assert!((domain_confidence_updates["email"] - 0.85).abs() < 0.01);
            }
            other => panic!("Expected StrategyReview, got {other:?}"),
        }
    }

    #[test]
    fn parse_encourage_action() {
        let json = r#"{
            "reasoning": "Great work",
            "actions": [{
                "action": "encourage",
                "achievement": "Completed 3 goals this week",
                "message": "You're on a roll!",
                "streak": 3
            }]
        }"#;
        let resp = parse_response(json);
        assert_eq!(resp.actions.len(), 1);
        match &resp.actions[0] {
            HeartbeatAction::Encourage {
                achievement,
                streak,
                ..
            } => {
                assert!(achievement.contains("3 goals"));
                assert_eq!(*streak, Some(3));
            }
            other => panic!("Expected Encourage, got {other:?}"),
        }
    }

    #[test]
    fn parse_track_mood_action() {
        let json = r#"{
            "reasoning": "User seems stressed",
            "actions": [{
                "action": "track_mood",
                "observed_mood": "frustrated",
                "adjustment": "reducing notifications"
            }]
        }"#;
        let resp = parse_response(json);
        assert_eq!(resp.actions.len(), 1);
        match &resp.actions[0] {
            HeartbeatAction::TrackMood { observed_mood, .. } => {
                assert_eq!(observed_mood, "frustrated");
            }
            other => panic!("Expected TrackMood, got {other:?}"),
        }
    }

    #[test]
    fn build_prompt_includes_plan_review_when_enabled() {
        let config = HeartbeatConfig {
            can_plan_review: true,
            ..Default::default()
        };
        let mut ctx = HeartbeatContext::default();
        ctx.add("Goals", "test goal");
        let prompt = build_heartbeat_prompt(&config, &ctx, None);
        assert!(prompt.contains("plan_review"));
    }

    #[test]
    fn build_prompt_includes_encourage_when_enabled() {
        let config = HeartbeatConfig {
            can_encourage: true,
            ..Default::default()
        };
        let mut ctx = HeartbeatContext::default();
        ctx.add("Goals", "test");
        let prompt = build_heartbeat_prompt(&config, &ctx, None);
        assert!(prompt.contains("encourage"));
    }

    #[test]
    fn milestone_detection_finds_week_old_goal() {
        use aivyx_brain::{Goal, Priority};
        use aivyx_core::GoalId;
        let now = Utc::now();
        let goal = Goal {
            id: GoalId::new(),
            description: "Learn Rust".into(),
            priority: Priority::Medium,
            status: GoalStatus::Active,
            parent: None,
            success_criteria: "Complete the book".into(),
            progress: 0.5,
            tags: vec![],
            created_at: now - chrono::Duration::days(7),
            updated_at: now,
            deadline: None,
            failure_count: 0,
            consecutive_failures: 0,
            cooldown_until: None,
        };
        let milestones = check_milestones(&[goal], now);
        assert_eq!(milestones.len(), 1);
        assert!(milestones[0].contains("1 week"));
    }

    #[test]
    fn milestone_detection_skips_recent_goals() {
        use aivyx_brain::{Goal, Priority};
        use aivyx_core::GoalId;
        let now = Utc::now();
        let goal = Goal {
            id: GoalId::new(),
            description: "Fresh goal".into(),
            priority: Priority::Medium,
            status: GoalStatus::Active,
            parent: None,
            success_criteria: "".into(),
            progress: 0.0,
            tags: vec![],
            created_at: now - chrono::Duration::days(2),
            updated_at: now,
            deadline: None,
            failure_count: 0,
            consecutive_failures: 0,
            cooldown_until: None,
        };
        let milestones = check_milestones(&[goal], now);
        assert!(milestones.is_empty());
    }

    #[test]
    fn gather_achievements_finds_completed_goals() {
        use aivyx_brain::{Goal, Priority};
        use aivyx_core::GoalId;
        let now = Utc::now();
        let goal = Goal {
            id: GoalId::new(),
            description: "Write docs".into(),
            priority: Priority::Medium,
            status: GoalStatus::Completed,
            parent: None,
            success_criteria: "".into(),
            progress: 1.0,
            tags: vec![],
            created_at: now - chrono::Duration::days(7),
            updated_at: now - chrono::Duration::minutes(10),
            deadline: None,
            failure_count: 0,
            consecutive_failures: 0,
            cooldown_until: None,
        };
        let since = now - chrono::Duration::hours(1);
        let result = gather_achievements(&[goal], Some(since));
        assert!(result.is_some());
        assert!(result.unwrap().contains("Write docs"));
    }

    #[test]
    fn gather_achievements_empty_when_no_completions() {
        use aivyx_brain::{Goal, Priority};
        use aivyx_core::GoalId;
        let now = Utc::now();
        let goal = Goal {
            id: GoalId::new(),
            description: "Pending goal".into(),
            priority: Priority::Medium,
            status: GoalStatus::Active,
            parent: None,
            success_criteria: "".into(),
            progress: 0.3,
            tags: vec![],
            created_at: now - chrono::Duration::days(1),
            updated_at: now,
            deadline: None,
            failure_count: 0,
            consecutive_failures: 0,
            cooldown_until: None,
        };
        let result = gather_achievements(&[goal], Some(now - chrono::Duration::hours(1)));
        assert!(result.is_none());
    }

    #[test]
    fn phase6_config_defaults_to_false() {
        let config = HeartbeatConfig::default();
        assert!(!config.can_plan_review);
        assert!(!config.can_strategy_review);
        assert!(!config.can_track_mood);
        assert!(!config.can_encourage);
        assert!(!config.can_track_milestones);
        assert!(!config.notification_pacing);
        assert_eq!(config.max_notifications_per_hour, 5);
    }
}
