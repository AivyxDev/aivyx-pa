//! Reminders — time-based triggers stored in the encrypted local database.
//!
//! Each reminder is stored in the EncryptedStore under `reminder:{uuid}`.
//! The loop checks for due reminders each tick and emits notifications.

use crate::Action;
use aivyx_core::Result;
use aivyx_crypto::{EncryptedStore, MasterKey};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

/// Key prefix for reminder entries in the encrypted store.
const REMINDER_PREFIX: &str = "reminder:";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reminder {
    pub id: String,
    pub message: String,
    pub due_at: DateTime<Utc>,
    pub completed: bool,
    pub created_at: DateTime<Utc>,
}

/// Persist a reminder to the store.
fn save_reminder(store: &EncryptedStore, key: &MasterKey, reminder: &Reminder) -> Result<()> {
    let json = serde_json::to_vec(reminder).map_err(aivyx_core::AivyxError::Serialization)?;
    store.put(&format!("{REMINDER_PREFIX}{}", reminder.id), &json, key)
}

/// Load all reminders from the store.
pub fn load_all_reminders(store: &EncryptedStore, key: &MasterKey) -> Result<Vec<Reminder>> {
    let keys = store.list_keys()?;
    let mut reminders = Vec::new();

    for store_key in &keys {
        if !store_key.starts_with(REMINDER_PREFIX) {
            continue;
        }
        if let Some(bytes) = store.get(store_key, key)? {
            match serde_json::from_slice::<Reminder>(&bytes) {
                Ok(r) => reminders.push(r),
                Err(e) => tracing::warn!("Corrupt reminder entry '{store_key}': {e}"),
            }
        }
    }

    // Sort by due date, earliest first
    reminders.sort_by_key(|r| r.due_at);
    Ok(reminders)
}

/// Load only pending (not completed) reminders that are due.
pub fn load_due_reminders(store: &EncryptedStore, key: &MasterKey) -> Result<Vec<Reminder>> {
    let now = Utc::now();
    let all = load_all_reminders(store, key)?;
    Ok(all
        .into_iter()
        .filter(|r| !r.completed && r.due_at <= now)
        .collect())
}

// ── SetReminder action ────────────────────────────────────────

pub struct SetReminder {
    pub store: Arc<EncryptedStore>,
    pub key: MasterKey,
}

#[async_trait::async_trait]
impl Action for SetReminder {
    fn name(&self) -> &str {
        "set_reminder"
    }

    fn description(&self) -> &str {
        "Set a reminder for a specific date/time. The reminder will fire as a notification when due."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "message": { "type": "string", "description": "What to remind about" },
                "due_at": { "type": "string", "description": "ISO 8601 datetime when the reminder fires (e.g. 2026-04-03T09:00:00Z)" }
            },
            "required": ["message", "due_at"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let message = input["message"].as_str().unwrap_or_default();
        let due_at_str = input["due_at"].as_str().unwrap_or_default();

        let due_at: DateTime<Utc> = due_at_str
            .parse()
            .map_err(|_| aivyx_core::AivyxError::Validation("invalid ISO 8601 datetime".into()))?;

        let id = Uuid::new_v4().to_string();

        let reminder = Reminder {
            id: id.clone(),
            message: message.to_string(),
            due_at,
            completed: false,
            created_at: Utc::now(),
        };

        save_reminder(&self.store, &self.key, &reminder)?;

        tracing::info!("Reminder set: '{}' due at {}", message, due_at.to_rfc3339());

        Ok(serde_json::json!({
            "status": "set",
            "id": id,
            "message": message,
            "due_at": due_at.to_rfc3339(),
        }))
    }
}

// ── ListReminders action ──────────────────────────────────────

pub struct ListReminders {
    pub store: Arc<EncryptedStore>,
    pub key: MasterKey,
}

#[async_trait::async_trait]
impl Action for ListReminders {
    fn name(&self) -> &str {
        "list_reminders"
    }

    fn description(&self) -> &str {
        "List all pending reminders (not yet completed)"
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "include_completed": { "type": "boolean", "description": "Include completed reminders (default: false)" }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let include_completed = input["include_completed"].as_bool().unwrap_or(false);

        let all = load_all_reminders(&self.store, &self.key)?;

        let filtered: Vec<&Reminder> = if include_completed {
            all.iter().collect()
        } else {
            all.iter().filter(|r| !r.completed).collect()
        };

        Ok(serde_json::to_value(filtered)?)
    }
}

// ── DismissReminder action ────────────────────────────────────

pub struct DismissReminder {
    pub store: Arc<EncryptedStore>,
    pub key: MasterKey,
}

#[async_trait::async_trait]
impl Action for DismissReminder {
    fn name(&self) -> &str {
        "dismiss_reminder"
    }

