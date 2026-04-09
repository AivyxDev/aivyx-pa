//! Matrix CS API action tools.
//!
//! Provides `send_matrix` and `read_matrix` actions that interact with
//! the Matrix Client-Server API via REST. The access token is loaded
//! from the encrypted keystore at startup.

use super::{MatrixConfig, Message};
use crate::Action;
use aivyx_core::{AivyxError, Result};

// ── API Client ─────────────────────────────────────────────────

/// Send a message to a Matrix room.
async fn send_room_message(
    config: &MatrixConfig,
    room_id: &str,
    text: &str,
) -> Result<serde_json::Value> {
    let txn_id = uuid::Uuid::new_v4().to_string();
    let url = format!(
        "{}/_matrix/client/v3/rooms/{}/send/m.room.message/{}",
        config.homeserver,
        urlencoded(room_id),
        txn_id,
    );

    let client = crate::http_client();
    let resp = client
        .put(&url)
        .bearer_auth(&config.access_token)
        .json(&serde_json::json!({
            "msgtype": "m.text",
            "body": text
        }))
        .send()
        .await
        .map_err(|e| AivyxError::Channel(format!("Matrix send failed: {e}")))?;

    let status = resp.status();
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| AivyxError::Channel(format!("Matrix response parse error: {e}")))?;

    if !status.is_success() {
        let errcode = body["errcode"].as_str().unwrap_or("UNKNOWN");
        let error = body["error"].as_str().unwrap_or("unknown error");
        return Err(AivyxError::Channel(format!(
            "Matrix API error ({errcode}): {error}"
        )));
    }

    Ok(serde_json::json!({
        "status": "sent",
        "event_id": body["event_id"],
    }))
}

/// Fetch recent messages from a Matrix room using the /messages endpoint.
async fn get_room_messages(
    config: &MatrixConfig,
    room_id: &str,
    limit: usize,
) -> Result<Vec<Message>> {
    let url = format!(
        "{}/_matrix/client/v3/rooms/{}/messages",
        config.homeserver,
        urlencoded(room_id),
    );

    let client = crate::http_client();
    let resp = client
        .get(&url)
        .bearer_auth(&config.access_token)
        .query(&[("dir", "b"), ("limit", &limit.to_string())])
        .send()
        .await
        .map_err(|e| AivyxError::Channel(format!("Matrix messages failed: {e}")))?;

    let status = resp.status();
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| AivyxError::Channel(format!("Matrix response parse error: {e}")))?;

    if !status.is_success() {
        let errcode = body["errcode"].as_str().unwrap_or("UNKNOWN");
        let error = body["error"].as_str().unwrap_or("unknown error");
        return Err(AivyxError::Channel(format!(
            "Matrix API error ({errcode}): {error}"
        )));
    }

    Ok(parse_messages_response(&body))
}

/// Parse the /messages response into normalized Message structs.
fn parse_messages_response(body: &serde_json::Value) -> Vec<Message> {
    let chunk = match body["chunk"].as_array() {
        Some(arr) => arr,
        None => return Vec::new(),
    };

    chunk
        .iter()
        .filter_map(|event| {
            // Only m.room.message events with m.text type
            if event["type"].as_str()? != "m.room.message" {
                return None;
            }
            let content = event.get("content")?;
            if content["msgtype"].as_str()? != "m.text" {
                return None;
            }

            let sender = event["sender"].as_str().unwrap_or("unknown");
            let text = content["body"].as_str().unwrap_or("");
            let event_id = event["event_id"].as_str().unwrap_or("");

            // origin_server_ts is milliseconds since epoch
            let ts_ms = event["origin_server_ts"].as_i64().unwrap_or(0);
            let ts = chrono::DateTime::from_timestamp(ts_ms / 1000, 0)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default();

            Some(Message {
                id: event_id.to_string(),
                from: sender.to_string(),
                text: text.to_string(),
                timestamp: ts,
            })
        })
        .collect()
}

/// URL-encode a Matrix room ID (handles `!` and `:` characters).
fn urlencoded(s: &str) -> String {
    s.replace('%', "%25")
        .replace('!', "%21")
        .replace('#', "%23")
        .replace('$', "%24")
        .replace('&', "%26")
        .replace('\'', "%27")
        .replace('(', "%28")
        .replace(')', "%29")
        .replace('+', "%2B")
        .replace(',', "%2C")
        .replace('/', "%2F")
        .replace(':', "%3A")
        .replace(';', "%3B")
        .replace('=', "%3D")
        .replace('?', "%3F")
        .replace('@', "%40")
        .replace('[', "%5B")
        .replace(']', "%5D")
}

// ── Action: Send Matrix ────────────────────────────────────────

/// Send a message to a Matrix room.
pub struct SendMatrix {
    pub config: MatrixConfig,
}

#[async_trait::async_trait]
impl Action for SendMatrix {
    fn name(&self) -> &str {
        "send_matrix"
    }

