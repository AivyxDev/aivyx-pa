//! Triage tools — user-facing actions for viewing and managing email triage.
//!
//! These tools let the user (via the agent) inspect what autonomous triage
//! has done and configure rules on the fly.

use crate::Action;
use aivyx_core::Result;
use aivyx_crypto::{EncryptedStore, MasterKey};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

const TRIAGE_LOG_PREFIX: &str = "triage-log:";
const TRIAGE_RULES_KEY: &str = "triage-custom-rules";

// ── List triage log ──────────────────────────────────────────

pub struct ListTriageLog {
    pub store: Arc<EncryptedStore>,
    pub key: MasterKey,
}

#[async_trait::async_trait]
impl Action for ListTriageLog {
    fn name(&self) -> &str {
        "list_triage_log"
    }

    fn description(&self) -> &str {
        "Show recent email triage activity — what the agent did autonomously with incoming emails."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "limit": {
                    "type": "integer",
                    "description": "Maximum entries to return (default 20)",
                    "default": 20
                }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let limit = input.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;

        let keys = self.store.list_keys()?;
        let mut entries: Vec<serde_json::Value> = keys
            .iter()
            .filter(|k| k.starts_with(TRIAGE_LOG_PREFIX))
            .filter_map(|k| {
                self.store
                    .get(k, &self.key)
                    .ok()
                    .flatten()
                    .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            })
            .collect();

        // Sort by timestamp descending (most recent first)
        entries.sort_by(|a, b| {
            let ta = a.get("timestamp").and_then(|v| v.as_str()).unwrap_or("");
            let tb = b.get("timestamp").and_then(|v| v.as_str()).unwrap_or("");
            tb.cmp(ta)
        });

        entries.truncate(limit);

        Ok(serde_json::json!({
            "count": entries.len(),
            "entries": entries,
        }))
    }
}

// ── Set triage rule ──────────────────────────────────────────

/// A custom auto-reply rule stored in the encrypted store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredRule {
    pub name: String,
    #[serde(default)]
    pub sender_contains: Option<String>,
    #[serde(default)]
    pub subject_contains: Option<String>,
    pub reply_body: String,
}

pub struct SetTriageRule {
    pub store: Arc<EncryptedStore>,
    pub key: MasterKey,
}

#[async_trait::async_trait]
impl Action for SetTriageRule {
    fn name(&self) -> &str {
        "set_triage_rule"
    }

    fn description(&self) -> &str {
        "Add or update an auto-reply triage rule. Rules are matched on sender or subject."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "Rule name (unique identifier)" },
                "sender_contains": { "type": "string", "description": "Match emails where sender contains this string" },
                "subject_contains": { "type": "string", "description": "Match emails where subject contains this string" },
                "reply_body": { "type": "string", "description": "Auto-reply message body" }
            },
            "required": ["name", "reply_body"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let name = input["name"].as_str().unwrap_or("").to_string();
        let sender_contains = input
            .get("sender_contains")
            .and_then(|v| v.as_str())
            .map(String::from);
        let subject_contains = input
            .get("subject_contains")
            .and_then(|v| v.as_str())
            .map(String::from);
        let reply_body = input["reply_body"].as_str().unwrap_or("").to_string();

        if name.is_empty() || reply_body.is_empty() {
            return Err(aivyx_core::AivyxError::Validation(
                "name and reply_body are required".into(),
            ));
        }

        if sender_contains.is_none() && subject_contains.is_none() {
            return Err(aivyx_core::AivyxError::Validation(
                "at least one of sender_contains or subject_contains must be set".into(),
            ));
        }

        // Load existing rules
        let mut rules: Vec<StoredRule> = self
            .store
            .get(TRIAGE_RULES_KEY, &self.key)
            .ok()
            .flatten()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();

        // Upsert: replace existing rule with same name, or append
        let new_rule = StoredRule {
            name: name.clone(),
            sender_contains,
            subject_contains,
            reply_body,
        };

        if let Some(existing) = rules.iter_mut().find(|r| r.name == name) {
            *existing = new_rule;
        } else {
            rules.push(new_rule);
        }

        // Save
        let json = serde_json::to_vec(&rules).map_err(aivyx_core::AivyxError::Serialization)?;
        self.store.put(TRIAGE_RULES_KEY, &json, &self.key)?;

        Ok(serde_json::json!({
            "status": "saved",
            "name": name,
            "total_rules": rules.len(),
        }))
    }
}

/// Load custom triage rules from the encrypted store.
///
/// These are merged with config-file rules at runtime. Config rules
/// take precedence (matched first); stored rules are appended.
pub fn load_custom_rules(store: &EncryptedStore, key: &MasterKey) -> Vec<StoredRule> {
    store
        .get(TRIAGE_RULES_KEY, key)
        .ok()
        .flatten()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_rule_serialization_roundtrip() {
        let rule = StoredRule {
            name: "test".into(),
            sender_contains: Some("alice@".into()),
            subject_contains: None,
            reply_body: "Thanks!".into(),
        };
        let json = serde_json::to_vec(&rule).unwrap();
        let parsed: StoredRule = serde_json::from_slice(&json).unwrap();
        assert_eq!(parsed.name, "test");
        assert_eq!(parsed.sender_contains.unwrap(), "alice@");
        assert!(parsed.subject_contains.is_none());
    }
}
