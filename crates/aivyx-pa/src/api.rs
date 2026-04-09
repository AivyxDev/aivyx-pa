//! HTTP API layer for the personal assistant.
//!
//! Provides a JSON + SSE API that the frontend consumes. Runs alongside
//! or instead of the TUI — both share the same agent, brain, and loop
//! infrastructure via [`AppState`].
//!
//! ## Endpoints
//!
//! | Method | Path                           | Description                          |
//! |--------|--------------------------------|--------------------------------------|
//! | POST   | `/api/chat`                    | Send a message, receive SSE stream   |
//! | GET    | `/api/notifications`           | SSE stream of loop notifications     |
//! | GET    | `/api/notifications/history`   | Past notifications (JSON)            |
//! | POST   | `/api/notifications/:id/rate`  | Rate a notification                  |
//! | GET    | `/api/goals`                   | List brain goals (JSON)              |
//! | POST   | `/api/goals`                   | Create a new goal                    |
//! | PUT    | `/api/goals/:id`               | Update a goal                        |
//! | POST   | `/api/goals/:id/complete`      | Mark a goal as completed             |
//! | POST   | `/api/goals/:id/abandon`       | Mark a goal as abandoned             |
//! | GET    | `/api/audit`                   | Recent audit entries (JSON)          |
//! | GET    | `/api/metrics`                 | Audit metrics summary (JSON)         |
//! | GET    | `/api/settings`                | Current settings snapshot (JSON)     |
//! | PUT    | `/api/settings/toggle`         | Toggle a boolean setting             |
//! | PUT    | `/api/settings/list`           | Update a string list setting         |
//! | PUT    | `/api/settings/value`          | Write a string or number setting     |
//! | POST   | `/api/settings/integration`    | Configure an integration             |
//! | GET    | `/api/sessions`                | List chat sessions (JSON)            |
//! | POST   | `/api/sessions`                | Create a new chat session            |
//! | GET    | `/api/sessions/:id/messages`   | Load session messages (JSON)         |
//! | DELETE | `/api/sessions/:id`            | Delete a chat session                |
//! | GET    | `/api/approvals`               | List pending approvals (JSON)        |
//! | POST   | `/api/approvals/:id/approve`   | Approve a pending item               |
//! | POST   | `/api/approvals/:id/deny`      | Deny a pending item                  |
//! | GET    | `/api/memories`                | List/search memories (JSON)          |
//! | GET    | `/api/memories/:id`            | Get a single memory (JSON)           |
//! | DELETE | `/api/memories/:id`            | Delete a memory                      |
//! | GET    | `/api/missions`                | List missions (JSON)                 |
//! | GET    | `/api/missions/:id`            | Get mission detail with steps (JSON) |
//! | POST   | `/api/missions/:id/cancel`     | Cancel an active mission             |
//! | POST   | `/api/missions/:id/resume`     | Resume a paused/failed mission       |
//! | POST   | `/api/missions/:id/approve`    | Resolve a mission approval gate      |
//! | DELETE | `/api/missions/:id`            | Delete a terminal mission            |
//! | GET    | `/api/dashboard`               | Dashboard hydration data (JSON)      |
//! | GET    | `/api/health`                  | Liveness check                       |

use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Json};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tower_http::cors::CorsLayer;

use aivyx_agent::agent::Agent;
use aivyx_audit::AuditLog;
use aivyx_brain::{BrainStore, Goal, GoalFilter, GoalStatus, Priority};
use aivyx_config::AivyxDirs;
use aivyx_core::TaskId;
use aivyx_core::{GoalId, MemoryId};
use aivyx_crypto::{EncryptedStore, MasterKey};
use aivyx_loop::Notification;
use aivyx_memory::MemoryManager;
use aivyx_task_engine::MissionToolContext;

use crate::config::PaConfig;

// ── Validation ────────────────────────────────────────────────

/// Maximum length for a chat message (32 KB).
const MAX_CHAT_MESSAGE_LEN: usize = 32_768;
/// Maximum length for a settings key or section name.
const MAX_SETTING_KEY_LEN: usize = 128;
/// Maximum number of values in a list setting update.
const MAX_LIST_VALUES: usize = 100;
/// Maximum length for a single list value or integration field.
const MAX_VALUE_LEN: usize = 4_096;

/// Reject empty or oversized strings at API boundaries.
fn validate_non_empty(
    field: &str,
    value: &str,
    max_len: usize,
) -> Result<(), (StatusCode, String)> {
    if value.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("{field} must not be empty"),
        ));
    }
    if value.len() > max_len {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("{field} exceeds maximum length of {max_len}"),
        ));
    }
    Ok(())
}

// ── Health Check ──────────────────────────────────────────────

/// Subsystem health status — computed at startup and refreshed periodically.
#[derive(Debug, Clone)]
pub struct HealthStatus {
    /// LLM provider reachability.
    pub provider: SubsystemHealth,
    /// Email (IMAP) connectivity.
    pub email: SubsystemHealth,
    /// Config file parseable.
    pub config: SubsystemHealth,
    /// Disk space adequate (>100 MB free in data dir).
    pub disk: SubsystemHealth,
    /// When this health snapshot was taken.
    pub checked_at: DateTime<Utc>,
}

/// Health state of a single subsystem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubsystemHealth {
    /// Working normally.
    Healthy,
    /// Not configured / not applicable.
    NotConfigured,
    /// Check failed with a message.
    Degraded(String),
}

impl SubsystemHealth {
    pub fn label(&self) -> &str {
        match self {
            SubsystemHealth::Healthy => "healthy",
            SubsystemHealth::NotConfigured => "n/a",
            SubsystemHealth::Degraded(_) => "degraded",
        }
    }

    pub fn is_healthy(&self) -> bool {
        matches!(
            self,
            SubsystemHealth::Healthy | SubsystemHealth::NotConfigured
        )
    }
}

impl Default for HealthStatus {
    fn default() -> Self {
        Self {
            provider: SubsystemHealth::NotConfigured,
            email: SubsystemHealth::NotConfigured,
            config: SubsystemHealth::NotConfigured,
            disk: SubsystemHealth::NotConfigured,
            checked_at: Utc::now(),
        }
    }
}

/// Run startup health checks and return a snapshot.
///
/// This is intentionally non-blocking for each subsystem — a failure in
/// one check doesn't prevent the others from running.
///
/// Accepts a reference to the LLM provider for health checking — this avoids
/// needing to add a public accessor to the `Agent` struct in aivyx-core.
pub async fn run_health_checks(
    provider: &dyn aivyx_llm::LlmProvider,
    email_config: Option<&aivyx_actions::email::EmailConfig>,
    config_path: &std::path::Path,
    data_dir: &std::path::Path,
) -> HealthStatus {
    let mut status = HealthStatus {
        checked_at: Utc::now(),
        ..Default::default()
    };

    // 1. LLM provider health check (3-second timeout)
    status.provider =
        match tokio::time::timeout(Duration::from_secs(3), provider.health_check()).await {
            Ok(Ok(())) => SubsystemHealth::Healthy,
            Ok(Err(e)) => SubsystemHealth::Degraded(format!("provider error: {e}")),
            Err(_) => SubsystemHealth::Degraded("provider health check timed out (3s)".into()),
        };

    // 2. Email (IMAP) — quick connection test if configured
    if let Some(email_cfg) = email_config {
        match tokio::time::timeout(
            Duration::from_secs(5),
            aivyx_actions::email::imap_connect(email_cfg),
        )
        .await
        {
            Ok(Ok(_)) => status.email = SubsystemHealth::Healthy,
            Ok(Err(e)) => status.email = SubsystemHealth::Degraded(format!("IMAP: {e}")),
            Err(_) => {
                status.email = SubsystemHealth::Degraded("IMAP connection timed out (5s)".into())
            }
        }
    }
    // else: remains NotConfigured

    // 3. Config parse check
    status.config = match std::fs::read_to_string(config_path) {
        Ok(content) => match toml::from_str::<toml::Value>(&content) {
            Ok(_) => SubsystemHealth::Healthy,
            Err(e) => SubsystemHealth::Degraded(format!("parse error: {e}")),
        },
        Err(e) => SubsystemHealth::Degraded(format!("read error: {e}")),
    };

    // 4. Disk space check (>100 MB free)
    status.disk = check_disk_space(data_dir);

    status
}

