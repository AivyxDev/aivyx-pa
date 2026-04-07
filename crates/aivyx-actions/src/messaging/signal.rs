//! Signal messaging action tools via signal-cli JSON-RPC.
//!
//! Provides `send_signal` and `read_signal` actions that communicate with
//! signal-cli's JSON-RPC daemon over a unix socket or TCP connection.
//! The agent's phone number is configured; signal-cli must be registered
//! and running separately.

use super::{Message, SignalConfig};
use crate::Action;
use aivyx_core::{AivyxError, Result};

// ── JSON-RPC helpers ──────────────────────────────────────────

/// Send a JSON-RPC request to signal-cli and return the result.
async fn jsonrpc_call(
    socket_path: &str,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value> {
    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
        "id": 1,
    });

    // Determine if this is a TCP or Unix socket connection
    let response_text = if socket_path.contains(':') {
        // TCP connection (e.g., "localhost:7583")
        jsonrpc_tcp(socket_path, &payload).await?
    } else {
        // Unix socket
        jsonrpc_unix(socket_path, &payload).await?
    };

    let response: serde_json::Value = serde_json::from_str(&response_text)
        .map_err(|e| AivyxError::Channel(format!("Signal JSON-RPC parse error: {e}")))?;

    if let Some(error) = response.get("error") {
        let msg = error["message"].as_str().unwrap_or("unknown error");
        let code = error["code"].as_i64().unwrap_or(-1);
        return Err(AivyxError::Channel(format!(
            "Signal JSON-RPC error ({code}): {msg}"
        )));
    }

    Ok(response["result"].clone())
}

/// Send JSON-RPC over a Unix domain socket.
async fn jsonrpc_unix(
    socket_path: &str,
    payload: &serde_json::Value,
) -> Result<String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut stream = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::net::UnixStream::connect(socket_path),
    )
    .await
    .map_err(|_| AivyxError::Channel("Signal socket connect timed out".into()))?
    .map_err(|e| AivyxError::Channel(format!("Signal socket connect failed: {e}")))?;

    let request = format!("{}\n", payload);
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| AivyxError::Channel(format!("Signal socket write failed: {e}")))?;

    let mut buf = Vec::with_capacity(4096);
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        // Read until we get a complete JSON line
        let mut temp = [0u8; 4096];
        loop {
            let n = stream.read(&mut temp).await
                .map_err(|e| AivyxError::Channel(format!("Signal socket read failed: {e}")))?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&temp[..n]);
            if buf.contains(&b'\n') {
                break;
            }
        }
        Ok::<_, AivyxError>(())
    })
    .await
    .map_err(|_| AivyxError::Channel("Signal response timed out".into()))??;

    String::from_utf8(buf)
        .map_err(|e| AivyxError::Channel(format!("Signal response not valid UTF-8: {e}")))
}

/// Send JSON-RPC over TCP.
async fn jsonrpc_tcp(
    addr: &str,
    payload: &serde_json::Value,
) -> Result<String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut stream = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::net::TcpStream::connect(addr),
    )
    .await
    .map_err(|_| AivyxError::Channel("Signal TCP connect timed out".into()))?
    .map_err(|e| AivyxError::Channel(format!("Signal TCP connect failed: {e}")))?;

    let request = format!("{}\n", payload);
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| AivyxError::Channel(format!("Signal TCP write failed: {e}")))?;

    let mut buf = Vec::with_capacity(4096);
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        let mut temp = [0u8; 4096];
        loop {
            let n = stream.read(&mut temp).await
                .map_err(|e| AivyxError::Channel(format!("Signal TCP read failed: {e}")))?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&temp[..n]);
            if buf.contains(&b'\n') {
                break;
            }
        }
        Ok::<_, AivyxError>(())
    })
    .await
    .map_err(|_| AivyxError::Channel("Signal response timed out".into()))??;

    String::from_utf8(buf)
        .map_err(|e| AivyxError::Channel(format!("Signal response not valid UTF-8: {e}")))
}

// ── Action: Send Signal ──────────────────────────────────────

pub struct SendSignal {
    pub config: SignalConfig,
}

#[async_trait::async_trait]
impl Action for SendSignal {
    fn name(&self) -> &str {
        "send_signal"
    }

    fn description(&self) -> &str {
        "Send a Signal message to a phone number or group."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "recipient": {
                    "type": "string",
                    "description": "Phone number (e.g. +15551234567) or group ID to send to."
                },
                "text": {
                    "type": "string",
                    "description": "Message text to send."
                }
            },
            "required": ["recipient", "text"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let recipient = input["recipient"]
            .as_str()
            .ok_or_else(|| AivyxError::Validation("recipient is required".into()))?;
        let text = input["text"]
            .as_str()
            .ok_or_else(|| AivyxError::Validation("text is required".into()))?;

        if text.trim().is_empty() {
            return Err(AivyxError::Validation("text must not be empty".into()));
        }

        let params = if recipient.starts_with('+') {
            // Direct message to phone number
            serde_json::json!({
                "account": self.config.account,
                "recipient": [recipient],
                "message": text,
            })
        } else {
            // Group message
            serde_json::json!({
                "account": self.config.account,
                "groupId": recipient,
                "message": text,
            })
        };

        crate::retry::retry(
            &crate::retry::RetryConfig::network(),
            || jsonrpc_call(&self.config.socket_path, "send", params.clone()),
            crate::retry::is_transient,
        )
        .await?;

        Ok(serde_json::json!({
            "status": "sent",
            "recipient": recipient,
        }))
    }
}

