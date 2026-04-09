//! Undo system — record reversible actions and provide one-click rollback.
//!
//! Destructive tools (write_file, send_email, cancel_reminder, etc.) can record
//! an undo entry before executing. The user or agent can then list recent undoable
//! actions and reverse them within a configurable window (default 24h).
//!
//! Storage: EncryptedStore, domain key `"undo"`, keys `"undo:record:{id}"`.

use crate::Action;
use aivyx_core::{AivyxError, Result};
use aivyx_crypto::{EncryptedStore, MasterKey};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use zeroize::Zeroizing;

/// What can be undone and how.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UndoAction {
    /// Restore a file to its original content.
    RestoreFile {
        path: String,
        original_content: String,
    },
    /// Cancel a previously-set reminder.
    CancelReminder { reminder_id: String },
    /// Void a recorded transaction.
    VoidTransaction { transaction_id: String },
    /// Action that can only be undone manually (e.g. sent email).
    ManualOnly { instructions: String },
}

/// A recorded undo entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UndoRecord {
    pub id: String,
    pub tool_name: String,
    pub action_summary: String,
    pub undo_action: UndoAction,
    pub performed_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub undone: bool,
}

impl UndoRecord {
    /// Whether this record has expired and should be skipped.
    pub fn is_expired(&self) -> bool {
        Utc::now() > self.expires_at
    }
}

/// Shared context for undo tools — holds store reference and key bytes.
#[derive(Clone)]
pub struct UndoContext {
    store: Arc<EncryptedStore>,
    key_bytes: Zeroizing<Vec<u8>>,
}

impl UndoContext {
    pub fn new(store: Arc<EncryptedStore>, key: &MasterKey) -> Self {
        Self {
            store,
            key_bytes: Zeroizing::new(key.expose_secret().to_vec()),
        }
    }

    fn reconstruct_key(&self) -> Result<MasterKey> {
        let bytes: [u8; 32] = self
            .key_bytes
            .as_slice()
            .try_into()
            .map_err(|_| AivyxError::Other("undo key bytes not 32 bytes".into()))?;
        Ok(MasterKey::from_bytes(bytes))
    }

    fn record_key(id: &str) -> String {
        format!("undo:record:{id}")
    }

    /// Save an undo record to encrypted storage.
    pub fn save_record(&self, record: &UndoRecord) -> Result<()> {
        let key = self.reconstruct_key()?;
        let data = serde_json::to_vec(record)
            .map_err(|e| AivyxError::Other(format!("serialize undo record: {e}")))?;
        self.store
            .put(&Self::record_key(&record.id), &data, &key)
            .map_err(|e| AivyxError::Other(format!("store undo record: {e}")))?;
        Ok(())
    }

    /// Load an undo record by ID.
    pub fn load_record(&self, id: &str) -> Result<Option<UndoRecord>> {
        let key = self.reconstruct_key()?;
        match self.store.get(&Self::record_key(id), &key) {
            Ok(Some(data)) => {
                let record: UndoRecord = serde_json::from_slice(&data)
                    .map_err(|e| AivyxError::Other(format!("deserialize undo record: {e}")))?;
                Ok(Some(record))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(AivyxError::Other(format!("load undo record: {e}"))),
        }
    }

    /// List all non-expired, non-undone undo records.
    pub fn list_active_records(&self) -> Result<Vec<UndoRecord>> {
        let key = self.reconstruct_key()?;
        let keys = self
            .store
            .list_keys()
            .map_err(|e| AivyxError::Other(format!("list undo keys: {e}")))?;

        let mut records = Vec::new();
        for store_key in keys {
            if !store_key.starts_with("undo:record:") {
                continue;
            }
            if let Ok(Some(data)) = self.store.get(&store_key, &key)
                && let Ok(record) = serde_json::from_slice::<UndoRecord>(&data)
                && !record.is_expired()
                && !record.undone
            {
                records.push(record);
            }
        }

        // Sort by most recent first
        records.sort_by(|a, b| b.performed_at.cmp(&a.performed_at));
        Ok(records)
    }

    /// Mark a record as undone.
    pub fn mark_undone(&self, id: &str) -> Result<()> {
        if let Some(mut record) = self.load_record(id)? {
            record.undone = true;
            self.save_record(&record)?;
        }
        Ok(())
    }
}

// ── Actions ─────────────────────────────────────────────────────

/// Record an undo entry before performing a destructive action.
pub struct RecordUndoAction {
    ctx: UndoContext,
}

impl RecordUndoAction {
    pub fn new(ctx: UndoContext) -> Self {
        Self { ctx }
    }
}

#[async_trait::async_trait]
impl Action for RecordUndoAction {
    fn name(&self) -> &str {
        "record_undo"
    }

