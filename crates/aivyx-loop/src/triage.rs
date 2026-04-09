//! Email triage — autonomous inbox processing on heartbeat ticks.
//!
//! The triage system processes new emails the agent hasn't seen yet,
//! applies rule-based and LLM-based classification, and takes autonomous
//! action within configured permission boundaries.
//!
//! Flow per tick:
//! 1. Fetch unread emails since last triage cursor
//! 2. Apply rule-based matching (fast path, zero LLM cost)
//! 3. For unmatched emails, call LLM for classification
//! 4. Execute permitted actions (auto-reply, forward, classify)
//! 5. Log all actions and advance cursor
//!
//! The cursor (highest processed IMAP seq) is persisted in the encrypted
//! store so triage resumes correctly across restarts.

use aivyx_actions::email::{EmailConfig, EmailSummary};
use aivyx_crypto::{EncryptedStore, MasterKey};
use aivyx_llm::{ChatMessage, ChatRequest, LlmProvider};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ── Configuration ────────────────────────────────────────────

/// Triage configuration — controls what the agent does with incoming email.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriageConfig {
    /// Whether autonomous triage is active.
    #[serde(default)]
    pub enabled: bool,
    /// Maximum emails to process per tick (prevents runaway on large inboxes).
    #[serde(default = "default_max_per_tick")]
    pub max_per_tick: usize,
    /// Allow the agent to send auto-replies.
    #[serde(default)]
    pub can_auto_reply: bool,
    /// Allow the agent to forward emails.
    #[serde(default)]
    pub can_forward: bool,
    /// Address to forward important emails to (e.g., the human owner).
    #[serde(default)]
    pub forward_to: Option<String>,
    /// Auto-reply rules: if sender or subject matches, send a canned reply.
    #[serde(default)]
    pub auto_reply_rules: Vec<AutoReplyRule>,
    /// Senders to always ignore (no classification, no action).
    #[serde(default)]
    pub ignore_senders: Vec<String>,
    /// Custom classification categories (defaults used if empty).
    #[serde(default)]
    pub categories: Vec<String>,
}

fn default_max_per_tick() -> usize {
    10
}

impl Default for TriageConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_per_tick: default_max_per_tick(),
            can_auto_reply: false,
            can_forward: false,
            forward_to: None,
            auto_reply_rules: vec![],
            ignore_senders: Vec::new(),
            categories: Vec::new(),
        }
    }
}

/// A rule that triggers an automatic reply.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoReplyRule {
    /// Human-readable name for this rule.
    pub name: String,
    /// Match condition: sender contains this string (case-insensitive).
    #[serde(default)]
    pub sender_contains: Option<String>,
    /// Match condition: subject contains this string (case-insensitive).
    #[serde(default)]
    pub subject_contains: Option<String>,
    /// The reply body to send.
    pub reply_body: String,
}

impl AutoReplyRule {
    /// Check if this rule matches an email.
    pub fn matches(&self, from: &str, subject: &str) -> bool {
        let from_lower = from.to_lowercase();
        let subject_lower = subject.to_lowercase();

        let sender_match = self
            .sender_contains
            .as_ref()
            .is_some_and(|s| from_lower.contains(&s.to_lowercase()));

        let subject_match = self
            .subject_contains
            .as_ref()
            .is_some_and(|s| subject_lower.contains(&s.to_lowercase()));

        // At least one condition must be set and match
        (self.sender_contains.is_some() && sender_match)
            || (self.subject_contains.is_some() && subject_match)
    }
}

// ── Triage results ───────────────────────────────────────────

/// The action taken on a triaged email.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TriageAction {
    /// Email was classified but no autonomous action taken.
    Classified {
        category: String,
        urgency: Urgency,
        summary: String,
    },
    /// An auto-reply was sent.
    AutoReplied { rule_name: String },
    /// Email was forwarded to the owner.
    Forwarded { to: String },
    /// Email was ignored (matched ignore list).
    Ignored,
    /// Triage failed for this email.
    Error { reason: String },
}

