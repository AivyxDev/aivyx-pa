//! Email action — read inbox via IMAP, send via SMTP (lettre).
//!
//! Uses raw IMAP commands over TLS (same pattern as aivyx-server email adapter).
//! Credentials are stored in the encrypted keystore, never in config.
//!
//! IMAP connections are pooled via [`ImapPool`] to avoid the TLS handshake +
//! LOGIN overhead on every call. The pool holds a single cached connection
//! with TTL-based expiry and NOOP health checks before reuse.

use crate::Action;
use aivyx_core::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailSummary {
    pub from: String,
    pub subject: String,
    pub preview: String,
    pub seq: u32,
    /// RFC 2822 Message-ID for reply threading (None if server didn't provide it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
}

/// Full email content returned by `fetch_email`.
#[derive(Debug, Serialize, Deserialize)]
pub struct EmailFull {
    pub from: String,
    pub to: String,
    pub subject: String,
    pub date: String,
    pub message_id: Option<String>,
    pub body: String,
    pub seq: u32,
}

// ── IMAP helpers ────────────────────────────────────────────────

type ImapReader = BufReader<tokio::io::ReadHalf<tokio_native_tls::TlsStream<tokio::net::TcpStream>>>;
type ImapWriter = tokio::io::WriteHalf<tokio_native_tls::TlsStream<tokio::net::TcpStream>>;

