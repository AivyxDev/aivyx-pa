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
use aivyx_crypto::{EncryptedStore, MasterKey};
use aivyx_llm::message::{ChatMessage, Role, ToolResult};

/// Maximum number of chat messages to persist per session.
const MAX_CONVERSATION_MESSAGES: usize = 500;

// ── Session message (v2 format) ────────────────────────────────

/// A single persisted message in a chat session.
///
/// Richer than the legacy `(role, content)` tuple: preserves tool
/// call names, tool results, and system notes so that restored
/// conversations give the LLM full context about past tool interactions.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionMessage {
    /// Message role: `"user"`, `"assistant"`, `"tool"`, or `"system"`.
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
        format!("chat:{}:meta", self.id)
    }
    fn messages_key(&self) -> String {
        format!("chat:{}:messages", self.id)
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
        .filter(|k| k.starts_with("chat:") && k.ends_with(":meta"))
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

/// Save a chat session (messages + metadata).
///
/// Accepts `&[SessionMessage]` (v2 format). Use [`conversation_to_session`]
/// to convert from the agent's `ChatMessage` conversation history.
pub fn save_chat_session(
    store: &EncryptedStore,
    key: &MasterKey,
    meta: &ChatSessionMeta,
    messages: &[SessionMessage],
) {
    let to_save = if messages.len() > MAX_CONVERSATION_MESSAGES {
        &messages[messages.len() - MAX_CONVERSATION_MESSAGES..]
    } else {
        messages
    };

    if let Ok(json) = serde_json::to_vec(to_save)
        && let Err(e) = store.put(&meta.messages_key(), &json, key)
    {
        tracing::warn!("Failed to save chat messages: {e}");
    }
    if let Ok(json) = serde_json::to_vec(meta)
        && let Err(e) = store.put(&meta.meta_key(), &json, key)
    {
        tracing::warn!("Failed to save chat metadata: {e}");
    }
}

/// Load messages for a specific chat session.
///
/// Auto-detects storage format:
/// - **v2** (current): `Vec<SessionMessage>` — used directly.
/// - **v1** (legacy): `Vec<(String, String)>` — upgraded to `SessionMessage`.
pub fn load_chat_messages(
    store: &EncryptedStore,
    key: &MasterKey,
    session_id: &str,
) -> Option<Vec<SessionMessage>> {
    let msg_key = format!("chat:{session_id}:messages");
    let bytes = match store.get(&msg_key, key) {
        Ok(Some(b)) => b,
        _ => return None,
    };

    // Try v2 format first (Vec<SessionMessage>)
    if let Ok(messages) = serde_json::from_slice::<Vec<SessionMessage>>(&bytes) {
        return Some(messages);
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
        return Some(upgraded);
    }

    None
}

/// Delete a chat session (messages + metadata + resume token).
pub fn delete_chat_session(store: &EncryptedStore, session_id: &str) {
    let _ = store.delete(&format!("chat:{session_id}:meta"));
    let _ = store.delete(&format!("chat:{session_id}:messages"));
    let _ = store.delete(&format!("chat:{session_id}:resume"));
}

// ── Conversion helpers ─────────────────────────────────────────

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
                    let truncated = if content_str.len() > 2000 {
                        let boundary = content_str.ceil_char_boundary(1997);
                        format!("{}...", &content_str[..boundary])
                    } else {
                        content_str
                    };

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
            match sm.role.as_str() {
                "you" | "user" => ChatMessage::user(&sm.content),
                "tool" => {
                    let call_id = sm.tool_call_id.as_deref().unwrap_or("restored");
                    ChatMessage::tool(ToolResult {
                        tool_call_id: call_id.to_string(),
                        content: serde_json::Value::String(sm.content.clone()),
                        is_error: sm.is_error,
                    })
                }
                "system" => {
                    // Most providers don't support mid-conversation system messages.
                    // Restore as user messages (same as how rescue guidance is injected).
                    ChatMessage::user(&sm.content)
                }
                _ => ChatMessage::assistant(&sm.content),
            }
        })
        .collect()
}

/// Convert `SessionMessage` values to legacy display pairs for the TUI.
///
/// Filters out tool and system messages, returning only user/assistant
/// content suitable for the chat view.
pub fn to_display_pairs(messages: &[SessionMessage]) -> Vec<(String, String)> {
    messages
        .iter()
        .filter_map(|sm| match sm.role.as_str() {
            "you" | "user" => Some(("you".into(), sm.content.clone())),
            "assistant" => Some(("assistant".into(), sm.content.clone())),
            _ => None,
        })
        .collect()
}

/// Count user/assistant turn pairs (for metadata).
pub fn count_turns(messages: &[SessionMessage]) -> usize {
    let user_count = messages
        .iter()
        .filter(|m| m.role == "user" || m.role == "you")
        .count();
    let assistant_count = messages.iter().filter(|m| m.role == "assistant").count();
    user_count.min(assistant_count)
}

// ── Migration ──────────────────────────────────────────────────

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
    meta.turn_count = messages.len() / 2;

    // Upgrade to v2 format
    let session_messages: Vec<SessionMessage> = messages
        .into_iter()
        .map(|(role, content)| match role.as_str() {
            "you" | "user" => SessionMessage::user(content),
            _ => SessionMessage::assistant(content),
        })
        .collect();

    save_chat_session(store, key, &meta, &session_messages);
    let _ = store.delete(legacy_key);
    tracing::info!("Migrated legacy conversation to session '{}'", meta.id);
}

/// Rename a chat session by updating its metadata title.
pub fn rename_chat_session(
    store: &EncryptedStore,
    key: &MasterKey,
    session_id: &str,
    new_title: &str,
) {
    let meta_key = format!("chat:{session_id}:meta");
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
    let store_key = format!("chat:{session_id}:resume");
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
    let store_key = format!("chat:{session_id}:resume");
    match store.get(&store_key, key) {
        Ok(Some(bytes)) => serde_json::from_slice(&bytes).ok(),
        _ => None,
    }
}

/// Delete a resume token (called when deleting a session).
pub fn delete_resume_token(store: &EncryptedStore, session_id: &str) {
    let _ = store.delete(&format!("chat:{session_id}:resume"));
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
        let messages = vec![
            SessionMessage::user("Q1"),
            SessionMessage::assistant("A1"),
            SessionMessage::user("Q2"),
            SessionMessage::tool("t", "c", "r", false),
            SessionMessage::assistant("A2"),
        ];
        assert_eq!(count_turns(&messages), 2);
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
        assert!(session_msgs[0].content.len() <= 2010); // 1997 + "..."
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
        // because struct deserialization accepts sequences matching field order.
        // This means the v2 parse in load_chat_messages transparently handles v1 data.
        let v2_result = serde_json::from_slice::<Vec<SessionMessage>>(&json).unwrap();
        assert_eq!(v2_result.len(), 2);
        assert_eq!(v2_result[0].role, "you");
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
}
