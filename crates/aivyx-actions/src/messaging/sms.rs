//! SMS gateway action tools — send text messages via Twilio or Vonage.
//!
//! Provides `send_sms` for sending text messages. SMS is send-only by
//! design — receiving SMS requires webhook infrastructure which is handled
//! separately by the webhook receiver module.

use super::{SmsConfig, SmsProvider};
use crate::Action;
use aivyx_core::{AivyxError, Result};

// ── API Clients ───────────────────────────────────────────────

/// Send an SMS via Twilio REST API.
async fn send_twilio(config: &SmsConfig, to: &str, body: &str) -> Result<serde_json::Value> {
    let base = config
        .api_url
        .as_deref()
        .unwrap_or("https://api.twilio.com");
    let url = format!(
        "{base}/2010-04-01/Accounts/{}/Messages.json",
        config.account_id
    );

    let client = crate::http_client();
    let resp = client
        .post(&url)
        .basic_auth(&config.account_id, Some(&config.auth_token))
        .form(&[
            ("From", config.from_number.as_str()),
            ("To", to),
            ("Body", body),
        ])
        .send()
        .await
        .map_err(|e| AivyxError::Channel(format!("Twilio send failed: {e}")))?;

    let status = resp.status();
    let resp_body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| AivyxError::Channel(format!("Twilio response parse error: {e}")))?;

    if !status.is_success() {
        let msg = resp_body["message"].as_str().unwrap_or("unknown error");
        return Err(AivyxError::Channel(format!(
            "Twilio API error ({}): {msg}",
            status.as_u16()
        )));
    }

    Ok(serde_json::json!({
        "status": "sent",
        "sid": resp_body["sid"],
        "to": to,
    }))
}

/// Send an SMS via Vonage (Nexmo) REST API.
async fn send_vonage(config: &SmsConfig, to: &str, body: &str) -> Result<serde_json::Value> {
    let base = config
        .api_url
        .as_deref()
        .unwrap_or("https://rest.nexmo.com");
    let url = format!("{base}/sms/json");

    let client = crate::http_client();
    let resp = client
        .post(&url)
        .json(&serde_json::json!({
            "api_key": config.account_id,
            "api_secret": config.auth_token,
            "from": config.from_number,
            "to": to,
            "text": body,
        }))
        .send()
        .await
        .map_err(|e| AivyxError::Channel(format!("Vonage send failed: {e}")))?;

    let status = resp.status();
    let resp_body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| AivyxError::Channel(format!("Vonage response parse error: {e}")))?;

    if !status.is_success() {
        return Err(AivyxError::Channel(format!(
            "Vonage API error ({})",
            status.as_u16()
        )));
    }

    // Vonage returns messages[0].status — "0" means success
    let msg_status = resp_body["messages"][0]["status"].as_str().unwrap_or("?");
    if msg_status != "0" {
        let error_text = resp_body["messages"][0]["error-text"]
            .as_str()
            .unwrap_or("unknown error");
        return Err(AivyxError::Channel(format!("Vonage error: {error_text}")));
    }

    Ok(serde_json::json!({
        "status": "sent",
        "message_id": resp_body["messages"][0]["message-id"],
        "to": to,
    }))
}

// ── Action: Send SMS ─────────────────────────────────────────

pub struct SendSms {
    pub config: SmsConfig,
}

#[async_trait::async_trait]
impl Action for SendSms {
    fn name(&self) -> &str {
        "send_sms"
    }

    fn description(&self) -> &str {
        "Send a text message (SMS) to a phone number. Message length is \
         limited to 1600 characters (concatenated SMS)."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "to": {
                    "type": "string",
                    "description": "Recipient phone number in E.164 format (e.g. +15551234567)."
                },
                "text": {
                    "type": "string",
                    "description": "Message text (max 1600 characters)."
                }
            },
            "required": ["to", "text"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let to = input["to"]
            .as_str()
            .ok_or_else(|| AivyxError::Validation("to is required".into()))?;
        let text = input["text"]
            .as_str()
            .ok_or_else(|| AivyxError::Validation("text is required".into()))?;

        if text.trim().is_empty() {
            return Err(AivyxError::Validation("text must not be empty".into()));
        }
        if text.len() > 1600 {
            return Err(AivyxError::Validation(format!(
                "text exceeds 1600 character limit ({} chars)",
                text.len()
            )));
        }
        if !to.starts_with('+') {
            return Err(AivyxError::Validation(
                "phone number must be in E.164 format (start with +)".into(),
            ));
        }

        match self.config.provider {
            SmsProvider::Twilio => {
                crate::retry::retry(
                    &crate::retry::RetryConfig::network(),
                    || send_twilio(&self.config, to, text),
                    crate::retry::is_transient,
                )
                .await
            }
            SmsProvider::Vonage => {
                crate::retry::retry(
                    &crate::retry::RetryConfig::network(),
                    || send_vonage(&self.config, to, text),
                    crate::retry::is_transient,
                )
                .await
            }
        }
    }
}