/// Urgency level assigned by LLM classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Urgency {
    Low,
    Normal,
    High,
    Urgent,
}

impl Urgency {
    pub fn as_str(&self) -> &'static str {
        match self {
            Urgency::Low => "low",
            Urgency::Normal => "normal",
            Urgency::High => "high",
            Urgency::Urgent => "urgent",
        }
    }
}

impl std::fmt::Display for Urgency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Result of triaging a single email.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriageResult {
    pub seq: u32,
    pub from: String,
    pub subject: String,
    pub action: TriageAction,
    pub timestamp: DateTime<Utc>,
}

/// Summary of a triage tick.
#[derive(Debug, Default)]
pub struct TriageTickSummary {
    pub processed: usize,
    pub auto_replied: usize,
    pub forwarded: usize,
    pub classified: usize,
    pub ignored: usize,
    pub errors: usize,
}

// ── Cursor persistence ───────────────────────────────────────

const TRIAGE_CURSOR_KEY: &str = "email-triage-cursor";
const TRIAGE_LOG_PREFIX: &str = "triage-log:";

/// Load the last triaged IMAP sequence number.
pub fn load_cursor(store: &EncryptedStore, key: &MasterKey) -> u32 {
    store
        .get(TRIAGE_CURSOR_KEY, key)
        .ok()
        .flatten()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// Save the triage cursor.
pub fn save_cursor(store: &EncryptedStore, key: &MasterKey, seq: u32) {
    let _ = store.put(TRIAGE_CURSOR_KEY, seq.to_string().as_bytes(), key);
}

/// Maximum number of triage log entries to retain.
const MAX_TRIAGE_LOG_ENTRIES: usize = 500;

/// Save a triage log entry, pruning old entries if the log exceeds the cap.
fn save_triage_log(store: &EncryptedStore, key: &MasterKey, result: &TriageResult) {
    let log_key = format!("{TRIAGE_LOG_PREFIX}{}", result.seq);
    if let Ok(json) = serde_json::to_vec(result) {
        let _ = store.put(&log_key, &json, key);
    }

    // Prune oldest entries if over the cap
    prune_triage_log(store, MAX_TRIAGE_LOG_ENTRIES);
}

/// Remove the oldest triage log entries to stay within `max_entries`.
fn prune_triage_log(store: &EncryptedStore, max_entries: usize) {
    let Ok(keys) = store.list_keys() else { return };
    let mut triage_keys: Vec<&String> = keys
        .iter()
        .filter(|k| k.starts_with(TRIAGE_LOG_PREFIX))
        .collect();

    if triage_keys.len() <= max_entries {
        return;
    }

    // Sort ascending by numeric seq suffix — oldest first.
    // String sort would order "9" after "10000", so we parse numerically.
    triage_keys.sort_by_key(|k| {
        k.strip_prefix(TRIAGE_LOG_PREFIX)
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0)
    });
    let to_remove = triage_keys.len() - max_entries;
    for key in triage_keys.into_iter().take(to_remove) {
        let _ = store.delete(key);
    }
}

/// Load recent triage log entries.
pub fn load_triage_log(store: &EncryptedStore, key: &MasterKey, limit: usize) -> Vec<TriageResult> {
    let Ok(keys) = store.list_keys() else {
        return vec![];
    };

    let mut entries: Vec<TriageResult> = keys
        .iter()
        .filter(|k| k.starts_with(TRIAGE_LOG_PREFIX))
        .filter_map(|k| {
            store
                .get(k, key)
                .ok()
                .flatten()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        })
        .collect();

    entries.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    entries.truncate(limit);
    entries
}

// ── Rule-based triage ────────────────────────────────────────

/// Check if a sender should be ignored.
fn should_ignore(from: &str, ignore_senders: &[String]) -> bool {
    let from_lower = from.to_lowercase();
    ignore_senders
        .iter()
        .any(|s| from_lower.contains(&s.to_lowercase()))
}