/// Check available disk space in the data directory.
fn check_disk_space(path: &std::path::Path) -> SubsystemHealth {
    // Use statvfs on Unix
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let c_path = match std::ffi::CString::new(path.as_os_str().as_bytes()) {
            Ok(p) => p,
            Err(_) => return SubsystemHealth::Degraded("invalid path".into()),
        };
        unsafe {
            let mut stat: libc::statvfs = std::mem::zeroed();
            if libc::statvfs(c_path.as_ptr(), &mut stat) == 0 {
                let free_bytes = stat.f_bavail * stat.f_frsize;
                let free_mb = free_bytes / (1024 * 1024);
                if free_mb < 100 {
                    SubsystemHealth::Degraded(format!("low disk space: {free_mb} MB free"))
                } else {
                    SubsystemHealth::Healthy
                }
            } else {
                SubsystemHealth::Degraded("statvfs failed".into())
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        SubsystemHealth::Healthy // Assume OK on non-Unix
    }
}

// ── Application State ──────────────────────────────────────────

/// Shared state for the API server.
///
/// Wraps the agent and supporting subsystems in `Arc` so axum handlers
/// can access them concurrently. The agent itself is behind a `Mutex`
/// because `turn_stream` takes `&mut self`.
#[derive(Clone)]
pub struct AppState {
    /// The agent (mutable — only one turn at a time).
    pub agent: Arc<Mutex<Agent>>,
    /// Brain store for goal queries.
    pub brain_store: Option<Arc<BrainStore>>,
    /// Brain encryption key.
    pub brain_key: Option<Arc<MasterKey>>,
    /// Audit log for reading entries.
    pub audit_log: Arc<AuditLog>,
    /// Broadcast sender for notifications from the agent loop.
    pub notification_tx: broadcast::Sender<Notification>,
    /// PA configuration snapshot (read-only).
    pub pa_config: Arc<PaConfig>,
    /// Aivyx directories.
    pub dirs: Arc<AivyxDirs>,
    /// Encrypted store for conversation persistence.
    pub store: Arc<EncryptedStore>,
    /// Domain-separated key for conversation encryption.
    pub conversation_key: Arc<MasterKey>,
    /// Raw master key for writing secrets to EncryptedStore from TUI/API.
    pub master_key: Arc<MasterKey>,
    /// Memory manager for memory CRUD and outcome rating.
    pub memory_manager: Option<Arc<Mutex<MemoryManager>>>,
    /// Pending approval queue shared with the notification bridge.
    pub approvals: Arc<Mutex<Vec<ApprovalItem>>>,
    /// Notification history buffer for the activity view.
    pub notification_history: Arc<Mutex<Vec<Notification>>>,
    /// Path to config.toml for settings editing.
    pub config_path: PathBuf,
    /// Agent display name (from PaConfig).
    pub agent_name: String,
    /// Mission tool context for task-engine queries (TUI Missions view).
    pub mission_ctx: Option<MissionToolContext>,
    /// Startup health check results (refreshed periodically).
    pub health: Arc<tokio::sync::RwLock<HealthStatus>>,
    /// Channel back to the agent loop for bidirectional approval decisions.
    /// When the user approves or denies something in the TUI or HTTP API,
    /// the response is sent here so the heartbeat can react immediately.
    pub approval_tx: Option<tokio::sync::mpsc::Sender<aivyx_loop::ApprovalResponse>>,
}

// ── Approval Types ────────────────────────────────────────────

/// Status of a pending approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Denied,
    Expired,
}

/// An item in the approval queue.
#[derive(Debug, Clone, Serialize)]
pub struct ApprovalItem {
    /// The notification that requested approval.
    pub notification: Notification,
    /// Current status.
    pub status: ApprovalStatus,
    /// When the approval was resolved (None if still pending).
    pub resolved_at: Option<DateTime<Utc>>,
    /// When the pending approval expires. Computed at creation time.
    pub expires_at: Option<DateTime<Utc>>,
}

// ── Router ─────────────────────────────────────────────────────

/// Build the axum router with all API routes.
pub fn router(state: AppState) -> Router {
    Router::new()
        // Chat
        .route("/api/chat", axum::routing::post(chat_stream))
        // Notifications
        .route(
            "/api/notifications",
            axum::routing::get(notifications_stream),
        )
        .route(
            "/api/notifications/history",
            axum::routing::get(notification_history),
        )
        .route(
            "/api/notifications/{id}/rate",
            axum::routing::post(rate_notification),
        )
        // Goals
        .route(
            "/api/goals",
            axum::routing::get(list_goals).post(create_goal),
        )
        .route("/api/goals/{id}", axum::routing::put(update_goal))
        .route(
            "/api/goals/{id}/complete",
            axum::routing::post(complete_goal),
        )
        .route("/api/goals/{id}/abandon", axum::routing::post(abandon_goal))
        // Audit
        .route("/api/audit", axum::routing::get(list_audit))
        .route("/api/metrics", axum::routing::get(get_metrics))
        // Settings
        .route("/api/settings", axum::routing::get(get_settings))
        .route("/api/settings/toggle", axum::routing::put(toggle_setting))
        .route(
            "/api/settings/list",
            axum::routing::put(update_list_setting),
        )
        .route(
            "/api/settings/value",
            axum::routing::put(update_value_setting),
        )
        .route(
            "/api/settings/integration",
            axum::routing::post(configure_integration),
        )
        // Sessions
        .route(
            "/api/sessions",
            axum::routing::get(list_sessions).post(create_session),
        )
        .route("/api/sessions/{id}", axum::routing::delete(delete_session))
        .route(
            "/api/sessions/{id}/messages",
            axum::routing::get(get_session_messages),
        )
        // Approvals
        .route("/api/approvals", axum::routing::get(list_approvals))
        .route(
            "/api/approvals/{id}/approve",
            axum::routing::post(approve_item),
        )
        .route("/api/approvals/{id}/deny", axum::routing::post(deny_item))
        // Memories
        .route("/api/memories", axum::routing::get(list_memories))
        .route(
            "/api/memories/{id}",
            axum::routing::get(get_memory).delete(delete_memory),
        )
        // Missions
        .route("/api/missions", axum::routing::get(list_missions))
        .route(
            "/api/missions/{id}",
            axum::routing::get(get_mission).delete(delete_mission),
        )
        .route(
            "/api/missions/{id}/cancel",
            axum::routing::post(cancel_mission),
        )
        .route(
            "/api/missions/{id}/resume",
            axum::routing::post(resume_mission),
        )
        .route(
            "/api/missions/{id}/approve",
            axum::routing::post(approve_mission),
        )
        // Dashboard
        .route("/api/dashboard", axum::routing::get(get_dashboard))
        // Health
        .route("/api/health", axum::routing::get(health))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

/// Spawn the API server as a background task on the given port.
///
/// Returns a `JoinHandle` and a `CancellationToken` for graceful shutdown.
pub async fn spawn_api_server(
    state: AppState,
    port: u16,
) -> anyhow::Result<(tokio::task::JoinHandle<()>, CancellationToken)> {
    let app = router(state);
    let addr = format!("127.0.0.1:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    tracing::info!("API server listening on {addr}");

    let handle = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(cancel_clone.cancelled_owned())
            .await
            .ok();
    });

    Ok((handle, cancel))
}

