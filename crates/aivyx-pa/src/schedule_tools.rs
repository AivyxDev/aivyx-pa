//! Schedule management tools — CRUD for `[[schedules]]` in config.toml.
//!
//! These tools let the agent create, edit, and delete cron-based schedules
//! conversationally. Each tool writes directly to config.toml using the
//! same atomic-write pattern as the settings module.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;

use aivyx_config::schedule::validate_cron;
use aivyx_core::{AivyxError, CapabilityScope, Result, Tool, ToolId};

use crate::settings;

// ---------------------------------------------------------------------------
// ScheduleCreateTool
// ---------------------------------------------------------------------------

/// Tool that creates a new `[[schedules]]` entry in config.toml.
pub struct ScheduleCreateTool {
    id: ToolId,
    config_path: Arc<PathBuf>,
}

impl ScheduleCreateTool {
    pub fn new(config_path: Arc<PathBuf>) -> Self {
        Self {
            id: ToolId::new(),
            config_path,
        }
    }
}

#[async_trait]
impl Tool for ScheduleCreateTool {
    fn id(&self) -> ToolId {
        self.id
    }
    fn name(&self) -> &str {
        "schedule_create"
    }

    fn description(&self) -> &str {
        "Create a new recurring scheduled task. The schedule fires an agent turn \
         on a cron timer and surfaces the result as a notification. Use standard \
         5-field cron syntax (minute hour dom month dow). Examples: \
         '0 7 * * *' = daily at 7 AM, '0 9 * * 1-5' = weekday mornings, \
         '*/30 * * * *' = every 30 minutes."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Unique slug-style name (e.g., 'evening-review', 'check-email-hourly')"
                },
                "cron": {
                    "type": "string",
                    "description": "5-field cron expression (minute hour dom month dow)"
                },
                "prompt": {
                    "type": "string",
                    "description": "The prompt sent to the agent when the schedule fires"
                },
                "notify": {
                    "type": "boolean",
                    "description": "Whether to surface the result as a notification. Default: true"
                }
            },
            "required": ["name", "cron", "prompt"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Custom("admin".into()))
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let name = input["name"]
            .as_str()
            .ok_or_else(|| AivyxError::Validation("schedule_create: missing 'name'".into()))?;
        let cron = input["cron"]
            .as_str()
            .ok_or_else(|| AivyxError::Validation("schedule_create: missing 'cron'".into()))?;
        let prompt = input["prompt"]
            .as_str()
            .ok_or_else(|| AivyxError::Validation("schedule_create: missing 'prompt'".into()))?;
        let notify = input["notify"].as_bool().unwrap_or(true);

        // Validate name is slug-like
        if name.is_empty() || name.len() > 64 {
            return Err(AivyxError::Validation(
                "schedule name must be 1-64 characters".into(),
            ));
        }
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(AivyxError::Validation(
                "schedule name must be slug-style (alphanumeric, hyphens, underscores)".into(),
            ));
        }

        // Validate cron expression
        validate_cron(cron)?;

        // Validate prompt is non-empty
        if prompt.trim().is_empty() {
            return Err(AivyxError::Validation(
                "schedule prompt must not be empty".into(),
            ));
        }

        // Check for duplicate name
        if settings::schedule_exists(&self.config_path, name) {
            return Err(AivyxError::Validation(format!(
                "schedule '{name}' already exists — use schedule_edit to modify it"
            )));
        }

        settings::append_schedule(&self.config_path, name, cron, prompt, notify)
            .map_err(|e| AivyxError::Other(format!("failed to write schedule: {e}")))?;

        Ok(serde_json::json!({
            "status": "created",
            "name": name,
            "cron": cron,
            "prompt": prompt,
            "notify": notify,
        }))
    }
}

// ---------------------------------------------------------------------------
// ScheduleEditTool
// ---------------------------------------------------------------------------

/// Tool that edits an existing `[[schedules]]` entry in config.toml.
pub struct ScheduleEditTool {
    id: ToolId,
    config_path: Arc<PathBuf>,
}

impl ScheduleEditTool {
    pub fn new(config_path: Arc<PathBuf>) -> Self {
        Self {
            id: ToolId::new(),
            config_path,
        }
    }
}

#[async_trait]
impl Tool for ScheduleEditTool {
    fn id(&self) -> ToolId {
        self.id
    }
    fn name(&self) -> &str {
        "schedule_edit"
    }