fn escape_imap(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Maximum time to wait for a single IMAP tagged response (30 seconds).
const IMAP_TAG_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// How long an idle pooled IMAP connection stays valid (5 minutes).
/// Most IMAP servers drop idle connections after 10-30 minutes;
/// 5 minutes gives comfortable margin while still avoiding stale connections.
const IMAP_POOL_TTL: std::time::Duration = std::time::Duration::from_secs(300);

/// A cached IMAP connection with its creation timestamp.
struct PooledConnection {
    reader: ImapReader,
    writer: ImapWriter,
    /// Monotonic instant when this connection was last validated.
    last_active: std::time::Instant,
    /// Rolling IMAP tag counter (starts at A003 since LOGIN=A001, SELECT=A002).
    next_tag: u32,
}

impl PooledConnection {
    /// Generate the next IMAP tag (A003, A004, ...).
    fn next_tag(&mut self) -> String {
        let tag = format!("A{:03}", self.next_tag);
        self.next_tag += 1;
        tag
    }

    /// Whether this connection has exceeded the idle TTL.
    fn is_expired(&self) -> bool {
        self.last_active.elapsed() > IMAP_POOL_TTL
    }

    /// Send a NOOP command to verify the connection is still alive.
    /// Updates `last_active` on success.
    async fn health_check(&mut self) -> bool {
        let tag = self.next_tag();
        let cmd = format!("{tag} NOOP\r\n");
        if self.writer.write_all(cmd.as_bytes()).await.is_err() {
            return false;
        }
        match wait_for_tag(&mut self.reader, &tag).await {
            Ok(_) => {
                self.last_active = std::time::Instant::now();
                true
            }
            Err(_) => false,
        }
    }

    /// Mark connection as active (after a successful operation).
    fn touch(&mut self) {
        self.last_active = std::time::Instant::now();
    }
}

/// Connection pool for IMAP — holds at most one cached connection.
///
/// IMAP is inherently single-session-per-mailbox, so a pool of one is
/// the right size. The pool handles:
/// - TTL-based expiry (discard if idle > 5 minutes)
/// - NOOP health check before reuse
/// - Transparent reconnect on stale/broken connections
///
/// # Usage
///
/// ```ignore
/// let pool = ImapPool::new(config.clone());
/// let (reader, writer, tag_fn) = pool.checkout().await?;
/// // ... use connection ...
/// pool.checkin(reader, writer, tag_counter).await;
/// ```
pub struct ImapPool {
    config: EmailConfig,
    conn: tokio::sync::Mutex<Option<PooledConnection>>,
}

impl ImapPool {
    /// Create a new pool for the given email configuration.
    pub fn new(config: EmailConfig) -> Arc<Self> {
        Arc::new(Self {
            config,
            conn: tokio::sync::Mutex::new(None),
        })
    }

    /// Acquire an IMAP connection — reuses the pooled one if healthy,
    /// otherwise creates a fresh connection.
    ///
    /// Returns `(reader, writer, starting_tag_number)`. The caller must
    /// use tags starting from the returned number (they won't collide
    /// with tags already used on this connection).
    async fn checkout(&self) -> Result<(ImapReader, ImapWriter, u32)> {
        let mut guard = self.conn.lock().await;

        // Try reusing the cached connection
        if let Some(mut pooled) = guard.take() {
            if !pooled.is_expired() && pooled.health_check().await {
                // Re-SELECT INBOX to refresh mailbox state
                let tag = pooled.next_tag();
                let cmd = format!("{tag} SELECT INBOX\r\n");
                if pooled.writer.write_all(cmd.as_bytes()).await.is_ok()
                    && wait_for_tag(&mut pooled.reader, &tag).await.is_ok()
                {
                    pooled.touch();
                    let start_tag = pooled.next_tag;
                    return Ok((pooled.reader, pooled.writer, start_tag));
                }
                // SELECT failed — fall through to fresh connection
                tracing::debug!("IMAP pool: cached connection failed SELECT, reconnecting");
            } else {
                tracing::debug!("IMAP pool: cached connection expired or unhealthy, reconnecting");
            }
            // Drop the stale connection (writer/reader drop closes the socket)
        }

        drop(guard); // Release lock during connect (it's slow)

        // Fresh connection
        let (reader, writer) = imap_connect(&self.config).await?;
        // imap_connect uses A001 (LOGIN) and A002 (SELECT), so next tag is A003
        Ok((reader, writer, 3))
    }

    /// Return a connection to the pool for reuse.
    /// Pass the current tag counter so the next checkout knows where to resume.
    async fn checkin(&self, reader: ImapReader, writer: ImapWriter, next_tag: u32) {
        let mut guard = self.conn.lock().await;
        *guard = Some(PooledConnection {
            reader,
            writer,
            last_active: std::time::Instant::now(),
            next_tag,
        });
    }

    /// Fetch recent messages from IMAP inbox using a pooled connection.
    pub async fn fetch_inbox(
        &self,
        limit: usize,
        unread_only: bool,
    ) -> Result<Vec<EmailSummary>> {
        let (mut reader, mut writer, mut tag_num) = self.checkout().await?;

        let result = fetch_inbox_with_conn(&mut reader, &mut writer, &mut tag_num, limit, unread_only).await;

        match &result {
            Ok(_) => self.checkin(reader, writer, tag_num).await,
            Err(_) => {} // Don't return broken connections to pool
        }

        result
    }

    /// Set flags on a message using a pooled connection.
    pub async fn store_flags(&self, seq: u32, flags: &str) -> Result<()> {
        let (mut reader, mut writer, mut tag_num) = self.checkout().await?;

        let result = store_flags_with_conn(&mut reader, &mut writer, &mut tag_num, seq, flags).await;

        match &result {
            Ok(_) => self.checkin(reader, writer, tag_num).await,
            Err(_) => {}
        }

        result
    }

    /// Copy a message to another folder, then delete from current folder.
    pub async fn copy_and_delete(&self, seq: u32, folder: &str) -> Result<()> {
        let (mut reader, mut writer, mut tag_num) = self.checkout().await?;

        let result = copy_and_delete_with_conn(
            &mut reader, &mut writer, &mut tag_num, seq, folder,
        ).await;

        match &result {
            Ok(_) => self.checkin(reader, writer, tag_num).await,
            Err(_) => {}
        }

        result
    }

    /// Delete a message (mark \Deleted + EXPUNGE).
    pub async fn delete_message(&self, seq: u32) -> Result<()> {
        let (mut reader, mut writer, mut tag_num) = self.checkout().await?;

        let result = delete_message_with_conn(&mut reader, &mut writer, &mut tag_num, seq).await;

        match &result {
            Ok(_) => self.checkin(reader, writer, tag_num).await,
            Err(_) => {}
        }

        result
    }

    /// Fetch a single email by sequence number using a pooled connection.
    pub async fn fetch_single(&self, seq: u32) -> Result<EmailFull> {
        let (mut reader, mut writer, mut tag_num) = self.checkout().await?;

        let result = fetch_single_with_conn(&mut reader, &mut writer, &mut tag_num, seq).await;

        match &result {
            Ok(_) => self.checkin(reader, writer, tag_num).await,
            Err(_) => {}
        }

        result
    }
}

async fn wait_for_tag(
    reader: &mut ImapReader,
    tag: &str,
) -> Result<String> {
    tokio::time::timeout(IMAP_TAG_TIMEOUT, wait_for_tag_inner(reader, tag))
        .await
        .map_err(|_| aivyx_core::AivyxError::Channel(
            format!("IMAP timeout waiting for tag '{tag}' ({}s)", IMAP_TAG_TIMEOUT.as_secs()),
        ))?
}

/// Maximum IMAP response size (5 MB). Prevents memory exhaustion from
/// a malicious or misconfigured server sending unbounded data.
const MAX_IMAP_RESPONSE_BYTES: usize = 5 * 1024 * 1024;

async fn wait_for_tag_inner(
    reader: &mut ImapReader,
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
        if collected.len() > MAX_IMAP_RESPONSE_BYTES {
            return Err(aivyx_core::AivyxError::Channel(format!(
                "IMAP response exceeded {} MB limit", MAX_IMAP_RESPONSE_BYTES / (1024 * 1024)
            )));
        }
        if line.starts_with(tag) {
            if line.contains("OK") {
                return Ok(collected);
            }
            return Err(aivyx_core::AivyxError::Channel(format!("IMAP error: {line}")));
        }
    }
}

