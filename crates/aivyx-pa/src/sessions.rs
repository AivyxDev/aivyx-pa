//! Chat session persistence.
//!
//! Manages multi-session conversations stored in the encrypted store.
//! Each session has metadata (title, timestamps, turn count) and a
//! message history capped at 500 messages.

use aivyx_crypto::{EncryptedStore, MasterKey};

/// Maximum number of chat messages to persist per session.
const MAX_CONVERSATION_MESSAGES: usize = 500;

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

    fn meta_key(&self) -> String { format!("chat:{}:meta", self.id) }
    fn messages_key(&self) -> String { format!("chat:{}:messages", self.id) }
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

/// List all saved chat sessions, sorted by most recent first.
pub fn list_chat_sessions(store: &EncryptedStore, key: &MasterKey) -> Vec<ChatSessionMeta> {
    let keys = match store.list_keys() {
        Ok(k) => k,
        Err(_) => return Vec::new(),
    };
    let mut sessions: Vec<ChatSessionMeta> = keys.iter()
        .filter(|k| k.starts_with("chat:") && k.ends_with(":meta"))
        .filter_map(|k| {
            store.get(k, key).ok().flatten()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        })
        .collect();
    sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    sessions
}

/// Save a chat session (messages + metadata).
pub fn save_chat_session(
    store: &EncryptedStore,
    key: &MasterKey,
    meta: &ChatSessionMeta,
    messages: &[(String, String)],
) {
    let to_save = if messages.len() > MAX_CONVERSATION_MESSAGES {
        &messages[messages.len() - MAX_CONVERSATION_MESSAGES..]
    } else {
        messages
    };

    if let Ok(json) = serde_json::to_vec(to_save)
        && let Err(e) = store.put(&meta.messages_key(), &json, key) {
            tracing::warn!("Failed to save chat messages: {e}");
        }
    if let Ok(json) = serde_json::to_vec(meta)
        && let Err(e) = store.put(&meta.meta_key(), &json, key) {
            tracing::warn!("Failed to save chat metadata: {e}");
        }
}

/// Load messages for a specific chat session.
pub fn load_chat_messages(
    store: &EncryptedStore,
    key: &MasterKey,
    session_id: &str,
) -> Option<Vec<(String, String)>> {
    let msg_key = format!("chat:{session_id}:messages");
    match store.get(&msg_key, key) {
        Ok(Some(bytes)) => serde_json::from_slice(&bytes).ok(),
        _ => None,
    }
}

/// Delete a chat session (messages + metadata).
pub fn delete_chat_session(store: &EncryptedStore, session_id: &str) {
    let _ = store.delete(&format!("chat:{session_id}:meta"));
    let _ = store.delete(&format!("chat:{session_id}:messages"));
}

/// Migrate legacy single-conversation format to the new multi-session format.
/// If `conversation:display` exists, move it into a new chat session.
pub fn migrate_legacy_conversation(store: &EncryptedStore, key: &MasterKey) {
    let legacy_key = "conversation:display";
    let messages = match store.get(legacy_key, key) {
        Ok(Some(bytes)) => {
            match serde_json::from_slice::<Vec<(String, String)>>(&bytes) {
                Ok(msgs) if !msgs.is_empty() => msgs,
                _ => return,
            }
        }
        _ => return,
    };

    let title = messages.first()
        .filter(|(role, _)| role == "you")
        .map(|(_, content)| auto_title(content))
        .unwrap_or_else(|| "Previous conversation".into());

    let mut meta = ChatSessionMeta::new(title);
    meta.turn_count = messages.len() / 2;

    save_chat_session(store, key, &meta, &messages);
    let _ = store.delete(legacy_key);
    tracing::info!("Migrated legacy conversation to session '{}'", meta.id);
}