    fn description(&self) -> &str {
        "Edit an existing scheduled task. You can change the cron expression, prompt, \
         enabled state, or notification behavior. Only the fields you provide will be \
         updated — others remain unchanged."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name of the schedule to edit"
                },
                "cron": {
                    "type": "string",
                    "description": "New cron expression (optional)"
                },
                "prompt": {
                    "type": "string",
                    "description": "New prompt text (optional)"
                },
                "enabled": {
                    "type": "boolean",
                    "description": "Enable or disable the schedule (optional)"
                },
                "notify": {
                    "type": "boolean",
                    "description": "Whether to surface results as notifications (optional)"
                }
            },
            "required": ["name"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Custom("admin".into()))
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let name = input["name"]
            .as_str()
            .ok_or_else(|| AivyxError::Validation("schedule_edit: missing 'name'".into()))?;

        if !settings::schedule_exists(&self.config_path, name) {
            return Err(AivyxError::Validation(format!(
                "schedule '{name}' not found — use schedule_create to add it"
            )));
        }

        let mut changed = Vec::new();

        if let Some(cron) = input["cron"].as_str() {
            validate_cron(cron)?;
            settings::edit_schedule_field(&self.config_path, name, "cron", &format!("\"{cron}\""))
                .map_err(|e| AivyxError::Other(format!("failed to update cron: {e}")))?;
            changed.push(format!("cron → {cron}"));
        }

        if let Some(prompt) = input["prompt"].as_str() {
            if prompt.trim().is_empty() {
                return Err(AivyxError::Validation("prompt must not be empty".into()));
            }
            let escaped = prompt.replace('\\', "\\\\").replace('"', "\\\"");
            settings::edit_schedule_field(
                &self.config_path,
                name,
                "prompt",
                &format!("\"{escaped}\""),
            )
            .map_err(|e| AivyxError::Other(format!("failed to update prompt: {e}")))?;
            changed.push("prompt updated".into());
        }

        if let Some(enabled) = input["enabled"].as_bool() {
            settings::toggle_schedule_enabled(&self.config_path, name, enabled)
                .map_err(|e| AivyxError::Other(format!("failed to update enabled: {e}")))?;
            changed.push(format!("enabled → {enabled}"));
        }

        if let Some(notify) = input["notify"].as_bool() {
            settings::edit_schedule_field(&self.config_path, name, "notify", &notify.to_string())
                .map_err(|e| AivyxError::Other(format!("failed to update notify: {e}")))?;
            changed.push(format!("notify → {notify}"));
        }

        if changed.is_empty() {
            return Ok(serde_json::json!({
                "status": "no_changes",
                "name": name,
                "message": "No fields provided to update. Pass cron, prompt, enabled, or notify.",
            }));
        }

        Ok(serde_json::json!({
            "status": "updated",
            "name": name,
            "changes": changed,
        }))
    }
}

// ---------------------------------------------------------------------------
// ScheduleDeleteTool
// ---------------------------------------------------------------------------

/// Tool that removes a `[[schedules]]` entry from config.toml.
pub struct ScheduleDeleteTool {
    id: ToolId,
    config_path: Arc<PathBuf>,
}

impl ScheduleDeleteTool {
    pub fn new(config_path: Arc<PathBuf>) -> Self {
        Self {
            id: ToolId::new(),
            config_path,
        }
    }
}

#[async_trait]
impl Tool for ScheduleDeleteTool {
    fn id(&self) -> ToolId {
        self.id
    }
    fn name(&self) -> &str {
        "schedule_delete"
    }

    fn description(&self) -> &str {
        "Delete a scheduled task permanently. The schedule will stop firing \
         immediately. This cannot be undone — to temporarily stop a schedule, \
         use schedule_edit with enabled=false instead."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name of the schedule to delete"
                }
            },
            "required": ["name"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Custom("admin".into()))
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let name = input["name"]
            .as_str()
            .ok_or_else(|| AivyxError::Validation("schedule_delete: missing 'name'".into()))?;

        if !settings::schedule_exists(&self.config_path, name) {
            return Err(AivyxError::Validation(format!(
                "schedule '{name}' not found"
            )));
        }

        settings::remove_schedule(&self.config_path, name)
            .map_err(|e| AivyxError::Other(format!("failed to remove schedule: {e}")))?;

        Ok(serde_json::json!({
            "status": "deleted",
            "name": name,
        }))
    }
}