/// Open a TLS IMAP connection, authenticate, and SELECT INBOX.
/// Returns the reader/writer pair ready for commands (tag A003+).
///
/// Public so startup health checks can verify email credentials without
/// going through `ImapPool`.
pub async fn imap_connect(config: &EmailConfig) -> Result<(ImapReader, ImapWriter)> {
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

    Ok((reader, write_half))
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

/// Parse IMAP FETCH response for inbox summaries.
/// Extracts From, Subject, Message-ID headers and a 200-char body preview.
fn parse_fetch_headers(response: &str) -> (String, String, String, Option<String>) {
    let mut from = String::new();
    let mut subject = String::new();
    let mut message_id: Option<String> = None;
    let mut body = String::new();
    let mut in_body = false;

    for line in response.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("From:") {
            from = trimmed.strip_prefix("From:").unwrap_or("").trim().to_string();
        } else if trimmed.starts_with("Subject:") {
            subject = trimmed.strip_prefix("Subject:").unwrap_or("").trim().to_string();
        } else if trimmed.starts_with("Message-ID:") || trimmed.starts_with("Message-Id:") {
            let raw = trimmed.split_once(':').map(|(_, v)| v.trim().to_string());
            message_id = raw;
        } else if trimmed.contains("BODY[TEXT]") {
            in_body = true;
        } else if in_body {
            if trimmed == ")" || trimmed.starts_with("A0") || trimmed.starts_with("F0") {
                in_body = false;
            } else if !trimmed.starts_with('{')
                && body.len() < 200 {
                    if !body.is_empty() {
                        body.push(' ');
                    }
                    body.push_str(trimmed);
                }
        }
    }

    (from, subject, body, message_id)
}

/// Parse IMAP FETCH response for a full email.
/// Extracts From, To, Subject, Date, Message-ID headers and full body (up to 32,000 chars).
fn parse_full_email(response: &str) -> EmailFull {
    let mut from = String::new();
    let mut to = String::new();
    let mut subject = String::new();
    let mut date = String::new();
    let mut message_id: Option<String> = None;
    let mut body = String::new();
    let mut in_body = false;

    for line in response.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("From:") {
            from = trimmed.strip_prefix("From:").unwrap_or("").trim().to_string();
        } else if trimmed.starts_with("To:") {
            to = trimmed.strip_prefix("To:").unwrap_or("").trim().to_string();
        } else if trimmed.starts_with("Subject:") {
            subject = trimmed.strip_prefix("Subject:").unwrap_or("").trim().to_string();
        } else if trimmed.starts_with("Date:") {
            date = trimmed.strip_prefix("Date:").unwrap_or("").trim().to_string();
        } else if trimmed.starts_with("Message-ID:") || trimmed.starts_with("Message-Id:") {
            let raw = trimmed.split_once(':').map(|(_, v)| v.trim().to_string());
            message_id = raw;
        } else if trimmed.contains("BODY[TEXT]") {
            in_body = true;
        } else if in_body {
            if trimmed == ")" || trimmed.starts_with("A0") || trimmed.starts_with("F0") {
                in_body = false;
            } else if !trimmed.starts_with('{')
                && body.len() < 32_000 {
                    if !body.is_empty() {
                        body.push('\n');
                    }
                    body.push_str(trimmed);
                }
        }
    }

    EmailFull { from, to, subject, date, message_id, body, seq: 0 }
}

