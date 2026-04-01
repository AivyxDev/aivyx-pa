//! Email action — read inbox, draft replies, send messages.
//!
//! Uses IMAP for reading and SMTP (via lettre) for sending.
//! Credentials are stored in the encrypted keystore, never in config.

use crate::Action;
use aivyx_core::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct EmailConfig {
    pub imap_host: String,
    pub imap_port: u16,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub address: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EmailSummary {
    pub from: String,
    pub subject: String,
    pub date: String,
    pub preview: String,
    pub uid: u32,
}

pub struct ReadInbox {
    pub config: EmailConfig,
}

#[async_trait::async_trait]
impl Action for ReadInbox {
    fn name(&self) -> &str {
        "read_email"
    }

    fn description(&self) -> &str {
        "Check email inbox and return a summary of recent messages"
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "limit": { "type": "integer", "description": "Max messages to fetch", "default": 10 },
                "unread_only": { "type": "boolean", "description": "Only unread messages", "default": true }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let limit = input.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
        let unread_only = input.get("unread_only").and_then(|v| v.as_bool()).unwrap_or(true);

        let summaries = fetch_inbox(&self.config, limit, unread_only).await?;
        Ok(serde_json::to_value(summaries).unwrap())
    }
}

/// Fetch messages from IMAP inbox.
async fn fetch_inbox(
    config: &EmailConfig,
    limit: usize,
    unread_only: bool,
) -> Result<Vec<EmailSummary>> {
    // TODO: Implement IMAP fetch — reuse pattern from aivyx-server email adapter
    let _ = (config, limit, unread_only);
    Ok(vec![])
}

pub struct SendEmail {
    pub config: EmailConfig,
}

#[async_trait::async_trait]
impl Action for SendEmail {
    fn name(&self) -> &str {
        "send_email"
    }

    fn description(&self) -> &str {
        "Send an email message"
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "to": { "type": "string", "description": "Recipient email address" },
                "subject": { "type": "string", "description": "Email subject" },
                "body": { "type": "string", "description": "Email body text" }
            },
            "required": ["to", "subject", "body"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let to = input["to"].as_str().unwrap_or_default();
        let subject = input["subject"].as_str().unwrap_or_default();
        let body = input["body"].as_str().unwrap_or_default();

        send_smtp(&self.config, to, subject, body).await?;
        Ok(serde_json::json!({ "status": "sent", "to": to }))
    }
}

/// Send via SMTP using lettre.
async fn send_smtp(
    config: &EmailConfig,
    to: &str,
    subject: &str,
    body: &str,
) -> Result<()> {
    // TODO: Implement SMTP send — reuse pattern from aivyx-server email adapter
    let _ = (config, to, subject, body);
    Ok(())
}