/// Find the first matching auto-reply rule.
fn find_auto_reply_rule<'a>(
    from: &str,
    subject: &str,
    rules: &'a [AutoReplyRule],
) -> Option<&'a AutoReplyRule> {
    rules.iter().find(|r| r.matches(from, subject))
}

// ── LLM classification ──────────────────────────────────────

/// Count how many "Re:" prefixes appear in a subject line.
/// Used to detect deep reply chains that may indicate an auto-reply loop.
fn count_reply_depth(subject: &str) -> usize {
    let lower = subject.to_lowercase();
    lower.matches("re:").count()
}

/// LLM classification response for a single email.
#[derive(Debug, Deserialize)]
#[allow(dead_code)] // suggested_reply parsed for future use
struct ClassificationResponse {
    category: String,
    urgency: String,
    summary: String,
    #[serde(default)]
    should_forward: bool,
    #[serde(default)]
    suggested_reply: Option<String>,
}

/// Default categories when none are configured.
const DEFAULT_CATEGORIES: &[&str] = &[
    "personal",
    "work",
    "newsletter",
    "notification",
    "billing",
    "spam",
    "inquiry",
    "urgent",
];

/// Build the classification prompt for a batch of emails.
fn build_classification_prompt(
    emails: &[(u32, &str, &str, &str)], // (seq, from, subject, body_preview)
    categories: &[String],
) -> String {
    let cats = if categories.is_empty() {
        DEFAULT_CATEGORIES
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
    } else {
        categories.to_vec()
    };

    let mut prompt = format!(
        "You are an email triage assistant. Classify each email below.\n\n\
         Available categories: {}\n\
         Urgency levels: low, normal, high, urgent\n\n\
         For each email, respond with a JSON array of objects:\n\
         [{{\"seq\": N, \"category\": \"...\", \"urgency\": \"...\", \"summary\": \"one-line summary\", \
         \"should_forward\": false, \"suggested_reply\": null}}]\n\n\
         Set should_forward=true for emails that seem important and need human attention.\n\
         Set suggested_reply to a short draft reply ONLY if the email clearly expects a response.\n\n\
         Respond with ONLY valid JSON, no markdown fences.\n\n\
         --- EMAILS ---\n",
        cats.join(", ")
    );

    for (seq, from, subject, body) in emails {
        prompt.push_str(&format!(
            "\n[seq={}]\nFrom: {}\nSubject: {}\nPreview: {}\n",
            seq,
            from,
            subject,
            &body[..body.len().min(500)],
        ));
    }

    prompt
}

/// Parse the LLM classification response.
fn parse_classification_response(text: &str) -> Vec<(u32, ClassificationResponse)> {
    // Use the shared JSON array extractor — handles markdown fences and surrounding prose.
    let json_str = match crate::extract_json_array(text) {
        Some(s) => s,
        None => {
            tracing::warn!("Triage: no JSON array found in LLM classification response");
            return vec![];
        }
    };

    #[derive(Deserialize)]
    struct SeqClassification {
        seq: u32,
        #[serde(flatten)]
        classification: ClassificationResponse,
    }

    match serde_json::from_str::<Vec<SeqClassification>>(json_str) {
        Ok(results) => results
            .into_iter()
            .map(|r| (r.seq, r.classification))
            .collect(),
        Err(e) => {
            tracing::warn!("Triage: failed to parse LLM classification: {e}");
            vec![]
        }
    }
}

fn parse_urgency(s: &str) -> Urgency {
    match s.to_ascii_lowercase().as_str() {
        "low" => Urgency::Low,
        "high" => Urgency::High,
        "urgent" => Urgency::Urgent,
        _ => Urgency::Normal,
    }
}

// ── Main triage function ─────────────────────────────────────