// ── Connection-reusing fetch internals ────────────────────────

/// Fetch inbox summaries on an existing IMAP connection.
/// `tag_num` is updated to reflect tags consumed.
async fn fetch_inbox_with_conn(
    reader: &mut ImapReader,
    writer: &mut ImapWriter,
    tag_num: &mut u32,
    limit: usize,
    unread_only: bool,
) -> Result<Vec<EmailSummary>> {
    // SEARCH
    let search_tag = format!("A{:03}", *tag_num);
    *tag_num += 1;
    let search_cmd = if unread_only {
        format!("{search_tag} SEARCH UNSEEN\r\n")
    } else {
        format!("{search_tag} SEARCH ALL\r\n")
    };
    writer.write_all(search_cmd.as_bytes()).await.map_err(|e| {
        aivyx_core::AivyxError::Channel(format!("IMAP write error: {e}"))
    })?;
    let search_result = wait_for_tag(reader, &search_tag).await?;
    let seq_numbers = parse_search_response(&search_result);

    // Take only the last N messages
    let to_fetch: Vec<_> = seq_numbers.iter().rev().take(limit).copied().collect();

    let mut summaries = Vec::new();
    for seq in &to_fetch {
        let tag = format!("A{:03}", *tag_num);
        *tag_num += 1;
        let fetch_cmd = format!(
            "{tag} FETCH {seq} (BODY[HEADER.FIELDS (FROM SUBJECT MESSAGE-ID)] BODY[TEXT])\r\n"
        );
        writer.write_all(fetch_cmd.as_bytes()).await.map_err(|e| {
            aivyx_core::AivyxError::Channel(format!("IMAP write error: {e}"))
        })?;
        let fetch_result = wait_for_tag(reader, &tag).await?;
        let (from, subject, preview, message_id) = parse_fetch_headers(&fetch_result);

        summaries.push(EmailSummary {
            from,
            subject,
            preview,
            seq: *seq,
            message_id,
        });
    }

    Ok(summaries)
}

/// Fetch a single email on an existing IMAP connection.
async fn fetch_single_with_conn(
    reader: &mut ImapReader,
    writer: &mut ImapWriter,
    tag_num: &mut u32,
    seq: u32,
) -> Result<EmailFull> {
    let tag = format!("A{:03}", *tag_num);
    *tag_num += 1;
    let fetch_cmd = format!(
        "{tag} FETCH {seq} (BODY[HEADER.FIELDS (FROM TO SUBJECT DATE MESSAGE-ID)] BODY[TEXT])\r\n"
    );
    writer.write_all(fetch_cmd.as_bytes()).await.map_err(|e| {
        aivyx_core::AivyxError::Channel(format!("IMAP write error: {e}"))
    })?;
    let fetch_result = wait_for_tag(reader, &tag).await?;

    let mut email = parse_full_email(&fetch_result);
    email.seq = seq;

    Ok(email)
}

// ── IMAP STORE / COPY / EXPUNGE internals ─────────────────────

/// Set flags on a message.
async fn store_flags_with_conn(
    reader: &mut ImapReader,
    writer: &mut ImapWriter,
    tag_num: &mut u32,
    seq: u32,
    flags: &str,
) -> Result<()> {
    let tag = format!("A{:03}", *tag_num);
    *tag_num += 1;
    let cmd = format!("{tag} STORE {seq} {flags}\r\n");
    writer.write_all(cmd.as_bytes()).await.map_err(|e| {
        aivyx_core::AivyxError::Channel(format!("IMAP write error: {e}"))
    })?;
    let response = wait_for_tag(reader, &tag).await?;
    if response.contains("NO") || response.contains("BAD") {
        return Err(aivyx_core::AivyxError::Channel(
            format!("IMAP STORE failed: {response}"),
        ));
    }
    Ok(())
}