    fn description(&self) -> &str {
        "Record an undo entry before a destructive action so it can be reversed later"
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "tool_name": {
                    "type": "string",
                    "description": "Name of the tool that will perform the destructive action"
                },
                "action_summary": {
                    "type": "string",
                    "description": "Brief description of what will be done"
                },
                "undo_type": {
                    "type": "string",
                    "enum": ["restore_file", "cancel_reminder", "void_transaction", "manual_only"],
                    "description": "Type of undo action"
                },
                "undo_data": {
                    "type": "object",
                    "description": "Data needed for undo: {path, original_content} for restore_file, {reminder_id} for cancel_reminder, {transaction_id} for void_transaction, {instructions} for manual_only"
                },
                "ttl_hours": {
                    "type": "integer",
                    "description": "Hours until this undo entry expires (default: 24)"
                }
            },
            "required": ["tool_name", "action_summary", "undo_type", "undo_data"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let tool_name = input["tool_name"].as_str().unwrap_or_default().to_string();
        let summary = input["action_summary"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let undo_type = input["undo_type"].as_str().unwrap_or_default();
        let undo_data = &input["undo_data"];
        let ttl_hours = input["ttl_hours"].as_i64().unwrap_or(24);

        let undo_action = match undo_type {
            "restore_file" => UndoAction::RestoreFile {
                path: undo_data["path"].as_str().unwrap_or_default().to_string(),
                original_content: undo_data["original_content"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            },
            "cancel_reminder" => UndoAction::CancelReminder {
                reminder_id: undo_data["reminder_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            },
            "void_transaction" => UndoAction::VoidTransaction {
                transaction_id: undo_data["transaction_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            },
            "manual_only" => UndoAction::ManualOnly {
                instructions: undo_data["instructions"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            },
            other => {
                return Err(AivyxError::Other(format!("unknown undo_type: {other}")));
            }
        };

        let now = Utc::now();
        let record = UndoRecord {
            id: uuid::Uuid::new_v4().to_string(),
            tool_name,
            action_summary: summary,
            undo_action,
            performed_at: now,
            expires_at: now + Duration::hours(ttl_hours),
            undone: false,
        };

        self.ctx.save_record(&record)?;

        Ok(serde_json::json!({
            "status": "recorded",
            "undo_id": record.id,
            "expires_at": record.expires_at.to_rfc3339(),
        }))
    }
}

/// List recent undoable actions.
pub struct ListUndoHistoryAction {
    ctx: UndoContext,
}

impl ListUndoHistoryAction {
    pub fn new(ctx: UndoContext) -> Self {
        Self { ctx }
    }
}

#[async_trait::async_trait]
impl Action for ListUndoHistoryAction {
    fn name(&self) -> &str {
        "list_undo_history"
    }

    fn description(&self) -> &str {
        "List recent actions that can be undone"
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of entries to return (default: 20)"
                }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let limit = input["limit"].as_u64().unwrap_or(20) as usize;
        let records = self.ctx.list_active_records()?;

        let entries: Vec<serde_json::Value> = records
            .into_iter()
            .take(limit)
            .map(|r| {
                let undo_type = match &r.undo_action {
                    UndoAction::RestoreFile { path, .. } => {
                        format!("restore_file ({})", path)
                    }
                    UndoAction::CancelReminder { reminder_id } => {
                        format!("cancel_reminder ({})", reminder_id)
                    }
                    UndoAction::VoidTransaction { transaction_id } => {
                        format!("void_transaction ({})", transaction_id)
                    }
                    UndoAction::ManualOnly { instructions } => {
                        format!("manual_only: {}", instructions)
                    }
                };
                serde_json::json!({
                    "id": r.id,
                    "tool": r.tool_name,
                    "summary": r.action_summary,
                    "undo_type": undo_type,
                    "performed_at": r.performed_at.to_rfc3339(),
                    "expires_at": r.expires_at.to_rfc3339(),
                })
            })
            .collect();

        Ok(serde_json::json!({
            "count": entries.len(),
            "entries": entries,
        }))
    }
}

/// Execute an undo action to reverse a previous operation.
pub struct UndoActionTool {
    ctx: UndoContext,
}

impl UndoActionTool {
    pub fn new(ctx: UndoContext) -> Self {
        Self { ctx }
    }
}

#[async_trait::async_trait]
impl Action for UndoActionTool {
    fn name(&self) -> &str {
        "undo_action"
    }

    fn description(&self) -> &str {
        "Undo a previous action by its undo ID"
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "undo_id": {
                    "type": "string",
                    "description": "The undo record ID to reverse"
                }
            },
            "required": ["undo_id"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let undo_id = input["undo_id"].as_str().unwrap_or_default();

        let record = self
            .ctx
            .load_record(undo_id)?
            .ok_or_else(|| AivyxError::Other(format!("undo record '{undo_id}' not found")))?;

        if record.undone {
            return Err(AivyxError::Other(
                "this action has already been undone".into(),
            ));
        }
        if record.is_expired() {
            return Err(AivyxError::Other(format!(
                "undo window expired at {}",
                record.expires_at.to_rfc3339()
            )));
        }

        let result = match &record.undo_action {
            UndoAction::RestoreFile {
                path,
                original_content,
            } => {
                // Validate path before writing — undo records contain user/LLM-provided
                // paths that could target sensitive locations if crafted maliciously.
                crate::files::validate_path(path)?;
                tokio::fs::write(path, original_content)
                    .await
                    .map_err(aivyx_core::AivyxError::Io)?;
                serde_json::json!({
                    "status": "restored",
                    "path": path,
                })
            }
            UndoAction::CancelReminder { reminder_id } => {
                // Reminder cancellation would need access to the reminder store.
                // For now, return instructions for the agent to cancel it.
                serde_json::json!({
                    "status": "pending",
                    "instruction": format!("Cancel reminder with ID: {reminder_id}"),
                    "note": "Use the delete_reminder tool to complete this undo",
                })
            }
            UndoAction::VoidTransaction { transaction_id } => {
                serde_json::json!({
                    "status": "pending",
                    "instruction": format!("Void transaction with ID: {transaction_id}"),
                    "note": "Use the appropriate finance tool to void this transaction",
                })
            }
            UndoAction::ManualOnly { instructions } => {
                serde_json::json!({
                    "status": "manual",
                    "instructions": instructions,
                    "note": "This action cannot be automatically undone",
                })
            }
        };

        self.ctx.mark_undone(undo_id)?;

        Ok(serde_json::json!({
            "undo_id": undo_id,
            "result": result,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_record(id: &str, hours_ago: i64, ttl_hours: i64) -> UndoRecord {
        let performed = Utc::now() - Duration::hours(hours_ago);
        UndoRecord {
            id: id.to_string(),
            tool_name: "write_file".to_string(),
            action_summary: "Overwrote config.txt".to_string(),
            undo_action: UndoAction::RestoreFile {
                path: "/tmp/config.txt".to_string(),
                original_content: "original data".to_string(),
            },
            performed_at: performed,
            expires_at: performed + Duration::hours(ttl_hours),
            undone: false,
        }
    }

    #[test]
    fn record_not_expired_within_window() {
        let record = test_record("r1", 1, 24);
        assert!(!record.is_expired());
    }

    #[test]
    fn record_expired_after_window() {
        let record = test_record("r2", 25, 24);
        assert!(record.is_expired());
    }

    #[test]
    fn undo_action_serialization_roundtrip() {
        let record = test_record("r3", 0, 24);
        let json = serde_json::to_string(&record).unwrap();
        let deserialized: UndoRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, "r3");
        assert_eq!(deserialized.tool_name, "write_file");
        assert!(!deserialized.undone);
        match deserialized.undo_action {
            UndoAction::RestoreFile { ref path, .. } => {
                assert_eq!(path, "/tmp/config.txt");
            }
            _ => panic!("expected RestoreFile"),
        }
    }

    #[test]
    fn manual_only_serialization() {
        let record = UndoRecord {
            id: "r4".to_string(),
            tool_name: "send_email".to_string(),
            action_summary: "Sent reply to boss".to_string(),
            undo_action: UndoAction::ManualOnly {
                instructions: "Email cannot be recalled. Ask recipient to disregard.".to_string(),
            },
            performed_at: Utc::now(),
            expires_at: Utc::now() + Duration::hours(24),
            undone: false,
        };
        let json = serde_json::to_string(&record).unwrap();
        let deserialized: UndoRecord = serde_json::from_str(&json).unwrap();
        match deserialized.undo_action {
            UndoAction::ManualOnly { ref instructions } => {
                assert!(instructions.contains("cannot be recalled"));
            }
            _ => panic!("expected ManualOnly"),
        }
    }

    #[test]
    fn all_undo_variants_serialize() {
        let variants = vec![
            UndoAction::RestoreFile {
                path: "/test".into(),
                original_content: "data".into(),
            },
            UndoAction::CancelReminder {
                reminder_id: "rem-1".into(),
            },
            UndoAction::VoidTransaction {
                transaction_id: "txn-1".into(),
            },
            UndoAction::ManualOnly {
                instructions: "manual step".into(),
            },
        ];
        for variant in variants {
            let json = serde_json::to_string(&variant).unwrap();
            let _: UndoAction = serde_json::from_str(&json).unwrap();
        }
    }
}