    fn description(&self) -> &str {
        "Mark a reminder as completed/dismissed"
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "id": { "type": "string", "description": "The reminder ID to dismiss" }
            },
            "required": ["id"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let id = input["id"].as_str().unwrap_or_default();
        let store_key = format!("{REMINDER_PREFIX}{id}");

        let bytes = self.store.get(&store_key, &self.key)?.ok_or_else(|| {
            aivyx_core::AivyxError::Validation(format!("reminder '{id}' not found"))
        })?;

        let mut reminder: Reminder =
            serde_json::from_slice(&bytes).map_err(aivyx_core::AivyxError::Serialization)?;

        reminder.completed = true;
        save_reminder(&self.store, &self.key, &reminder)?;

        tracing::info!("Reminder dismissed: '{}'", reminder.message);

        Ok(serde_json::json!({
            "status": "dismissed",
            "id": id,
            "message": reminder.message,
        }))
    }
}

/// Tool: update a reminder's message or due time.
pub struct UpdateReminder {
    pub store: Arc<EncryptedStore>,
    pub key: MasterKey,
}

#[async_trait::async_trait]
impl Action for UpdateReminder {
    fn name(&self) -> &str {
        "update_reminder"
    }

    fn description(&self) -> &str {
        "Update a reminder's message or reschedule its due time. \
         Only the fields you provide will be changed."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["id"],
            "properties": {
                "id": { "type": "string", "description": "Reminder ID to update" },
                "message": { "type": "string", "description": "New reminder message" },
                "due_at": { "type": "string", "description": "New due time (ISO 8601)" }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let id = input["id"]
            .as_str()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("'id' is required".into()))?;

        let store_key = format!("{REMINDER_PREFIX}{id}");
        let bytes = self.store.get(&store_key, &self.key)?.ok_or_else(|| {
            aivyx_core::AivyxError::Validation(format!("Reminder '{id}' not found"))
        })?;
        let mut reminder: Reminder =
            serde_json::from_slice(&bytes).map_err(aivyx_core::AivyxError::Serialization)?;

        if let Some(msg) = input["message"].as_str() {
            reminder.message = msg.into();
        }
        if let Some(due) = input["due_at"].as_str() {
            reminder.due_at = due
                .parse::<DateTime<Utc>>()
                .map_err(|e| aivyx_core::AivyxError::Validation(format!("Invalid due_at: {e}")))?;
        }

        save_reminder(&self.store, &self.key, &reminder)?;

        Ok(serde_json::json!({
            "status": "updated",
            "id": id,
            "message": reminder.message,
            "due_at": reminder.due_at.to_rfc3339(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aivyx_crypto::MasterKey;

    fn setup() -> (Arc<EncryptedStore>, MasterKey, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("aivyx-reminder-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = EncryptedStore::open(dir.join("store.db")).unwrap();
        let key = MasterKey::generate();
        (Arc::new(store), key, dir)
    }

    fn cleanup(dir: std::path::PathBuf) {
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn set_and_list_reminder() {
        let (store, key, dir) = setup();
        let reminder_key = aivyx_crypto::derive_domain_key(&key, b"reminders");

        let setter = SetReminder {
            store: store.clone(),
            key: aivyx_crypto::derive_domain_key(&key, b"reminders"),
        };

        let result = setter
            .execute(serde_json::json!({
                "message": "Buy groceries",
                "due_at": "2026-12-25T10:00:00Z"
            }))
            .await
            .unwrap();

        assert_eq!(result["status"], "set");
        assert_eq!(result["message"], "Buy groceries");
        let id = result["id"].as_str().unwrap();

        // List should return the reminder
        let lister = ListReminders {
            store: store.clone(),
            key: aivyx_crypto::derive_domain_key(&key, b"reminders"),
        };
        let list = lister.execute(serde_json::json!({})).await.unwrap();
        let reminders: Vec<Reminder> = serde_json::from_value(list).unwrap();
        assert_eq!(reminders.len(), 1);
        assert_eq!(reminders[0].message, "Buy groceries");
        assert!(!reminders[0].completed);

        // Verify it's actually in the store
        let all = load_all_reminders(&store, &reminder_key).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, id);

        cleanup(dir);
    }

    #[tokio::test]
    async fn dismiss_reminder() {
        let (store, key, dir) = setup();

        let setter = SetReminder {
            store: store.clone(),
            key: aivyx_crypto::derive_domain_key(&key, b"reminders"),
        };

        let result = setter
            .execute(serde_json::json!({
                "message": "Call dentist",
                "due_at": "2026-06-01T14:00:00Z"
            }))
            .await
            .unwrap();
        let id = result["id"].as_str().unwrap().to_string();

        // Dismiss it
        let dismisser = DismissReminder {
            store: store.clone(),
            key: aivyx_crypto::derive_domain_key(&key, b"reminders"),
        };
        let dismiss_result = dismisser
            .execute(serde_json::json!({
                "id": id
            }))
            .await
            .unwrap();
        assert_eq!(dismiss_result["status"], "dismissed");

        // List without completed should be empty
        let lister = ListReminders {
            store: store.clone(),
            key: aivyx_crypto::derive_domain_key(&key, b"reminders"),
        };
        let list = lister.execute(serde_json::json!({})).await.unwrap();
        let reminders: Vec<Reminder> = serde_json::from_value(list).unwrap();
        assert_eq!(reminders.len(), 0);

        // List with completed should show it
        let list = lister
            .execute(serde_json::json!({ "include_completed": true }))
            .await
            .unwrap();
        let reminders: Vec<Reminder> = serde_json::from_value(list).unwrap();
        assert_eq!(reminders.len(), 1);
        assert!(reminders[0].completed);

        cleanup(dir);
    }

    #[tokio::test]
    async fn load_due_reminders_filters_correctly() {
        let (store, key, dir) = setup();
        let reminder_key = aivyx_crypto::derive_domain_key(&key, b"reminders");

        // Past reminder — should be due
        let past = Reminder {
            id: Uuid::new_v4().to_string(),
            message: "Past task".into(),
            due_at: Utc::now() - chrono::Duration::hours(1),
            completed: false,
            created_at: Utc::now(),
        };
        save_reminder(&store, &reminder_key, &past).unwrap();

        // Future reminder — should NOT be due
        let future = Reminder {
            id: Uuid::new_v4().to_string(),
            message: "Future task".into(),
            due_at: Utc::now() + chrono::Duration::hours(24),
            completed: false,
            created_at: Utc::now(),
        };
        save_reminder(&store, &reminder_key, &future).unwrap();

        // Completed past reminder — should NOT be due
        let completed = Reminder {
            id: Uuid::new_v4().to_string(),
            message: "Done task".into(),
            due_at: Utc::now() - chrono::Duration::hours(2),
            completed: true,
            created_at: Utc::now(),
        };
        save_reminder(&store, &reminder_key, &completed).unwrap();

        let due = load_due_reminders(&store, &reminder_key).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].message, "Past task");

        let all = load_all_reminders(&store, &reminder_key).unwrap();
        assert_eq!(all.len(), 3);

        cleanup(dir);
    }

    #[tokio::test]
    async fn dismiss_nonexistent_reminder_fails() {
        let (store, key, dir) = setup();

        let dismisser = DismissReminder {
            store: store.clone(),
            key: aivyx_crypto::derive_domain_key(&key, b"reminders"),
        };
        let result = dismisser
            .execute(serde_json::json!({
                "id": "nonexistent-id-12345"
            }))
            .await;

        assert!(result.is_err());
        cleanup(dir);
    }

    #[tokio::test]
    async fn set_reminder_rejects_invalid_datetime() {
        let (store, key, dir) = setup();

        let setter = SetReminder {
            store: store.clone(),
            key: aivyx_crypto::derive_domain_key(&key, b"reminders"),
        };
        let result = setter
            .execute(serde_json::json!({
                "message": "Bad date",
                "due_at": "not-a-date"
            }))
            .await;

        assert!(result.is_err());
        cleanup(dir);
    }

    #[tokio::test]
    async fn update_reminder_reschedules() {
        let (store, key, dir) = setup();

        let setter = SetReminder {
            store: store.clone(),
            key: aivyx_crypto::derive_domain_key(&key, b"reminders"),
        };
        let result = setter
            .execute(serde_json::json!({
                "message": "Original message",
                "due_at": "2026-12-25T10:00:00Z"
            }))
            .await
            .unwrap();
        let id = result["id"].as_str().unwrap().to_string();

        let updater = UpdateReminder {
            store: store.clone(),
            key: aivyx_crypto::derive_domain_key(&key, b"reminders"),
        };
        let result = updater
            .execute(serde_json::json!({
                "id": id,
                "message": "Updated message",
                "due_at": "2026-12-26T15:00:00Z"
            }))
            .await
            .unwrap();
        assert_eq!(result["status"], "updated");
        assert_eq!(result["message"], "Updated message");

        let reminder_key = aivyx_crypto::derive_domain_key(&key, b"reminders");
        let all = load_all_reminders(&store, &reminder_key).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].message, "Updated message");
        assert!(all[0].due_at.to_rfc3339().contains("2026-12-26"));

        cleanup(dir);
    }

    #[test]
    fn update_reminder_schema() {
        let (store, key, dir) = setup();
        let tool = UpdateReminder {
            store: store.clone(),
            key: aivyx_crypto::derive_domain_key(&key, b"reminders"),
        };
        assert_eq!(tool.name(), "update_reminder");
        let schema = tool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "id"));
        cleanup(dir);
    }

    #[test]
    fn reminders_sorted_by_due_date() {
        let (store, key, dir) = setup();
        let reminder_key = aivyx_crypto::derive_domain_key(&key, b"reminders");

        let later = Reminder {
            id: Uuid::new_v4().to_string(),
            message: "Later".into(),
            due_at: Utc::now() + chrono::Duration::hours(48),
            completed: false,
            created_at: Utc::now(),
        };
        let sooner = Reminder {
            id: Uuid::new_v4().to_string(),
            message: "Sooner".into(),
            due_at: Utc::now() + chrono::Duration::hours(1),
            completed: false,
            created_at: Utc::now(),
        };

        // Insert in reverse order
        save_reminder(&store, &reminder_key, &later).unwrap();
        save_reminder(&store, &reminder_key, &sooner).unwrap();

        let all = load_all_reminders(&store, &reminder_key).unwrap();
        assert_eq!(all[0].message, "Sooner");
        assert_eq!(all[1].message, "Later");

        cleanup(dir);
    }
}