/// Copy a message to another folder, mark it \Deleted in current folder, and EXPUNGE.
async fn copy_and_delete_with_conn(
    reader: &mut ImapReader,
    writer: &mut ImapWriter,
    tag_num: &mut u32,
    seq: u32,
    folder: &str,
) -> Result<()> {
    // COPY
    let copy_tag = format!("A{:03}", *tag_num);
    *tag_num += 1;
    let copy_cmd = format!("{copy_tag} COPY {seq} \"{}\"\r\n", escape_imap(folder));
    writer.write_all(copy_cmd.as_bytes()).await.map_err(|e| {
        aivyx_core::AivyxError::Channel(format!("IMAP write error: {e}"))
    })?;
    let copy_resp = wait_for_tag(reader, &copy_tag).await?;
    if copy_resp.contains("NO") || copy_resp.contains("BAD") {
        return Err(aivyx_core::AivyxError::Channel(
            format!("IMAP COPY to '{folder}' failed: {copy_resp}"),
        ));
    }

    // STORE \Deleted + EXPUNGE
    delete_message_with_conn(reader, writer, tag_num, seq).await
}

/// Mark a message as \Deleted and EXPUNGE it.
async fn delete_message_with_conn(
    reader: &mut ImapReader,
    writer: &mut ImapWriter,
    tag_num: &mut u32,
    seq: u32,
) -> Result<()> {
    // STORE +FLAGS (\Deleted)
    store_flags_with_conn(reader, writer, tag_num, seq, "+FLAGS (\\Deleted)").await?;

    // EXPUNGE
    let exp_tag = format!("A{:03}", *tag_num);
    *tag_num += 1;
    let exp_cmd = format!("{exp_tag} EXPUNGE\r\n");
    writer.write_all(exp_cmd.as_bytes()).await.map_err(|e| {
        aivyx_core::AivyxError::Channel(format!("IMAP write error: {e}"))
    })?;
    let exp_resp = wait_for_tag(reader, &exp_tag).await?;
    if exp_resp.contains("NO") || exp_resp.contains("BAD") {
        return Err(aivyx_core::AivyxError::Channel(
            format!("IMAP EXPUNGE failed: {exp_resp}"),
        ));
    }
    Ok(())
}

// ── Legacy per-call functions (used where no pool is available) ──

/// Fetch recent messages from IMAP inbox (public for loop triage).
///
/// This creates a fresh connection per call. Prefer [`ImapPool::fetch_inbox`]
/// when a pool is available.
pub async fn fetch_inbox_internal(
    config: &EmailConfig,
    limit: usize,
    unread_only: bool,
) -> Result<Vec<EmailSummary>> {
    let (mut reader, mut writer) = imap_connect(config).await?;
    let mut tag_num: u32 = 3; // A001=LOGIN, A002=SELECT already used

    let result = fetch_inbox_with_conn(&mut reader, &mut writer, &mut tag_num, limit, unread_only).await;

    // LOGOUT (best-effort)
    let _ = writer.write_all(b"A099 LOGOUT\r\n").await;

    result
}

/// Fetch a single email by IMAP sequence number with full headers and body.
///
/// This creates a fresh connection per call. Prefer [`ImapPool::fetch_single`]
/// when a pool is available.
pub async fn fetch_single(config: &EmailConfig, seq: u32) -> Result<EmailFull> {
    let (mut reader, mut writer) = imap_connect(config).await?;
    let mut tag_num: u32 = 3;

    let result = fetch_single_with_conn(&mut reader, &mut writer, &mut tag_num, seq).await;

    // LOGOUT (best-effort)
    let _ = writer.write_all(b"A099 LOGOUT\r\n").await;

    result
}

/// Send email via SMTP using lettre.
async fn send_smtp(
    config: &EmailConfig,
    to: &str,
    subject: &str,
    body: &str,
    in_reply_to: Option<&str>,
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

    let mut builder = lettre::Message::builder()
        .from(Mailbox::new(Some("Aivyx Assistant".into()), from_addr))
        .to(Mailbox::new(None, to_addr))
        .subject(subject);

    if let Some(reply_id) = in_reply_to {
        builder = builder.in_reply_to(reply_id.to_string());
        builder = builder.references(reply_id.to_string());
    }

    let email = builder
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

/// Public wrapper for sending a reply (used by triage and other loop modules).
pub async fn send_reply(
    config: &EmailConfig,
    to: &str,
    subject: &str,
    body: &str,
    in_reply_to: Option<&str>,
) -> Result<()> {
    send_smtp(config, to, subject, body, in_reply_to).await
}

// ── Action implementations ──────────────────────────────────────

pub struct ReadInbox {
    pub config: EmailConfig,
    pub pool: Option<Arc<ImapPool>>,
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
        let summaries = if let Some(ref pool) = self.pool {
            crate::retry::retry(
                &crate::retry::RetryConfig::network(),
                || pool.fetch_inbox(limit, unread_only),
                crate::retry::is_transient,
            ).await?
        } else {
            let config = self.config.clone();
            crate::retry::retry(
                &crate::retry::RetryConfig::network(),
                || fetch_inbox_internal(&config, limit, unread_only),
                crate::retry::is_transient,
            ).await?
        };
        Ok(serde_json::to_value(summaries)?)
    }
}