// ── Chat (SSE streaming) ───────────────────────────────────────

/// Request body for `POST /api/chat`.
#[derive(Debug, Deserialize)]
pub struct ChatRequest {
    /// The user's message.
    pub message: String,
    /// Optional session ID to associate this turn with.
    pub session_id: Option<String>,
}

/// `POST /api/chat` — send a user message and receive the assistant's
/// response as a Server-Sent Event stream.
///
/// Each token is emitted as an SSE event with `event: token`.
/// When the turn completes, a final `event: done` is sent with the
/// full response text and session metadata.
async fn chat_stream(
    State(state): State<AppState>,
    Json(req): Json<ChatRequest>,
) -> Result<Sse<impl futures_core::Stream<Item = Result<Event, Infallible>>>, (StatusCode, String)>
{
    validate_non_empty("message", &req.message, MAX_CHAT_MESSAGE_LEN)?;
    if let Some(ref sid) = req.session_id {
        validate_non_empty("session_id", sid, MAX_SETTING_KEY_LEN)?;
    }

    let (token_tx, mut token_rx) = mpsc::channel::<String>(256);
    let agent = Arc::clone(&state.agent);
    let message = req.message.clone();
    let store = Arc::clone(&state.store);
    let conv_key = Arc::clone(&state.conversation_key);
    let session_id = req.session_id.clone();
    let user_msg = req.message;

    // Spawn the agent turn in a background task with a 5-minute timeout
    tokio::spawn(async move {
        let mut agent = agent.lock().await;

        // Restore conversation history when resuming an existing session.
        // If the agent's conversation is empty but the session has prior
        // messages, inject them so the LLM has full conversational context.
        if let Some(ref sid) = session_id
            && agent.conversation().is_empty()
        {
            if let Some(pairs) = crate::sessions::load_chat_messages(&store, &conv_key, sid)
                && !pairs.is_empty()
            {
                let history = crate::sessions::to_chat_messages(&pairs);
                agent.restore_conversation(history);
            }
            // Restore ephemeral learned state (tool results, domain
            // confidence, cost tracking, etc.) from the resume token.
            if let Some(token) = crate::sessions::load_resume_token(&store, &conv_key, sid) {
                agent.apply_resume_state(token.into_snapshot());
            }
        }

        let cancel = CancellationToken::new();
        let turn_timeout = std::time::Duration::from_secs(300);
        let turn_future = agent.turn_stream(&message, None, token_tx.clone(), Some(cancel.clone()));
        let result = match tokio::time::timeout(turn_timeout, turn_future).await {
            Ok(r) => r,
            Err(_) => {
                cancel.cancel();
                Err(aivyx_core::AivyxError::LlmProvider(
                    "agent turn timed out after 5 minutes".into(),
                ))
            }
        };

        // Persist the full conversation (including tool results) after the turn.
        // Extract from the agent's in-memory conversation to preserve tool call
        // context, system notes, and rescue guidance — not just user/assistant text.
        let response_text = match &result {
            Ok(text) => text.clone(),
            Err(e) => format!("[Error: {e}]"),
        };

        // Save to session
        let sid =
            session_id.unwrap_or_else(|| format!("{}", chrono::Utc::now().timestamp_millis()));

        // Extract the full conversation from the agent, preserving tool results.
        let messages = crate::sessions::conversation_to_session(agent.conversation());

        // If the agent errored and we have no conversation, fall back to manual.
        let messages = if messages.is_empty() {
            vec![
                crate::sessions::SessionMessage::user(&user_msg),
                crate::sessions::SessionMessage::assistant(&response_text),
            ]
        } else {
            messages
        };

        let title = {
            // Find the first user message for the title
            let first_user = messages
                .iter()
                .find(|m| m.role == "user")
                .map(|m| m.content.as_str())
                .unwrap_or(&user_msg);

            // Check if we already have a title from a prior save
            crate::sessions::list_chat_sessions(&store, &conv_key)
                .into_iter()
                .find(|s| s.id == sid)
                .map(|s| s.title)
                .unwrap_or_else(|| crate::sessions::auto_title(first_user))
        };

        let mut meta = crate::sessions::ChatSessionMeta::new(title);
        meta.id = sid.clone();
        meta.turn_count = crate::sessions::count_turns(&messages);
        meta.updated_at = chrono::Utc::now();
        crate::sessions::save_chat_session(&store, &conv_key, &meta, &messages);

        // Persist the agent's ephemeral learned state alongside the conversation.
        let resume = crate::sessions::ResumeToken::from_snapshot(agent.export_resume_state());
        crate::sessions::save_resume_token(&store, &conv_key, &sid, &resume);

        // Send done event with session ID
        let done_data = serde_json::json!({ "session_id": sid });
        let _ = token_tx.send(format!("\n[[DONE:{done_data}]]")).await;
    });

    // Convert the mpsc receiver into an SSE stream
    let stream = async_stream::stream! {
        while let Some(chunk) = token_rx.recv().await {
            if chunk.starts_with("\n[[DONE:") {
                let data = &chunk[8..chunk.len()-2]; // extract JSON
                yield Ok(Event::default().event("done").data(data));
                break;
            }
            yield Ok(Event::default().event("token").data(&chunk));
        }
    };

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

// ── Notifications (SSE) ────────────────────────────────────────

/// `GET /api/notifications` — subscribe to real-time notifications
/// from the agent loop (briefings, reminders, approvals, etc.).
///
/// Uses `broadcast` so multiple frontend clients can subscribe.
async fn notifications_stream(
    State(state): State<AppState>,
) -> Sse<impl futures_core::Stream<Item = Result<Event, Infallible>>> {
    let mut rx = state.notification_tx.subscribe();

    let stream = async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(notification) => {
                    if let Ok(json) = serde_json::to_string(&notification) {
                        yield Ok(Event::default()
                            .event("notification")
                            .data(json));
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("Notification SSE client lagged, missed {n} events");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    };

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    )
}

// ── Notification History & Rating ──────────────────────────────

/// Query parameters for `GET /api/notifications/history`.
#[derive(Debug, Deserialize, Default)]
pub struct NotificationHistoryQuery {
    /// Maximum number of notifications to return. Default: 100.
    pub limit: Option<usize>,
    /// Filter by source prefix (e.g., "heartbeat").
    pub source: Option<String>,
}

/// `GET /api/notifications/history` — list past notifications.
async fn notification_history(
    State(state): State<AppState>,
    Query(query): Query<NotificationHistoryQuery>,
) -> impl IntoResponse {
    let history = state.notification_history.lock().await;
    let limit = query.limit.unwrap_or(100);

    let filtered: Vec<&Notification> = history
        .iter()
        .rev()
        .filter(|n| {
            if let Some(ref src) = query.source {
                n.source.starts_with(src.as_str())
            } else {
                true
            }
        })
        .take(limit)
        .collect();

    Json(serde_json::json!({
        "notifications": filtered,
        "total": history.len(),
    }))
}

/// Request body for `POST /api/notifications/:id/rate`.
#[derive(Debug, Deserialize)]
pub struct RateRequest {
    /// Rating: "useful", "partial", or "useless".
    pub rating: aivyx_memory::Rating,
}

/// `POST /api/notifications/:id/rate` — rate a notification.
async fn rate_notification(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<RateRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let mm = state
        .memory_manager
        .as_ref()
        .ok_or((StatusCode::NOT_FOUND, "Memory manager not available".into()))?;

    // Find the notification
    let history = state.notification_history.lock().await;
    let notif = history.iter().find(|n| n.id == id).ok_or((
        StatusCode::NOT_FOUND,
        format!("Notification {id} not found"),
    ))?;

    // Only heartbeat items are ratable
    if !notif.source.starts_with("heartbeat") {
        return Err((
            StatusCode::BAD_REQUEST,
            "Only heartbeat notifications can be rated".into(),
        ));
    }

    let outcome = aivyx_memory::OutcomeRecord::new(
        aivyx_memory::OutcomeSource::ToolCall {
            tool_name: "heartbeat_suggest".into(),
        },
        req.rating != aivyx_memory::Rating::Useless,
        notif.title.clone(),
        0,
        state.agent_name.clone(),
        notif.body.clone(),
    )
    .with_tags(vec!["suggestion".into(), notif.source.clone()])
    .with_rating(req.rating);

    drop(history); // release lock before acquiring memory manager lock

    let mm_guard = mm.lock().await;
    mm_guard.record_outcome(&outcome).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to record rating: {e}"),
        )
    })?;

    Ok(Json(
        serde_json::json!({ "status": "rated", "rating": req.rating }),
    ))
}

