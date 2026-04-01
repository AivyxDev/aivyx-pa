//! Reminders — time-based triggers stored in the encrypted local database.

use crate::Action;
use aivyx_core::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reminder {
    pub id: String,
    pub message: String,
    pub due_at: DateTime<Utc>,
    pub completed: bool,
}

pub struct SetReminder;

#[async_trait::async_trait]
impl Action for SetReminder {
    fn name(&self) -> &str { "set_reminder" }

    fn description(&self) -> &str {
        "Set a reminder for a specific date/time"
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "message": { "type": "string", "description": "What to remind about" },
                "due_at": { "type": "string", "description": "ISO 8601 datetime when the reminder fires" }
            },
            "required": ["message", "due_at"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let message = input["message"].as_str().unwrap_or_default();
        let due_at_str = input["due_at"].as_str().unwrap_or_default();

        let due_at: DateTime<Utc> = due_at_str.parse().map_err(|_| {
            aivyx_core::AivyxError::Validation("invalid ISO 8601 datetime".into())
        })?;

        let id = Uuid::new_v4().to_string();

        let reminder = Reminder {
            id: id.clone(),
            message: message.to_string(),
            due_at,
            completed: false,
        };

        // TODO: persist to encrypted store
        let _ = reminder;

        Ok(serde_json::json!({
            "status": "set",
            "id": id,
            "message": message,
            "due_at": due_at.to_rfc3339(),
        }))
    }
}

pub struct ListReminders;

#[async_trait::async_trait]
impl Action for ListReminders {
    fn name(&self) -> &str { "list_reminders" }

    fn description(&self) -> &str {
        "List all pending reminders"
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }

    async fn execute(&self, _input: serde_json::Value) -> Result<serde_json::Value> {
        // TODO: read from encrypted store
        let reminders: Vec<Reminder> = vec![];
        Ok(serde_json::to_value(reminders).unwrap())
    }
}