pub struct FetchEmail {
    pub config: EmailConfig,
    pub pool: Option<Arc<ImapPool>>,
}

#[async_trait::async_trait]
impl Action for FetchEmail {
    fn name(&self) -> &str { "fetch_email" }

    fn description(&self) -> &str {
        "Fetch the full content of a specific email by its IMAP sequence number. \
         Use this after read_email to get the complete body for drafting replies."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "seq": {
                    "type": "integer",
                    "description": "IMAP sequence number from read_email results"
                }
            },
            "required": ["seq"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let seq = input["seq"].as_u64().ok_or_else(|| {
            aivyx_core::AivyxError::Validation("'seq' must be an integer".into())
        })? as u32;
        let email = if let Some(ref pool) = self.pool {
            crate::retry::retry(
                &crate::retry::RetryConfig::network(),
                || pool.fetch_single(seq),
                crate::retry::is_transient,
            ).await?
        } else {
            let config = self.config.clone();
            crate::retry::retry(
                &crate::retry::RetryConfig::network(),
                || fetch_single(&config, seq),
                crate::retry::is_transient,
            ).await?
        };
        Ok(serde_json::to_value(email)?)
    }
}

pub struct SendEmail {
    pub config: EmailConfig,
}

#[async_trait::async_trait]
impl Action for SendEmail {
    fn name(&self) -> &str { "send_email" }

    fn description(&self) -> &str {
        "Send an email message. Include in_reply_to with the original Message-ID when replying."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "to": { "type": "string", "description": "Recipient email address" },
                "subject": { "type": "string", "description": "Email subject" },
                "body": { "type": "string", "description": "Email body text" },
                "in_reply_to": {
                    "type": "string",
                    "description": "Message-ID of the email being replied to (for threading)"
                }
            },
            "required": ["to", "subject", "body"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let to = input["to"].as_str()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("'to' is required".into()))?;
        let subject = input["subject"].as_str()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("'subject' is required".into()))?;
        let body = input["body"].as_str()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("'body' is required".into()))?;
        if to.is_empty() {
            return Err(aivyx_core::AivyxError::Validation("'to' must not be empty".into()));
        }
        let in_reply_to = input["in_reply_to"].as_str();
        send_smtp(&self.config, to, subject, body, in_reply_to).await?;
        Ok(serde_json::json!({ "status": "sent", "to": to }))
    }
}

// ── Email management tools ──────────────────────────────────────

/// Tool: mark an email as read (\Seen flag).
pub struct MarkEmailRead {
    pub config: EmailConfig,
    pub pool: Option<Arc<ImapPool>>,
}

#[async_trait::async_trait]
impl Action for MarkEmailRead {
    fn name(&self) -> &str { "mark_email_read" }

    fn description(&self) -> &str {
        "Mark an email as read by setting the \\Seen flag."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["seq"],
            "properties": {
                "seq": { "type": "integer", "description": "IMAP sequence number" }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let seq = input["seq"].as_u64().ok_or_else(|| {
            aivyx_core::AivyxError::Validation("'seq' must be an integer".into())
        })? as u32;

        if let Some(ref pool) = self.pool {
            pool.store_flags(seq, "+FLAGS (\\Seen)").await?;
        } else {
            let (mut reader, mut writer) = imap_connect(&self.config).await?;
            let mut tag_num: u32 = 3;
            store_flags_with_conn(&mut reader, &mut writer, &mut tag_num, seq, "+FLAGS (\\Seen)").await?;
            let _ = writer.write_all(b"A099 LOGOUT\r\n").await;
        }

        Ok(serde_json::json!({ "status": "marked_read", "seq": seq }))
    }
}

/// Tool: archive an email (move to archive folder).
pub struct ArchiveEmail {
    pub config: EmailConfig,
    pub pool: Option<Arc<ImapPool>>,
}

#[async_trait::async_trait]
impl Action for ArchiveEmail {
    fn name(&self) -> &str { "archive_email" }