/// Run one triage tick: process new emails since last cursor.
///
/// Returns a summary of actions taken. The caller (loop tick) is
/// responsible for emitting notifications based on the results.
pub async fn triage_inbox(
    config: &TriageConfig,
    email_config: &EmailConfig,
    imap_pool: Option<&std::sync::Arc<aivyx_actions::email::ImapPool>>,
    provider: &dyn LlmProvider,
    store: &EncryptedStore,
    key: &MasterKey,
) -> TriageTickSummary {
    let cursor = load_cursor(store, key);
    let mut summary = TriageTickSummary::default();

    // Fetch unread emails (prefer pooled connection)
    let emails = match if let Some(pool) = imap_pool {
        aivyx_actions::retry::retry(
            &aivyx_actions::retry::RetryConfig::network(),
            || pool.fetch_inbox(config.max_per_tick, true),
            aivyx_actions::retry::is_transient,
        )
        .await
    } else {
        aivyx_actions::retry::retry(
            &aivyx_actions::retry::RetryConfig::network(),
            || aivyx_actions::email::fetch_inbox_internal(email_config, config.max_per_tick, true),
            aivyx_actions::retry::is_transient,
        )
        .await
    } {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("Triage: failed to fetch inbox: {e}");
            return summary;
        }
    };

    // Filter to emails we haven't triaged yet
    let new_emails: Vec<&EmailSummary> = emails.iter().filter(|e| e.seq > cursor).collect();

    if new_emails.is_empty() {
        return summary;
    }

    tracing::info!("Triage: {} new email(s) to process", new_emails.len());

    let mut highest_seq = cursor;
    let mut to_classify: Vec<(u32, String, String, String)> = Vec::new(); // (seq, from, subject, preview)

    // ── Auto-reply loop protection ──
    // Cap auto-replies per tick to prevent runaway loops.
    const MAX_AUTO_REPLIES_PER_TICK: usize = 3;
    let mut auto_replies_this_tick: usize = 0;
    // Track senders we've already replied to this tick (dedup).
    let mut replied_senders: std::collections::HashSet<String> = std::collections::HashSet::new();
    // Our own email address — never auto-reply to ourselves.
    let own_address = email_config.address.to_lowercase();

    for email in &new_emails {
        highest_seq = highest_seq.max(email.seq);

        // Check ignore list
        if should_ignore(&email.from, &config.ignore_senders) {
            let result = TriageResult {
                seq: email.seq,
                from: email.from.clone(),
                subject: email.subject.clone(),
                action: TriageAction::Ignored,
                timestamp: Utc::now(),
            };
            save_triage_log(store, key, &result);
            summary.ignored += 1;
            summary.processed += 1;
            continue;
        }

        // Check auto-reply rules
        if config.can_auto_reply
            && let Some(rule) =
                find_auto_reply_rule(&email.from, &email.subject, &config.auto_reply_rules)
        {
            // Extract sender's email address for reply
            if let Some(reply_to) = extract_email_address(&email.from) {
                // ── Loop protection checks ──

                // 1. Never reply to our own address
                if reply_to.to_lowercase() == own_address {
                    tracing::debug!("Triage: skipping auto-reply to self ({})", reply_to);
                    // Fall through to classification
                }
                // 2. Per-tick cap reached
                else if auto_replies_this_tick >= MAX_AUTO_REPLIES_PER_TICK {
                    tracing::info!(
                        "Triage: auto-reply cap reached ({MAX_AUTO_REPLIES_PER_TICK}/tick), \
                             skipping reply to '{}'",
                        email.from,
                    );
                    // Fall through to classification
                }
                // 3. Already replied to this sender this tick
                else if replied_senders.contains(&reply_to.to_lowercase()) {
                    tracing::debug!(
                        "Triage: already replied to '{}' this tick, skipping duplicate",
                        reply_to,
                    );
                    // Fall through to classification
                }
                // 4. Deep reply chain (3+ Re: prefixes) suggests a loop
                else if count_reply_depth(&email.subject) >= 3 {
                    tracing::info!(
                        "Triage: deep reply chain detected in '{}', skipping auto-reply",
                        email.subject,
                    );
                    // Fall through to classification
                } else {
                    // ── Safe to auto-reply ──

                    let subject = if email.subject.starts_with("Re:") {
                        email.subject.clone()
                    } else {
                        format!("Re: {}", email.subject)
                    };

                    match aivyx_actions::email::send_reply(
                        email_config,
                        &reply_to,
                        &subject,
                        &rule.reply_body,
                        email.message_id.as_deref(),
                    )
                    .await
                    {
                        Ok(()) => {
                            let result = TriageResult {
                                seq: email.seq,
                                from: email.from.clone(),
                                subject: email.subject.clone(),
                                action: TriageAction::AutoReplied {
                                    rule_name: rule.name.clone(),
                                },
                                timestamp: Utc::now(),
                            };
                            save_triage_log(store, key, &result);
                            summary.auto_replied += 1;
                            summary.processed += 1;
                            auto_replies_this_tick += 1;
                            replied_senders.insert(reply_to.to_lowercase());
                            tracing::info!(
                                "Triage: auto-replied to '{}' (rule: {})",
                                email.from,
                                rule.name
                            );
                            continue;
                        }
                        Err(e) => {
                            tracing::warn!("Triage: auto-reply failed for '{}': {e}", email.from);
                            // Fall through to classification
                        }
                    }
                } // end else (safe to auto-reply)
            }
        }

        // Queue for LLM classification
        to_classify.push((
            email.seq,
            email.from.clone(),
            email.subject.clone(),
            email.preview.clone(),
        ));
    }

    // LLM classification for remaining emails
    if !to_classify.is_empty() {
        let refs: Vec<(u32, &str, &str, &str)> = to_classify
            .iter()
            .map(|(seq, from, subj, body)| (*seq, from.as_str(), subj.as_str(), body.as_str()))
            .collect();

        let prompt = build_classification_prompt(&refs, &config.categories);
        let request = ChatRequest {
            system_prompt: Some(
                "You are an email triage assistant. Be concise and accurate.".into(),
            ),
            messages: vec![ChatMessage::user(prompt)],
            tools: vec![],
            model: None,
            max_tokens: 1024,
        };

        match aivyx_actions::retry::retry(
            &aivyx_actions::retry::RetryConfig::llm(),
            || async {
                tokio::time::timeout(std::time::Duration::from_secs(60), provider.chat(&request))
                    .await
                    .map_err(|_| aivyx_core::AivyxError::LlmProvider("triage timeout".into()))?
            },
            aivyx_actions::retry::is_transient,
        )
        .await
        {
            Ok(response) => {
                let text = response.message.content.to_text();
                let classifications = parse_classification_response(&text);

                // Build a lookup of classify items by seq
                let classify_map: std::collections::HashMap<u32, &(u32, String, String, String)> =
                    to_classify.iter().map(|item| (item.0, item)).collect();

                for (seq, classification) in &classifications {
                    let (_, from, subject, _) = classify_map
                        .get(seq)
                        .map(|item| (item.0, item.1.as_str(), item.2.as_str(), item.3.as_str()))
                        .unwrap_or((0, "unknown", "unknown", ""));

                    let urgency = parse_urgency(&classification.urgency);

                    // Forward if LLM recommends and forwarding is permitted
                    if classification.should_forward
                        && config.can_forward
                        && let Some(ref fwd_to) = config.forward_to
                    {
                        let fwd_subject = format!("[FWD by Agent] {}", subject);
                        let fwd_body = format!(
                            "Forwarded by your AI assistant.\n\n\
                                 Original from: {}\n\
                                 Category: {} | Urgency: {}\n\
                                 Summary: {}\n\n\
                                 ---\n\
                                 (Original preview: see inbox seq={})",
                            from, classification.category, urgency, classification.summary, seq,
                        );

                        if let Err(e) = aivyx_actions::email::send_reply(
                            email_config,
                            fwd_to,
                            &fwd_subject,
                            &fwd_body,
                            None,
                        )
                        .await
                        {
                            tracing::warn!("Triage: forward failed for seq={seq}: {e}");
                        } else {
                            let result = TriageResult {
                                seq: *seq,
                                from: from.to_string(),
                                subject: subject.to_string(),
                                action: TriageAction::Forwarded { to: fwd_to.clone() },
                                timestamp: Utc::now(),
                            };
                            save_triage_log(store, key, &result);
                            summary.forwarded += 1;
                            summary.processed += 1;
                            continue;
                        }
                    }

                    // Store classification
                    let result = TriageResult {
                        seq: *seq,
                        from: from.to_string(),
                        subject: subject.to_string(),
                        action: TriageAction::Classified {
                            category: classification.category.clone(),
                            urgency,
                            summary: classification.summary.clone(),
                        },
                        timestamp: Utc::now(),
                    };
                    save_triage_log(store, key, &result);
                    summary.classified += 1;
                    summary.processed += 1;
                }

                // Handle emails that the LLM didn't return classification for
                for item in &to_classify {
                    if !classifications.iter().any(|(seq, _)| *seq == item.0) {
                        let result = TriageResult {
                            seq: item.0,
                            from: item.1.clone(),
                            subject: item.2.clone(),
                            action: TriageAction::Error {
                                reason: "LLM did not classify this email".into(),
                            },
                            timestamp: Utc::now(),
                        };
                        save_triage_log(store, key, &result);
                        summary.errors += 1;
                        summary.processed += 1;
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Triage: LLM classification failed: {e}");
                for item in &to_classify {
                    let result = TriageResult {
                        seq: item.0,
                        from: item.1.clone(),
                        subject: item.2.clone(),
                        action: TriageAction::Error {
                            reason: format!("LLM error: {e}"),
                        },
                        timestamp: Utc::now(),
                    };
                    save_triage_log(store, key, &result);
                    summary.errors += 1;
                    summary.processed += 1;
                }
            }
        }
    }

    // Advance cursor
    if highest_seq > cursor {
        save_cursor(store, key, highest_seq);
        tracing::info!(
            "Triage: cursor advanced {} → {} ({} processed: {} classified, {} auto-replied, {} forwarded, {} ignored, {} errors)",
            cursor,
            highest_seq,
            summary.processed,
            summary.classified,
            summary.auto_replied,
            summary.forwarded,
            summary.ignored,
            summary.errors,
        );
    }

    summary
}

/// Extract a bare email address from a "Name <addr>" or "addr" string.
fn extract_email_address(from: &str) -> Option<String> {
    if let Some(start) = from.find('<')
        && let Some(end) = from.find('>')
    {
        return Some(from[start + 1..end].to_string());
    }
    // Bare address
    if from.contains('@') {
        Some(from.trim().to_string())
    } else {
        None
    }
}

// ── Tests ────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_reply_rule_matches_sender() {
        let rule = AutoReplyRule {
            name: "noreply".into(),
            sender_contains: Some("noreply@".into()),
            subject_contains: None,
            reply_body: "Thanks!".into(),
        };
        assert!(rule.matches("No-Reply <noreply@example.com>", "Hello"));
        assert!(!rule.matches("alice@example.com", "Hello"));
    }

    #[test]
    fn auto_reply_rule_matches_subject() {
        let rule = AutoReplyRule {
            name: "meeting".into(),
            sender_contains: None,
            subject_contains: Some("meeting request".into()),
            reply_body: "I'll check my schedule.".into(),
        };
        assert!(rule.matches("alice@example.com", "Meeting Request for Monday"));
        assert!(!rule.matches("alice@example.com", "Hello"));
    }

    #[test]
    fn auto_reply_rule_case_insensitive() {
        let rule = AutoReplyRule {
            name: "test".into(),
            sender_contains: Some("BOSS@CORP.COM".into()),
            subject_contains: None,
            reply_body: "Acknowledged.".into(),
        };
        assert!(rule.matches("boss@corp.com", "Update"));
    }

    #[test]
    fn auto_reply_rule_needs_at_least_one_condition() {
        let rule = AutoReplyRule {
            name: "empty".into(),
            sender_contains: None,
            subject_contains: None,
            reply_body: "Hi".into(),
        };
        assert!(!rule.matches("anyone@anywhere.com", "Anything"));
    }

    #[test]
    fn ignore_senders_case_insensitive() {
        assert!(should_ignore(
            "Newsletter <news@SPAM.com>",
            &["spam.com".into()]
        ));
        assert!(!should_ignore("alice@work.com", &["spam.com".into()]));
    }

    #[test]
    fn extract_email_from_display_name() {
        assert_eq!(
            extract_email_address("Alice Smith <alice@example.com>"),
            Some("alice@example.com".into())
        );
    }

    #[test]
    fn extract_email_bare_address() {
        assert_eq!(
            extract_email_address("alice@example.com"),
            Some("alice@example.com".into())
        );
    }

    #[test]
    fn extract_email_no_address() {
        assert_eq!(extract_email_address("just a name"), None);
    }

    #[test]
    fn parse_classification_valid() {
        let json = r#"[
            {"seq": 42, "category": "work", "urgency": "high", "summary": "Q2 report from boss", "should_forward": true},
            {"seq": 43, "category": "newsletter", "urgency": "low", "summary": "Weekly digest"}
        ]"#;
        let results = parse_classification_response(json);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, 42);
        assert_eq!(results[0].1.category, "work");
        assert!(results[0].1.should_forward);
        assert_eq!(results[1].0, 43);
        assert_eq!(results[1].1.urgency, "low");
    }

    #[test]
    fn parse_classification_with_fences() {
        let text = "```json\n[{\"seq\": 1, \"category\": \"spam\", \"urgency\": \"low\", \"summary\": \"junk\"}]\n```";
        let results = parse_classification_response(text);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn parse_classification_invalid_falls_back() {
        let results = parse_classification_response("not json at all");
        assert!(results.is_empty());
    }

    #[test]
    fn parse_urgency_variants() {
        assert_eq!(parse_urgency("low"), Urgency::Low);
        assert_eq!(parse_urgency("HIGH"), Urgency::High);
        assert_eq!(parse_urgency("urgent"), Urgency::Urgent);
        assert_eq!(parse_urgency("Normal"), Urgency::Normal);
        assert_eq!(parse_urgency("unknown"), Urgency::Normal);
    }

    #[test]
    fn build_prompt_includes_categories() {
        let emails = vec![(1, "alice@test.com", "Hello", "Hi there")];
        let prompt = build_classification_prompt(&emails, &["work".into(), "personal".into()]);
        assert!(prompt.contains("work, personal"));
        assert!(prompt.contains("alice@test.com"));
    }

    #[test]
    fn build_prompt_uses_defaults_when_no_categories() {
        let emails = vec![(1, "alice@test.com", "Hello", "Hi there")];
        let prompt = build_classification_prompt(&emails, &[]);
        assert!(prompt.contains("personal"));
        assert!(prompt.contains("newsletter"));
    }

    #[test]
    fn triage_config_defaults() {
        let config = TriageConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.max_per_tick, 10);
        assert!(!config.can_auto_reply);
        assert!(!config.can_forward);
    }

    #[test]
    fn find_rule_returns_first_match() {
        let rules = vec![
            AutoReplyRule {
                name: "first".into(),
                sender_contains: Some("alice".into()),
                subject_contains: None,
                reply_body: "Reply 1".into(),
            },
            AutoReplyRule {
                name: "second".into(),
                sender_contains: Some("alice".into()),
                subject_contains: None,
                reply_body: "Reply 2".into(),
            },
        ];
        let matched = find_auto_reply_rule("alice@test.com", "Hello", &rules);
        assert_eq!(matched.unwrap().name, "first");
    }

    #[test]
    fn reply_depth_none() {
        assert_eq!(count_reply_depth("Hello world"), 0);
    }

    #[test]
    fn reply_depth_one() {
        assert_eq!(count_reply_depth("Re: Hello world"), 1);
    }

    #[test]
    fn reply_depth_deep_chain() {
        assert_eq!(count_reply_depth("Re: Re: Re: Hello"), 3);
        assert_eq!(count_reply_depth("RE: RE: RE: RE: Meeting"), 4);
    }

    #[test]
    fn reply_depth_case_insensitive() {
        assert_eq!(count_reply_depth("RE: re: Re: Hello"), 3);
    }
}