// ── Goals ──────────────────────────────────────────────────────

/// Query parameters for `GET /api/goals`.
#[derive(Debug, Deserialize, Default)]
pub struct GoalsQuery {
    /// Filter by status: "active", "completed", "abandoned", or "all" (default).
    pub status: Option<String>,
}

/// `GET /api/goals` — list brain goals.
async fn list_goals(
    State(state): State<AppState>,
    Query(query): Query<GoalsQuery>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let (Some(store), Some(key)) = (&state.brain_store, &state.brain_key) else {
        return Ok(Json(serde_json::json!({ "goals": [], "available": false })));
    };

    let filter = match query.status.as_deref() {
        Some("active") => GoalFilter {
            status: Some(GoalStatus::Active),
            ..Default::default()
        },
        Some("completed") => GoalFilter {
            status: Some(GoalStatus::Completed),
            ..Default::default()
        },
        Some("abandoned") => GoalFilter {
            status: Some(GoalStatus::Abandoned),
            ..Default::default()
        },
        _ => GoalFilter::default(),
    };

    match store.list_goals(&filter, key) {
        Ok(goals) => Ok(Json(
            serde_json::json!({ "goals": goals, "available": true }),
        )),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to load goals: {e}"),
        )),
    }
}

/// Maximum length for a goal description or criteria.
const MAX_GOAL_TEXT_LEN: usize = 2_048;

/// Helper: get brain store and key or return 404.
#[allow(clippy::type_complexity)]
fn brain_store_or_err(
    state: &AppState,
) -> Result<(&Arc<BrainStore>, &Arc<MasterKey>), (StatusCode, String)> {
    state
        .brain_store
        .as_ref()
        .zip(state.brain_key.as_ref())
        .ok_or((
            StatusCode::SERVICE_UNAVAILABLE,
            "Brain not available".into(),
        ))
}

/// Parse a GoalId from a URL path segment.
fn parse_goal_id(id: &str) -> Result<GoalId, (StatusCode, String)> {
    let uuid = uuid::Uuid::parse_str(id)
        .map_err(|_| (StatusCode::BAD_REQUEST, format!("Invalid goal ID: {id}")))?;
    Ok(GoalId::from_uuid(uuid))
}

/// Request body for `POST /api/goals`.
#[derive(Debug, Deserialize)]
pub struct CreateGoalRequest {
    /// What the agent should achieve.
    pub description: String,
    /// Measurable success criteria.
    pub criteria: String,
    /// Priority level: "background", "low", "medium", "high", "critical".
    pub priority: Option<String>,
    /// Optional deadline (ISO 8601).
    pub deadline: Option<DateTime<Utc>>,
}

/// Parse a priority string to the enum, defaulting to Medium.
fn parse_priority(s: Option<&str>) -> Priority {
    match s {
        Some("background") => Priority::Background,
        Some("low") => Priority::Low,
        Some("high") => Priority::High,
        Some("critical") => Priority::Critical,
        _ => Priority::Medium,
    }
}

/// `POST /api/goals` — create a new goal.
async fn create_goal(
    State(state): State<AppState>,
    Json(req): Json<CreateGoalRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    validate_non_empty("description", &req.description, MAX_GOAL_TEXT_LEN)?;
    validate_non_empty("criteria", &req.criteria, MAX_GOAL_TEXT_LEN)?;

    let (store, key) = brain_store_or_err(&state)?;
    let priority = parse_priority(req.priority.as_deref());
    let mut goal = Goal::new(&req.description, &req.criteria).with_priority(priority);
    if let Some(deadline) = req.deadline {
        goal.deadline = Some(deadline);
    }

    store.upsert_goal(&goal, key).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to create goal: {e}"),
        )
    })?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "goal": goal })),
    ))
}

/// Request body for `PUT /api/goals/:id`.
#[derive(Debug, Deserialize)]
pub struct UpdateGoalRequest {
    /// Updated description (optional).
    pub description: Option<String>,
    /// Updated success criteria (optional).
    pub criteria: Option<String>,
    /// Updated priority (optional).
    pub priority: Option<String>,
    /// Updated deadline (optional; null to clear).
    pub deadline: Option<Option<DateTime<Utc>>>,
}

/// `PUT /api/goals/:id` — update a goal's fields.
async fn update_goal(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateGoalRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let (store, key) = brain_store_or_err(&state)?;
    let goal_id = parse_goal_id(&id)?;

    let mut goal = store
        .get_goal(goal_id, key)
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to load goal: {e}"),
            )
        })?
        .ok_or((StatusCode::NOT_FOUND, format!("Goal {id} not found")))?;

    if let Some(ref desc) = req.description {
        validate_non_empty("description", desc, MAX_GOAL_TEXT_LEN)?;
        goal.description = desc.clone();
    }
    if let Some(ref criteria) = req.criteria {
        validate_non_empty("criteria", criteria, MAX_GOAL_TEXT_LEN)?;
        goal.success_criteria = criteria.clone();
    }
    if let Some(ref priority) = req.priority {
        goal.priority = parse_priority(Some(priority.as_str()));
    }
    if let Some(ref deadline) = req.deadline {
        goal.deadline = *deadline;
    }
    goal.updated_at = Utc::now();

    store.upsert_goal(&goal, key).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to update goal: {e}"),
        )
    })?;

    Ok(Json(serde_json::json!({ "goal": goal })))
}

/// `POST /api/goals/:id/complete` — mark a goal as completed.
async fn complete_goal(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let (store, key) = brain_store_or_err(&state)?;
    let goal_id = parse_goal_id(&id)?;

    let mut goal = store
        .get_goal(goal_id, key)
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to load goal: {e}"),
            )
        })?
        .ok_or((StatusCode::NOT_FOUND, format!("Goal {id} not found")))?;

    if goal.status != GoalStatus::Active && goal.status != GoalStatus::Dormant {
        return Err((
            StatusCode::CONFLICT,
            format!("Cannot complete goal in {:?} state", goal.status),
        ));
    }

    goal.status = GoalStatus::Completed;
    goal.progress = 1.0;
    goal.updated_at = Utc::now();

    store.upsert_goal(&goal, key).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to complete goal: {e}"),
        )
    })?;

    Ok(Json(
        serde_json::json!({ "status": "completed", "goal": goal }),
    ))
}

/// `POST /api/goals/:id/abandon` — mark a goal as abandoned.
async fn abandon_goal(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let (store, key) = brain_store_or_err(&state)?;
    let goal_id = parse_goal_id(&id)?;

    let mut goal = store
        .get_goal(goal_id, key)
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to load goal: {e}"),
            )
        })?
        .ok_or((StatusCode::NOT_FOUND, format!("Goal {id} not found")))?;

    if goal.status != GoalStatus::Active && goal.status != GoalStatus::Dormant {
        return Err((
            StatusCode::CONFLICT,
            format!("Cannot abandon goal in {:?} state", goal.status),
        ));
    }

    goal.status = GoalStatus::Abandoned;
    goal.updated_at = Utc::now();

    store.upsert_goal(&goal, key).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to abandon goal: {e}"),
        )
    })?;

    Ok(Json(
        serde_json::json!({ "status": "abandoned", "goal": goal }),
    ))
}

