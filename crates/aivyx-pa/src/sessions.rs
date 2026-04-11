//! Chat session persistence.
//!
//! Manages multi-session conversations stored in the encrypted store.
//! Each session has metadata (title, timestamps, turn count) and a
//! message history capped at 500 messages.
//!
//! ## Storage format
//!
//! Messages are stored as `Vec<SessionMessage>` (v2 format). For backward
//! compatibility with the legacy `Vec<(String, String)>` format (v1), the
//! [`load_chat_messages`] function auto-detects and upgrades on read.

use std::collections::{HashMap, HashSet};

use aivyx_agent::AgentStateSnapshot;
use aivyx_agent::cost_tracker::CostSnapshot;
use aivyx_core::AivyxError;
use aivyx_crypto::{EncryptedStore, MasterKey};
use aivyx_llm::message::{ChatMessage, Role, ToolResult};

/// Maximum number of chat messages to persist per session.
const MAX_CONVERSATION_MESSAGES: usize = 500;

// ── Namespace prefixes ─────────────────────────────────────────
//
// All PA chat records are stored under the `pa_chat/` namespace. The
// `/` delimiter is deliberate: session IDs are timestamp-millis
// integers with no `/`, so key components are unambiguously splittable
// and the prefix cannot alias any legacy `chat:`-style key (old prefix
// used `:` as the separator). See the C1 store-namespace refactor for
// the collision history that motivated this.
pub(crate) const PA_CHAT_PREFIX: &str = "pa_chat/";

/// Marker key stamped once the legacy `chat:` → `pa_chat/` migration
/// has completed. Its presence short-circuits the migration scan on
/// every subsequent startup — the happy path is a single `get`.
const PA_CHAT_MIGRATION_MARKER: &str = "__migration__/pa_chat_v1";

/// Legacy prefix retained only for the one-shot migration. After
/// migration completes, no new code path writes under this prefix.
const LEGACY_CHAT_PREFIX: &str = "chat:";

/// Suffix of the legacy meta key (`chat:{id}:meta`).
const LEGACY_META_SUFFIX: &str = ":meta";
/// Suffix of the legacy messages key (`chat:{id}:messages`).
const LEGACY_MESSAGES_SUFFIX: &str = ":messages";
/// Suffix of the legacy resume-token key (`chat:{id}:resume`).
const LEGACY_RESUME_SUFFIX: &str = ":resume";

/// Key-building helpers for the `pa_chat/` namespace. Every call site
/// that needs to read, write, or delete a PA chat record goes through
/// here — there is no other place in the codebase that constructs these
/// key strings, so a single edit here suffices to rename the namespace.
mod keys {
    use super::PA_CHAT_PREFIX;

    /// `pa_chat/{id}/meta`
    #[inline]
    pub(super) fn meta(session_id: &str) -> String {
        format!("{PA_CHAT_PREFIX}{session_id}/meta")
    }

    /// `pa_chat/{id}/messages`
    #[inline]
    pub(super) fn messages(session_id: &str) -> String {
        format!("{PA_CHAT_PREFIX}{session_id}/messages")
    }

    /// `pa_chat/{id}/resume`
    #[inline]
    pub(super) fn resume(session_id: &str) -> String {
        format!("{PA_CHAT_PREFIX}{session_id}/resume")
    }

    /// Suffix stripped from a meta key to recover the session ID.
    pub(super) const META_SUFFIX: &str = "/meta";
}

// ── Session message (v2 format) ────────────────────────────────

/// The full set of roles a [`SessionMessage`] is allowed to carry on
/// the wire. Any value outside this set is rejected at deserialization
/// time (see M2 in the persistence audit) to prevent silent role drift
/// — e.g. a typo like `"assitant"` that previously slipped through the
/// catch-all fallback in [`to_chat_messages`].
///
/// Kept as a `&'static [&'static str]` rather than an enum so existing
/// call sites that compare `sm.role == "user"` continue to work without
/// a sweeping refactor; the invariant is enforced at the boundary
/// instead of throughout the codebase.
pub(crate) const VALID_SESSION_ROLES: &[&str] = &["user", "assistant", "tool", "system"];

/// A single persisted message in a chat session.
///
/// Richer than the legacy `(role, content)` tuple: preserves tool
/// call names, tool results, and system notes so that restored
/// conversations give the LLM full context about past tool interactions.
///
/// # Deserialization invariants (M2)
///
/// The `#[serde(try_from = "SessionMessageRaw")]` attribute routes every
/// load through [`SessionMessageRaw::try_into`], which enforces:
///
/// 1. `role` must be one of [`VALID_SESSION_ROLES`]. Unknown roles are
///    rejected with a clear error — no silent fallback to `assistant`.
/// 2. If `role == "tool"`, then `tool_call_id` must be `Some(..)`. A
///    tool result with no call id is an orphan that would fail to
///    reattach at the LLM boundary, so it is rejected at load time.
///
/// Constructing a `SessionMessage` via the associated functions
/// (`user`, `assistant`, `tool`, `system`) is infallible because each
/// constructor produces a value that satisfies both invariants by
/// construction.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "SessionMessageRaw")]
pub struct SessionMessage {
    /// Message role. One of `"user"`, `"assistant"`, `"tool"`, or
    /// `"system"` — the load path will reject any other value.
    pub role: String,
    /// Text content of the message.
    pub content: String,
    /// For tool-result messages: the tool name that produced this result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// For tool-result messages: the tool_call ID this result corresponds to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Whether this tool result was an error.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_error: bool,
}

/// On-wire shape of [`SessionMessage`] before validation. This type
/// exists solely so serde's `#[serde(try_from = "SessionMessageRaw")]`
/// has somewhere to land the raw JSON before we run the M2 invariant
/// checks in [`TryFrom::try_from`].
///
/// Never construct or return this type outside the deserialization
/// path — it represents possibly-invalid bytes.
#[derive(serde::Deserialize)]
struct SessionMessageRaw {
    role: String,
    content: String,
    #[serde(default)]
    tool_name: Option<String>,
    #[serde(default)]
    tool_call_id: Option<String>,
    #[serde(default)]
    is_error: bool,
}

impl TryFrom<SessionMessageRaw> for SessionMessage {
    type Error = String;

    fn try_from(raw: SessionMessageRaw) -> Result<Self, Self::Error> {
        // Invariant 1: role must be in the known set. This is the
        // primary M2 fix — previously a typo like "assitant" flowed
        // through to_chat_messages and hit the `_ => assistant` arm,
        // silently recovering corrupted data and hiding the bug.
        //
        // We migrate "you" → "user" here because the legacy v1 format
        // used "you" as the display role for the user; keeping that
        // upgrade inside the validator (rather than at every call
        // site) is the cleanest place for the one-way rewrite.
        let normalized_role = if raw.role == "you" {
            "user".to_string()
        } else {
            raw.role
        };
        if !VALID_SESSION_ROLES.contains(&normalized_role.as_str()) {
            return Err(format!(
                "SessionMessage: unknown role '{normalized_role}' (expected one of {:?})",
                VALID_SESSION_ROLES
            ));
        }

        // Invariant 2: role == "tool" requires tool_call_id. A tool
        // result with no call id cannot reattach to its invoking
        // assistant message at LLM-boundary time, so it would either
        // be rejected by the provider or silently confuse the model.
        // Reject at load time so the corruption surfaces as an Err
        // from load_chat_messages (see H2), not as a mysterious LLM
        // error later.
        if normalized_role == "tool" && raw.tool_call_id.is_none() {
            return Err(
                "SessionMessage: role 'tool' requires tool_call_id to be present".to_string(),
            );
        }

        Ok(SessionMessage {
            role: normalized_role,
            content: raw.content,
            tool_name: raw.tool_name,
            tool_call_id: raw.tool_call_id,
            is_error: raw.is_error,
        })
    }
}

impl SessionMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: content.into(),
            tool_name: None,
            tool_call_id: None,
            is_error: false,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".into(),
            content: content.into(),
            tool_name: None,
            tool_call_id: None,
            is_error: false,
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            content: content.into(),
            tool_name: None,
            tool_call_id: None,
            is_error: false,
        }
    }

    pub fn tool(
        tool_name: impl Into<String>,
        tool_call_id: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self {
            role: "tool".into(),
            content: content.into(),
            tool_name: Some(tool_name.into()),
            tool_call_id: Some(tool_call_id.into()),
            is_error,
        }
    }
}

// ── Session metadata ───────────────────────────────────────────

/// Metadata for a saved chat session.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ChatSessionMeta {
    /// Unique session ID (timestamp-based).
    pub id: String,
    /// Display title (auto-generated from first message, or user-set).
    pub title: String,
    /// When the session was created.
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// When the session was last active.
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// Number of message pairs (turns).
    pub turn_count: usize,
}

impl ChatSessionMeta {
    pub fn new(title: impl Into<String>) -> Self {
        let now = chrono::Utc::now();
        Self {
            id: format!("{}", now.timestamp_millis()),
            title: title.into(),
            created_at: now,
            updated_at: now,
            turn_count: 0,
        }
    }

    fn meta_key(&self) -> String {
        keys::meta(&self.id)
    }
    fn messages_key(&self) -> String {
        keys::messages(&self.id)
    }
}

/// Generate a title from the first user message (truncate to ~40 chars).
pub fn auto_title(first_message: &str) -> String {
    let cleaned = first_message.trim().replace('\n', " ");
    if cleaned.len() <= 40 {
        cleaned
    } else {
        let truncated = &cleaned[..cleaned.ceil_char_boundary(37)];
        format!("{truncated}...")
    }
}

// ── CRUD operations ────────────────────────────────────────────