    fn description(&self) -> &str {
        "Archive an email by moving it to the archive folder. \
         Defaults to 'Archive' but a custom folder can be specified."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["seq"],
            "properties": {
                "seq": { "type": "integer", "description": "IMAP sequence number" },
                "folder": { "type": "string", "description": "Target folder (default: Archive)", "default": "Archive" }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let seq = input["seq"].as_u64().ok_or_else(|| {
            aivyx_core::AivyxError::Validation("'seq' must be an integer".into())
        })? as u32;
        let folder = input["folder"].as_str().unwrap_or("Archive");

        if let Some(ref pool) = self.pool {
            pool.copy_and_delete(seq, folder).await?;
        } else {
            let (mut reader, mut writer) = imap_connect(&self.config).await?;
            let mut tag_num: u32 = 3;
            copy_and_delete_with_conn(&mut reader, &mut writer, &mut tag_num, seq, folder).await?;
            let _ = writer.write_all(b"A099 LOGOUT\r\n").await;
        }

        Ok(serde_json::json!({ "status": "archived", "seq": seq, "folder": folder }))
    }
}

/// Tool: permanently delete an email.
pub struct DeleteEmail {
    pub config: EmailConfig,
    pub pool: Option<Arc<ImapPool>>,
}

#[async_trait::async_trait]
impl Action for DeleteEmail {
    fn name(&self) -> &str { "delete_email" }