// ── Audit ──────────────────────────────────────────────────────

/// Query parameters for `GET /api/audit`.
#[derive(Debug, Deserialize, Default)]
pub struct AuditQuery {
    /// Maximum number of recent entries to return. Default: 100.
    pub limit: Option<usize>,
}

/// `GET /api/audit` — return recent audit log entries.
async fn list_audit(
    State(state): State<AppState>,
    Query(query): Query<AuditQuery>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let limit = query.limit.unwrap_or(100);
    match state.audit_log.recent(limit) {
        Ok(entries) => Ok(Json(
            serde_json::json!({ "entries": entries, "count": entries.len() }),
        )),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to read audit log: {e}"),
        )),
    }
}

// ── Metrics ────────────────────────────────────────────────────

/// Query parameters for `GET /api/metrics`.
#[derive(Debug, Deserialize, Default)]
pub struct MetricsQuery {
    /// Start of the time range (ISO 8601). Default: 24 hours ago.
    pub from: Option<DateTime<Utc>>,
    /// End of the time range (ISO 8601). Default: now.
    pub to: Option<DateTime<Utc>>,
}

/// `GET /api/metrics` — compute aggregated audit metrics.
async fn get_metrics(
    State(state): State<AppState>,
    Query(query): Query<MetricsQuery>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let now = Utc::now();
    let from = query
        .from
        .unwrap_or_else(|| now - chrono::Duration::hours(24));
    let to = query.to.unwrap_or(now);

    let entries = state.audit_log.read_all_entries().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to read audit log: {e}"),
        )
    })?;

    let summary = aivyx_audit::compute_summary(&entries, from, to, &|_, _, _| 0.0);
    Ok(Json(summary))
}

// ── Settings ───────────────────────────────────────────────────

/// Allowed (section, key) pairs for boolean toggle writes.
/// Derived from TUI `settings_toggle_current` — only heartbeat flags.
const ALLOWED_TOGGLES: &[(&str, &str)] = &[
    ("[heartbeat]", "enabled"),
    ("[heartbeat]", "can_reflect"),
    ("[heartbeat]", "can_consolidate_memory"),
    ("[heartbeat]", "can_analyze_failures"),
    ("[heartbeat]", "can_extract_knowledge"),
    ("[heartbeat]", "can_plan_review"),
    ("[heartbeat]", "can_strategy_review"),
    ("[heartbeat]", "can_track_mood"),
    ("[heartbeat]", "can_encourage"),
    ("[heartbeat]", "can_track_milestones"),
    ("[heartbeat]", "notification_pacing"),
    ("[autonomy]", "require_approval_for_destructive"),
    ("[memory]", "use_graph_recall"),
];

/// Allowed (section, key) pairs for string/number value writes.
/// Derived from TUI `settings_activate_current`.
const ALLOWED_VALUES: &[(&str, &str)] = &[
    // Provider
    ("[provider]", "model"),
    ("[provider]", "base_url"),
    // Autonomy
    ("[autonomy]", "default_tier"),
    ("[autonomy]", "max_tool_calls_per_min"),
    ("[autonomy]", "max_cost_usd"),
    // Agent
    ("[agent]", "name"),
    // Persona dimensions
    ("[persona]", "formality"),
    ("[persona]", "verbosity"),
    ("[persona]", "warmth"),
    ("[persona]", "humor"),
    ("[persona]", "confidence"),
    // Tool discovery
    ("[tool_discovery]", "mode"),
];

/// Allowed (section, key) pairs for string list writes.
const ALLOWED_LISTS: &[(&str, &str)] = &[
    ("[agent]", "skills"),
    ("[tool_discovery]", "always_include"),
];

/// Check whether a (section, key) pair is in an allowlist.
fn is_allowed(section: &str, key: &str, allowlist: &[(&str, &str)]) -> bool {
    allowlist.iter().any(|(s, k)| *s == section && *k == key)
}

/// `GET /api/settings` — return the full settings snapshot.
///
/// Returns the same rich `SettingsSnapshot` the TUI uses, giving the
/// frontend complete visibility into all configured features.
async fn get_settings(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let snapshot = crate::settings::reload_settings_snapshot(&state.config_path).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to load settings: {e}"),
        )
    })?;
    Ok(Json(serde_json::json!({
        "settings": snapshot,
        "config_path": state.config_path,
    })))
}

/// Request body for `PUT /api/settings/toggle`.
#[derive(Debug, Deserialize)]
pub struct ToggleRequest {
    /// TOML section name (e.g., "heartbeat", "autonomy").
    pub section: String,
    /// Key within the section (e.g., "enabled", "can_reflect").
    pub key: String,
    /// New boolean value.
    pub value: bool,
}

/// `PUT /api/settings/toggle` — toggle a boolean setting in config.toml.
async fn toggle_setting(
    State(state): State<AppState>,
    Json(req): Json<ToggleRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    validate_non_empty("section", &req.section, MAX_SETTING_KEY_LEN)?;
    validate_non_empty("key", &req.key, MAX_SETTING_KEY_LEN)?;
    if !is_allowed(&req.section, &req.key, ALLOWED_TOGGLES) {
        return Err((
            StatusCode::FORBIDDEN,
            format!(
                "Setting [{}/{}] is not modifiable via this endpoint",
                req.section, req.key
            ),
        ));
    }
    crate::settings::toggle_config_bool(&state.config_path, &req.section, &req.key, req.value)
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to toggle setting: {e}"),
            )
        })?;

    let snapshot = crate::settings::reload_settings_snapshot(&state.config_path).ok();
    Ok(Json(
        serde_json::json!({ "status": "updated", "settings": snapshot }),
    ))
}

/// Request body for `PUT /api/settings/list`.
#[derive(Debug, Deserialize)]
pub struct ListSettingRequest {
    /// TOML section name.
    pub section: String,
    /// Key within the section.
    pub key: String,
    /// New list values.
    pub values: Vec<String>,
}

/// `PUT /api/settings/list` — update a string list setting.
async fn update_list_setting(
    State(state): State<AppState>,
    Json(req): Json<ListSettingRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    validate_non_empty("section", &req.section, MAX_SETTING_KEY_LEN)?;
    validate_non_empty("key", &req.key, MAX_SETTING_KEY_LEN)?;
    if !is_allowed(&req.section, &req.key, ALLOWED_LISTS) {
        return Err((
            StatusCode::FORBIDDEN,
            format!(
                "Setting [{}/{}] is not modifiable via this endpoint",
                req.section, req.key
            ),
        ));
    }
    if req.values.len() > MAX_LIST_VALUES {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("values list exceeds maximum of {MAX_LIST_VALUES} entries"),
        ));
    }
    for (i, v) in req.values.iter().enumerate() {
        if v.len() > MAX_VALUE_LEN {
            return Err((
                StatusCode::PAYLOAD_TOO_LARGE,
                format!("values[{i}] exceeds maximum length of {MAX_VALUE_LEN}"),
            ));
        }
    }
    crate::settings::write_toml_string_array(
        &state.config_path,
        &req.section,
        &req.key,
        &req.values,
    );

    let snapshot = crate::settings::reload_settings_snapshot(&state.config_path).ok();
    Ok(Json(
        serde_json::json!({ "status": "updated", "settings": snapshot }),
    ))
}

/// Request body for `POST /api/settings/integration`.
#[derive(Debug, Deserialize)]
pub struct IntegrationRequest {
    /// Which integration to configure.
    pub kind: crate::settings::IntegrationKind,
    /// Key-value pairs for the integration config.
    pub fields: Vec<(String, String)>,
}

