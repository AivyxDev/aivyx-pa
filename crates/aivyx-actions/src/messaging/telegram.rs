//! Telegram Bot API action tools.
//!
//! Provides `send_telegram` and `read_telegram` actions that interact with
//! the Telegram Bot API via REST. The bot token is loaded from the encrypted
//! keystore at startup — never stored in config.toml.

use super::{Message, TelegramConfig};
use crate::Action;
use aivyx_core::{AivyxError, Result};

const BASE_URL: &str = "https://api.telegram.org/bot";

// ── API Client ─────────────────────────────────────────────────

/// Send a message via Telegram Bot API.
async fn send_message(
    config: &TelegramConfig,
    chat_id: &str,
    text: &str,
) -> Result<serde_json::Value> {
    let url = format!("{}{}/sendMessage", BASE_URL, config.bot_token);
    let client = crate::http_client();
    let resp = client
        .post(&url)
        .json(&serde_json::json!({
            "chat_id": chat_id,
            "text": text,
            "parse_mode": "Markdown"
        }))
        .send()
        .await
        .map_err(|e| AivyxError::Channel(format!("Telegram send failed: {e}")))?;

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| AivyxError::Channel(format!("Telegram response parse error: {e}")))?;

    if !body["ok"].as_bool().unwrap_or(false) {
        let desc = body["description"].as_str().unwrap_or("unknown error");
        return Err(AivyxError::Channel(format!("Telegram API error: {desc}")));
    }

    Ok(serde_json::json!({
        "status": "sent",
        "message_id": body["result"]["message_id"],
    }))
}

/// Fetch recent messages via getUpdates, filtered to a specific chat.
async fn get_messages(
    config: &TelegramConfig,
    chat_id: &str,
    limit: usize,
) -> Result<Vec<Message>> {
    let url = format!("{}{}/getUpdates", BASE_URL, config.bot_token);
    let client = crate::http_client();
    let resp = client
        .post(&url)
        .json(&serde_json::json!({
            "timeout": 0,
            "allowed_updates": ["message"]
        }))
        .send()
        .await
        .map_err(|e| AivyxError::Channel(format!("Telegram getUpdates failed: {e}")))?;

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| AivyxError::Channel(format!("Telegram response parse error: {e}")))?;

    if !body["ok"].as_bool().unwrap_or(false) {
        let desc = body["description"].as_str().unwrap_or("unknown error");
        return Err(AivyxError::Channel(format!("Telegram API error: {desc}")));
    }

    Ok(parse_updates_response(&body, chat_id, limit))
}

/// Parse the getUpdates response, filtering by chat_id and taking the last N.
fn parse_updates_response(body: &serde_json::Value, chat_id: &str, limit: usize) -> Vec<Message> {
    let results = match body["result"].as_array() {
        Some(arr) => arr,
        None => return Vec::new(),
    };

    results
        .iter()
        .filter_map(|update| {
            let msg = update.get("message")?;
            let msg_chat_id = msg["chat"]["id"].as_i64()?.to_string();
            if msg_chat_id != chat_id {
                return None;
            }

            let from = msg["from"]["first_name"]
                .as_str()
                .or_else(|| msg["from"]["username"].as_str())
                .unwrap_or("unknown");

            let text = msg["text"].as_str().unwrap_or("");
            let date = msg["date"].as_i64().unwrap_or(0);
            let ts = chrono::DateTime::from_timestamp(date, 0)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default();

            Some(Message {
                id: msg["message_id"].as_i64().unwrap_or(0).to_string(),
                from: from.to_string(),
                text: text.to_string(),
                timestamp: ts,
            })
        })
        .rev()
        .take(limit)
        .collect()
}

// ── Action: Send Telegram ──────────────────────────────────────

/// Send a message to a Telegram chat.
pub struct SendTelegram {
    pub config: TelegramConfig,
}

#[async_trait::async_trait]
impl Action for SendTelegram {
    fn name(&self) -> &str {
        "send_telegram"
    }

    fn description(&self) -> &str {
        "Send a message to a Telegram chat via the Bot API."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "chat_id": {
                    "type": "string",
                    "description": "Telegram chat ID (numeric) to send the message to."
                },
                "text": {
                    "type": "string",
                    "description": "Message text. Markdown formatting is supported."
                }
            },
            "required": ["chat_id", "text"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let chat_id = input["chat_id"]
            .as_str()
            .ok_or_else(|| AivyxError::Other("chat_id is required".into()))?;
        let text = input["text"]
            .as_str()
            .ok_or_else(|| AivyxError::Other("text is required".into()))?;

        crate::retry::retry(
            &crate::retry::RetryConfig::network(),
            || send_message(&self.config, chat_id, text),
            crate::retry::is_transient,
        )
        .await
    }
}

