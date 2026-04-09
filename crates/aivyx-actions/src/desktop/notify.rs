//! Desktop notification action — send system notifications via notify-send.

use crate::Action;
use aivyx_core::Result;

use super::{DEFAULT_TIMEOUT_SECS, run_desktop_command};

const VALID_URGENCIES: &[&str] = &["low", "normal", "critical"];

pub struct SendNotification;

#[async_trait::async_trait]
impl Action for SendNotification {
    fn name(&self) -> &str {
        "send_notification"
    }

    fn description(&self) -> &str {
        "Send a desktop notification to the user. Appears as a system notification popup. \
         Use for alerting the user about completed tasks, important updates, or \
         time-sensitive information. Supports urgency levels (low, normal, critical)."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "Notification title (short, descriptive)"
                },
                "body": {
                    "type": "string",
                    "description": "Notification body text"
                },
                "urgency": {
                    "type": "string",
                    "enum": ["low", "normal", "critical"],
                    "description": "Urgency level (default: normal)"
                },
                "icon": {
                    "type": "string",
                    "description": "Icon name or path (e.g., 'dialog-information')"
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Auto-dismiss timeout in milliseconds (0 = persistent)"
                }
            },
            "required": ["title", "body"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let title = input["title"]
            .as_str()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("title is required".into()))?;
        let body = input["body"]
            .as_str()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("body is required".into()))?;

        if title.trim().is_empty() {
            return Err(aivyx_core::AivyxError::Validation(
                "title must not be empty".into(),
            ));
        }

        let urgency = input
            .get("urgency")
            .and_then(|v| v.as_str())
            .unwrap_or("normal");

        if !VALID_URGENCIES.contains(&urgency) {
            return Err(aivyx_core::AivyxError::Validation(format!(
                "urgency must be one of: {}",
                VALID_URGENCIES.join(", ")
            )));
        }

        let mut args: Vec<String> = vec!["--urgency".into(), urgency.into()];

        if let Some(icon) = input.get("icon").and_then(|v| v.as_str()) {
            args.push(format!("--icon={icon}"));
        }

        if let Some(ms) = input.get("timeout_ms").and_then(|v| v.as_i64()) {
            args.push("-t".into());
            args.push(ms.to_string());
        }

        args.push(title.into());
        args.push(body.into());

        let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

        let output = run_desktop_command("notify-send", &arg_refs, DEFAULT_TIMEOUT_SECS).await?;

        if output.exit_code != 0 {
            return Err(aivyx_core::AivyxError::Other(format!(
                "notify-send failed (exit {}): {}",
                output.exit_code,
                output.stderr.trim()
            )));
        }

        Ok(serde_json::json!({
            "status": "sent",
            "title": title,
            "urgency": urgency,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_invalid_urgency() {
        let action = SendNotification;
        let result = action
            .execute(serde_json::json!({
                "title": "Test",
                "body": "Hello",
                "urgency": "mega-urgent",
            }))
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("urgency"), "error: {err}");
    }

    #[tokio::test]
    async fn rejects_empty_title() {
        let action = SendNotification;
        let result = action
            .execute(serde_json::json!({
                "title": "",
                "body": "Hello",
            }))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn rejects_missing_body() {
        let action = SendNotification;
        let result = action.execute(serde_json::json!({ "title": "Test" })).await;
        assert!(result.is_err());
    }
}