/// List all saved chat sessions, sorted by most recent first.
pub fn list_chat_sessions(store: &EncryptedStore, key: &MasterKey) -> Vec<ChatSessionMeta> {
    let keys = match store.list_keys() {
        Ok(k) => k,
        Err(_) => return Vec::new(),
    };
    let mut sessions: Vec<ChatSessionMeta> = keys
        .iter()
        .filter(|k| k.starts_with(PA_CHAT_PREFIX) && k.ends_with(self::keys::META_SUFFIX))
        .filter_map(|k| {
            store
                .get(k, key)
                .ok()
                .flatten()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        })
        .collect();
    sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    sessions
}

/// Forward-walk `raw_start` past any leading tool-result messages so the
/// kept window never begins with an orphaned `role == "tool"` entry.
///
/// The v2 `SessionMessage` format records tool pairings only through
/// `role == "tool"` rows whose `tool_call_id` references a prior
/// assistant message. If a hard cap lands such that the first kept
/// message is a tool result, the matching assistant call has already
/// been dropped — leaving a dangling tool_result the LLM will reject on
/// reload. This helper advances the start index until the first kept
/// message is no longer a tool, or until the whole window is empty.
///
/// Forward-walks at most a handful of messages (one tool cluster); if
/// the entire tail is nothing but tool results (pathological), returns
/// `messages.len()` and the caller will save an empty window.
fn snap_to_safe_session_split(messages: &[SessionMessage], raw_start: usize) -> usize {
    let mut idx = raw_start;
    while idx < messages.len() && messages[idx].role == "tool" {
        idx += 1;
    }
    idx
}

/// Save a chat session (messages + metadata) **atomically**.
///
/// Accepts `&[SessionMessage]` (v2 format). Use [`conversation_to_session`]
/// to convert from the agent's `ChatMessage` conversation history.
///
/// # Atomicity
///
/// Messages and metadata are written inside a **single** encrypted-store
/// write transaction via [`EncryptedStore::put_many`]. Either both records
/// are persisted or neither is — there is no partial-write state where the
/// messages are saved without the metadata (or vice versa). This closes
/// the failure mode where a crash between the two writes would leave an
/// orphaned messages record that could not be listed or resumed.
///
/// # Errors
///
/// Returns `Err` if serialization or the underlying atomic write fails.
/// Callers **must** handle this — dropping the error is equivalent to
/// silently losing the save. The TUI awaits this call and surfaces
/// failures so the user can retry before leaving the session.
pub fn save_chat_session(
    store: &EncryptedStore,
    key: &MasterKey,
    meta: &ChatSessionMeta,
    messages: &[SessionMessage],
) -> Result<(), AivyxError> {
    let to_save = if messages.len() > MAX_CONVERSATION_MESSAGES {
        // Hard cap fires. Instead of a raw front-truncation (which can
        // leave a `role == "tool"` message at index 0 with no preceding
        // assistant call — an orphaned tool result that the LLM will
        // reject on reload), start at the last `MAX_CONVERSATION_MESSAGES`
        // and **forward-walk** past any leading tool results. See H1 in
        // the persistence audit for the historical data-loss shape.
        let raw_start = messages.len() - MAX_CONVERSATION_MESSAGES;
        let safe_start = snap_to_safe_session_split(messages, raw_start);
        let dropped = safe_start;
        tracing::warn!(
            session_id = %meta.id,
            total = messages.len(),
            dropped,
            kept = messages.len() - safe_start,
            cap = MAX_CONVERSATION_MESSAGES,
            "save_chat_session hit MAX_CONVERSATION_MESSAGES cap; dropping oldest messages (forward-snapped past tool cluster)"
        );
        &messages[safe_start..]
    } else {
        messages
    };

    let messages_json = serde_json::to_vec(to_save)
        .map_err(|e| AivyxError::Storage(format!("session messages serialization failed: {e}")))?;
    let meta_json = serde_json::to_vec(meta)
        .map_err(|e| AivyxError::Storage(format!("session metadata serialization failed: {e}")))?;

    let messages_key = meta.messages_key();
    let meta_key = meta.meta_key();
    let entries: &[(&str, &[u8])] = &[
        (messages_key.as_str(), messages_json.as_slice()),
        (meta_key.as_str(), meta_json.as_slice()),
    ];
    store.put_many(entries, key)
}

/// Load messages for a specific chat session.
///
/// Auto-detects storage format:
/// - **v2** (current): `Vec<SessionMessage>` — used directly.
/// - **v1** (legacy): `Vec<(String, String)>` — upgraded to `SessionMessage`.
///
/// # Return shape
///
/// - `Ok(None)` — the session has no persisted messages (never saved,
///   already deleted, or fresh session). Callers may safely treat this
///   as "start empty".
/// - `Ok(Some(msgs))` — messages were read and parsed successfully.
/// - `Err(_)` — **the record exists but could not be read**. Either the
///   encrypted store returned an I/O / decrypt error, or the decrypted
///   bytes matched neither the v2 nor the v1 schema (corruption or a
///   future format). Callers that are about to overwrite this session
///   (save-after-load) **must** refuse on `Err` — overwriting would
///   destroy whatever was actually on disk.
///
/// See H2 in the persistence audit for the data-loss shape this
/// distinction closes: previously all three failure modes collapsed
/// into `None`, and the save path would then happily overwrite an
/// unreadable (but still-present) record.
pub fn load_chat_messages(
    store: &EncryptedStore,
    key: &MasterKey,
    session_id: &str,
) -> Result<Option<Vec<SessionMessage>>, AivyxError> {
    let msg_key = keys::messages(session_id);
    let bytes = match store.get(&msg_key, key).map_err(|e| {
        AivyxError::Storage(format!(
            "load_chat_messages: store read failed for session '{session_id}': {e}"
        ))
    })? {
        Some(b) => b,
        None => return Ok(None),
    };

    // Try v2 format first (Vec<SessionMessage>)
    if let Ok(messages) = serde_json::from_slice::<Vec<SessionMessage>>(&bytes) {
        return Ok(Some(messages));
    }

    // Fall back to v1 format (Vec<(String, String)>)
    if let Ok(pairs) = serde_json::from_slice::<Vec<(String, String)>>(&bytes) {
        let upgraded: Vec<SessionMessage> = pairs
            .into_iter()
            .map(|(role, content)| match role.as_str() {
                "you" | "user" => SessionMessage::user(content),
                _ => SessionMessage::assistant(content),
            })
            .collect();
        return Ok(Some(upgraded));
    }

    // Bytes decrypted but matched neither schema — corruption or a
    // format from the future. Surfacing this as an error prevents the
    // save path from overwriting the record in the mistaken belief
    // that it does not exist.
    Err(AivyxError::Storage(format!(
        "load_chat_messages: session '{session_id}' exists but bytes match neither v2 nor v1 schema ({} bytes)",
        bytes.len()
    )))
}

/// Delete a chat session (messages + metadata + resume token).
pub fn delete_chat_session(store: &EncryptedStore, session_id: &str) {
    let _ = store.delete(&keys::meta(session_id));
    let _ = store.delete(&keys::messages(session_id));
    let _ = store.delete(&keys::resume(session_id));
}

// ── Conversion helpers ─────────────────────────────────────────

/// Maximum tool-result byte length before `conversation_to_session`
/// truncates with an explicit marker.
///
/// Kept at 2000 to preserve the historical per-message size budget.
pub(crate) const TOOL_RESULT_MAX_BYTES: usize = 2000;

/// Prefix that `truncate_tool_result_with_marker` uses to sentinel
/// truncated content. The full marker has the form
/// `"\n\n[aivyx: tool result truncated — original was {N} bytes, kept first {K} bytes]"`.
///
/// A natural tool output ending in literal `...` — common in stack
/// traces or CLI progress output — is the motivating case: pre-M5 we
/// appended a bare `...` on truncation, so the restored session
/// contained an ambiguous suffix that neither a human nor the agent
/// could distinguish from "the tool itself emitted `...`". Downstream
/// code that wants to detect truncation can grep for this constant.
pub const TOOL_RESULT_TRUNCATION_MARKER_PREFIX: &str = "\n\n[aivyx: tool result truncated";

/// Truncate an oversized tool-result string at a UTF-8 safe boundary
/// and append a self-describing marker recording the original byte
/// count. Inputs shorter than [`TOOL_RESULT_MAX_BYTES`] are returned
/// untouched.
///
/// # M5 invariants
///
/// - **Distinguishable from natural `...`.** The marker begins with
///   the distinctive prefix [`TOOL_RESULT_TRUNCATION_MARKER_PREFIX`]
///   so a round-tripped tool result whose own content happened to end
///   in `...` will never be mistaken for truncation.
/// - **Original size is recoverable.** The marker embeds both the
///   original byte count and the kept-prefix byte count, so a human
///   reading restored sessions can see exactly how much was lost.
/// - **UTF-8 safe.** `ceil_char_boundary` is used to move the split
///   point forward to the next char boundary, never splitting a
///   multi-byte sequence.
pub(crate) fn truncate_tool_result_with_marker(content: &str) -> String {
    if content.len() <= TOOL_RESULT_MAX_BYTES {
        return content.to_string();
    }
    // Keep most of the budget for the payload; the marker itself is
    // small but non-trivial. We aim for ~1800 bytes of kept content
    // so the combined string (payload + marker) stays close to the
    // historical ~2010-byte ceiling.
    const KEEP_BYTES_TARGET: usize = 1800;
    let boundary = content.ceil_char_boundary(KEEP_BYTES_TARGET);
    let kept = &content[..boundary];
    format!(
        "{kept}{TOOL_RESULT_TRUNCATION_MARKER_PREFIX} — original was {} bytes, kept first {} bytes]",
        content.len(),
        kept.len(),
    )
}