/// `POST /api/settings/integration` — configure an integration.
async fn configure_integration(
    State(state): State<AppState>,
    Json(req): Json<IntegrationRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if req.fields.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "fields must not be empty".into()));
    }
    for (key, value) in &req.fields {
        validate_non_empty("field key", key, MAX_SETTING_KEY_LEN)?;
        if value.len() > MAX_VALUE_LEN {
            return Err((
                StatusCode::PAYLOAD_TOO_LARGE,
                format!("field '{key}' value exceeds maximum length of {MAX_VALUE_LEN}"),
            ));
        }
    }
    crate::settings::write_integration_config(&state.config_path, req.kind, &req.fields).map_err(
        |e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to configure integration: {e}"),
            )
        },
    )?;

    let snapshot = crate::settings::reload_settings_snapshot(&state.config_path).ok();
    Ok(Json(
        serde_json::json!({ "status": "configured", "settings": snapshot }),
    ))
}

/// Request body for `PUT /api/settings/value`.
#[derive(Debug, Deserialize)]
pub struct ValueSettingRequest {
    /// TOML section name (e.g., "[provider]", "[persona]").
    pub section: String,
    /// Key within the section (e.g., "model", "warmth").
    pub key: String,
    /// New value (string or number as string).
    pub value: String,
}

/// `PUT /api/settings/value` — write a string or number setting.
///
/// Handles all scalar settings: model name, base URL, agent name,
/// persona dimensions, autonomy tier, rate limits, etc. The value
/// is written as-is for strings or validated as a number for numeric keys.
async fn update_value_setting(
    State(state): State<AppState>,
    Json(req): Json<ValueSettingRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    validate_non_empty("section", &req.section, MAX_SETTING_KEY_LEN)?;
    validate_non_empty("key", &req.key, MAX_SETTING_KEY_LEN)?;
    validate_non_empty("value", &req.value, MAX_VALUE_LEN)?;
    if !is_allowed(&req.section, &req.key, ALLOWED_VALUES) {
        return Err((
            StatusCode::FORBIDDEN,
            format!(
                "Setting [{}/{}] is not modifiable via this endpoint",
                req.section, req.key
            ),
        ));
    }

    // Numeric keys are written without quotes; string keys with quotes
    let numeric_keys = [
        "max_tool_calls_per_min",
        "max_cost_usd",
        "formality",
        "verbosity",
        "warmth",
        "humor",
        "confidence",
    ];

    if numeric_keys.contains(&req.key.as_str()) {
        // Validate it parses as a number
        if req.value.parse::<f64>().is_err() {
            return Err((
                StatusCode::BAD_REQUEST,
                format!(
                    "'{key}' requires a numeric value, got: {val}",
                    key = req.key,
                    val = req.value
                ),
            ));
        }
        crate::settings::write_toml_number(&state.config_path, &req.section, &req.key, &req.value)
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to write setting: {e}"),
                )
            })?;
    } else {
        crate::settings::write_toml_string(&state.config_path, &req.section, &req.key, &req.value)
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to write setting: {e}"),
                )
            })?;
    }

    let snapshot = crate::settings::reload_settings_snapshot(&state.config_path).ok();
    Ok(Json(
        serde_json::json!({ "status": "updated", "settings": snapshot }),
    ))
}

// ── Sessions ───────────────────────────────────────────────────

/// `GET /api/sessions` — list all chat sessions.
async fn list_sessions(State(state): State<AppState>) -> impl IntoResponse {
    let sessions = crate::sessions::list_chat_sessions(&state.store, &state.conversation_key);
    Json(serde_json::json!({ "sessions": sessions }))
}

/// Request body for `POST /api/sessions`.
#[derive(Debug, Deserialize)]
pub struct CreateSessionRequest {
    /// Optional title. Auto-generated if omitted.
    pub title: Option<String>,
}

/// `POST /api/sessions` — create a new empty chat session.
async fn create_session(
    State(state): State<AppState>,
    Json(req): Json<CreateSessionRequest>,
) -> impl IntoResponse {
    let title = req.title.unwrap_or_else(|| "New conversation".into());
    let meta = crate::sessions::ChatSessionMeta::new(title);
    crate::sessions::save_chat_session(&state.store, &state.conversation_key, &meta, &[]);
    Json(serde_json::json!({ "session": meta }))
}

/// `GET /api/sessions/:id/messages` — load messages for a session.
async fn get_session_messages(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let messages = crate::sessions::load_chat_messages(&state.store, &state.conversation_key, &id)
        .ok_or((StatusCode::NOT_FOUND, format!("Session {id} not found")))?;

    Ok(Json(serde_json::json!({
        "session_id": id,
        "messages": messages,
        "count": messages.len(),
    })))
}

/// `DELETE /api/sessions/:id` — delete a chat session.
async fn delete_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    // Verify the session exists before deleting
    crate::sessions::load_chat_messages(&state.store, &state.conversation_key, &id)
        .ok_or((StatusCode::NOT_FOUND, format!("Session {id} not found")))?;
    crate::sessions::delete_chat_session(&state.store, &id);
    Ok(Json(
        serde_json::json!({ "status": "deleted", "session_id": id }),
    ))
}

// ── Approvals ──────────────────────────────────────────────────

/// `GET /api/approvals` — list approval items.
async fn list_approvals(State(state): State<AppState>) -> impl IntoResponse {
    let approvals = state.approvals.lock().await;
    let pending: Vec<&ApprovalItem> = approvals
        .iter()
        .filter(|a| a.status == ApprovalStatus::Pending)
        .collect();
    Json(serde_json::json!({
        "approvals": *approvals,
        "pending_count": pending.len(),
    }))
}

/// `POST /api/approvals/:id/approve` — approve a pending item.
async fn approve_item(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let mut approvals = state.approvals.lock().await;
    let item = approvals
        .iter_mut()
        .find(|a| a.notification.id == id && a.status == ApprovalStatus::Pending)
        .ok_or((
            StatusCode::NOT_FOUND,
            format!("Pending approval {id} not found"),
        ))?;

    item.status = ApprovalStatus::Approved;
    item.resolved_at = Some(Utc::now());

    // Send back to the agent loop for immediate reaction
    if let Some(ref tx) = state.approval_tx {
        let _ = tx.try_send(aivyx_loop::ApprovalResponse {
            notification_id: id.clone(),
            approved: true,
            message: None,
        });
    }

    Ok(Json(serde_json::json!({ "status": "approved", "id": id })))
}

/// `POST /api/approvals/:id/deny` — deny a pending item.
async fn deny_item(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let mut approvals = state.approvals.lock().await;
    let item = approvals
        .iter_mut()
        .find(|a| a.notification.id == id && a.status == ApprovalStatus::Pending)
        .ok_or((
            StatusCode::NOT_FOUND,
            format!("Pending approval {id} not found"),
        ))?;

    item.status = ApprovalStatus::Denied;
    item.resolved_at = Some(Utc::now());

    // Send back to the agent loop for immediate reaction
    if let Some(ref tx) = state.approval_tx {
        let _ = tx.try_send(aivyx_loop::ApprovalResponse {
            notification_id: id.clone(),
            approved: false,
            message: None,
        });
    }

    Ok(Json(serde_json::json!({ "status": "denied", "id": id })))
}

// ── Memories ───────────────────────────────────────────────────

/// Query parameters for `GET /api/memories`.
#[derive(Debug, Deserialize, Default)]
pub struct MemoriesQuery {
    /// Search query (case-insensitive substring match on content + tags).
    pub q: Option<String>,
    /// Maximum number of memories to return. Default: 200.
    pub limit: Option<usize>,
}