    fn description(&self) -> &str {
        "Permanently delete an email by marking it \\Deleted and expunging."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["seq"],
            "properties": {
                "seq": { "type": "integer", "description": "IMAP sequence number" }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let seq = input["seq"].as_u64().ok_or_else(|| {
            aivyx_core::AivyxError::Validation("'seq' must be an integer".into())
        })? as u32;

        if let Some(ref pool) = self.pool {
            pool.delete_message(seq).await?;
        } else {
            let (mut reader, mut writer) = imap_connect(&self.config).await?;
            let mut tag_num: u32 = 3;
            delete_message_with_conn(&mut reader, &mut writer, &mut tag_num, seq).await?;
            let _ = writer.write_all(b"A099 LOGOUT\r\n").await;
        }

        Ok(serde_json::json!({ "status": "deleted", "seq": seq }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_headers_extracts_message_id() {
        let response = "\
* 1 FETCH (BODY[HEADER.FIELDS (FROM SUBJECT MESSAGE-ID)] {120}\r\n\
From: alice@example.com\r\n\
Subject: Hello\r\n\
Message-ID: <abc123@mail.example.com>\r\n\
\r\n\
 BODY[TEXT] {5}\r\n\
Hello\r\n\
)\r\n\
F000 OK FETCH completed\r\n";

        let (from, subject, _preview, message_id) = parse_fetch_headers(response);
        assert_eq!(from, "alice@example.com");
        assert_eq!(subject, "Hello");
        assert_eq!(message_id.as_deref(), Some("<abc123@mail.example.com>"));
    }

    #[test]
    fn parse_headers_no_message_id() {
        let response = "\
* 1 FETCH (BODY[HEADER.FIELDS (FROM SUBJECT MESSAGE-ID)] {80}\r\n\
From: bob@example.com\r\n\
Subject: No ID\r\n\
\r\n\
 BODY[TEXT] {3}\r\n\
Hi\r\n\
)\r\n\
F000 OK FETCH completed\r\n";

        let (from, subject, _preview, message_id) = parse_fetch_headers(response);
        assert_eq!(from, "bob@example.com");
        assert_eq!(subject, "No ID");
        assert!(message_id.is_none());
    }

    #[test]
    fn parse_headers_message_id_lowercase() {
        let response = "\
From: carol@example.com\r\n\
Subject: Test\r\n\
Message-Id: <def456@mx.example.com>\r\n\
\r\n\
 BODY[TEXT] {4}\r\n\
Test\r\n\
)\r\n\
F000 OK FETCH completed\r\n";

        let (_from, _subject, _preview, message_id) = parse_fetch_headers(response);
        assert_eq!(message_id.as_deref(), Some("<def456@mx.example.com>"));
    }

    #[test]
    fn parse_full_email_complete_body() {
        let response = "\
* 1 FETCH (BODY[HEADER.FIELDS (FROM TO SUBJECT DATE MESSAGE-ID)] {200}\r\n\
From: alice@example.com\r\n\
To: julian@example.com\r\n\
Subject: Project deadline\r\n\
Date: Wed, 02 Apr 2026 10:30:00 +0000\r\n\
Message-ID: <abc@mail.example.com>\r\n\
\r\n\
 BODY[TEXT] {100}\r\n\
Hi Julian,\r\n\
\r\n\
The deadline is next Friday. Can you confirm?\r\n\
\r\n\
Thanks,\r\n\
Alice\r\n\
)\r\n\
A003 OK FETCH completed\r\n";

        let email = parse_full_email(response);
        assert_eq!(email.from, "alice@example.com");
        assert_eq!(email.to, "julian@example.com");
        assert_eq!(email.subject, "Project deadline");
        assert!(email.date.contains("2026"));
        assert_eq!(email.message_id.as_deref(), Some("<abc@mail.example.com>"));
        assert!(email.body.contains("deadline is next Friday"));
        assert!(email.body.contains("Alice"));
    }

    #[test]
    fn parse_full_email_no_truncation_at_200() {
        let long_line = "x".repeat(300);
        let response = format!(
            "From: a@b.com\r\n\
Subject: Long\r\n\
\r\n\
 BODY[TEXT] {{300}}\r\n\
{long_line}\r\n\
)\r\n\
A003 OK FETCH completed\r\n"
        );
        let email = parse_full_email(&response);
        // parse_full_email allows up to 32k, not 200
        assert_eq!(email.body.len(), 300);
    }

    #[test]
    fn fetch_email_schema_requires_seq() {
        let action = FetchEmail {
            config: EmailConfig {
                imap_host: String::new(),
                imap_port: 993,
                smtp_host: String::new(),
                smtp_port: 587,
                address: String::new(),
                username: String::new(),
                password: String::new(),
            },
            pool: None,
        };
        let schema = action.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "seq"));
    }

    #[test]
    fn send_email_schema_has_optional_in_reply_to() {
        let action = SendEmail {
            config: EmailConfig {
                imap_host: String::new(),
                imap_port: 993,
                smtp_host: String::new(),
                smtp_port: 587,
                address: String::new(),
                username: String::new(),
                password: String::new(),
            },
        };
        let schema = action.input_schema();
        assert!(schema["properties"]["in_reply_to"].is_object());
        // in_reply_to should NOT be in required
        let required = schema["required"].as_array().unwrap();
        assert!(!required.iter().any(|v| v == "in_reply_to"));
    }

    #[test]
    fn pool_ttl_constant_is_5_minutes() {
        assert_eq!(IMAP_POOL_TTL, std::time::Duration::from_secs(300));
    }

    #[test]
    fn pool_creates_arc() {
        let config = EmailConfig {
            imap_host: "imap.example.com".into(),
            imap_port: 993,
            smtp_host: "smtp.example.com".into(),
            smtp_port: 587,
            address: "test@example.com".into(),
            username: "test".into(),
            password: "secret".into(),
        };
        let pool = ImapPool::new(config);
        // Pool is wrapped in Arc, so we can clone cheaply
        let pool2 = Arc::clone(&pool);
        // Both point to the same allocation
        assert!(Arc::ptr_eq(&pool, &pool2));
    }

    // ── Email management tool tests ─────────────────────────────

    fn test_email_config() -> EmailConfig {
        EmailConfig {
            imap_host: String::new(),
            imap_port: 993,
            smtp_host: String::new(),
            smtp_port: 587,
            address: String::new(),
            username: String::new(),
            password: String::new(),
        }
    }

    #[test]
    fn mark_email_read_schema() {
        let tool = MarkEmailRead { config: test_email_config(), pool: None };
        assert_eq!(tool.name(), "mark_email_read");
        let schema = tool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "seq"));
        assert_eq!(required.len(), 1);
    }

    #[test]
    fn archive_email_schema() {
        let tool = ArchiveEmail { config: test_email_config(), pool: None };
        assert_eq!(tool.name(), "archive_email");
        let schema = tool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "seq"));
        // folder is optional with default
        assert!(!required.iter().any(|v| v == "folder"));
        assert!(schema["properties"]["folder"].is_object());
    }

    #[test]
    fn delete_email_schema() {
        let tool = DeleteEmail { config: test_email_config(), pool: None };
        assert_eq!(tool.name(), "delete_email");
        let schema = tool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "seq"));
        assert_eq!(required.len(), 1);
    }
}