// ── Notification Forwarding ──────────────────────────────────

/// Forward a notification via SMS to the default recipient.
///
/// Returns `Ok(())` silently if no `default_recipient` is configured.
pub async fn forward_notification(config: &SmsConfig, title: &str, body: &str) -> Result<()> {
    if let Some(ref to) = config.default_recipient {
        let text = format!("{}: {}", title, body);
        // Truncate to SMS limit (char-boundary safe to avoid panic on multi-byte UTF-8)
        let text = if text.len() > 1600 {
            let mut end = 1600;
            while end > 0 && !text.is_char_boundary(end) {
                end -= 1;
            }
            &text[..end]
        } else {
            &text
        };

        match config.provider {
            SmsProvider::Twilio => send_twilio(config, to, text).await?,
            SmsProvider::Vonage => send_vonage(config, to, text).await?,
        };
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn twilio_config() -> SmsConfig {
        SmsConfig {
            provider: SmsProvider::Twilio,
            api_url: None,
            account_id: "AC123456".into(),
            auth_token: "test_token".into(),
            from_number: "+15551234567".into(),
            default_recipient: Some("+15559876543".into()),
        }
    }

    fn vonage_config() -> SmsConfig {
        SmsConfig {
            provider: SmsProvider::Vonage,
            api_url: None,
            account_id: "api_key".into(),
            auth_token: "api_secret".into(),
            from_number: "+15551234567".into(),
            default_recipient: None,
        }
    }

    #[test]
    fn send_sms_name_and_schema() {
        let action = SendSms {
            config: twilio_config(),
        };
        assert_eq!(action.name(), "send_sms");
        let schema = action.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "to"));
        assert!(required.iter().any(|v| v == "text"));
    }

    #[test]
    fn vonage_config_valid() {
        let cfg = vonage_config();
        assert_eq!(cfg.provider, SmsProvider::Vonage);
        assert_eq!(cfg.account_id, "api_key");
    }

    #[tokio::test]
    async fn send_sms_rejects_empty_text() {
        let action = SendSms {
            config: twilio_config(),
        };
        let result = action
            .execute(serde_json::json!({
                "to": "+15559876543",
                "text": "  "
            }))
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("empty"));
    }

    #[tokio::test]
    async fn send_sms_rejects_missing_to() {
        let action = SendSms {
            config: twilio_config(),
        };
        let result = action.execute(serde_json::json!({ "text": "hello" })).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn send_sms_rejects_missing_text() {
        let action = SendSms {
            config: twilio_config(),
        };
        let result = action.execute(serde_json::json!({ "to": "+1555" })).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn send_sms_rejects_non_e164() {
        let action = SendSms {
            config: twilio_config(),
        };
        let result = action
            .execute(serde_json::json!({
                "to": "5551234567",
                "text": "hello"
            }))
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("E.164"));
    }

    #[tokio::test]
    async fn send_sms_rejects_too_long() {
        let action = SendSms {
            config: twilio_config(),
        };
        let long_text = "x".repeat(1601);
        let result = action
            .execute(serde_json::json!({
                "to": "+15559876543",
                "text": long_text
            }))
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("1600"));
    }

    #[tokio::test]
    async fn send_sms_accepts_max_length() {
        // Validates that exactly 1600 chars passes validation
        // (will fail at network level, but validation should pass)
        let action = SendSms {
            config: twilio_config(),
        };
        let text = "x".repeat(1600);
        let result = action
            .execute(serde_json::json!({
                "to": "+15559876543",
                "text": text
            }))
            .await;
        // Should fail with network error, not validation
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(!err.contains("1600")); // Not a length error
    }
}