    fn description(&self) -> &str {
        "Send a message to a Matrix room."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "room_id": {
                    "type": "string",
                    "description": "Matrix room ID (e.g., !abc123:example.com)."
                },
                "text": {
                    "type": "string",
                    "description": "Message text to send."
                }
            },
            "required": ["room_id", "text"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let room_id = input["room_id"]
            .as_str()
            .ok_or_else(|| AivyxError::Other("room_id is required".into()))?;
        let text = input["text"]
            .as_str()
            .ok_or_else(|| AivyxError::Other("text is required".into()))?;

        crate::retry::retry(
            &crate::retry::RetryConfig::network(),
            || send_room_message(&self.config, room_id, text),
            crate::retry::is_transient,
        )
        .await
    }
}

// ── Action: Read Matrix ────────────────────────────────────────

/// Read recent messages from a Matrix room.
pub struct ReadMatrix {
    pub config: MatrixConfig,
}

#[async_trait::async_trait]
impl Action for ReadMatrix {
    fn name(&self) -> &str {
        "read_matrix"
    }

    fn description(&self) -> &str {
        "Read recent messages from a Matrix room."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "room_id": {
                    "type": "string",
                    "description": "Matrix room ID (e.g., !abc123:example.com)."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of messages to return. Default: 10.",
                    "default": 10
                }
            },
            "required": ["room_id"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let room_id = input["room_id"]
            .as_str()
            .ok_or_else(|| AivyxError::Other("room_id is required".into()))?;
        let limit = input.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;

        let messages = crate::retry::retry(
            &crate::retry::RetryConfig::network(),
            || get_room_messages(&self.config, room_id, limit),
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

/// Forward a notification to the default Matrix room.
///
/// Returns `Ok(())` silently if no `default_room_id` is configured.
pub async fn forward_notification(config: &MatrixConfig, title: &str, body: &str) -> Result<()> {
    if let Some(ref room_id) = config.default_room_id {
        let text = format!("**{}**\n{}", title, body);
        send_room_message(config, room_id, &text).await?;
    }
    Ok(())
}

// ── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> MatrixConfig {
        MatrixConfig {
            homeserver: "https://matrix.example.com".into(),
            access_token: "test-token".into(),
            default_room_id: Some("!abc:example.com".into()),
        }
    }

    #[test]
    fn parse_messages_extracts_text() {
        let response = serde_json::json!({
            "chunk": [
                {
                    "type": "m.room.message",
                    "content": { "msgtype": "m.text", "body": "Hello world" },
                    "sender": "@alice:example.com",
                    "origin_server_ts": 1712150400000_i64,
                    "event_id": "$event1"
                },
                {
                    "type": "m.room.message",
                    "content": { "msgtype": "m.text", "body": "Second message" },
                    "sender": "@bob:example.com",
                    "origin_server_ts": 1712150401000_i64,
                    "event_id": "$event2"
                }
            ]
        });

        let messages = parse_messages_response(&response);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].from, "@alice:example.com");
        assert_eq!(messages[0].text, "Hello world");
        assert_eq!(messages[0].id, "$event1");
        assert_eq!(messages[1].from, "@bob:example.com");
    }

    #[test]
    fn parse_messages_filters_non_text() {
        let response = serde_json::json!({
            "chunk": [
                {
                    "type": "m.room.message",
                    "content": { "msgtype": "m.text", "body": "text msg" },
                    "sender": "@alice:example.com",
                    "origin_server_ts": 1712150400000_i64,
                    "event_id": "$1"
                },
                {
                    "type": "m.room.message",
                    "content": { "msgtype": "m.image", "body": "photo.jpg" },
                    "sender": "@alice:example.com",
                    "origin_server_ts": 1712150401000_i64,
                    "event_id": "$2"
                },
                {
                    "type": "m.room.member",
                    "content": { "membership": "join" },
                    "sender": "@bob:example.com",
                    "origin_server_ts": 1712150402000_i64,
                    "event_id": "$3"
                }
            ]
        });

        let messages = parse_messages_response(&response);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].text, "text msg");
    }

    #[test]
    fn parse_messages_empty_chunk() {
        let response = serde_json::json!({ "chunk": [] });
        let messages = parse_messages_response(&response);
        assert!(messages.is_empty());
    }

    #[test]
    fn parse_messages_missing_chunk() {
        let response = serde_json::json!({});
        let messages = parse_messages_response(&response);
        assert!(messages.is_empty());
    }

    #[test]
    fn urlencoded_room_id() {
        assert_eq!(urlencoded("!abc:example.com"), "%21abc%3Aexample.com");
    }

    #[test]
    fn urlencoded_no_special_chars() {
        assert_eq!(urlencoded("simple"), "simple");
    }

    #[test]
    fn send_matrix_schema_valid() {
        let action = SendMatrix {
            config: test_config(),
        };
        let schema = action.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "room_id"));
        assert!(required.iter().any(|v| v == "text"));
    }

    #[test]
    fn read_matrix_schema_valid() {
        let action = ReadMatrix {
            config: test_config(),
        };
        let schema = action.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "room_id"));
        assert!(!required.iter().any(|v| v == "limit"));
        assert!(schema["properties"]["limit"].is_object());
    }
}
