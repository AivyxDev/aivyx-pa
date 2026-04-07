//! Workflow trigger engine — evaluates triggers each loop tick and
//! instantiates workflow templates when conditions are met.
//!
//! Triggers are defined inline on `WorkflowTemplate` objects. The engine
//! loads all templates, checks each trigger against the current state,
//! and emits notifications to create missions when triggers fire.

use aivyx_actions::workflow::{
    WorkflowTemplate, WorkflowTrigger, list_templates, load_template,
};
use aivyx_crypto::{EncryptedStore, MasterKey};
use chrono::{DateTime, Utc};
use std::collections::HashMap;

/// Result of evaluating all triggers in a single tick.
#[derive(Debug, Default)]
pub struct TriggerResult {
    /// Templates that fired, with their instantiation parameters.
    pub fired: Vec<FiredTrigger>,
    /// Number of templates checked.
    pub templates_checked: usize,
    /// Number of triggers evaluated.
    pub triggers_evaluated: usize,
}

/// A trigger that fired, ready to instantiate.
#[derive(Debug, Clone)]
pub struct FiredTrigger {
    /// The template that should be instantiated.
    pub template_name: String,
    /// Parameters extracted from the trigger context (e.g., email sender).
    pub params: HashMap<String, String>,
    /// Description of why the trigger fired.
    pub reason: String,
}

/// Persistent state for the trigger engine across ticks.
#[derive(Debug, Default)]
pub struct TriggerState {
    /// Tracks when each (template_name, trigger_index) last fired.
    pub last_fired: HashMap<String, DateTime<Utc>>,
    /// When triggers were last evaluated — used as the baseline for cron matching.
    pub last_evaluated_at: Option<DateTime<Utc>>,
}

impl TriggerState {
    /// Build a composite key for dedup tracking.
    fn trigger_key(template_name: &str, trigger_index: usize) -> String {
        format!("{template_name}:{trigger_index}")
    }

    /// Check if a trigger has fired recently (within cooldown period).
    fn recently_fired(&self, template_name: &str, trigger_index: usize, cooldown_secs: i64) -> bool {
        let key = Self::trigger_key(template_name, trigger_index);
        self.last_fired.get(&key).is_some_and(|last| {
            (Utc::now() - *last).num_seconds() < cooldown_secs
        })
    }

    /// Record that a trigger fired.
    fn mark_fired(&mut self, template_name: &str, trigger_index: usize) {
        let key = Self::trigger_key(template_name, trigger_index);
        self.last_fired.insert(key, Utc::now());
    }

    /// Remove entries older than `max_age_secs` to prevent unbounded growth.
    pub fn prune(&mut self, max_age_secs: i64) {
        let cutoff = Utc::now() - chrono::Duration::seconds(max_age_secs);
        self.last_fired.retain(|_, fired_at| *fired_at > cutoff);
    }
}

/// Context available during trigger evaluation.
pub struct TriggerContext<'a> {
    /// Recent email subjects/senders from the last triage or inbox check.
    pub recent_emails: &'a [(String, String)], // (sender, subject)
    /// Active goal descriptions and progress.
    pub active_goals: &'a [(String, f32)], // (description, progress 0.0-1.0)
    /// Minimum seconds between re-fires of the same trigger.
    pub cooldown_secs: i64,
    /// When the last tick ran — used as baseline for cron trigger evaluation.
    /// Falls back to 61 seconds ago if not set (first tick).
    pub last_tick_at: Option<DateTime<Utc>>,
}

impl<'a> Default for TriggerContext<'a> {
    fn default() -> Self {
        Self {
            recent_emails: &[],
            active_goals: &[],
            cooldown_secs: 300, // 5 minute default cooldown
            last_tick_at: None,
        }
    }
}

/// Evaluate all workflow triggers for the current tick.
///
/// Loads templates from the store, checks each trigger against the context,
/// and returns a list of triggers that fired.
pub fn evaluate_triggers(
    store: &EncryptedStore,
    key: &MasterKey,
    state: &mut TriggerState,
    ctx: &TriggerContext<'_>,
) -> TriggerResult {
    let mut result = TriggerResult::default();

    // Prune stale entries older than 24 hours to prevent unbounded growth
    state.prune(24 * 60 * 60);

    // Load template names
    let names = match list_templates(store) {
        Ok(n) => n,
        Err(e) => {
            tracing::warn!("Failed to list workflow templates: {e}");
            return result;
        }
    };

    result.templates_checked = names.len();

    for name in &names {
        let template = match load_template(store, key, name) {
            Ok(Some(t)) => t,
            Ok(None) => continue,
            Err(e) => {
                tracing::warn!("Failed to load template '{name}': {e}");
                continue;
            }
        };

        for (idx, trigger) in template.triggers.iter().enumerate() {
            result.triggers_evaluated += 1;

            // Skip Manual triggers — only fired explicitly
            if matches!(trigger, WorkflowTrigger::Manual) {
                continue;
            }

            // Cooldown check
            if state.recently_fired(name, idx, ctx.cooldown_secs) {
                continue;
            }

            if let Some(fired) = evaluate_single_trigger(&template, idx, trigger, ctx) {
                state.mark_fired(name, idx);
                result.fired.push(fired);
            }
        }
    }

    result
}