/// `GET /api/memories` — list or search memories.
async fn list_memories(
    State(state): State<AppState>,
    Query(query): Query<MemoriesQuery>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let mm = state
        .memory_manager
        .as_ref()
        .ok_or((StatusCode::NOT_FOUND, "Memory manager not available".into()))?;

    let mm_guard = mm.lock().await;
    let ids = mm_guard.list_memories().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to list memories: {e}"),
        )
    })?;

    let limit = query.limit.unwrap_or(200);
    let search = query.q.as_deref().unwrap_or("").to_lowercase();

    let mut memories: Vec<serde_json::Value> = Vec::new();
    for id in &ids {
        if memories.len() >= limit {
            break;
        }

        if let Ok(Some(entry)) = mm_guard.load_memory(id) {
            // Apply search filter if present
            if !search.is_empty() {
                let content_match = entry.content.to_lowercase().contains(&search);
                let tag_match = entry
                    .tags
                    .iter()
                    .any(|t| t.to_lowercase().contains(&search));
                if !content_match && !tag_match {
                    continue;
                }
            }

            memories.push(serde_json::json!({
                "id": id,
                "entry": entry,
            }));
        }
    }

    // Sort by updated_at descending
    memories.sort_by(|a, b| {
        let a_time = a["entry"]["updated_at"].as_str().unwrap_or("");
        let b_time = b["entry"]["updated_at"].as_str().unwrap_or("");
        b_time.cmp(a_time)
    });

    Ok(Json(serde_json::json!({
        "memories": memories,
        "total": ids.len(),
        "returned": memories.len(),
    })))
}

/// `GET /api/memories/:id` — get a single memory by ID.
async fn get_memory(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let mm = state
        .memory_manager
        .as_ref()
        .ok_or((StatusCode::NOT_FOUND, "Memory manager not available".into()))?;

    let uuid = uuid::Uuid::parse_str(&id)
        .map_err(|_| (StatusCode::BAD_REQUEST, format!("Invalid memory ID: {id}")))?;
    let memory_id = MemoryId::from_uuid(uuid);

    let mm_guard = mm.lock().await;
    let entry = mm_guard
        .load_memory(&memory_id)
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to load memory: {e}"),
            )
        })?
        .ok_or((StatusCode::NOT_FOUND, format!("Memory {id} not found")))?;

    Ok(Json(serde_json::json!({
        "id": id,
        "entry": entry,
    })))
}

/// `DELETE /api/memories/:id` — delete a memory.
async fn delete_memory(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let mm = state
        .memory_manager
        .as_ref()
        .ok_or((StatusCode::NOT_FOUND, "Memory manager not available".into()))?;

    let uuid = uuid::Uuid::parse_str(&id)
        .map_err(|_| (StatusCode::BAD_REQUEST, format!("Invalid memory ID: {id}")))?;
    let memory_id = MemoryId::from_uuid(uuid);

    let mut mm_guard = mm.lock().await;
    mm_guard.forget(&memory_id).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to delete memory: {e}"),
        )
    })?;

    Ok(Json(serde_json::json!({ "status": "deleted", "id": id })))
}

// ── Missions ──────────────────────────────────────────────────

/// Maximum length for a mission approval message.
const MAX_APPROVAL_MESSAGE_LEN: usize = 4_096;

/// Helper: build a `TaskEngine` from the AppState's mission context.
fn build_engine(state: &AppState) -> Result<aivyx_task_engine::TaskEngine, (StatusCode, String)> {
    let ctx = state.mission_ctx.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "Mission engine not available".into(),
    ))?;
    crate::agent::build_task_engine(ctx).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to build task engine: {e}"),
        )
    })
}

/// Parse a TaskId from a URL path segment.
fn parse_task_id(id: &str) -> Result<TaskId, (StatusCode, String)> {
    id.parse::<TaskId>()
        .map_err(|_| (StatusCode::BAD_REQUEST, format!("Invalid mission ID: {id}")))
}

/// Query parameters for `GET /api/missions`.
#[derive(Debug, Deserialize, Default)]
pub struct MissionsQuery {
    /// Filter: "all" (default), "active", "completed", "failed".
    pub status: Option<String>,
    /// Maximum number of missions to return. Default: 50.
    pub limit: Option<usize>,
}

/// `GET /api/missions` — list missions with optional status filter.
async fn list_missions(
    State(state): State<AppState>,
    Query(query): Query<MissionsQuery>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let engine = build_engine(&state)?;
    let mut missions = engine.list_missions().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to list missions: {e}"),
        )
    })?;

    // Filter by status
    if let Some(ref status) = query.status {
        missions.retain(|m| match status.as_str() {
            "active" => !m.status.is_terminal() && !m.status.is_awaiting_approval(),
            "completed" => matches!(m.status, aivyx_task_engine::TaskStatus::Completed),
            "failed" => matches!(m.status, aivyx_task_engine::TaskStatus::Failed { .. }),
            "cancelled" => matches!(m.status, aivyx_task_engine::TaskStatus::Cancelled),
            "awaiting_approval" => m.status.is_awaiting_approval(),
            _ => true, // "all" or unrecognized
        });
    }

    // Sort: active first, then by updated_at descending
    missions.sort_by(|a, b| {
        let a_active = !a.status.is_terminal();
        let b_active = !b.status.is_terminal();
        b_active
            .cmp(&a_active)
            .then(b.updated_at.cmp(&a.updated_at))
    });

    let limit = query.limit.unwrap_or(50);
    let total = missions.len();
    missions.truncate(limit);

    Ok(Json(serde_json::json!({
        "missions": missions,
        "total": total,
        "returned": missions.len(),
    })))
}

/// `GET /api/missions/:id` — get full mission detail including all steps.
async fn get_mission(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let engine = build_engine(&state)?;
    let task_id = parse_task_id(&id)?;

    let mission = engine
        .get_mission(&task_id)
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to load mission: {e}"),
            )
        })?
        .ok_or((StatusCode::NOT_FOUND, format!("Mission {id} not found")))?;

    Ok(Json(serde_json::json!({ "mission": mission })))
}

/// `POST /api/missions/:id/cancel` — cancel an active mission.
async fn cancel_mission(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let engine = build_engine(&state)?;
    let task_id = parse_task_id(&id)?;

    engine.cancel(&task_id).map_err(|e| {
        (
            StatusCode::CONFLICT,
            format!("Failed to cancel mission: {e}"),
        )
    })?;

    Ok(Json(serde_json::json!({ "status": "cancelled", "id": id })))
}

/// `POST /api/missions/:id/resume` — resume a paused or failed mission.
///
/// Spawns background execution with a 30-minute timeout (matches the
/// agent's `mission_control` tool behaviour).
async fn resume_mission(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    // Validate the mission exists and is resumable before spawning.
    let engine = build_engine(&state)?;
    let task_id = parse_task_id(&id)?;

    let mission = engine
        .get_mission(&task_id)
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to load mission: {e}"),
            )
        })?
        .ok_or((StatusCode::NOT_FOUND, format!("Mission {id} not found")))?;

    if mission.status.is_terminal() {
        return Err((
            StatusCode::CONFLICT,
            format!(
                "Cannot resume mission in terminal state: {:?}",
                mission.status
            ),
        ));
    }

    // Spawn background resume — build a fresh engine for the background task.
    let bg_engine = build_engine(&state)?;
    let bg_task_id = task_id;
    tokio::spawn(async move {
        let timeout = std::time::Duration::from_secs(1800);
        match tokio::time::timeout(timeout, bg_engine.resume(&bg_task_id, None, None)).await {
            Ok(Err(e)) => tracing::error!("Background mission resume failed: {e}"),
            Err(_) => tracing::error!("Background mission resume timed out (30 min)"),
            Ok(Ok(_)) => {}
        }
    });

    Ok(Json(serde_json::json!({
        "status": "resuming",
        "id": id,
        "from_step": mission.next_pending_step().unwrap_or(0),
    })))
}

