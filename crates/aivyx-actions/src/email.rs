//! Email action — read inbox via IMAP, send via SMTP (lettre).
//!
//! Uses raw IMAP commands over TLS (same pattern as aivyx-server email adapter).
//! Credentials are stored in the encrypted keystore, never in config.

use crate::Action;
use aivyx_core::Result;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailConfig {
    pub imap_host: String,
    pub imap_port: u16,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub address: String,
    pub username: String,
    /// Password — loaded from encrypted keystore at runtime.
    #[serde(skip)]
    pub password: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EmailSummary {
    pub from: String,
    pub subject: String,
    pub preview: String,
    pub seq: u32,
}

// ── IMAP helpers ────────────────────────────────────────────────

fn escape_imap(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

async fn wait_for_tag(
    reader: &mut BufReader<tokio::io::ReadHalf<tokio_native_tls::TlsStream<tokio::net::TcpStream>>>,
    tag: &str,
) -> Result<String> {
    let mut collected = String::new();
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).await.map_err(|e| {
            aivyx_core::AivyxError::Channel(format!("IMAP read error: {e}"))
        })?;
        if n == 0 {
            return Err(aivyx_core::AivyxError::Channel("IMAP connection closed".into()));
        }
        collected.push_str(&line);
        if line.starts_with(tag) {
            if line.contains("OK") {
                return Ok(collected);
            }
            return Err(aivyx_core::AivyxError::Channel(format!("IMAP error: {line}")));
        }
    }
}

fn parse_search_response(response: &str) -> Vec<u32> {
    for line in response.lines() {
        if line.starts_with("* SEARCH") {
            return line
                .strip_prefix("* SEARCH")
                .unwrap_or("")
                .split_whitespace()
                .filter_map(|s| s.parse().ok())
                .collect();
        }
    }
    vec![]
}

fn parse_fetch_headers(response: &str) -> (String, String, String) {
    let mut from = String::new();
    let mut subject = String::new();
    let mut body = String::new();
    let mut in_body = false;

    for line in response.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("From:") {
            from = trimmed.strip_prefix("From:").unwrap_or("").trim().to_string();
        } else if trimmed.starts_with("Subject:") {
            subject = trimmed.strip_prefix("Subject:").unwrap_or("").trim().to_string();
        } else if trimmed.contains("BODY[TEXT]") {
            in_body = true;
        } else if in_body {
            if trimmed == ")" || trimmed.starts_with("A0") {
                in_body = false;
            } else if !trimmed.starts_with('{') {
                if body.len() < 200 {
                    if !body.is_empty() {
                        body.push(' ');
                    }
                    body.push_str(trimmed);
                }
            }
        }
    }

    (from, subject, body)
}