/// Evaluate a single trigger against the current context.
fn evaluate_single_trigger(
    template: &WorkflowTemplate,
    _trigger_index: usize,
    trigger: &WorkflowTrigger,
    ctx: &TriggerContext<'_>,
) -> Option<FiredTrigger> {
    match trigger {
        WorkflowTrigger::Cron { expression } => {
            evaluate_cron_trigger(template, expression, ctx.last_tick_at)
        }
        WorkflowTrigger::Email { sender_contains, subject_contains } => {
            evaluate_email_trigger(template, sender_contains, subject_contains, ctx)
        }
        WorkflowTrigger::GoalProgress { goal_match, threshold } => {
            evaluate_goal_trigger(template, goal_match, *threshold, ctx)
        }
        WorkflowTrigger::FileChange { path_glob } => {
            // File change detection requires an OS-level watcher (e.g. `notify` crate)
            // running in the background. This is deferred to a future release.
            // Log at warn level so users who configure this trigger are aware it's a no-op.
            tracing::warn!(
                workflow = %template.name,
                glob = ?path_glob,
                "FileChange trigger is not yet implemented — skipping"
            );
            None
        }
        WorkflowTrigger::Webhook { .. } => {
            // Webhook triggers fire via the HTTP endpoint, not on tick.
            None
        }
        WorkflowTrigger::Manual => None,
    }
}

fn evaluate_cron_trigger(
    template: &WorkflowTemplate,
    expression: &str,
    last_tick_at: Option<DateTime<Utc>>,
) -> Option<FiredTrigger> {
    let cron = match croner::Cron::new(expression).parse() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                "Invalid cron '{}' on template '{}': {e}",
                expression, template.name
            );
            return None;
        }
    };

    // Use the last tick time as baseline so we don't miss cron triggers
    // when the loop interval is longer than 61 seconds.
    let from = last_tick_at.unwrap_or_else(|| Utc::now() - chrono::Duration::seconds(61));
    if cron.find_next_occurrence(&from, false).is_ok_and(|next| next <= Utc::now()) {
        Some(FiredTrigger {
            template_name: template.name.clone(),
            params: HashMap::new(),
            reason: format!("Cron schedule matched: {expression}"),
        })
    } else {
        None
    }
}

fn evaluate_email_trigger(
    template: &WorkflowTemplate,
    sender_contains: &Option<String>,
    subject_contains: &Option<String>,
    ctx: &TriggerContext<'_>,
) -> Option<FiredTrigger> {
    for (sender, subject) in ctx.recent_emails {
        let sender_lower = sender.to_lowercase();
        let subject_lower = subject.to_lowercase();
        let sender_match = sender_contains
            .as_ref()
            .is_none_or(|pat| sender_lower.contains(&pat.to_lowercase()));
        let subject_match = subject_contains
            .as_ref()
            .is_none_or(|pat| subject_lower.contains(&pat.to_lowercase()));

        if sender_match && subject_match {
            let mut params = HashMap::new();
            params.insert("trigger_sender".into(), sender.clone());
            params.insert("trigger_subject".into(), subject.clone());
            return Some(FiredTrigger {
                template_name: template.name.clone(),
                params,
                reason: format!("Email matched: from={sender}, subject={subject}"),
            });
        }
    }
    None
}

fn evaluate_goal_trigger(
    template: &WorkflowTemplate,
    goal_match: &str,
    threshold: f32,
    ctx: &TriggerContext<'_>,
) -> Option<FiredTrigger> {
    for (description, progress) in ctx.active_goals {
        if description.contains(goal_match) && *progress >= threshold {
            let mut params = HashMap::new();
            params.insert("trigger_goal".into(), description.clone());
            params.insert("trigger_progress".into(), format!("{progress:.0}%"));
            return Some(FiredTrigger {
                template_name: template.name.clone(),
                params,
                reason: format!(
                    "Goal '{}' reached {:.0}% (threshold: {:.0}%)",
                    description,
                    progress * 100.0,
                    threshold * 100.0,
                ),
            });
        }
    }
    None
}

// ── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_state_cooldown() {
        let mut state = TriggerState::default();
        assert!(!state.recently_fired("t1", 0, 300));

        state.mark_fired("t1", 0);
        assert!(state.recently_fired("t1", 0, 300));
        assert!(!state.recently_fired("t1", 1, 300)); // different index
        assert!(!state.recently_fired("t2", 0, 300)); // different template
    }

    #[test]
    fn email_trigger_matches() {
        let template = WorkflowTemplate {
            name: "vendor-invoice".into(),
            description: "Process vendor invoice".into(),
            steps: vec![],
            parameters: vec![],
            triggers: vec![],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let emails = vec![
            ("billing@vendor.com".into(), "Invoice #1234".into()),
            ("alice@example.com".into(), "Hello".into()),
        ];

        let ctx = TriggerContext {
            recent_emails: &emails,
            ..Default::default()
        };

        // Match by sender
        let result = evaluate_email_trigger(
            &template,
            &Some("vendor.com".into()),
            &None,
            &ctx,
        );
        assert!(result.is_some());
        let fired = result.unwrap();
        assert_eq!(fired.params["trigger_sender"], "billing@vendor.com");

        // Match by subject
        let result = evaluate_email_trigger(
            &template,
            &None,
            &Some("Invoice".into()),
            &ctx,
        );
        assert!(result.is_some());

        // No match
        let result = evaluate_email_trigger(
            &template,
            &Some("noreply@other.com".into()),
            &None,
            &ctx,
        );
        assert!(result.is_none());
    }

    #[test]
    fn goal_trigger_matches() {
        let template = WorkflowTemplate {
            name: "quarterly-review".into(),
            description: "Run quarterly review".into(),
            steps: vec![],
            parameters: vec![],
            triggers: vec![],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let goals = vec![
            ("Complete quarterly review preparation".into(), 0.85),
            ("Learn Rust".into(), 0.4),
        ];

        let ctx = TriggerContext {
            active_goals: &goals,
            ..Default::default()
        };

        // Match — progress above threshold
        let result = evaluate_goal_trigger(&template, "quarterly review", 0.8, &ctx);
        assert!(result.is_some());

        // No match — threshold not reached
        let result = evaluate_goal_trigger(&template, "quarterly review", 0.9, &ctx);
        assert!(result.is_none());

        // No match — wrong goal
        let result = evaluate_goal_trigger(&template, "annual report", 0.5, &ctx);
        assert!(result.is_none());
    }

    #[test]
    fn trigger_state_prune_removes_old_entries() {
        let mut state = TriggerState::default();
        // Insert an entry 2 hours ago
        let key = TriggerState::trigger_key("old-template", 0);
        state.last_fired.insert(key.clone(), Utc::now() - chrono::Duration::hours(2));
        // Insert a recent entry
        state.mark_fired("recent-template", 0);

        assert_eq!(state.last_fired.len(), 2);

        // Prune entries older than 1 hour
        state.prune(3600);
        assert_eq!(state.last_fired.len(), 1);
        assert!(!state.last_fired.contains_key(&key));
        assert!(state.last_fired.contains_key(&TriggerState::trigger_key("recent-template", 0)));
    }

    #[test]
    fn email_trigger_case_insensitive() {
        let template = WorkflowTemplate {
            name: "case-test".into(),
            description: "Case insensitive test".into(),
            steps: vec![],
            parameters: vec![],
            triggers: vec![],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let emails = vec![
            ("Billing@VENDOR.COM".into(), "INVOICE #1234".into()),
        ];
        let ctx = TriggerContext {
            recent_emails: &emails,
            ..Default::default()
        };

        // Lowercase pattern should match uppercase sender
        let result = evaluate_email_trigger(
            &template,
            &Some("vendor.com".into()),
            &None,
            &ctx,
        );
        assert!(result.is_some());

        // Lowercase pattern should match uppercase subject
        let result = evaluate_email_trigger(
            &template,
            &None,
            &Some("invoice".into()),
            &ctx,
        );
        assert!(result.is_some());

        // Mixed case pattern should still match
        let result = evaluate_email_trigger(
            &template,
            &Some("Vendor.Com".into()),
            &Some("Invoice".into()),
            &ctx,
        );
        assert!(result.is_some());
    }

    #[test]
    fn manual_trigger_never_fires() {
        let template = WorkflowTemplate {
            name: "manual-only".into(),
            description: "Manual workflow".into(),
            steps: vec![],
            parameters: vec![],
            triggers: vec![WorkflowTrigger::Manual],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let ctx = TriggerContext::default();
        let result = evaluate_single_trigger(&template, 0, &template.triggers[0], &ctx);
        assert!(result.is_none());
    }
}