/// Request body for `POST /api/missions/:id/approve`.
#[derive(Debug, Deserialize)]
pub struct MissionApprovalRequest {
    /// Whether to approve (true) or deny (false).
    pub approved: bool,
    /// Optional message explaining the decision.
    pub message: Option<String>,
}

/// `POST /api/missions/:id/approve` — resolve a mission approval gate.
///
/// If approved, the mission transitions back to `Executing` and can be
/// resumed. If denied, the mission is marked as `Failed`.
async fn approve_mission(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<MissionApprovalRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if let Some(ref msg) = req.message
        && msg.len() > MAX_APPROVAL_MESSAGE_LEN
    {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("message exceeds maximum length of {MAX_APPROVAL_MESSAGE_LEN}"),
        ));
    }

    let engine = build_engine(&state)?;
    let task_id = parse_task_id(&id)?;

    // Find which step is awaiting approval
    let mission = engine
        .get_mission(&task_id)
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to load mission: {e}"),
            )
        })?
        .ok_or((StatusCode::NOT_FOUND, format!("Mission {id} not found")))?;

    let step_index = match &mission.status {
        aivyx_task_engine::TaskStatus::AwaitingApproval { step_index, .. } => *step_index,
        _ => {
            return Err((
                StatusCode::CONFLICT,
                format!(
                    "Mission is not awaiting approval; current status: {:?}",
                    mission.status
                ),
            ));
        }
    };

    engine
        .resolve_approval(&task_id, step_index, req.approved, req.message)
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to resolve approval: {e}"),
            )
        })?;

    let action = if req.approved { "approved" } else { "denied" };

    // If approved, auto-resume so the mission continues executing.
    if req.approved {
        let bg_engine = build_engine(&state)?;
        let bg_task_id = task_id;
        tokio::spawn(async move {
            let timeout = std::time::Duration::from_secs(1800);
            match tokio::time::timeout(timeout, bg_engine.resume(&bg_task_id, None, None)).await {
                Ok(Err(e)) => {
                    tracing::error!("Background mission resume after approval failed: {e}")
                }
                Err(_) => tracing::error!("Background mission resume after approval timed out"),
                Ok(Ok(_)) => {}
            }
        });
    }

    Ok(Json(serde_json::json!({
        "status": action,
        "id": id,
        "step_index": step_index,
        "auto_resumed": req.approved,
    })))
}

/// `DELETE /api/missions/:id` — delete a terminal (completed/failed/cancelled) mission.
async fn delete_mission(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let engine = build_engine(&state)?;
    let task_id = parse_task_id(&id)?;

    engine.delete_mission(&task_id).map_err(|e| {
        (
            StatusCode::CONFLICT,
            format!("Failed to delete mission: {e}"),
        )
    })?;

    Ok(Json(serde_json::json!({ "status": "deleted", "id": id })))
}

// ── Dashboard ──────────────────────────────────────────────────

/// Dashboard hydration data — everything the frontend home screen needs.
#[derive(Serialize)]
struct DashboardResponse {
    agent_name: String,
    persona: String,
    version: &'static str,
    brain_available: bool,
    memory_available: bool,
    missions_available: bool,
    schedules: Vec<ScheduleSummary>,
    goal_count: usize,
    heartbeat_configured: bool,
    active_goals: usize,
    pending_approvals: usize,
    notification_count: usize,
    missions_total: usize,
    missions_active: usize,
    missions_awaiting_approval: usize,
    settings: Option<crate::settings::SettingsSnapshot>,
}

#[derive(Serialize)]
struct ScheduleSummary {
    name: String,
    cron: String,
    enabled: bool,
}

/// `GET /api/dashboard` — hydrate the frontend dashboard.
async fn get_dashboard(State(state): State<AppState>) -> impl IntoResponse {
    let pending = state
        .approvals
        .lock()
        .await
        .iter()
        .filter(|a| a.status == ApprovalStatus::Pending)
        .count();
    let notif_count = state.notification_history.lock().await.len();

    let active_goals = state
        .brain_store
        .as_ref()
        .zip(state.brain_key.as_ref())
        .and_then(|(store, key)| {
            let filter = GoalFilter {
                status: Some(GoalStatus::Active),
                ..Default::default()
            };
            store.list_goals(&filter, key).ok().map(|g| g.len())
        })
        .unwrap_or(0);

    let goal_count = state
        .brain_store
        .as_ref()
        .zip(state.brain_key.as_ref())
        .and_then(|(store, key)| {
            store
                .list_goals(&GoalFilter::default(), key)
                .ok()
                .map(|g| g.len())
        })
        .unwrap_or(0);

    let settings = crate::settings::reload_settings_snapshot(&state.config_path).ok();
    let persona = state
        .pa_config
        .agent
        .as_ref()
        .map(|a| a.persona.clone())
        .unwrap_or_else(|| "assistant".into());

    let schedules: Vec<ScheduleSummary> = state
        .pa_config
        .schedules
        .iter()
        .map(|s| ScheduleSummary {
            name: s.name.clone(),
            cron: s.cron.clone(),
            enabled: s.enabled,
        })
        .collect();

    // Mission summary counts
    let (missions_available, missions_total, missions_active, missions_awaiting_approval) =
        if let Ok(engine) = build_engine(&state) {
            if let Ok(list) = engine.list_missions() {
                let total = list.len();
                let active = list
                    .iter()
                    .filter(|m| !m.status.is_terminal() && !m.status.is_awaiting_approval())
                    .count();
                let awaiting = list
                    .iter()
                    .filter(|m| m.status.is_awaiting_approval())
                    .count();
                (true, total, active, awaiting)
            } else {
                (true, 0, 0, 0)
            }
        } else {
            (false, 0, 0, 0)
        };

    Json(DashboardResponse {
        agent_name: state.agent_name.clone(),
        persona,
        version: env!("CARGO_PKG_VERSION"),
        brain_available: state.brain_store.is_some(),
        memory_available: state.memory_manager.is_some(),
        missions_available,
        schedules,
        goal_count,
        heartbeat_configured: state.pa_config.heartbeat.is_some(),
        active_goals,
        pending_approvals: pending,
        notification_count: notif_count,
        missions_total,
        missions_active,
        missions_awaiting_approval,
        settings,
    })
}

// ── Health ─────────────────────────────────────────────────────

/// Health check response.
#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    version: &'static str,
    brain_available: bool,
    memory_available: bool,
}

/// `GET /api/health` — liveness check with subsystem status.
async fn health(State(state): State<AppState>) -> impl IntoResponse {
    Json(HealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
        brain_available: state.brain_store.is_some(),
        memory_available: state.memory_manager.is_some(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_status_default() {
        let h = HealthStatus::default();
        assert_eq!(h.provider, SubsystemHealth::NotConfigured);
        assert_eq!(h.email, SubsystemHealth::NotConfigured);
        assert_eq!(h.config, SubsystemHealth::NotConfigured);
        assert_eq!(h.disk, SubsystemHealth::NotConfigured);
    }

    #[test]
    fn subsystem_health_labels() {
        assert_eq!(SubsystemHealth::Healthy.label(), "healthy");
        assert_eq!(SubsystemHealth::NotConfigured.label(), "n/a");
        assert_eq!(SubsystemHealth::Degraded("oops".into()).label(), "degraded");
    }

    #[test]
    fn subsystem_health_is_healthy() {
        assert!(SubsystemHealth::Healthy.is_healthy());
        assert!(SubsystemHealth::NotConfigured.is_healthy());
        assert!(!SubsystemHealth::Degraded("error".into()).is_healthy());
    }

    #[test]
    fn disk_check_current_dir() {
        // Running on a real filesystem — should be healthy (we need >100MB to run tests)
        let health = check_disk_space(std::path::Path::new("/tmp"));
        assert!(health.is_healthy(), "Expected /tmp to have >100MB free");
    }
}