// ── Action: Read Signal ──────────────────────────────────────

pub struct ReadSignal {
    pub config: SignalConfig,
}

#[async_trait::async_trait]
impl Action for ReadSignal {
    fn name(&self) -> &str {
        "read_signal"
    }

    fn description(&self) -> &str {
        "Read recent Signal messages. Returns messages received since last check."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "timeout": {
                    "type": "integer",
                    "description": "Seconds to wait for new messages (default 1, max 10).",
                    "default": 1
                }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let timeout = input.get("timeout")
            .and_then(|v| v.as_u64())
            .unwrap_or(1)
            .min(10);

        let params = serde_json::json!({
            "account": self.config.account,
            "timeout": timeout,
        });

        let result = crate::retry::retry(
            &crate::retry::RetryConfig::network(),
            || jsonrpc_call(&self.config.socket_path, "receive", params.clone()),
            crate::retry::is_transient,
        )
        .await?;

        let messages: Vec<Message> = result
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .filter_map(parse_signal_message)
            .collect();

        Ok(serde_json::json!({
            "messages": messages,
            "count": messages.len(),
        }))
    }
}

/// Parse a signal-cli receive result entry into a normalized Message.
fn parse_signal_message(entry: &serde_json::Value) -> Option<Message> {
    let envelope = entry.get("envelope")?;
    let data = envelope.get("dataMessage")?;
    let text = data["message"].as_str().unwrap_or("");
    if text.is_empty() {
        return None;
    }

    let source = envelope["source"].as_str().unwrap_or("unknown");
    let source_name = envelope["sourceName"].as_str().unwrap_or(source);
    let timestamp = data["timestamp"].as_u64().unwrap_or(0);

    let ts = chrono::DateTime::from_timestamp_millis(timestamp as i64)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default();

    Some(Message {
        id: timestamp.to_string(),
        from: source_name.to_string(),
        text: text.to_string(),
        timestamp: ts,
    })
}

// ── Notification Forwarding ──────────────────────────────────

/// Forward a notification to the default Signal recipient.
///
/// Returns `Ok(())` silently if no `default_recipient` is configured.
pub async fn forward_notification(
    config: &SignalConfig,
    title: &str,
    body: &str,
) -> Result<()> {
    if let Some(ref recipient) = config.default_recipient {
        let text = format!("*{}*\n{}", title, body);
        let params = serde_json::json!({
            "account": config.account,
            "recipient": [recipient],
            "message": text,
        });
        jsonrpc_call(&config.socket_path, "send", params).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> SignalConfig {
        SignalConfig {
            account: "+15551234567".into(),
            socket_path: "/var/run/signal-cli/socket".into(),
            default_recipient: Some("+15559876543".into()),
        }
    }

    #[test]
    fn send_signal_schema_valid() {
        let action = SendSignal { config: test_config() };
        assert_eq!(action.name(), "send_signal");
        let schema = action.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "recipient"));
        assert!(required.iter().any(|v| v == "text"));
    }

    #[test]
    fn read_signal_schema_valid() {
        let action = ReadSignal { config: test_config() };
        assert_eq!(action.name(), "read_signal");
        let schema = action.input_schema();
        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("timeout"));
    }

    #[test]
    fn parse_signal_message_extracts_fields() {
        let entry = serde_json::json!({
            "envelope": {
                "source": "+15559876543",
                "sourceName": "Alice",
                "dataMessage": {
                    "timestamp": 1712150400000_u64,
                    "message": "Hello from Signal"
                }
            }
        });

        let msg = parse_signal_message(&entry).unwrap();
        assert_eq!(msg.from, "Alice");
        assert_eq!(msg.text, "Hello from Signal");
        assert_eq!(msg.id, "1712150400000");
        assert!(!msg.timestamp.is_empty());
    }

    #[test]
    fn parse_signal_message_uses_source_as_fallback_name() {
        let entry = serde_json::json!({
            "envelope": {
                "source": "+15559876543",
                "dataMessage": {
                    "timestamp": 1712150400000_u64,
                    "message": "No name"
                }
            }
        });

        let msg = parse_signal_message(&entry).unwrap();
        assert_eq!(msg.from, "+15559876543");
    }

    #[test]
    fn parse_signal_message_skips_empty_text() {
        let entry = serde_json::json!({
            "envelope": {
                "source": "+15559876543",
                "dataMessage": {
                    "timestamp": 1712150400000_u64,
                    "message": ""
                }
            }
        });

        assert!(parse_signal_message(&entry).is_none());
    }

    #[test]
    fn parse_signal_message_skips_non_data() {
        // Receipt messages have no dataMessage
        let entry = serde_json::json!({
            "envelope": {
                "source": "+15559876543",
                "receiptMessage": {
                    "type": "DELIVERY"
                }
            }
        });

        assert!(parse_signal_message(&entry).is_none());
    }

    #[tokio::test]
    async fn send_signal_rejects_empty_text() {
        let action = SendSignal { config: test_config() };
        let result = action.execute(serde_json::json!({
            "recipient": "+15559876543",
            "text": "  "
        })).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("empty"));
    }

    #[tokio::test]
    async fn send_signal_rejects_missing_recipient() {
        let action = SendSignal { config: test_config() };
        let result = action.execute(serde_json::json!({ "text": "hello" })).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn send_signal_rejects_missing_text() {
        let action = SendSignal { config: test_config() };
        let result = action.execute(serde_json::json!({ "recipient": "+1555" })).await;
        assert!(result.is_err());
    }
}