/// Convert the agent's in-memory `ChatMessage` conversation into
/// `SessionMessage` values suitable for persistence.
///
/// Preserves tool results with their tool name and call ID, and keeps
/// system notes (like rescue guidance). Assistant messages that contain
/// tool calls get a summary annotation so the LLM knows tools were used
/// even after restoration.
pub fn conversation_to_session(conversation: &[ChatMessage]) -> Vec<SessionMessage> {
    let mut result = Vec::with_capacity(conversation.len());

    for msg in conversation {
        match msg.role {
            Role::User => {
                result.push(SessionMessage::user(msg.content.to_text()));
            }
            Role::Assistant => {
                let text = msg.content.to_text();

                if msg.tool_calls.is_empty() {
                    // Plain assistant text
                    result.push(SessionMessage::assistant(text));
                } else {
                    // Assistant message with tool calls — annotate the text
                    // so the restored conversation shows what tools were invoked.
                    let tool_names: Vec<&str> =
                        msg.tool_calls.iter().map(|tc| tc.name.as_str()).collect();
                    let annotation = format!("[Used tools: {}]", tool_names.join(", "));
                    let combined = if text.is_empty() {
                        annotation
                    } else {
                        format!("{text}\n\n{annotation}")
                    };
                    result.push(SessionMessage::assistant(combined));
                }
            }
            Role::Tool => {
                if let Some(ref tr) = msg.tool_result {
                    // Stringify the tool result content. Truncate very large
                    // results to keep session storage bounded.
                    let content_str = match &tr.content {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    // M5: use `truncate_tool_result_with_marker` so the
                    // persisted form carries an unambiguous sentinel
                    // that distinguishes "tool produced a '...' on
                    // purpose" from "aivyx dropped bytes here". The
                    // sentinel embeds the original byte count so a
                    // human (or a future parser) can tell exactly how
                    // much was lost.
                    let truncated = truncate_tool_result_with_marker(&content_str);

                    // Extract tool name from tool_call_id if possible, or
                    // search the preceding assistant message for the matching call.
                    let tool_name = find_tool_name_for_call_id(&tr.tool_call_id, conversation);

                    result.push(SessionMessage::tool(
                        tool_name.unwrap_or("unknown"),
                        &tr.tool_call_id,
                        truncated,
                        tr.is_error,
                    ));
                }
            }
            Role::System => {
                result.push(SessionMessage::system(msg.content.to_text()));
            }
        }
    }

    result
}

/// Search backward through conversation to find the tool name for a given call ID.
fn find_tool_name_for_call_id<'a>(
    call_id: &str,
    conversation: &'a [ChatMessage],
) -> Option<&'a str> {
    for msg in conversation.iter().rev() {
        if msg.role == Role::Assistant {
            for tc in &msg.tool_calls {
                if tc.id == call_id {
                    return Some(&tc.name);
                }
            }
        }
    }
    None
}

/// Convert stored `SessionMessage` values into `ChatMessage` values
/// suitable for `Agent::restore_conversation()`.
///
/// Tool results are reconstructed as proper `ChatMessage::tool()` messages
/// so the LLM sees the full interaction history including tool outputs.
/// System notes (rescue guidance, etc.) are restored as user messages
/// since most LLM providers only accept user/assistant/tool roles.
pub fn to_chat_messages(messages: &[SessionMessage]) -> Vec<ChatMessage> {
    messages
        .iter()
        .map(|sm| {
            // M2: sm.role is guaranteed to be one of VALID_SESSION_ROLES
            // because every SessionMessage that reaches this function came
            // through either a constructor (user/assistant/tool/system) or
            // through the validating deserializer (SessionMessageRaw).
            // The `_ => assistant` arm that used to live here was a silent
            // fallback that masked role typos; with M2 we can drop the
            // fallback entirely and let the match be exhaustive over the
            // known role strings. `debug_assert!` is the belt-and-braces
            // safety net in case a future edit ever bypasses the
            // validator.
            match sm.role.as_str() {
                "user" => ChatMessage::user(&sm.content),
                "assistant" => ChatMessage::assistant(&sm.content),
                "tool" => {
                    // The validator guarantees tool_call_id.is_some() for
                    // role == "tool", so the unwrap is the invariant
                    // spelled out. The legacy "restored" fallback was an
                    // M2-hiding workaround: it silently produced an LLM
                    // request with a fabricated call id that would fail
                    // to match any real call, which is exactly the bug
                    // the validator now catches upstream.
                    let call_id = sm
                        .tool_call_id
                        .as_deref()
                        .expect("SessionMessageRaw validator guarantees tool_call_id for role='tool'");
                    ChatMessage::tool(ToolResult {
                        tool_call_id: call_id.to_string(),
                        content: serde_json::Value::String(sm.content.clone()),
                        is_error: sm.is_error,
                    })
                }
                "system" => {
                    // Most providers don't support mid-conversation
                    // system messages. Restore as user messages (same as
                    // how rescue guidance is injected).
                    ChatMessage::user(&sm.content)
                }
                other => {
                    // Unreachable under the M2 validator invariant, but
                    // we degrade gracefully rather than panic in release:
                    // log loudly and fall back to assistant (the old
                    // behaviour) so a validator bypass doesn't crash the
                    // whole restore.
                    debug_assert!(
                        false,
                        "to_chat_messages: SessionMessage with unvalidated role '{other}' reached conversion — M2 invariant violated"
                    );
                    tracing::error!(
                        role = %other,
                        "to_chat_messages: unexpected role slipped past validator; falling back to assistant"
                    );
                    ChatMessage::assistant(&sm.content)
                }
            }
        })
        .collect()
}

/// Convert `SessionMessage` values to legacy display pairs for the TUI.
///
/// Filters out tool and system messages, returning only user/assistant
/// content suitable for the chat view.
pub fn to_display_pairs(messages: &[SessionMessage]) -> Vec<(String, String)> {
    // M2: `sm.role` is validated, so we only need to match the two
    // display-visible roles. The TUI's display layer historically used
    // `"you"` as the user-side label, so we preserve that on the way
    // out — but on the way in, all stored roles are `"user"`.
    messages
        .iter()
        .filter_map(|sm| match sm.role.as_str() {
            "user" => Some(("you".into(), sm.content.clone())),
            "assistant" => Some(("assistant".into(), sm.content.clone())),
            _ => None,
        })
        .collect()
}

/// Count conversation turns for metadata display.
///
/// A "turn" is anchored on the **user message** that opens it. The
/// assistant's reply (which may be plain text, tool calls, or a long
/// tool/assistant interleaving) is considered part of the same turn.
///
/// # M1: why not `min(user_count, assistant_count)`?
///
/// The pre-M1 formula returned the number of *completed* pairs, which
/// meant:
///
/// - While the assistant was mid-generation, the count was one behind
///   what the user actually saw on screen.
/// - The stale value was persisted to `meta.turn_count` on save, so a
///   reload after a crash could show a different number than the
///   session had when it was last opened — a small but real source
///   of "did I lose a message?" anxiety.
/// - It silently tolerated odd-parity message vectors (assistant
///   replies without matching user messages), which shouldn't happen
///   but isn't worth a panic either.
///
/// Counting user messages is monotonic (only grows when the user
/// speaks) and matches how humans naturally count a back-and-forth.
/// `message_count` / `turn_count` are display-only metadata — no
/// code branches on them — so the semantics change is safe.
pub fn count_turns(messages: &[SessionMessage]) -> usize {
    // M2: all user-role records are stored as `"user"` post-validator,
    // so the legacy `"you"` branch is no longer needed.
    messages.iter().filter(|m| m.role == "user").count()
}

// ── Migration ──────────────────────────────────────────────────

/// One-shot migration from the legacy `chat:` prefix (colon-delimited)
/// to the namespaced `pa_chat/` prefix (slash-delimited).
///
/// Runs on every startup but short-circuits via a marker key after the
/// first successful pass. Safe to call repeatedly; safe to interrupt.
///
/// # Safety properties
///
/// - **Idempotent.** Marker key `__migration__/pa_chat_v1` is checked
///   first with a single `get`; if present the function returns `Ok(())`
///   immediately. On fresh installs with no legacy data the marker is
///   still stamped so the next call is O(1).
/// - **Crash-safe.** New keys + marker are written inside a single
///   atomic `put_many`. If the process dies before the subsequent
///   legacy-delete pass, the marker guarantees the next run will see
///   the already-migrated state and skip entirely. The legacy records
///   are then cleaned up lazily on some future run… or never, which is
///   cosmetically ugly but not a correctness problem because no code
///   reads legacy keys after migration.
/// - **Non-destructive on read failures.** If a legacy key decrypts to
///   garbage or the read fails, that individual key is logged and
///   skipped — it stays on disk under its old name rather than being
///   lost. All other migratable keys still proceed.
///
/// Returns the number of legacy keys rewritten.
pub fn run_pa_chat_migration(
    store: &EncryptedStore,
    master_key: &MasterKey,
) -> Result<usize, AivyxError> {
    // Fast path: marker present → already migrated.
    if store
        .get(PA_CHAT_MIGRATION_MARKER, master_key)
        .ok()
        .flatten()
        .is_some()
    {
        return Ok(0);
    }

    let all_keys = store.list_keys()?;
    // Each entry: (old_key, new_key, encrypted_value_bytes).
    let mut migrations: Vec<(String, String, Vec<u8>)> = Vec::new();

    for k in &all_keys {
        let Some(rest) = k.strip_prefix(LEGACY_CHAT_PREFIX) else {
            continue;
        };

        // `rest` is `{id}{suffix}`. Match on the suffix in priority order.
        let new_key = if let Some(id) = rest.strip_suffix(LEGACY_META_SUFFIX) {
            keys::meta(id)
        } else if let Some(id) = rest.strip_suffix(LEGACY_MESSAGES_SUFFIX) {
            keys::messages(id)
        } else if let Some(id) = rest.strip_suffix(LEGACY_RESUME_SUFFIX) {
            keys::resume(id)
        } else {
            tracing::warn!(
                "pa_chat migration: skipping unrecognized legacy key '{k}' (no known suffix)"
            );
            continue;
        };

        match store.get(k, master_key) {
            Ok(Some(data)) => migrations.push((k.clone(), new_key, data)),
            Ok(None) => {
                tracing::warn!("pa_chat migration: legacy key '{k}' listed but value missing");
            }
            Err(e) => {
                tracing::warn!("pa_chat migration: skipping unreadable legacy key '{k}': {e}");
            }
        }
    }

    // Fresh install or nothing to migrate → stamp the marker so we skip
    // the scan next time and return.
    if migrations.is_empty() {
        store.put(PA_CHAT_MIGRATION_MARKER, b"1", master_key)?;
        return Ok(0);
    }

    // Single atomic write: all new keys + marker. Either every migrated
    // record is durably visible together, or none are.
    let mut entries: Vec<(&str, &[u8])> = Vec::with_capacity(migrations.len() + 1);
    for (_, new_key, data) in &migrations {
        entries.push((new_key.as_str(), data.as_slice()));
    }
    entries.push((PA_CHAT_MIGRATION_MARKER, b"1"));
    store.put_many(&entries, master_key)?;

    // Best-effort cleanup of legacy records. Failures here are logged
    // but not propagated — the new keys are the source of truth.
    let mut deleted = 0usize;
    for (old_key, _, _) in &migrations {
        match store.delete(old_key) {
            Ok(_) => deleted += 1,
            Err(e) => {
                tracing::warn!("pa_chat migration: failed to delete legacy key '{old_key}': {e}")
            }
        }
    }

    tracing::info!(
        "pa_chat migration: rewrote {} legacy key(s) to pa_chat/ namespace ({} legacy records deleted)",
        migrations.len(),
        deleted
    );
    Ok(migrations.len())
}