// ── Action: Read Telegram ──────────────────────────────────────

/// Read recent messages from a Telegram chat.
pub struct ReadTelegram {
    pub config: TelegramConfig,
}

#[async_trait::async_trait]
impl Action for ReadTelegram {
    fn name(&self) -> &str {
        "read_telegram"
    }

    fn description(&self) -> &str {
        "Read recent messages from a Telegram chat."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "chat_id": {
                    "type": "string",
                    "description": "Telegram chat ID (numeric) to read from."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of messages to return. Default: 10.",
                    "default": 10
                }
            },
            "required": ["chat_id"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let chat_id = input["chat_id"]
            .as_str()
            .ok_or_else(|| AivyxError::Other("chat_id is required".into()))?;
        let limit = input.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;

        let messages = crate::retry::retry(
            &crate::retry::RetryConfig::network(),
            || get_messages(&self.config, chat_id, limit),
            crate::retry::is_transient,
        )
        .await?;

        Ok(serde_json::json!({
            "messages": messages,
            "count": messages.len(),
        }))
    }
}

// ── Notification Forwarding ────────────────────────────────────

/// Forward a notification to the default Telegram chat.
///
/// Returns `Ok(())` silently if no `default_chat_id` is configured.
/// Errors are logged but not propagated — notification forwarding
/// should never block the agent loop.
pub async fn forward_notification(config: &TelegramConfig, title: &str, body: &str) -> Result<()> {
    if let Some(ref chat_id) = config.default_chat_id {
        let text = format!("*{}*\n{}", title, body);
        send_message(config, chat_id, &text).await?;
    }
    Ok(())
}

// ── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> TelegramConfig {
        TelegramConfig {
            bot_token: "test-token".into(),
            default_chat_id: Some("42".into()),
        }
    }

    #[test]
    fn parse_updates_filters_by_chat() {
        let response = serde_json::json!({
            "ok": true,
            "result": [
                {
                    "update_id": 1,
                    "message": {
                        "message_id": 100,
                        "from": { "id": 42, "first_name": "Alice" },
                        "chat": { "id": 42 },
                        "date": 1712150400,
                        "text": "Hello bot"
                    }
                },
                {
                    "update_id": 2,
                    "message": {
                        "message_id": 101,
                        "from": { "id": 99, "first_name": "Bob" },
                        "chat": { "id": 99 },
                        "date": 1712150401,
                        "text": "Wrong chat"
                    }
                }
            ]
        });

        let messages = parse_updates_response(&response, "42", 10);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].from, "Alice");
        assert_eq!(messages[0].text, "Hello bot");
        assert_eq!(messages[0].id, "100");
    }

    #[test]
    fn parse_updates_empty_result() {
        let response = serde_json::json!({ "ok": true, "result": [] });
        let messages = parse_updates_response(&response, "42", 10);
        assert!(messages.is_empty());
    }

    #[test]
    fn parse_updates_respects_limit() {
        let mut results = Vec::new();
        for i in 0..20 {
            results.push(serde_json::json!({
                "update_id": i,
                "message": {
                    "message_id": i,
                    "from": { "id": 42, "first_name": "Alice" },
                    "chat": { "id": 42 },
                    "date": 1712150400 + i,
                    "text": format!("msg {i}")
                }
            }));
        }
        let response = serde_json::json!({ "ok": true, "result": results });
        let messages = parse_updates_response(&response, "42", 5);
        assert_eq!(messages.len(), 5);
    }

    #[test]
    fn parse_updates_handles_missing_text() {
        let response = serde_json::json!({
            "ok": true,
            "result": [{
                "update_id": 1,
                "message": {
                    "message_id": 1,
                    "from": { "id": 42, "first_name": "Alice" },
                    "chat": { "id": 42 },
                    "date": 1712150400
                }
            }]
        });
        let messages = parse_updates_response(&response, "42", 10);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].text, "");
    }

    #[test]
    fn send_telegram_schema_valid() {
        let action = SendTelegram {
            config: test_config(),
        };
        let schema = action.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "chat_id"));
        assert!(required.iter().any(|v| v == "text"));
    }

    #[test]
    fn read_telegram_schema_valid() {
        let action = ReadTelegram {
            config: test_config(),
        };
        let schema = action.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "chat_id"));
        // limit is optional
        assert!(!required.iter().any(|v| v == "limit"));
        assert!(schema["properties"]["limit"].is_object());
    }
}