/// Fetch recent messages from IMAP inbox.
async fn fetch_inbox(
    config: &EmailConfig,
    limit: usize,
    unread_only: bool,
) -> Result<Vec<EmailSummary>> {
    let tls = tokio_native_tls::TlsConnector::from(
        native_tls::TlsConnector::new().map_err(|e| {
            aivyx_core::AivyxError::Channel(format!("TLS error: {e}"))
        })?,
    );

    let tcp = tokio::net::TcpStream::connect((&*config.imap_host, config.imap_port))
        .await
        .map_err(|e| aivyx_core::AivyxError::Channel(format!("IMAP connect error: {e}")))?;

    let stream = tls.connect(&config.imap_host, tcp).await.map_err(|e| {
        aivyx_core::AivyxError::Channel(format!("TLS handshake error: {e}"))
    })?;

    let (read_half, mut write_half) = tokio::io::split(stream);
    let mut reader = BufReader::new(read_half);

    // Wait for server greeting
    wait_for_tag(&mut reader, "*").await.ok(); // Greeting is untagged

    // LOGIN
    let login = format!(
        "A001 LOGIN \"{}\" \"{}\"\r\n",
        escape_imap(&config.username),
        escape_imap(&config.password)
    );
    write_half.write_all(login.as_bytes()).await.map_err(|e| {
        aivyx_core::AivyxError::Channel(format!("IMAP write error: {e}"))
    })?;
    wait_for_tag(&mut reader, "A001").await?;

    // SELECT INBOX
    write_half.write_all(b"A002 SELECT INBOX\r\n").await.map_err(|e| {
        aivyx_core::AivyxError::Channel(format!("IMAP write error: {e}"))
    })?;
    wait_for_tag(&mut reader, "A002").await?;

    // SEARCH
    let search_cmd = if unread_only {
        "A003 SEARCH UNSEEN\r\n".to_string()
    } else {
        "A003 SEARCH ALL\r\n".to_string()
    };
    write_half.write_all(search_cmd.as_bytes()).await.map_err(|e| {
        aivyx_core::AivyxError::Channel(format!("IMAP write error: {e}"))
    })?;
    let search_result = wait_for_tag(&mut reader, "A003").await?;
    let seq_numbers = parse_search_response(&search_result);

    // Take only the last N messages
    let to_fetch: Vec<_> = seq_numbers.iter().rev().take(limit).copied().collect();

    let mut summaries = Vec::new();
    for (i, seq) in to_fetch.iter().enumerate() {
        let tag = format!("F{:03}", i);
        let fetch_cmd = format!(
            "{tag} FETCH {seq} (BODY[HEADER.FIELDS (FROM SUBJECT)] BODY[TEXT])\r\n"
        );
        write_half.write_all(fetch_cmd.as_bytes()).await.map_err(|e| {
            aivyx_core::AivyxError::Channel(format!("IMAP write error: {e}"))
        })?;
        let fetch_result = wait_for_tag(&mut reader, &tag).await?;
        let (from, subject, preview) = parse_fetch_headers(&fetch_result);

        summaries.push(EmailSummary {
            from,
            subject,
            preview,
            seq: *seq,
        });
    }

    // LOGOUT
    let _ = write_half.write_all(b"A099 LOGOUT\r\n").await;

    Ok(summaries)
}

/// Send email via SMTP using lettre.
async fn send_smtp(
    config: &EmailConfig,
    to: &str,
    subject: &str,
    body: &str,
) -> Result<()> {
    use lettre::message::Mailbox;
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};

    let from_addr: lettre::Address = config.address.parse().map_err(|e| {
        aivyx_core::AivyxError::Validation(format!("invalid from address: {e}"))
    })?;
    let to_addr: lettre::Address = to.parse().map_err(|e| {
        aivyx_core::AivyxError::Validation(format!("invalid to address: {e}"))
    })?;

    let email = lettre::Message::builder()
        .from(Mailbox::new(Some("Aivyx Assistant".into()), from_addr))
        .to(Mailbox::new(None, to_addr))
        .subject(subject)
        .body(body.to_string())
        .map_err(|e| aivyx_core::AivyxError::Channel(format!("email build error: {e}")))?;

    let creds = Credentials::new(config.username.clone(), config.password.clone());

    let transport = AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&config.smtp_host)
        .map_err(|e| aivyx_core::AivyxError::Channel(format!("SMTP relay error: {e}")))?
        .port(config.smtp_port)
        .credentials(creds)
        .build();

    transport.send(email).await.map_err(|e| {
        aivyx_core::AivyxError::Channel(format!("SMTP send error: {e}"))
    })?;

    Ok(())
}

// ── Action implementations ──────────────────────────────────────

pub struct ReadInbox {
    pub config: EmailConfig,
}

#[async_trait::async_trait]
impl Action for ReadInbox {
    fn name(&self) -> &str { "read_email" }

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

pub struct SendEmail {
    pub config: EmailConfig,
}

#[async_trait::async_trait]
impl Action for SendEmail {
    fn name(&self) -> &str { "send_email" }

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