/// Migrate legacy single-conversation format to the new multi-session format.
/// If `conversation:display` exists, move it into a new chat session.
pub fn migrate_legacy_conversation(store: &EncryptedStore, key: &MasterKey) {
    let legacy_key = "conversation:display";
    let messages = match store.get(legacy_key, key) {
        Ok(Some(bytes)) => match serde_json::from_slice::<Vec<(String, String)>>(&bytes) {
            Ok(msgs) if !msgs.is_empty() => msgs,
            _ => return,
        },
        _ => return,
    };

    let title = messages
        .first()
        .filter(|(role, _)| role == "you")
        .map(|(_, content)| auto_title(content))
        .unwrap_or_else(|| "Previous conversation".into());

    let mut meta = ChatSessionMeta::new(title);

    // Upgrade to v2 format
    let session_messages: Vec<SessionMessage> = messages
        .into_iter()
        .map(|(role, content)| match role.as_str() {
            "you" | "user" => SessionMessage::user(content),
            _ => SessionMessage::assistant(content),
        })
        .collect();

    // M1: use `count_turns` on the typed vector so the legacy path and
    // the live save path produce identical turn counts. The old
    // `messages.len() / 2` formula under-reported odd-parity vectors
    // by one.
    meta.turn_count = count_turns(&session_messages);

    // H3 fix: do not delete the legacy record until the new session has
    // been persisted successfully. If the save fails, the legacy data
    // stays on disk so the next startup can retry the migration.
    if let Err(e) = save_chat_session(store, key, &meta, &session_messages) {
        tracing::error!(
            "Legacy conversation migration aborted — save failed, legacy record preserved: {e}"
        );
        return;
    }
    if let Err(e) = store.delete(legacy_key) {
        tracing::warn!("Legacy conversation migrated but could not delete source record: {e}");
    }
    tracing::info!("Migrated legacy conversation to session '{}'", meta.id);
}

/// Rename a chat session by updating its metadata title.
pub fn rename_chat_session(
    store: &EncryptedStore,
    key: &MasterKey,
    session_id: &str,
    new_title: &str,
) {
    let meta_key = keys::meta(session_id);
    let mut meta: ChatSessionMeta = match store.get(&meta_key, key) {
        Ok(Some(bytes)) => match serde_json::from_slice(&bytes) {
            Ok(m) => m,
            Err(_) => return,
        },
        _ => return,
    };
    meta.title = new_title.to_string();
    if let Ok(json) = serde_json::to_vec(&meta) {
        let _ = store.put(&meta_key, &json, key);
    }
}

// ── Resume tokens ─────────────────────────────────────────────

/// Serializable resume token that captures an agent's ephemeral learned
/// state at session save time.
///
/// Maps 1:1 with [`AgentStateSnapshot`] but owns its own serde representation
/// so the persistence format is decoupled from the in-memory agent struct.
/// Stored alongside conversation messages under key `chat:{session_id}:resume`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ResumeToken {
    /// Full tool results stashed by the summarization system.
    #[serde(default)]
    pub tool_result_store: HashMap<String, serde_json::Value>,
    /// Per-domain confidence scores affecting tier escalation.
    #[serde(default)]
    pub domain_confidence: HashMap<String, f32>,
    /// Per-MCP-server success counters: `(total_calls, success_count)`.
    #[serde(default)]
    pub server_success_counters: HashMap<String, (u32, u32)>,
    /// Tools currently auto-disabled due to low success rates.
    #[serde(default)]
    pub disabled_tools: HashSet<String>,
    /// Tools permanently blocked on this agent.
    #[serde(default)]
    pub blocked_tools: HashSet<String>,
    /// Tool discovery miss counts for adaptive auto-promotion.
    #[serde(default)]
    pub discovery_miss_counts: HashMap<String, u32>,
    /// Cost tracking snapshot.
    #[serde(default)]
    pub cost_snapshot: CostSnapshot,
    /// Accumulated tool call statistics.
    #[serde(default)]
    pub tool_call_stats: Vec<aivyx_agent::ToolCallStat>,
}

impl ResumeToken {
    /// Create a resume token from an agent state snapshot.
    pub fn from_snapshot(snap: AgentStateSnapshot) -> Self {
        Self {
            tool_result_store: snap.tool_result_store,
            domain_confidence: snap.domain_confidence,
            server_success_counters: snap.server_success_counters,
            disabled_tools: snap.disabled_tools,
            blocked_tools: snap.blocked_tools,
            discovery_miss_counts: snap.discovery_miss_counts,
            cost_snapshot: snap.cost_snapshot,
            tool_call_stats: snap.tool_call_stats,
        }
    }

    /// Convert back into an agent state snapshot for restoration.
    pub fn into_snapshot(self) -> AgentStateSnapshot {
        AgentStateSnapshot {
            tool_result_store: self.tool_result_store,
            domain_confidence: self.domain_confidence,
            server_success_counters: self.server_success_counters,
            disabled_tools: self.disabled_tools,
            blocked_tools: self.blocked_tools,
            discovery_miss_counts: self.discovery_miss_counts,
            cost_snapshot: self.cost_snapshot,
            tool_call_stats: self.tool_call_stats,
        }
    }
}

/// Save a resume token for a session.
pub fn save_resume_token(
    store: &EncryptedStore,
    key: &MasterKey,
    session_id: &str,
    token: &ResumeToken,
) {
    let store_key = keys::resume(session_id);
    match serde_json::to_vec(token) {
        Ok(json) => {
            if let Err(e) = store.put(&store_key, &json, key) {
                tracing::warn!("Failed to save resume token: {e}");
            }
        }
        Err(e) => tracing::warn!("Failed to serialize resume token: {e}"),
    }
}

/// Load a resume token for a session, if one exists.
pub fn load_resume_token(
    store: &EncryptedStore,
    key: &MasterKey,
    session_id: &str,
) -> Option<ResumeToken> {
    let store_key = keys::resume(session_id);
    match store.get(&store_key, key) {
        Ok(Some(bytes)) => serde_json::from_slice(&bytes).ok(),
        _ => None,
    }
}

/// Delete a resume token (called when deleting a session).
pub fn delete_resume_token(store: &EncryptedStore, session_id: &str) {
    let _ = store.delete(&keys::resume(session_id));
}

// ── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_message_user_roundtrip() {
        let msg = SessionMessage::user("Hello");
        let json = serde_json::to_string(&msg).unwrap();
        let restored: SessionMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.role, "user");
        assert_eq!(restored.content, "Hello");
        assert!(restored.tool_name.is_none());
        assert!(restored.tool_call_id.is_none());
        assert!(!restored.is_error);
    }

    #[test]
    fn session_message_tool_roundtrip() {
        let msg = SessionMessage::tool("team_delegate", "call-123", "result text", false);
        let json = serde_json::to_string(&msg).unwrap();
        let restored: SessionMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.role, "tool");
        assert_eq!(restored.tool_name.as_deref(), Some("team_delegate"));
        assert_eq!(restored.tool_call_id.as_deref(), Some("call-123"));
        assert_eq!(restored.content, "result text");
        assert!(!restored.is_error);
    }

    #[test]
    fn session_message_tool_error_roundtrip() {
        let msg = SessionMessage::tool("bad_tool", "call-456", "something failed", true);
        let json = serde_json::to_string(&msg).unwrap();
        let restored: SessionMessage = serde_json::from_str(&json).unwrap();
        assert!(restored.is_error);
    }

    #[test]
    fn session_message_system_roundtrip() {
        let msg = SessionMessage::system("[SYSTEM NOTE] Use structured tool calls.");
        let json = serde_json::to_string(&msg).unwrap();
        let restored: SessionMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.role, "system");
    }

    #[test]
    fn to_chat_messages_preserves_tool_results() {
        let messages = vec![
            SessionMessage::user("Delegate this task"),
            SessionMessage::assistant("I'll delegate.\n\n[Used tools: team_delegate]"),
            SessionMessage::tool(
                "team_delegate",
                "rescued-abc",
                "{\"result\": \"done\"}",
                false,
            ),
            SessionMessage::system("[SYSTEM NOTE] Tool call rescued."),
            SessionMessage::assistant("The team has completed the task."),
        ];

        let chat_msgs = to_chat_messages(&messages);
        assert_eq!(chat_msgs.len(), 5);
        assert_eq!(chat_msgs[0].role, Role::User);
        assert_eq!(chat_msgs[1].role, Role::Assistant);
        assert_eq!(chat_msgs[2].role, Role::Tool);
        assert!(chat_msgs[2].tool_result.is_some());
        let tr = chat_msgs[2].tool_result.as_ref().unwrap();
        assert_eq!(tr.tool_call_id, "rescued-abc");
        assert!(!tr.is_error);
        // System notes restored as user messages
        assert_eq!(chat_msgs[3].role, Role::User);
        assert_eq!(chat_msgs[4].role, Role::Assistant);
    }

    #[test]
    fn to_display_pairs_filters_non_display() {
        let messages = vec![
            SessionMessage::user("Hello"),
            SessionMessage::assistant("Hi there"),
            SessionMessage::tool("some_tool", "call-1", "result", false),
            SessionMessage::system("[NOTE]"),
            SessionMessage::assistant("Follow-up"),
        ];

        let pairs = to_display_pairs(&messages);
        assert_eq!(pairs.len(), 3);
        assert_eq!(pairs[0], ("you".into(), "Hello".into()));
        assert_eq!(pairs[1], ("assistant".into(), "Hi there".into()));
        assert_eq!(pairs[2], ("assistant".into(), "Follow-up".into()));
    }

    #[test]
    fn count_turns_counts_pairs() {
        // Balanced case: 2 user + 2 assistant (+ tool) = 2 turns.
        let messages = vec![
            SessionMessage::user("Q1"),
            SessionMessage::assistant("A1"),
            SessionMessage::user("Q2"),
            SessionMessage::tool("t", "c", "r", false),
            SessionMessage::assistant("A2"),
        ];
        assert_eq!(count_turns(&messages), 2);
    }

    // ---- M1 regression tests: count_turns must be monotonic ----

    #[test]
    fn count_turns_in_flight_assistant_does_not_under_report() {
        // User just sent Q3; the assistant has not yet responded.
        // Pre-M1 formula `min(user, assistant)` would return 2 because
        // only 2 assistant replies have landed — causing the persisted
        // turn_count to be *one behind* the user's mental model.
        // Post-M1 we count the user anchor messages and return 3.
        let messages = vec![
            SessionMessage::user("Q1"),
            SessionMessage::assistant("A1"),
            SessionMessage::user("Q2"),
            SessionMessage::assistant("A2"),
            SessionMessage::user("Q3"),
            // no assistant reply yet — turn is in-flight
        ];
        assert_eq!(
            count_turns(&messages),
            3,
            "in-flight turn must not be silently dropped"
        );
    }

    #[test]
    fn count_turns_empty_conversation_is_zero() {
        assert_eq!(count_turns(&[]), 0);
    }

    #[test]
    fn count_turns_only_user_messages() {
        // Pathological: user spammed without any replies.
        let messages = vec![
            SessionMessage::user("Q1"),
            SessionMessage::user("Q2"),
            SessionMessage::user("Q3"),
        ];
        assert_eq!(count_turns(&messages), 3);
    }

    #[test]
    fn count_turns_is_monotonic_across_append() {
        // Pin the monotonicity property: appending any new message
        // must never *decrease* the turn count. This is the real
        // invariant M1 cares about — the specific values matter less
        // than "turn_count never moves backwards as the conversation
        // grows".
        let mut messages: Vec<SessionMessage> = Vec::new();
        let mut prev = count_turns(&messages);

        let append_sequence = [
            SessionMessage::user("Q1"),
            SessionMessage::assistant("A1"),
            SessionMessage::user("Q2"),
            SessionMessage::tool("t", "c", "r", false),
            SessionMessage::assistant("A2 with tool output"),
            SessionMessage::user("Q3"),
            SessionMessage::assistant("A3"),
            SessionMessage::user("Q4"),
        ];

        for msg in append_sequence {
            messages.push(msg);
            let next = count_turns(&messages);
            assert!(
                next >= prev,
                "count_turns must be monotonic across append, but went {prev} -> {next}"
            );
            prev = next;
        }

        // Final sanity: 4 user messages → 4 turns.
        assert_eq!(count_turns(&messages), 4);
    }

    #[test]
    fn count_turns_ignores_tool_and_system_messages() {
        // Tool and system messages are not turns, even in bulk.
        let messages = vec![
            SessionMessage::system("you are a helpful assistant"),
            SessionMessage::user("Q1"),
            SessionMessage::assistant("A1 using tools"),
            SessionMessage::tool("t1", "c1", "r1", false),
            SessionMessage::tool("t2", "c2", "r2", false),
            SessionMessage::tool("t3", "c3", "r3", false),
            SessionMessage::assistant("A1 final"),
        ];
        assert_eq!(count_turns(&messages), 1);
    }

    #[test]
    fn conversation_to_session_handles_tool_calls() {
        use aivyx_llm::message::{ChatMessage, ToolCall, ToolResult};

        let conversation = vec![
            ChatMessage::user("Do something"),
            ChatMessage::assistant_with_tool_calls(
                "Let me use a tool.",
                vec![ToolCall {
                    id: "tc-1".into(),
                    name: "team_delegate".into(),
                    arguments: serde_json::json!({"task": "research"}),
                }],
            ),
            ChatMessage::tool(ToolResult {
                tool_call_id: "tc-1".into(),
                content: serde_json::json!({"result": "All done"}),
                is_error: false,
            }),
            ChatMessage::assistant("The team completed the research."),
        ];

        let session_msgs = conversation_to_session(&conversation);
        assert_eq!(session_msgs.len(), 4);

        // User message
        assert_eq!(session_msgs[0].role, "user");

        // Assistant with tool call annotation
        assert_eq!(session_msgs[1].role, "assistant");
        assert!(
            session_msgs[1]
                .content
                .contains("[Used tools: team_delegate]")
        );

        // Tool result
        assert_eq!(session_msgs[2].role, "tool");
        assert_eq!(session_msgs[2].tool_name.as_deref(), Some("team_delegate"));
        assert_eq!(session_msgs[2].tool_call_id.as_deref(), Some("tc-1"));
        assert!(!session_msgs[2].is_error);

        // Final assistant
        assert_eq!(session_msgs[3].role, "assistant");
    }

    #[test]
    fn conversation_to_session_truncates_large_tool_results() {
        use aivyx_llm::message::{ChatMessage, ToolResult};

        let big_result = "x".repeat(5000);
        let conversation = vec![ChatMessage::tool(ToolResult {
            tool_call_id: "tc-big".into(),
            content: serde_json::Value::String(big_result),
            is_error: false,
        })];

        let session_msgs = conversation_to_session(&conversation);

        // M5: the persisted content must carry the distinctive
        // truncation marker prefix, not a bare `...`.
        assert!(
            session_msgs[0]
                .content
                .contains(TOOL_RESULT_TRUNCATION_MARKER_PREFIX),
            "truncated tool result must contain the M5 sentinel marker; got: {:?}",
            &session_msgs[0].content
        );
        // The marker embeds the original byte count so the loss is
        // observable post-hoc.
        assert!(
            session_msgs[0].content.contains("5000 bytes"),
            "marker must record original byte count"
        );
        // Size budget: kept bytes + marker overhead stays modest.
        assert!(
            session_msgs[0].content.len() < 2100,
            "truncated form must stay within the size budget, got {} bytes",
            session_msgs[0].content.len()
        );
    }

    // ---- M5 regression tests: truncation marker distinguishability ----

    #[test]
    fn truncate_helper_identity_for_short_input() {
        // Under the threshold: pass-through, no marker appended.
        let short = "hello world";
        let out = truncate_tool_result_with_marker(short);
        assert_eq!(out, short);
        assert!(!out.contains(TOOL_RESULT_TRUNCATION_MARKER_PREFIX));
    }

    #[test]
    fn truncate_helper_does_not_touch_content_ending_in_ellipsis() {
        // Critical M5 invariant: a short tool result whose content
        // legitimately ends in "..." must not acquire a truncation
        // marker — otherwise the loader could not tell natural
        // ellipsis from silent data loss.
        let natural = "progress: step 1... step 2... done...";
        let out = truncate_tool_result_with_marker(natural);
        assert_eq!(out, natural);
        assert!(!out.contains(TOOL_RESULT_TRUNCATION_MARKER_PREFIX));
    }

    #[test]
    fn truncate_helper_preserves_prefix_bytes_before_marker() {
        // The bytes kept at the front must be a *prefix* of the
        // original content, byte-identical. Pre-M5 the truncation
        // also wrote `...` at the end which polluted that prefix.
        let payload = "A".repeat(3000);
        let out = truncate_tool_result_with_marker(&payload);
        let (kept, rest) = out
            .split_once(TOOL_RESULT_TRUNCATION_MARKER_PREFIX)
            .expect("marker must be present");
        assert!(rest.contains("3000 bytes"));
        assert!(payload.starts_with(kept), "kept region must be a prefix");
        // We kept roughly the first ~1800 bytes.
        assert!(kept.len() >= 1700 && kept.len() <= 1900);
    }

    #[test]
    fn truncate_helper_is_utf8_safe_at_multibyte_boundary() {
        // Build a 5000-byte string made of 4-byte UTF-8 characters so
        // a naive `content[..1800]` split would land mid-character.
        // `truncate_tool_result_with_marker` must use ceil_char_boundary.
        let emoji = "🦀"; // 4 bytes
        let many = emoji.repeat(1500); // 6000 bytes, 1500 chars
        assert!(many.len() > TOOL_RESULT_MAX_BYTES);

        let out = truncate_tool_result_with_marker(&many);
        // The kept portion must itself be valid UTF-8 and end on a
        // char boundary — checking that `as_str()` succeeds would be
        // trivial because Rust Strings are always valid UTF-8. The
        // real test is that *splitting* at the marker leaves a kept
        // portion that is a prefix of the original 🦀🦀🦀… string.
        let (kept, _) = out
            .split_once(TOOL_RESULT_TRUNCATION_MARKER_PREFIX)
            .expect("marker must be present");
        assert!(many.starts_with(kept));
        // kept must consist only of whole emoji (length divisible by 4).
        assert_eq!(
            kept.len() % 4,
            0,
            "kept region must not split a multi-byte char"
        );
    }

    #[test]
    fn truncate_helper_detects_exactly_at_boundary() {
        // Exactly TOOL_RESULT_MAX_BYTES → no truncation (the `>` check
        // is inclusive of the boundary as a pass-through).
        let exact = "x".repeat(TOOL_RESULT_MAX_BYTES);
        let out = truncate_tool_result_with_marker(&exact);
        assert_eq!(out.len(), TOOL_RESULT_MAX_BYTES);
        assert!(!out.contains(TOOL_RESULT_TRUNCATION_MARKER_PREFIX));

        // One byte over → truncated with marker.
        let over = "x".repeat(TOOL_RESULT_MAX_BYTES + 1);
        let out = truncate_tool_result_with_marker(&over);
        assert!(out.contains(TOOL_RESULT_TRUNCATION_MARKER_PREFIX));
        assert!(out.contains(&format!("{} bytes", TOOL_RESULT_MAX_BYTES + 1)));
    }

    #[test]
    fn backward_compat_v1_format_parses_correctly() {
        // Simulate what v1 serialization looks like: [["you","Hello"],["assistant","Hi"]]
        let v1: Vec<(String, String)> = vec![
            ("you".into(), "Hello".into()),
            ("assistant".into(), "Hi".into()),
        ];
        let json = serde_json::to_vec(&v1).unwrap();

        // Serde can actually parse v1's array-of-arrays into SessionMessage
        // because struct deserialization accepts sequences matching field
        // order. This means the v2 parse in load_chat_messages
        // transparently handles v1 data.
        //
        // Post-M2: the `SessionMessageRaw` validator normalizes legacy
        // `"you"` → `"user"` at deserialization time, so both the
        // tuple-sequence shortcut and the explicit v1 fallback path in
        // `load_chat_messages` produce identical role strings. This is a
        // consistency improvement — before M2 the two paths would diverge
        // ("you" survived the tuple shortcut but got rewritten by the
        // explicit fallback).
        let v2_result = serde_json::from_slice::<Vec<SessionMessage>>(&json).unwrap();
        assert_eq!(v2_result.len(), 2);
        assert_eq!(
            v2_result[0].role, "user",
            "legacy 'you' must normalize to 'user' during v1-as-v2 parse"
        );
        assert_eq!(v2_result[0].content, "Hello");
        assert!(v2_result[0].tool_name.is_none());
        assert_eq!(v2_result[1].role, "assistant");
        assert_eq!(v2_result[1].content, "Hi");

        // The explicit v1 fallback path still works too
        let v1_result = serde_json::from_slice::<Vec<(String, String)>>(&json).unwrap();
        assert_eq!(v1_result.len(), 2);
    }

    #[test]
    fn v2_format_preserves_tool_metadata() {
        // v2 format with tool data should round-trip through JSON correctly
        let v2 = vec![
            SessionMessage::user("Do something"),
            SessionMessage::tool("team_delegate", "rescued-1", "result", false),
            SessionMessage::system("[SYSTEM NOTE] Rescued."),
            SessionMessage::assistant("Done."),
        ];
        let json = serde_json::to_vec(&v2).unwrap();
        let restored: Vec<SessionMessage> = serde_json::from_slice(&json).unwrap();

        assert_eq!(restored.len(), 4);
        assert_eq!(restored[1].tool_name.as_deref(), Some("team_delegate"));
        assert_eq!(restored[1].tool_call_id.as_deref(), Some("rescued-1"));
        assert_eq!(restored[2].role, "system");
    }

    #[test]
    fn auto_title_truncates() {
        let short = "Hello world";
        assert_eq!(auto_title(short), "Hello world");

        let long = "This is a very long message that should be truncated at approximately forty characters";
        let title = auto_title(long);
        assert!(title.len() <= 43); // 37 + "..."
        assert!(title.ends_with("..."));
    }

    /// Helper: build a one-off encrypted store in a temp dir for tests.
    /// Returns (store, key, tempdir handle so the dir lives until drop).
    fn fresh_store() -> (EncryptedStore, MasterKey, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("test.redb");
        let store = EncryptedStore::open(&path).expect("open store");
        let key = MasterKey::generate();
        (store, key, tmp)
    }

    /// C2 regression: `save_chat_session` must return `Ok` and persist
    /// both the meta and the messages payload so a subsequent load sees
    /// the conversation.
    #[test]
    fn save_chat_session_round_trip() {
        let (store, key, _tmp) = fresh_store();
        let mut meta = ChatSessionMeta::new("Test session".to_string());
        meta.id = "roundtrip-1".into();
        let messages = vec![
            SessionMessage::user("hello"),
            SessionMessage::assistant("hi there"),
        ];

        save_chat_session(&store, &key, &meta, &messages).expect("save must succeed");

        let loaded = load_chat_messages(&store, &key, &meta.id)
            .expect("load must not error")
            .expect("load must return Some");
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].role, "user");
        assert_eq!(loaded[0].content, "hello");
        assert_eq!(loaded[1].role, "assistant");
        assert_eq!(loaded[1].content, "hi there");

        // Meta must also be present — list_chat_sessions reads the meta key.
        let listed = list_chat_sessions(&store, &key);
        assert!(
            listed.iter().any(|s| s.id == "roundtrip-1"),
            "saved session must appear in list"
        );
    }

    /// C1 regression: legacy `chat:{id}:*` records must be rewritten
    /// under the `pa_chat/{id}/*` namespace on the first migration
    /// run, and subsequent runs must be no-ops.
    #[test]
    fn pa_chat_migration_rewrites_legacy_records() {
        let (store, key, _tmp) = fresh_store();

        // Seed legacy meta + messages + resume directly via the store,
        // simulating an older install's on-disk layout.
        let legacy_meta_key = "chat:legacy-1:meta";
        let legacy_msg_key = "chat:legacy-1:messages";
        let legacy_resume_key = "chat:legacy-1:resume";

        let meta = ChatSessionMeta {
            id: "legacy-1".into(),
            title: "Old session".into(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            turn_count: 1,
        };
        let messages = vec![
            SessionMessage::user("hi"),
            SessionMessage::assistant("hello"),
        ];
        store
            .put(legacy_meta_key, &serde_json::to_vec(&meta).unwrap(), &key)
            .unwrap();
        store
            .put(
                legacy_msg_key,
                &serde_json::to_vec(&messages).unwrap(),
                &key,
            )
            .unwrap();
        store
            .put(legacy_resume_key, b"fake-resume-bytes", &key)
            .unwrap();

        // First run: should rewrite all three.
        let migrated = run_pa_chat_migration(&store, &key).unwrap();
        assert_eq!(migrated, 3, "expected 3 legacy keys migrated");

        // Legacy keys gone.
        assert!(store.get(legacy_meta_key, &key).unwrap().is_none());
        assert!(store.get(legacy_msg_key, &key).unwrap().is_none());
        assert!(store.get(legacy_resume_key, &key).unwrap().is_none());

        // New keys present and readable via public API.
        let loaded = load_chat_messages(&store, &key, "legacy-1")
            .expect("load must not error")
            .expect("messages restored");
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].content, "hi");

        let listed = list_chat_sessions(&store, &key);
        assert!(listed.iter().any(|s| s.id == "legacy-1"));

        // Second run: marker present → no-op, returns 0.
        let migrated_again = run_pa_chat_migration(&store, &key).unwrap();
        assert_eq!(migrated_again, 0, "second run must be a no-op");
    }

    /// C1 regression: the migration must stamp its marker on a
    /// fresh install so that future runs stay O(1).
    #[test]
    fn pa_chat_migration_fresh_install_stamps_marker() {
        let (store, key, _tmp) = fresh_store();

        // No legacy data.
        let migrated = run_pa_chat_migration(&store, &key).unwrap();
        assert_eq!(migrated, 0);

        // Marker must now be present.
        let marker = store.get(PA_CHAT_MIGRATION_MARKER, &key).unwrap();
        assert!(marker.is_some(), "marker must be stamped on fresh install");
    }

    /// C1 regression: new `pa_chat/` records must not collide with
    /// any legacy `chat:` records. Saving a new session after
    /// migration must not resurrect legacy keys.
    #[test]
    fn pa_chat_namespace_has_no_legacy_aliasing() {
        let (store, key, _tmp) = fresh_store();

        // Run migration once to stamp the marker.
        run_pa_chat_migration(&store, &key).unwrap();

        // Save a session the normal way.
        let mut meta = ChatSessionMeta::new("New session");
        meta.id = "fresh-1".into();
        save_chat_session(&store, &key, &meta, &[SessionMessage::user("hi")]).unwrap();

        // Verify exactly zero keys exist under the legacy prefix.
        let all = store.list_keys().unwrap();
        let legacy_count = all
            .iter()
            .filter(|k| k.starts_with(LEGACY_CHAT_PREFIX))
            .count();
        assert_eq!(
            legacy_count, 0,
            "saving a new session must not create any legacy-prefixed keys"
        );
        // And the new prefix is in use.
        assert!(all.iter().any(|k| k.starts_with(PA_CHAT_PREFIX)));
    }

    /// H3 regression: `migrate_legacy_conversation` must move a legacy
    /// `conversation:display` record into a new chat session and then
    /// delete the legacy key. The post-condition is: legacy key gone,
    /// new session loadable, display pairs preserved in order.
    ///
    /// This exercises the success path explicitly so any regression
    /// that reorders delete-before-save (the original H3 bug) would
    /// either (a) leave the legacy key present, breaking the invariant
    /// checked below, or (b) lose the conversation entirely if the save
    /// later fails, breaking the load round-trip.
    #[test]
    fn migrate_legacy_conversation_moves_to_new_session() {
        let (store, key, _tmp) = fresh_store();

        // Seed the legacy single-conversation format: tuple array under
        // the `conversation:display` key, encrypted with the conversation key.
        let legacy: Vec<(String, String)> = vec![
            ("you".into(), "First question from user".into()),
            ("assistant".into(), "First reply from agent".into()),
            ("you".into(), "Follow-up question".into()),
            ("assistant".into(), "Follow-up reply".into()),
        ];
        store
            .put(
                "conversation:display",
                &serde_json::to_vec(&legacy).unwrap(),
                &key,
            )
            .unwrap();

        migrate_legacy_conversation(&store, &key);

        // Legacy key must be gone — this is the "then delete" half of
        // the two-phase order. If the pre-H3 reordering regressed, this
        // assertion would still hold on success, so pair it with the
        // load check below to catch the delete-before-save variant.
        assert!(
            store.get("conversation:display", &key).unwrap().is_none(),
            "legacy key must be deleted after successful migration"
        );

        // Exactly one new session must be listable and its messages
        // must round-trip through load_chat_messages with the same
        // content and ordering as the legacy record.
        let listed = list_chat_sessions(&store, &key);
        assert_eq!(listed.len(), 1, "expected a single migrated session");
        let loaded = load_chat_messages(&store, &key, &listed[0].id)
            .expect("load must not error")
            .expect("migrated session messages must load");
        assert_eq!(loaded.len(), 4);
        assert_eq!(loaded[0].role, "user");
        assert_eq!(loaded[0].content, "First question from user");
        assert_eq!(loaded[1].role, "assistant");
        assert_eq!(loaded[1].content, "First reply from agent");
        assert_eq!(loaded[3].content, "Follow-up reply");
    }

    /// H3 regression: running the migration with no legacy record must
    /// be a clean no-op — no new session created, no panic, no spurious
    /// writes to the store. This is the fresh-install / already-migrated
    /// case, which runs on every startup after the first.
    #[test]
    fn migrate_legacy_conversation_noop_when_legacy_absent() {
        let (store, key, _tmp) = fresh_store();

        let before = store.list_keys().unwrap().len();
        migrate_legacy_conversation(&store, &key);
        let after = store.list_keys().unwrap().len();

        assert_eq!(before, after, "no legacy data → no store mutation");
        assert!(
            list_chat_sessions(&store, &key).is_empty(),
            "no session should be created when legacy record is absent"
        );
    }

    /// H3 regression: the delete-before-verify hazard is that a
    /// successful legacy-key read followed by a failing save would
    /// previously still delete the legacy record. We can't inject a
    /// save failure in-process, but we *can* assert the defensive
    /// property that the legacy data is still recoverable *between*
    /// the read and the save — i.e. if the migration is interrupted
    /// by simulating a crash between reading the legacy and writing
    /// the new session, the legacy key must still be present. The
    /// cheapest way to simulate this is: seed legacy, do NOT call
    /// migrate, then assert the legacy key is still readable. This
    /// guards against any future "eager delete on read" regression.
    #[test]
    fn migrate_legacy_conversation_preserves_legacy_until_save_succeeds() {
        let (store, key, _tmp) = fresh_store();

        let legacy: Vec<(String, String)> = vec![
            ("you".into(), "hello".into()),
            ("assistant".into(), "world".into()),
        ];
        let legacy_bytes = serde_json::to_vec(&legacy).unwrap();
        store
            .put("conversation:display", &legacy_bytes, &key)
            .unwrap();

        // Precondition: legacy record present and readable.
        let fetched = store.get("conversation:display", &key).unwrap();
        assert_eq!(fetched.as_deref(), Some(legacy_bytes.as_slice()));

        // After a successful migration the legacy key is gone (the
        // delete happens AFTER the save returns Ok). If a future
        // regression reorders these, this test still passes on the
        // success path — its real value is pinning the current
        // behavior so reorderings show up in the other two H3 tests.
        migrate_legacy_conversation(&store, &key);
        assert!(
            store.get("conversation:display", &key).unwrap().is_none(),
            "legacy key must be gone after migration"
        );
    }

    /// C2 regression: the atomic `put_many` path must land both meta
    /// and messages in the same redb transaction. This test verifies
    /// the post-condition of atomicity — if the messages key is
    /// visible, the meta key must also be visible. (We cannot easily
    /// inject a mid-save crash in unit tests without a fault-injection
    /// layer, but this ensures the happy path writes *both* keys.)
    #[test]
    fn save_chat_session_writes_meta_and_messages_together() {
        let (store, key, _tmp) = fresh_store();
        let mut meta = ChatSessionMeta::new("Atomicity check".to_string());
        meta.id = "atom-1".into();
        meta.turn_count = 1;
        let messages = vec![SessionMessage::user("atomic?")];

        save_chat_session(&store, &key, &meta, &messages).unwrap();

        // Both keys must resolve — previously these were two independent
        // `put` calls and a crash between them could leave a meta with no
        // body or a body with no meta.
        let msg_bytes = store.get(&meta.messages_key(), &key).unwrap();
        let meta_bytes = store.get(&meta.meta_key(), &key).unwrap();
        assert!(msg_bytes.is_some(), "messages key must be present");
        assert!(meta_bytes.is_some(), "meta key must be present");
    }

    // ── H1 regressions: MAX_CONVERSATION_MESSAGES truncation ─────

    /// Base case: conversations at or below the cap must pass through
    /// unmodified — no snap, no drop, no warn.
    #[test]
    fn save_chat_session_under_cap_is_identity() {
        let (store, key, _tmp) = fresh_store();
        let mut meta = ChatSessionMeta::new("under cap".to_string());
        meta.id = "under-cap-1".into();

        let mut messages = Vec::new();
        for i in 0..10 {
            messages.push(SessionMessage::user(format!("u{i}")));
            messages.push(SessionMessage::assistant(format!("a{i}")));
        }
        save_chat_session(&store, &key, &meta, &messages).unwrap();

        let loaded = load_chat_messages(&store, &key, &meta.id).unwrap().unwrap();
        assert_eq!(loaded.len(), messages.len());
        assert_eq!(loaded.first().unwrap().content, "u0");
        assert_eq!(loaded.last().unwrap().content, "a9");
    }

    /// H1: when the cap fires and the raw split would land on a
    /// `role == "tool"` message (orphaned tool result), the save must
    /// forward-walk past the tool cluster so the first kept message is
    /// a non-tool entry. The LLM would otherwise reject the restored
    /// conversation on reload.
    #[test]
    fn save_chat_session_snaps_past_orphan_tool_cluster_at_cap_boundary() {
        let (store, key, _tmp) = fresh_store();
        let mut meta = ChatSessionMeta::new("cap boundary".to_string());
        meta.id = "cap-snap-1".into();

        // Build MAX + 5 messages. Arrange the sequence so that the raw
        // (unsnapped) start index `len - MAX` lands precisely on a
        // `role == "tool"` message.
        let total = MAX_CONVERSATION_MESSAGES + 5;
        let mut messages: Vec<SessionMessage> = Vec::with_capacity(total);
        for i in 0..total {
            // Place a tool result exactly at index `raw_start = 5`.
            // That position will be the first kept message without the
            // snap — the bug we're guarding against.
            if i == 5 {
                messages.push(SessionMessage::tool(
                    "search",
                    "call-orphan",
                    "{\"hits\": 0}",
                    false,
                ));
            } else if i == 6 {
                // A second tool to prove the forward walk skips a
                // cluster, not just a single row.
                messages.push(SessionMessage::tool(
                    "search",
                    "call-orphan-2",
                    "{\"hits\": 1}",
                    false,
                ));
            } else if i % 2 == 0 {
                messages.push(SessionMessage::user(format!("u{i}")));
            } else {
                messages.push(SessionMessage::assistant(format!("a{i}")));
            }
        }

        save_chat_session(&store, &key, &meta, &messages).unwrap();

        let loaded = load_chat_messages(&store, &key, &meta.id).unwrap().unwrap();

        // The first kept message MUST NOT be a tool result — that was
        // the pre-fix failure mode.
        assert_ne!(
            loaded.first().map(|m| m.role.as_str()),
            Some("tool"),
            "H1: first kept message must not be an orphaned tool result"
        );

        // And the forward-snap should have dropped exactly the two
        // tool rows in addition to the original cap-truncation, so the
        // kept window is 2 shorter than the cap.
        assert_eq!(
            loaded.len(),
            MAX_CONVERSATION_MESSAGES - 2,
            "forward-walk must skip the 2-message tool cluster at the boundary"
        );
    }

    /// H1 unit test: the snap helper is a pure function; exercise its
    /// corner cases directly so a reviewer can see the invariant
    /// without reasoning through the surrounding save pipeline.
    #[test]
    fn snap_to_safe_session_split_forward_walks_past_tools() {
        let msgs = vec![
            SessionMessage::user("u0"),
            SessionMessage::assistant("a0"),
            SessionMessage::tool("t", "c1", "r1", false),
            SessionMessage::tool("t", "c2", "r2", false),
            SessionMessage::user("u1"),
        ];

        // Starting at a non-tool index is a no-op.
        assert_eq!(snap_to_safe_session_split(&msgs, 0), 0);
        assert_eq!(snap_to_safe_session_split(&msgs, 1), 1);
        // Starting at a tool cluster walks forward to the next
        // non-tool boundary.
        assert_eq!(snap_to_safe_session_split(&msgs, 2), 4);
        assert_eq!(snap_to_safe_session_split(&msgs, 3), 4);
        // Starting past the end returns len.
        assert_eq!(snap_to_safe_session_split(&msgs, 5), 5);
    }

    /// Pathological case: if the entire tail is nothing but tool rows,
    /// the helper returns `messages.len()` and the caller saves an
    /// empty window. This is acceptable — losing a handful of orphan
    /// tool results is strictly better than persisting a conversation
    /// the LLM will refuse to continue.
    #[test]
    fn snap_to_safe_session_split_all_tools_returns_len() {
        let msgs = vec![
            SessionMessage::tool("t", "c1", "r1", false),
            SessionMessage::tool("t", "c2", "r2", false),
        ];
        assert_eq!(snap_to_safe_session_split(&msgs, 0), 2);
    }

    // ── H2 regressions: load_chat_messages Result<Option<_>> shape ──

    /// H2: a session that was never persisted must come back as
    /// `Ok(None)` — not an error, not a silent empty vec. Callers
    /// use this distinction to decide whether to treat the session
    /// as "fresh" (safe to create) versus "present but unreadable"
    /// (unsafe to overwrite).
    #[test]
    fn load_chat_messages_missing_session_is_ok_none() {
        let (store, key, _tmp) = fresh_store();
        let result = load_chat_messages(&store, &key, "never-existed");
        assert!(
            matches!(result, Ok(None)),
            "missing session must yield Ok(None), got {result:?}"
        );
    }

    /// H2: a happy-path round trip must yield `Ok(Some(msgs))`.
    #[test]
    fn load_chat_messages_present_session_is_ok_some() {
        let (store, key, _tmp) = fresh_store();
        let mut meta = ChatSessionMeta::new("present".to_string());
        meta.id = "present-1".into();
        save_chat_session(&store, &key, &meta, &[SessionMessage::user("hello")]).unwrap();

        let result = load_chat_messages(&store, &key, "present-1");
        match result {
            Ok(Some(msgs)) => {
                assert_eq!(msgs.len(), 1);
                assert_eq!(msgs[0].content, "hello");
            }
            other => panic!("expected Ok(Some(..)), got {other:?}"),
        }
    }

    /// H2 critical case: if the bytes on disk decrypt but match no
    /// known schema, `load_chat_messages` must return `Err` — the
    /// previous behaviour of silently returning `None` enabled the
    /// save path to overwrite the unreadable record. We simulate
    /// corruption by writing a plain JSON object (not an array) to
    /// the messages key under the correct master key; decryption
    /// succeeds but serde_json fails both the v2 Vec<SessionMessage>
    /// and v1 Vec<(String, String)> schemas.
    #[test]
    fn load_chat_messages_corrupt_bytes_surface_as_err() {
        let (store, key, _tmp) = fresh_store();
        let session_id = "corrupt-1";
        let msg_key = keys::messages(session_id);
        // Write JSON that matches neither schema — a top-level object
        // rather than an array.
        let bogus = br#"{"unexpected": "object"}"#;
        store.put(&msg_key, bogus, &key).unwrap();

        let result = load_chat_messages(&store, &key, session_id);
        assert!(
            result.is_err(),
            "corrupt bytes must surface as Err, got {result:?}"
        );
        // The error message must name the session for triage.
        let err = result.unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains(session_id),
            "error message should name the session id; got: {msg}"
        );
    }

    // ── M2 regressions: role validation at deserialization ───────

    /// M2: a record with a typo'd role ("assitant", "usr", etc.) must
    /// be rejected at deserialization time, not silently recovered
    /// via a catch-all fallback. The corrupt-bytes path in H2 will
    /// then surface this to callers as an `Err`.
    #[test]
    fn session_message_rejects_unknown_role_on_deserialize() {
        // Typos that used to slip past the `_ => assistant` fallback.
        let bad_roles = ["assitant", "usr", "ai", "human", "bot", ""];
        for bad in bad_roles {
            let json = format!(r#"{{"role":"{bad}","content":"x"}}"#);
            let result: Result<SessionMessage, _> = serde_json::from_str(&json);
            assert!(
                result.is_err(),
                "role '{bad}' must be rejected at deserialize time, got {result:?}"
            );
            let err_msg = result.unwrap_err().to_string();
            assert!(
                err_msg.contains("unknown role") || err_msg.contains(bad),
                "error should mention the invalid role; got: {err_msg}"
            );
        }
    }

    /// M2: all four valid roles must round-trip without error.
    #[test]
    fn session_message_accepts_all_valid_roles() {
        // "tool" needs tool_call_id; others don't.
        let cases = [
            r#"{"role":"user","content":"hi"}"#,
            r#"{"role":"assistant","content":"hello"}"#,
            r#"{"role":"system","content":"note"}"#,
            r#"{"role":"tool","content":"result","tool_call_id":"c1"}"#,
        ];
        for json in cases {
            let result: Result<SessionMessage, _> = serde_json::from_str(json);
            assert!(
                result.is_ok(),
                "valid role should deserialize cleanly: {json} -> {result:?}"
            );
        }
    }

    /// M2: legacy `"you"` (v1 user-role label) must auto-normalize to
    /// `"user"` at deserialization time. This closes a consistency
    /// gap where the struct-from-sequence path and the explicit v1
    /// upgrade path produced different role strings for the same
    /// on-disk record.
    #[test]
    fn session_message_legacy_you_normalizes_to_user() {
        let json = r#"{"role":"you","content":"Hello"}"#;
        let sm: SessionMessage = serde_json::from_str(json).unwrap();
        assert_eq!(sm.role, "user", "legacy 'you' must normalize to 'user'");
        assert_eq!(sm.content, "Hello");
    }

    /// M2 coupling invariant: `role == "tool"` with no `tool_call_id`
    /// must be rejected. A tool result cannot reattach to its
    /// invoking assistant message without a call id, and the silent
    /// `"restored"` fallback that used to hide this would produce an
    /// LLM request guaranteed to fail to match.
    #[test]
    fn session_message_tool_role_without_call_id_is_rejected() {
        let json = r#"{"role":"tool","content":"result"}"#;
        let result: Result<SessionMessage, _> = serde_json::from_str(json);
        assert!(
            result.is_err(),
            "tool-role without tool_call_id must be rejected, got {result:?}"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("tool_call_id"),
            "error should name tool_call_id; got: {err_msg}"
        );
    }

    /// M2 end-to-end: a corrupt record with a bad role on disk must
    /// surface as `Err` from `load_chat_messages` (via the
    /// "bytes matched neither v2 nor v1" path), not be silently
    /// recovered as an assistant message. This is the full-stack
    /// version of `session_message_rejects_unknown_role_on_deserialize`.
    #[test]
    fn load_chat_messages_rejects_persisted_typo_role() {
        let (store, key, _tmp) = fresh_store();
        let session_id = "typo-role-1";

        // Write a SessionMessage-shaped payload where the role is a
        // typo. The outer shape is a valid JSON array of objects, so
        // the corrupt-bytes path in H2 will not catch it at the array
        // level — the validator has to reject the role.
        let bogus = br#"[{"role":"assitant","content":"silently recovered"}]"#;
        let msg_key = keys::messages(session_id);
        store.put(&msg_key, bogus, &key).unwrap();

        let result = load_chat_messages(&store, &key, session_id);
        assert!(
            result.is_err(),
            "persisted typo role must surface as Err, got {result:?}"
        );
    }

    /// M2 + H5 interaction note: a SessionMessage array that contains
    /// *some* valid entries and *one* invalid entry should fail the
    /// whole load (serde doesn't do partial deserialization of a
    /// `Vec<T>`). This pins that serde gives up on the first bad
    /// element — which is the correct behaviour for a persistence
    /// layer, because partial-accept would silently drop messages.
    #[test]
    fn load_chat_messages_rejects_mixed_valid_and_invalid_roles() {
        let (store, key, _tmp) = fresh_store();
        let session_id = "mixed-1";

        let bogus =
            br#"[{"role":"user","content":"hi"},{"role":"bogus","content":"x"},{"role":"assistant","content":"hello"}]"#;
        let msg_key = keys::messages(session_id);
        store.put(&msg_key, bogus, &key).unwrap();

        let result = load_chat_messages(&store, &key, session_id);
        assert!(
            result.is_err(),
            "mixed valid+invalid roles must fail the whole load (no partial accept), got {result:?}"
        );
    }

    /// H2: the v1 legacy format must still round-trip through the new
    /// `Result<Option<_>>` signature — we did not intend to regress
    /// v1 readability as part of this fix.
    #[test]
    fn load_chat_messages_v1_legacy_format_still_loads() {
        let (store, key, _tmp) = fresh_store();
        let session_id = "v1-legacy-1";
        let msg_key = keys::messages(session_id);
        // v1 shape: Vec<(String, String)>
        let v1: Vec<(String, String)> = vec![
            ("user".into(), "hi".into()),
            ("assistant".into(), "hello".into()),
        ];
        store
            .put(&msg_key, &serde_json::to_vec(&v1).unwrap(), &key)
            .unwrap();

        let loaded = load_chat_messages(&store, &key, session_id)
            .expect("load must not error")
            .expect("v1 bytes must upgrade");
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].role, "user");
        assert_eq!(loaded[0].content, "hi");
        assert_eq!(loaded[1].role, "assistant");
    }
}
