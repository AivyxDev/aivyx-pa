//! App state and view routing.
//!
//! Central state struct that tracks the current view, navigation index,
//! and per-view cached data fetched from the PA backend via `AppState`.

use std::sync::Arc;

use aivyx_audit::AuditEntry;
use aivyx_brain::{Goal, GoalFilter, GoalStatus, Priority};
use aivyx_core::{AgentEvent, GoalId};
use aivyx_loop::Notification;
use aivyx_pa::api::{AppState, ApprovalItem, ApprovalStatus};
use aivyx_pa::settings::{IntegrationKind, SettingsSnapshot};
use aivyx_task_engine::{Mission, TaskMetadata, TaskStatus as MissionStatus};
use crossterm::event::{KeyCode, KeyEvent};
use tokio::sync::mpsc;

// ── Settings Popup (modal edit state) ─────────────────────────

/// Modal popup state for settings editing.
pub enum SettingsPopup {
    /// Single-line text input (model name, agent name, base_url, rate limit, etc.)
    TextInput {
        title: String,
        value: String,
        section: &'static str,
        key: &'static str,
        kind: InputKind,
    },
    /// Multi-line text input (soul editing).
    MultiLineInput {
        title: String,
        lines: Vec<String>,
        cursor_row: usize,
        cursor_col: usize,
        section: &'static str,
        key: &'static str,
    },
    /// Confirmation dialog.
    Confirm {
        message: String,
        action: ConfirmAction,
    },
    /// Skill list manager: view/add/remove skills.
    SkillManager {
        input: String,
        selected: usize,
        skills: Vec<String>,
    },
    /// Guided integration setup with labeled fields.
    IntegrationSetup {
        kind: IntegrationKind,
        fields: Vec<IntegrationField>,
        focused: usize,
        is_configured: bool,
    },
    /// Schedule create/edit wizard.
    ScheduleEditor {
        /// `None` = creating new, `Some(name)` = editing existing.
        editing: Option<String>,
        name: String,
        cron_builder: CronBuilder,
        prompt: Vec<String>,
        prompt_cursor_row: usize,
        prompt_cursor_col: usize,
        notify: bool,
        /// Which field has focus: 0=name, 1=cron, 2=prompt, 3=notify.
        focused: usize,
        /// Validation error displayed at the bottom.
        error: Option<String>,
    },
}

/// A single field in an integration setup popup.
pub struct IntegrationField {
    pub label: String,
    pub toml_key: String,
    pub value: String,
    pub is_secret: bool,
    /// Key name for EncryptedStore (e.g. "EMAIL_PASSWORD"). Empty if not a secret.
    pub store_key: &'static str,
}

/// Kind of value accepted in a TextInput popup.
pub enum InputKind {
    String,
    UInt,
    Float,
}

/// Action to execute when a Confirm dialog is accepted.
pub enum ConfirmAction {
    RemoveSkill(usize),
    RemoveIntegration(IntegrationKind),
    RemoveSchedule(String),
}

// ── Cron Builder ─────────────────────────────────────────────

pub const CRON_MODES: &[&str] = &[
    "Every N Min",
    "Hourly",
    "Daily",
    "Weekdays",
    "Weekly",
    "Monthly",
    "Custom",
];

pub const CRON_INTERVALS: &[u8] = &[5, 10, 15, 20, 30, 45];

pub const WEEKDAY_NAMES: &[&str] = &[
    "Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday",
];

/// Interactive schedule builder — the user selects frequency, time, and day
/// using arrow keys. The builder generates the cron expression automatically.
#[derive(Clone, Debug)]
pub struct CronBuilder {
    /// 0=EveryNMin, 1=Hourly, 2=Daily, 3=Weekdays, 4=Weekly, 5=Monthly, 6=Custom
    pub mode: usize,
    pub hour: u8,
    pub minute: u8,
    /// 0=Sun .. 6=Sat
    pub weekday: u8,
    /// 1-28
    pub month_day: u8,
    /// Index into CRON_INTERVALS
    pub interval_idx: usize,
    /// Raw cron for Custom mode
    pub custom: String,
    /// Which sub-field has focus within the schedule section.
    ///
    /// Layout per mode:
    ///   EveryNMin: 0=mode, 1=interval
    ///   Hourly:    0=mode, 1=minute
    ///   Daily:     0=mode, 1=hour, 2=minute
    ///   Weekdays:  0=mode, 1=hour, 2=minute
    ///   Weekly:    0=mode, 1=weekday, 2=hour, 3=minute
    ///   Monthly:   0=mode, 1=day, 2=hour, 3=minute
    ///   Custom:    0=mode, 1=input
    pub sub_focus: usize,
}

impl CronBuilder {
    /// Number of sub-fields for the current mode.
    pub fn sub_field_count(&self) -> usize {
        match self.mode {
            0 => 2, // EveryNMin: mode + interval
            1 => 2, // Hourly: mode + minute
            2 | 3 => 3, // Daily/Weekdays: mode + hour + minute
            4 | 5 => 4, // Weekly/Monthly: mode + day + hour + minute
            6 => 2, // Custom: mode + input
            _ => 2,
        }
    }

    /// Generate the cron expression from the builder state.
    pub fn build_cron(&self) -> String {
        match self.mode {
            0 => {
                let interval = CRON_INTERVALS[self.interval_idx];
                format!("*/{interval} * * * *")
            }
            1 => format!("{} * * * *", self.minute),
            2 => format!("{} {} * * *", self.minute, self.hour),
            3 => format!("{} {} * * 1-5", self.minute, self.hour),
            4 => format!("{} {} * * {}", self.minute, self.hour, self.weekday),
            5 => format!("{} {} {} * *", self.minute, self.hour, self.month_day),
            6 => self.custom.clone(),
            _ => self.custom.clone(),
        }
    }

    /// Parse a cron expression into builder state. Falls back to Custom mode
    /// for expressions that don't match a known pattern.
    pub fn from_cron(expr: &str) -> Self {
        let parts: Vec<&str> = expr.split_whitespace().collect();
        if parts.len() != 5 {
            return Self::custom(expr);
        }

        // */N * * * *  →  EveryNMin
        if parts[0].starts_with("*/")
            && parts[1] == "*"
            && parts[2] == "*"
            && parts[3] == "*"
            && parts[4] == "*"
        {
            if let Ok(n) = parts[0][2..].parse::<u8>() {
                if let Some(idx) = CRON_INTERVALS.iter().position(|&v| v == n) {
                    return Self {
                        mode: 0,
                        interval_idx: idx,
                        ..Self::default()
                    };
                }
            }
        }

        // M * * * *  →  Hourly
        if parts[1] == "*" && parts[2] == "*" && parts[3] == "*" && parts[4] == "*" {
            if let Ok(m) = parts[0].parse::<u8>() {
                return Self {
                    mode: 1,
                    minute: m,
                    ..Self::default()
                };
            }
        }

        // M H * * 1-5  →  Weekdays
        if parts[2] == "*" && parts[3] == "*" && parts[4] == "1-5" {
            if let (Ok(m), Ok(h)) = (parts[0].parse::<u8>(), parts[1].parse::<u8>()) {
                return Self {
                    mode: 3,
                    minute: m,
                    hour: h,
                    ..Self::default()
                };
            }
        }

        // M H * * D  →  Weekly (single weekday)
        if parts[2] == "*" && parts[3] == "*" && parts[4] != "*" {
            if let (Ok(m), Ok(h), Ok(d)) = (
                parts[0].parse::<u8>(),
                parts[1].parse::<u8>(),
                parts[4].parse::<u8>(),
            ) {
                if d <= 6 {
                    return Self {
                        mode: 4,
                        minute: m,
                        hour: h,
                        weekday: d,
                        ..Self::default()
                    };
                }
            }
        }

        // M H D * *  →  Monthly
        if parts[3] == "*" && parts[4] == "*" && parts[2] != "*" {
            if let (Ok(m), Ok(h), Ok(d)) = (
                parts[0].parse::<u8>(),
                parts[1].parse::<u8>(),
                parts[2].parse::<u8>(),
            ) {
                if (1..=28).contains(&d) {
                    return Self {
                        mode: 5,
                        minute: m,
                        hour: h,
                        month_day: d,
                        ..Self::default()
                    };
                }
            }
        }

        // M H * * *  →  Daily
        if parts[2] == "*" && parts[3] == "*" && parts[4] == "*" {
            if let (Ok(m), Ok(h)) = (parts[0].parse::<u8>(), parts[1].parse::<u8>()) {
                return Self {
                    mode: 2,
                    minute: m,
                    hour: h,
                    ..Self::default()
                };
            }
        }

        Self::custom(expr)
    }

    fn custom(expr: &str) -> Self {
        Self {
            mode: 6,
            custom: expr.to_string(),
            ..Self::default()
        }
    }
}

impl Default for CronBuilder {
    fn default() -> Self {
        Self {
            mode: 2, // Daily
            hour: 9,
            minute: 0,
            weekday: 1, // Monday
            month_day: 1,
            interval_idx: 2, // 15 min
            custom: String::new(),
            sub_focus: 0,
        }
    }
}

// ── Goal Popup (modal edit state) ─────────────────────────────

pub const PRIORITIES: [&str; 5] = ["Background", "Low", "Medium", "High", "Critical"];

/// Modal popup state for goal creation/editing.
pub enum GoalPopup {
    /// Create a new goal.
    Create {
        description: String,
        criteria: String,
        priority: usize,      // index into PRIORITIES
        focused_field: usize, // 0=desc, 1=criteria, 2=priority
    },
    /// Edit an existing goal.
    Edit {
        goal_id: GoalId,
        description: String,
        criteria: String,
        priority: usize,
        deadline: String,     // YYYY-MM-DD or empty
        focused_field: usize, // 0=desc, 1=criteria, 2=priority, 3=deadline
    },
    /// Confirm a goal action.
    Confirm { message: String, action: GoalAction },
}

pub enum GoalAction {
    Complete(GoalId),
    Abandon(GoalId),
}

// ── Chat Popup (modal overlays in Chat view) ────────────────

/// A saved session entry for the conversation history popup.
#[derive(Debug, Clone)]
pub struct SessionEntry {
    pub id: String,
    pub title: String,
    pub turn_count: usize,
    pub updated_at: String,
    pub created_at: String,
}

/// Sub-mode for the conversation history popup.
#[derive(Debug)]
pub enum HistoryMode {
    /// Browsing the session list.
    List,
    /// Renaming the selected session (inline text input).
    Rename { input: String },
    /// Confirming deletion of the selected session.
    ConfirmDelete,
    /// Previewing messages in the selected session.
    Preview {
        lines: Vec<(String, String)>,
        scroll: usize,
    },
}

/// Modal popup state for the Chat view.
#[derive(Debug)]
pub enum ChatPopup {
    /// Conversation history browser with CRUD operations.
    ConversationHistory {
        sessions: Vec<SessionEntry>,
        selected: usize,
        scroll_offset: usize,
        mode: HistoryMode,
    },
    /// System prompt preview (scrollable).
    SystemPrompt { lines: Vec<String>, scroll: usize },
    /// Export confirmation (shows path after export).
    ExportDone { path: String },
    /// Snapshot / branch manager.
    BranchManager {
        snapshots: Vec<SnapshotEntry>,
        selected: usize,
        /// Text input for new snapshot label.
        label_input: String,
        /// True when the label input field is focused.
        creating: bool,
    },
}

/// The 10 authenticated views (matching the GUI sidebar).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Home,
    Chat,
    Activity,
    Goals,
    Approvals,
    Missions,
    Audit,
    Memory,
    Help,
    Settings,
}

impl View {
    pub const ALL: [View; 10] = [
        View::Home,
        View::Chat,
        View::Activity,
        View::Goals,
        View::Approvals,
        View::Missions,
        View::Audit,
        View::Memory,
        View::Help,
        View::Settings,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            View::Home => "Home",
            View::Chat => "Chat",
            View::Activity => "Activity",
            View::Goals => "Goals",
            View::Approvals => "Approvals",
            View::Missions => "Missions",
            View::Audit => "Audit",
            View::Memory => "Memory",
            View::Help => "Help",
            View::Settings => "Settings",
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            View::Home => "◆",
            View::Chat => "◇",
            View::Activity => "⚡",
            View::Goals => "◎",
            View::Approvals => "⊕",
            View::Missions => "▣",
            View::Audit => "◈",
            View::Memory => "◉",
            View::Help => "?",
            View::Settings => "⚙",
        }
    }

    /// Group index for visual separation in sidebar.
    /// 0 = core (Home..Approvals), 1 = ops (Missions..Audit), 2 = sys (Help, Settings)
    pub fn group(&self) -> u8 {
        match self {
            View::Home | View::Chat | View::Activity | View::Goals | View::Approvals => 0,
            View::Missions | View::Memory | View::Audit => 1,
            View::Help | View::Settings => 2,
        }
    }
}

/// Which panel has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    Content,
}

// ── Chat message for display ──────────────────────────────────

/// A rendered chat message in the conversation view.
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    pub timestamp: String,
}

// ── Memory display types ───────────────────────────────────���──

// ── Agent event display types ─────────────────────────────────

/// A completed tool call for display in the chat and telemetry views.
#[derive(Debug, Clone)]
pub struct ToolStatusEntry {
    pub tool_name: String,
    pub tool_call_id: String,
    /// None = still running; Some(ms) = completed.
    pub duration_ms: Option<u64>,
    /// None = success or still running; Some(msg) = failed.
    pub error: Option<String>,
    /// True if the tool call was denied (auth/capability).
    pub denied: bool,
}

// ── Agent monitor display types ──────────────────────────────

/// Live status of a specialist agent during team delegation.
#[derive(Debug, Clone)]
pub struct AgentStatus {
    /// Specialist name (e.g. "researcher", "coder").
    pub name: String,
    /// Current state: "idle", "running", "completed", "error".
    pub state: String,
    /// The task currently delegated to this agent, if any.
    pub current_task: Option<String>,
    /// When the current delegation started.
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Duration of the last completed delegation in milliseconds.
    pub last_duration_ms: Option<u64>,
    /// Recent tool calls by this specialist (most recent first, capped).
    pub tool_log: Vec<SpecialistToolEntry>,
}

/// A tool call made by a specialist during delegation.
#[derive(Debug, Clone)]
pub struct SpecialistToolEntry {
    pub tool_name: String,
    /// "started", "completed", "failed", "denied".
    pub status: String,
    pub duration_ms: Option<u64>,
    pub error: Option<String>,
}

impl AgentStatus {
    fn new(name: String) -> Self {
        Self {
            name,
            state: "idle".into(),
            current_task: None,
            started_at: None,
            last_duration_ms: None,
            tool_log: Vec::new(),
        }
    }
}

/// Maximum tool call entries kept per specialist in the agent monitor.
const MAX_SPECIALIST_TOOL_LOG: usize = 20;

/// A conversation snapshot entry for the branching UI.
#[derive(Debug, Clone)]
pub struct SnapshotEntry {
    pub message_index: usize,
    pub label: Option<String>,
    pub created_at: String,
}

/// A memory entry for the memory view.
#[derive(Debug, Clone)]
pub struct MemoryEntry {
    pub id: String,
    pub content: String,
    pub tags: Vec<String>,
    pub updated_at: String,
}

/// Application state — view model backed by the live PA backend.
pub struct App {
    // ── Core ───────────────────────────────────────────────────
    pub running: bool,
    pub view: View,
    pub nav_index: usize,
    pub focus: Focus,
    pub frame_count: u64,

    // ── Backend handle ─────────────────────────────────────────
    pub state: Option<Arc<AppState>>,

    // ── Dashboard / status (cached from AppState) ──────────────
    pub agent_name: String,
    pub provider_label: String,
    pub model_name: String,
    pub autonomy_tier: String,
    pub memory_count: u64,
    pub goal_count: u64,
    pub active_goals: usize,
    pub pending_approvals: usize,
    pub heartbeat_interval: u32,
    pub version: String,

    // ── Chat state ─────────────────────────────────────────────
    pub chat_input: String,
    pub chat_input_cursor: usize,
    pub chat_input_scroll: u16,
    pub chat_messages: Vec<ChatMessage>,
    pub chat_streaming: bool,
    pub chat_scroll: usize,
    /// Cancellation handle for the in-flight `turn_stream` call.
    ///
    /// Populated while a chat turn is streaming, cleared when it
    /// finishes or is cancelled. Any code path that needs to lock
    /// `state.agent` (load a session, branch a snapshot, clear the
    /// conversation) calls `cancel_chat_stream()` first so the running
    /// turn releases the agent mutex promptly rather than making the
    /// competing action wait up to 5 minutes for the stream to finish.
    pub chat_cancel: Option<tokio_util::sync::CancellationToken>,
    /// Channel for receiving streamed tokens from the agent.
    pub chat_token_rx: Option<mpsc::Receiver<String>>,
    /// Current session ID (None = unsaved ephemeral chat).
    pub chat_session_id: Option<String>,
    /// Token usage counters (refreshed from agent).
    pub chat_input_tokens: u64,
    pub chat_output_tokens: u64,
    pub chat_context_window: u32,
    pub chat_cost_usd: f64,
    /// Chat popup overlay (session list, system prompt, export confirmation, branches).
    pub chat_popup: Option<ChatPopup>,
    /// Channel for receiving agent lifecycle events.
    pub chat_event_rx: Option<mpsc::Receiver<AgentEvent>>,
    /// Currently executing tool calls in this turn (for live status display).
    pub chat_tool_status: Vec<ToolStatusEntry>,
    /// True while conversation compaction is in progress.
    pub chat_compacting: bool,
    /// Tool calls completed in the most recent turn (for telemetry sidebar).
    pub turn_tool_log: Vec<ToolStatusEntry>,
    /// Total tool calls in the current/last turn (from TurnCompleted event).
    pub turn_tool_calls: u32,
    /// Total tokens in the current/last turn (from TurnCompleted event).
    pub turn_total_tokens: u64,

    // ── Goals state (cached from BrainStore) ───────────────────
    pub goals: Vec<Goal>,
    pub goal_filter: usize, // 0=all, 1=active, 2=completed, 3=abandoned
    pub goal_selected: usize,
    pub goal_popup: Option<GoalPopup>,

    // ── Approvals state (cached from AppState) ─────────────────
    pub approvals: Vec<ApprovalItem>,
    pub approval_selected: usize,
    /// Whether the full-body detail pane is open for the selected approval.
    pub approval_detail_open: bool,
    /// Scroll offset inside the detail pane (lines scrolled).
    pub approval_detail_scroll: u16,

    // ── Audit state (cached from AuditLog) ─────────────────────
    pub audit_entries: Vec<AuditEntry>,
    pub audit_filter: usize, // 0=all, 1=tool, 2=heartbeat, 3=security
    pub audit_selected: usize,
    pub audit_chain_valid: Option<bool>,

    // ── Activity state (notifications from loop) ───────────────
    pub notifications: Vec<Notification>,
    pub activity_selected: usize,
    pub activity_filter: usize, // 0=all, 1=agents, 2=schedule, 3=heartbeat, 4=triage

    // ── Agent monitor state ──────────────────────────────────
    /// Live status of specialist agents during team delegations.
    pub agent_statuses: Vec<AgentStatus>,
    /// Selected agent index in the Agents filter tab.
    pub agent_monitor_selected: usize,

    // ── Missions state (cached from TaskEngine) ─────────────────
    pub missions: Vec<TaskMetadata>,
    pub mission_selected: usize,
    pub mission_filter: usize,
    pub mission_detail: Option<Mission>,

    // ── Voice loop state ─────────────────────────────────────────
    pub voice_recording: bool,
    pub voice_transcribing: bool,
    pub voice_process: Option<std::process::Child>,
    pub voice_transcript_rx: Option<tokio::sync::mpsc::UnboundedReceiver<String>>,
    pub voice_transcript_tx: Option<tokio::sync::mpsc::UnboundedSender<String>>,

    // ── Memory state ───────────────────────────────────────────
    pub memories: Vec<MemoryEntry>,
    pub memory_selected: usize,
    pub memory_total: usize,

    // ── Help state ──────────────────────────────────────────────
    pub help_scroll: usize,

    // ── Settings state (cached SettingsSnapshot) ───────────────
    pub settings: Option<SettingsSnapshot>,
    pub settings_error: Option<String>,
    pub settings_card_index: usize, // 0-9: which card is focused (see views::settings)
    pub settings_item_index: usize, // row within the focused card
    pub settings_popup: Option<SettingsPopup>,

    // ── Tools / Extensions ──────────────────────────────────────
    pub tool_count: usize,

    // ── Health status ───────────────────────────────────────────
    pub health_provider: String, // "healthy" | "degraded" | "n/a"
    pub health_email: String,
    pub health_config: String,
    pub health_disk: String,
    pub health_provider_detail: Option<String>,
    pub health_email_detail: Option<String>,

    // ── Data refresh tracking ──────────────────────────────────
    pub last_refresh: std::time::Instant,

    // ── Chat persistence background tasks ──────────────────────
    /// Tracks in-flight chat-session save tasks spawned from the
    /// token-polling loop. The poll path is tick-driven and cannot
    /// `.await`, so saves must be spawned — but we still need to
    /// drain pending writes on shutdown to avoid losing the final
    /// turn if the user quits immediately after a streaming reply.
    ///
    /// Call [`App::drain_chat_saves`] after the main event loop exits.
    pub chat_save_tasks: tokio::task::JoinSet<()>,
}

impl App {
    /// Create a new app wired to the live PA backend.
    pub fn new_live(state: Arc<AppState>) -> Self {
        let agent_name = state.agent_name.clone();
        let (settings, settings_error) =
            match aivyx_pa::settings::reload_settings_snapshot(&state.config_path) {
                Ok(s) => (Some(s), None),
                Err(e) => {
                    tracing::warn!("Settings load failed: {e}");
                    (None, Some(e))
                }
            };

        let (provider_label, model_name, autonomy_tier, heartbeat_interval) =
            if let Some(ref s) = settings {
                (
                    s.provider_label.clone(),
                    s.model_name.clone(),
                    s.autonomy_tier.clone(),
                    s.heartbeat_interval,
                )
            } else {
                ("Unknown".into(), "unknown".into(), "trust".into(), 30)
            };

        Self {
            running: true,
            view: View::Home,
            nav_index: 0,
            focus: Focus::Sidebar,
            frame_count: 0,

            state: Some(state),

            agent_name,
            provider_label,
            model_name,
            autonomy_tier,
            memory_count: 0,
            goal_count: 0,
            active_goals: 0,
            pending_approvals: 0,
            heartbeat_interval,
            version: env!("CARGO_PKG_VERSION").into(),

            chat_input: String::new(),
            chat_input_cursor: 0,
            chat_input_scroll: 0,
            chat_messages: Vec::new(),
            chat_streaming: false,
            chat_cancel: None,
            chat_scroll: 0,
            chat_token_rx: None,
            chat_session_id: None,
            chat_input_tokens: 0,
            chat_output_tokens: 0,
            chat_context_window: 0,
            chat_cost_usd: 0.0,
            chat_popup: None,
            chat_event_rx: None,
            chat_tool_status: Vec::new(),
            chat_compacting: false,
            turn_tool_log: Vec::new(),
            turn_tool_calls: 0,
            turn_total_tokens: 0,

            goals: Vec::new(),
            goal_filter: 0,
            goal_selected: 0,
            goal_popup: None,

            approvals: Vec::new(),
            approval_selected: 0,
            approval_detail_open: false,
            approval_detail_scroll: 0,

            audit_entries: Vec::new(),
            audit_filter: 0,
            audit_selected: 0,
            audit_chain_valid: None,

            notifications: Vec::new(),
            activity_selected: 0,
            activity_filter: 0,
            agent_statuses: Vec::new(),
            agent_monitor_selected: 0,

            missions: Vec::new(),
            mission_selected: 0,
            mission_filter: 0,
            mission_detail: None,

            voice_recording: false,
            voice_transcribing: false,
            voice_process: None,
            voice_transcript_rx: None,
            voice_transcript_tx: None,

            memories: Vec::new(),
            memory_selected: 0,
            memory_total: 0,

            help_scroll: 0,

            settings,
            settings_error,
            settings_card_index: 0,
            settings_item_index: 0,
            settings_popup: None,

            tool_count: 0,

            health_provider: "...".into(),
            health_email: "...".into(),
            health_config: "...".into(),
            health_disk: "...".into(),
            health_provider_detail: None,
            health_email_detail: None,

            last_refresh: std::time::Instant::now(),

            chat_save_tasks: tokio::task::JoinSet::new(),
        }
    }

    /// Await all in-flight chat-session save tasks.
    ///
    /// Call this after the main event loop exits (i.e. the user has
    /// requested shutdown) so pending encrypted writes finish before
    /// the process exits. Any save that panicked or errored is logged
    /// but does not prevent shutdown — we've already told the user we
    /// are quitting.
    pub async fn drain_chat_saves(&mut self) {
        while let Some(res) = self.chat_save_tasks.join_next().await {
            if let Err(e) = res {
                tracing::warn!("chat save task failed during shutdown drain: {e}");
            }
        }
    }

    /// Switch to a view by index (0-8).
    pub fn go_to_view(&mut self, idx: usize) {
        if let Some(&view) = View::ALL.get(idx) {
            self.view = view;
            self.nav_index = idx;
        }
    }

    /// Navigate to a specific view by value rather than index.
    ///
    /// Prefer this over `go_to_view(idx)` + a `// View::Foo` comment —
    /// the enum value is checked at compile time, so reordering
    /// `View::ALL` can't silently break call sites.
    pub fn go_to(&mut self, view: View) {
        if let Some(idx) = View::ALL.iter().position(|v| *v == view) {
            self.view = view;
            self.nav_index = idx;
        }
    }

    /// Navigate sidebar up.
    pub fn nav_up(&mut self) {
        if self.nav_index > 0 {
            self.nav_index -= 1;
            self.view = View::ALL[self.nav_index];
        }
    }

    /// Navigate sidebar down.
    pub fn nav_down(&mut self) {
        if self.nav_index < View::ALL.len() - 1 {
            self.nav_index += 1;
            self.view = View::ALL[self.nav_index];
        }
    }

    /// Filtered goals for the current filter.
    pub fn filtered_goals(&self) -> Vec<&Goal> {
        let filter_status = match self.goal_filter {
            1 => Some(GoalStatus::Active),
            2 => Some(GoalStatus::Completed),
            3 => Some(GoalStatus::Abandoned),
            _ => None,
        };
        self.goals
            .iter()
            .filter(|g| filter_status.is_none_or(|s| g.status == s))
            .collect()
    }

    /// Filtered audit events for the current filter.
    pub fn filtered_audit(&self) -> Vec<&AuditEntry> {
        let filter = match self.audit_filter {
            1 => Some("tool"),
            2 => Some("heartbeat"),
            3 => Some("security"),
            _ => None,
        };
        self.audit_entries
            .iter()
            .filter(|e| {
                if let Some(f) = filter {
                    audit_event_type(&e.event).contains(f)
                } else {
                    true
                }
            })
            .collect()
    }

    /// Filtered notifications for the current activity filter.
    ///
    /// Filter indices: 0=all, 1=agents (handled separately in render),
    /// 2=schedule, 3=heartbeat, 4=triage.
    pub fn filtered_notifications(&self) -> Vec<&Notification> {
        self.notifications
            .iter()
            .filter(|n| match self.activity_filter {
                1 => n.source == "agent", // Agents tab: only agent notifications
                2 => n.source == "schedule" || n.source == "briefing",
                3 => n.source.contains("heartbeat"),
                4 => n.source == "triage" || n.source == "email",
                _ => true,
            })
            .collect()
    }

    /// Filtered missions for the current filter.
    pub fn filtered_missions(&self) -> Vec<&TaskMetadata> {
        self.missions
            .iter()
            .filter(|m| match self.mission_filter {
                1 => !m.status.is_terminal(), // active
                2 => matches!(m.status, MissionStatus::Completed),
                3 => matches!(
                    m.status,
                    MissionStatus::Failed { .. } | MissionStatus::Cancelled
                ),
                _ => true,
            })
            .collect()
    }

    /// Load full mission detail for the currently selected mission.
    pub fn load_mission_detail(&mut self) {
        let Some(ref state) = self.state else { return };
        let Some(ref ctx) = state.mission_ctx else {
            self.mission_detail = None;
            return;
        };
        let missions = self.filtered_missions();
        let Some(meta) = missions.get(self.mission_selected) else {
            self.mission_detail = None;
            return;
        };
        let task_id = meta.id;
        if let Ok(engine) = aivyx_pa::agent::build_task_engine(ctx) {
            self.mission_detail = engine.get_mission(&task_id).ok().flatten();
        }
    }

    /// Cancel the currently selected mission.
    pub fn cancel_mission(&mut self) {
        let Some(ref state) = self.state else { return };
        let Some(ref ctx) = state.mission_ctx else {
            return;
        };
        let missions = self.filtered_missions();
        let Some(meta) = missions.get(self.mission_selected) else {
            return;
        };
        if meta.status.is_terminal() {
            return;
        }
        if let Ok(engine) = aivyx_pa::agent::build_task_engine(ctx) {
            let _ = engine.cancel(&meta.id);
        }
    }

    /// Approve the currently selected mission's approval gate.
    pub fn approve_mission(&mut self) {
        let Some(ref state) = self.state else { return };
        let Some(ref ctx) = state.mission_ctx else {
            return;
        };
        let missions = self.filtered_missions();
        let Some(meta) = missions.get(self.mission_selected) else {
            return;
        };
        if !meta.status.is_awaiting_approval() {
            return;
        }

        if let Ok(engine) = aivyx_pa::agent::build_task_engine(ctx) {
            // Extract the step index from the mission status
            if let Ok(Some(mission)) = engine.get_mission(&meta.id)
                && let aivyx_task_engine::TaskStatus::AwaitingApproval { step_index, .. } =
                    &mission.status
            {
                let step_idx = *step_index;
                let _ = engine.resolve_approval(
                    &meta.id,
                    step_idx,
                    true,
                    Some("approved via TUI".into()),
                );
                // Auto-resume after approval
                self.resume_mission();
            }
        }
    }

    /// Deny the currently selected mission's approval gate.
    pub fn deny_mission(&mut self) {
        let Some(ref state) = self.state else { return };
        let Some(ref ctx) = state.mission_ctx else {
            return;
        };
        let missions = self.filtered_missions();
        let Some(meta) = missions.get(self.mission_selected) else {
            return;
        };
        if !meta.status.is_awaiting_approval() {
            return;
        }

        if let Ok(engine) = aivyx_pa::agent::build_task_engine(ctx)
            && let Ok(Some(mission)) = engine.get_mission(&meta.id)
            && let aivyx_task_engine::TaskStatus::AwaitingApproval { step_index, .. } =
                &mission.status
        {
            let _ = engine.resolve_approval(
                &meta.id,
                *step_index,
                false,
                Some("denied via TUI".into()),
            );
        }
    }

    /// Resume the currently selected mission (if resumable).
    pub fn resume_mission(&mut self) {
        let Some(ref state) = self.state else { return };
        let Some(ref ctx) = state.mission_ctx else {
            return;
        };
        let missions = self.filtered_missions();
        let Some(meta) = missions.get(self.mission_selected) else {
            return;
        };
        // Can only resume non-terminal, non-planning missions
        if meta.status.is_terminal() {
            return;
        }
        if matches!(meta.status, aivyx_task_engine::TaskStatus::Planning) {
            return;
        }

        if let Ok(bg_engine) = aivyx_pa::agent::build_task_engine(ctx) {
            let task_id = meta.id;
            tokio::spawn(async move {
                let timeout = std::time::Duration::from_secs(1800);
                match tokio::time::timeout(timeout, bg_engine.resume(&task_id, None, None)).await {
                    Ok(Err(e)) => tracing::error!("TUI mission resume failed: {e}"),
                    Err(_) => tracing::error!("TUI mission resume timed out (30 min)"),
                    Ok(Ok(_)) => {}
                }
            });
        }
    }

    /// Toggle the enabled state of the selected schedule.
    pub fn toggle_schedule(&mut self) {
        let Some(ref state) = self.state else { return };
        let Some(ref settings) = self.settings else {
            return;
        };
        let idx = self.settings_item_index;
        if idx >= settings.schedules.len() {
            return;
        }

        let (ref name, _, enabled, _) = settings.schedules[idx];
        let _ = aivyx_pa::settings::toggle_schedule_enabled(&state.config_path, name, !enabled);
        match aivyx_pa::settings::reload_settings_snapshot(&state.config_path) {
            Ok(s) => {
                self.settings = Some(s);
                self.settings_error = None;
            }
            Err(e) => {
                self.settings = None;
                self.settings_error = Some(e);
            }
        }
    }

    /// Refresh cached data from the backend. Called periodically from the
    /// event loop (every ~2 seconds) to keep views up-to-date.
    pub async fn refresh_data(&mut self) {
        let Some(ref state) = self.state else { return };

        // Chat context usage + tool count (from agent).
        // Use try_lock to avoid blocking the render loop while a chat
        // turn holds the agent mutex — stats simply stay stale until the
        // lock is free again.
        if let Ok(agent) = state.agent.try_lock() {
            self.chat_input_tokens = agent.total_input_tokens();
            self.chat_output_tokens = agent.total_output_tokens();
            self.chat_cost_usd = agent.current_cost_usd();
            self.chat_context_window = agent.conversation().len() as u32;
            self.tool_count = agent.tool_count();
        }

        // Goals
        if let (Some(brain_store), Some(brain_key)) = (&state.brain_store, &state.brain_key)
            && let Ok(goals) = brain_store.list_goals(&GoalFilter::default(), brain_key)
        {
            self.goal_count = goals.len() as u64;
            self.active_goals = goals
                .iter()
                .filter(|g| g.status == GoalStatus::Active)
                .count();
            self.goals = goals;
        }

        // Approvals
        {
            let approvals = state.approvals.lock().await;
            self.pending_approvals = approvals
                .iter()
                .filter(|a| a.status == ApprovalStatus::Pending)
                .count();
            self.approvals = approvals.clone();
        }

        // Audit
        if let Ok(entries) = state.audit_log.recent(100) {
            self.audit_chain_valid = Some(state.audit_log.verify().is_ok());
            self.audit_entries = entries;
        }

        // Notifications / activity
        {
            let history = state.notification_history.lock().await;
            self.notifications = history.iter().rev().take(50).cloned().collect();
        }

        // Missions
        if let Some(ref ctx) = state.mission_ctx
            && let Ok(engine) = aivyx_pa::agent::build_task_engine(ctx)
            && let Ok(mut list) = engine.list_missions()
        {
            // Sort: active first, then by updated_at desc
            list.sort_by(|a, b| {
                let a_active = !a.status.is_terminal();
                let b_active = !b.status.is_terminal();
                b_active
                    .cmp(&a_active)
                    .then(b.updated_at.cmp(&a.updated_at))
            });
            // Include missions awaiting approval in the approval count
            let mission_approvals = list
                .iter()
                .filter(|m| m.status.is_awaiting_approval())
                .count();
            self.pending_approvals += mission_approvals;
            self.missions = list;
        }

        // Memory count — use `try_lock` so a contended memory manager
        // (e.g. mid-heartbeat consolidation) doesn't stall the render
        // task. If the lock is held, we simply skip this refresh cycle
        // and keep whatever data we loaded last time; `refresh_data`
        // runs every 2s, so the UI catches up on the next tick.
        if let Some(ref mm) = state.memory_manager
            && let Ok(mm_guard) = mm.try_lock()
            && let Ok(ids) = mm_guard.list_memories()
        {
            self.memory_count = ids.len() as u64;
            self.memory_total = ids.len();

            let mut entries = Vec::new();
            for id in ids.iter().take(100) {
                if let Ok(Some(entry)) = mm_guard.load_memory(id) {
                    entries.push(MemoryEntry {
                        id: format!("{id}"),
                        content: entry.content.clone(),
                        tags: entry.tags.clone(),
                        updated_at: entry.updated_at.to_rfc3339(),
                    });
                }
            }
            self.memories = entries;
        }

        // Settings
        match aivyx_pa::settings::reload_settings_snapshot(&state.config_path) {
            Ok(s) => {
                self.settings = Some(s);
                self.settings_error = None;
            }
            Err(e) => {
                self.settings = None;
                self.settings_error = Some(e);
            }
        }

        // Health status (non-blocking read — written by background task)
        if let Ok(h) = state.health.try_read() {
            self.health_provider = h.provider.label().into();
            self.health_email = h.email.label().into();
            self.health_config = h.config.label().into();
            self.health_disk = h.disk.label().into();
            self.health_provider_detail = match &h.provider {
                aivyx_pa::api::SubsystemHealth::Degraded(msg) => Some(msg.clone()),
                _ => None,
            };
            self.health_email_detail = match &h.email {
                aivyx_pa::api::SubsystemHealth::Degraded(msg) => Some(msg.clone()),
                _ => None,
            };
        }

        self.last_refresh = std::time::Instant::now();
    }

    pub fn toggle_voice_recording(&mut self) {
        if self.voice_recording {
            self.stop_voice_recording();
        } else {
            self.start_voice_recording();
        }
    }

    /// Resolve the per-user runtime directory for voice pipeline artifacts.
    ///
    /// Returns `<dirs.root>/runtime/` after ensuring it exists. Returning
    /// `None` means the app state (and therefore the directory layout) isn't
    /// initialized yet — callers must treat that as "skip voice operation".
    ///
    /// Using a per-user path under the encrypted store root replaces the
    /// previous hardcoded `/tmp/aivyx-*.wav` paths, which were vulnerable to
    /// symlink attacks and cross-user observation on shared machines.
    fn voice_runtime_dir(&self) -> Option<std::path::PathBuf> {
        let state = self.state.as_ref()?;
        let dir = state.dirs.root().join("runtime");
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::error!("Failed to create voice runtime dir {}: {e}", dir.display());
            return None;
        }
        Some(dir)
    }

    pub fn start_voice_recording(&mut self) {
        if self.chat_streaming
            || self.voice_recording
            || !self
                .settings
                .as_ref()
                .map(|s| s.voice_enabled)
                .unwrap_or(true)
        {
            return;
        }

        let Some(runtime_dir) = self.voice_runtime_dir() else {
            tracing::error!("Voice recording skipped: runtime directory unavailable");
            return;
        };
        let voice_wav = runtime_dir.join("voice.wav");

        // Initialize the transcriber channels if missing
        if self.voice_transcript_tx.is_none() {
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            self.voice_transcript_tx = Some(tx);
            self.voice_transcript_rx = Some(rx);
        }

        self.voice_recording = true;

        // Spawn arecord standard sync child process
        match std::process::Command::new("arecord")
            .arg("-f")
            .arg("S16_LE")
            .arg("-c1")
            .arg("-r16000")
            .arg(&voice_wav)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(child) => {
                self.voice_process = Some(child);
            }
            Err(e) => {
                tracing::error!("Failed to start arecord: {e}");
                self.voice_recording = false;
            }
        }
    }

    pub fn stop_voice_recording(&mut self) {
        if !self.voice_recording {
            return;
        }
        self.voice_recording = false;
        self.voice_transcribing = true;

        if let Some(mut child) = self.voice_process.take() {
            let _ = child.kill();
            let _ = child.wait();
        }

        let Some(runtime_dir) = self.voice_runtime_dir() else {
            self.voice_transcribing = false;
            return;
        };
        let voice_wav = runtime_dir.join("voice.wav");

        // Resolve STT model: explicit setting wins, otherwise look under the
        // per-user models directory. A missing file is not fatal — we log and
        // skip rather than invoke whisper-cli on a path that won't exist off
        // the developer's machine.
        let stt_model = self
            .settings
            .as_ref()
            .and_then(|s| s.stt_model_path.clone())
            .or_else(|| {
                let candidate = self
                    .state
                    .as_ref()?
                    .dirs
                    .root()
                    .join("models/ggml-base.en.bin");
                candidate.exists().then(|| candidate.display().to_string())
            });

        let Some(stt_model) = stt_model else {
            tracing::warn!(
                "STT model not configured and default not found; skipping transcription"
            );
            if let Some(tx) = &self.voice_transcript_tx {
                let _ = tx.send("__CLEAR_TRANSCRIBING__".into());
            }
            return;
        };

        if let Some(tx) = &self.voice_transcript_tx {
            let tx_clone = tx.clone();
            tokio::spawn(async move {
                let out = tokio::process::Command::new("whisper-cli")
                    .arg("-m")
                    .arg(&stt_model)
                    .arg("-f")
                    .arg(&voice_wav)
                    .arg("-nt") // no timestamps
                    .arg("-np") // no prints (only results)
                    .output()
                    .await;

                if let Ok(output) = out {
                    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if !text.is_empty() {
                        let _ = tx_clone.send(text);
                    }
                }
                let _ = tx_clone.send("__CLEAR_TRANSCRIBING__".into());
            });
        }
    }

    /// Send a chat message to the agent. Spawns an async task that streams
    pub fn send_chat_message(&mut self) {
        if self.chat_input.is_empty() || self.chat_streaming {
            return;
        }
        let Some(ref state) = self.state else { return };

        let raw_input = self.chat_input.clone();
        let (priority_badge, message) = parse_priority_prefix(&raw_input);

        let display_content = if let Some(badge) = &priority_badge {
            format!("[{badge}] {message}")
        } else {
            message.clone()
        };
        self.chat_messages.push(ChatMessage {
            role: "user".into(),
            content: display_content,
            timestamp: chrono::Local::now().format("%H:%M").to_string(),
        });
        self.chat_input.clear();
        self.chat_input_cursor = 0;
        self.chat_input_scroll = 0;
        self.chat_streaming = true;

        // Add empty assistant message that we'll fill with streamed tokens
        self.chat_messages.push(ChatMessage {
            role: "assistant".into(),
            content: String::new(),
            timestamp: chrono::Local::now().format("%H:%M").to_string(),
        });

        let (token_tx, token_rx) = mpsc::channel::<String>(256);
        self.chat_token_rx = Some(token_rx);

        // Set up agent event channel for live tool status / compaction / turn stats
        let (event_tx, event_rx) = mpsc::channel::<AgentEvent>(64);
        self.chat_event_rx = Some(event_rx);
        self.chat_tool_status.clear();
        self.chat_compacting = false;

        // Create the cancellation token *before* spawning so the App
        // retains a handle. Anything that needs to lock state.agent
        // (session switch, branch, new conversation) can call
        // `cancel_chat_stream()` to abort the in-flight turn and release
        // the mutex promptly instead of waiting out the 5 min timeout.
        let cancel = tokio_util::sync::CancellationToken::new();
        self.chat_cancel = Some(cancel.clone());

        let agent = Arc::clone(&state.agent);
        let event_sink = Arc::new(aivyx_core::ChannelProgressSink::new(event_tx));
        tokio::spawn(async move {
            let mut agent = agent.lock().await;
            agent.set_event_sink(event_sink);

            // Wrap the agent turn in a timeout (5 minutes) for streaming reliability
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(300),
                agent.turn_stream(&message, None, token_tx.clone(), Some(cancel)),
            )
            .await;

            match result {
                Ok(Ok(_)) => {
                    let _ = token_tx.send("\n[[DONE]]".into()).await;
                }
                Ok(Err(e)) => {
                    let _ = token_tx.send(format!("\n\n⚠ Error: {e}")).await;
                    let _ = token_tx.send("\n[[DONE]]".into()).await;
                }
                Err(_) => {
                    let _ = token_tx
                        .send(
                            "\n\n⚠ Response timed out (5 min). Partial response preserved.".into(),
                        )
                        .await;
                    let _ = token_tx.send("\n[[DONE]]".into()).await;
                }
            }
        });
    }

    /// Cancel the in-flight chat stream (if any).
    ///
    /// Fires the stored cancellation token so `turn_stream` returns at
    /// its next checkpoint, releasing the `state.agent` mutex. Safe to
    /// call when no stream is running — it's a no-op in that case.
    ///
    /// Call this before any code path that needs to acquire the agent
    /// lock to mutate conversation state (e.g. loading a saved session,
    /// branching from a snapshot, clearing the conversation). Without
    /// this, competing actions queue up behind the 5-minute turn_stream
    /// timeout and the TUI appears frozen.
    pub fn cancel_chat_stream(&mut self) {
        if let Some(cancel) = self.chat_cancel.take() {
            cancel.cancel();
            tracing::debug!("chat stream cancelled by competing action");
        }
    }

    /// Apply a status to the approval item in `queue` whose notification
    /// id matches `notification_id`, preserving terminal statuses.
    ///
    /// Returns `true` if a matching pending item was updated. Extracted
    /// so the ID-match logic can be unit-tested without standing up a
    /// full `AppState`.
    pub(crate) fn apply_resolution_by_id(
        queue: &mut [ApprovalItem],
        notification_id: &str,
        new_status: ApprovalStatus,
        now: chrono::DateTime<chrono::Utc>,
    ) -> bool {
        if let Some(item) = queue
            .iter_mut()
            .find(|a| a.notification.id == notification_id)
            && item.status == ApprovalStatus::Pending
        {
            item.status = new_status;
            item.resolved_at = Some(now);
            return true;
        }
        false
    }

    /// Resolve the currently selected approval.
    ///
    /// Identifies the target by `notification.id` (a stable UUID) rather
    /// than by index into `state.approvals`. The shared queue is mutated
    /// by the notification bridge (new items pushed) and the expiry sweep
    /// (items marked Expired) independently of the TUI snapshot, so an
    /// index captured from `self.approval_selected` becomes stale the
    /// instant a new notification arrives. Keying off the UUID means the
    /// "approve item 3 actually approved item 4" race is impossible.
    pub fn resolve_approval(&mut self, status: ApprovalStatus) {
        let Some(item) = self.approvals.get_mut(self.approval_selected) else {
            return;
        };
        if item.status != ApprovalStatus::Pending {
            return;
        }

        let notif_id = item.notification.id.clone();
        item.status = status;
        item.resolved_at = Some(chrono::Utc::now());

        let Some(ref state) = self.state else { return };

        // 1. Sync back to the shared approval queue (for API reads) by
        //    matching on notification ID, not position.
        let approvals = state.approvals.clone();
        let lookup_id = notif_id.clone();
        let updated_status = status;
        tokio::spawn(async move {
            let mut queue = approvals.lock().await;
            let updated = Self::apply_resolution_by_id(
                &mut queue,
                &lookup_id,
                updated_status,
                chrono::Utc::now(),
            );
            if !updated {
                tracing::warn!(
                    notification_id = %lookup_id,
                    "resolve_approval: item missing or already resolved in shared queue"
                );
            }
        });

        // 2. Send decision back to the agent loop channel so the
        //    heartbeat can react immediately rather than waiting for
        //    the next tick to poll the shared Vec. The channel has
        //    always been ID-keyed — this path was never racy.
        if let Some(ref tx) = state.approval_tx {
            let _ = tx.try_send(aivyx_loop::ApprovalResponse {
                notification_id: notif_id,
                approved: status == ApprovalStatus::Approved,
                message: None,
            });
        }
    }

    /// Periodically called to check for expired pending approvals.
    ///
    /// Collects notification IDs of locally-expired items and syncs the
    /// shared queue by ID match, mirroring `resolve_approval`. Using
    /// position indices here was subtly wrong — the notification bridge
    /// can push new items between the TUI snapshot and the spawned task,
    /// shifting the shared queue under us.
    pub fn poll_approval_expiries(&mut self) {
        let now = chrono::Utc::now();
        let mut expired_ids: Vec<String> = Vec::new();

        for item in self.approvals.iter_mut() {
            if item.status == ApprovalStatus::Pending
                && let Some(expires) = item.expires_at
                && now >= expires
            {
                item.status = ApprovalStatus::Expired;
                item.resolved_at = Some(now);
                expired_ids.push(item.notification.id.clone());
            }
        }

        if !expired_ids.is_empty()
            && let Some(ref state) = self.state
        {
            let approvals = state.approvals.clone();
            tokio::spawn(async move {
                let mut queue = approvals.lock().await;
                let sweep_now = chrono::Utc::now();

                // Match by ID and re-check the expiry against the
                // canonical `expires_at` on the shared item (not the
                // snapshot's copy — defense-in-depth against clock skew
                // between the two paths).
                for expired_id in &expired_ids {
                    if let Some(shared) =
                        queue.iter_mut().find(|a| &a.notification.id == expired_id)
                        && shared.status == ApprovalStatus::Pending
                        && let Some(expires) = shared.expires_at
                        && sweep_now >= expires
                    {
                        shared.status = ApprovalStatus::Expired;
                        shared.resolved_at = Some(sweep_now);
                    }
                }
            });
        }
    }

    /// Canonical list of all integrations shown in the Settings Integrations card.
    /// Returns `(label, configured, kind)` for each.
    pub fn integrations_list(
        settings: &SettingsSnapshot,
    ) -> Vec<(&'static str, bool, IntegrationKind)> {
        vec![
            ("Email", settings.email_configured, IntegrationKind::Email),
            (
                "Telegram",
                settings.telegram_configured,
                IntegrationKind::Telegram,
            ),
            (
                "Matrix",
                settings.matrix_configured,
                IntegrationKind::Matrix,
            ),
            (
                "Calendar",
                settings.calendar_configured,
                IntegrationKind::Calendar,
            ),
            (
                "Contacts",
                settings.contacts_configured,
                IntegrationKind::Contacts,
            ),
            ("Vault", settings.vault_configured, IntegrationKind::Vault),
            (
                "Signal",
                settings.signal_configured,
                IntegrationKind::Signal,
            ),
            ("SMS", settings.sms_configured, IntegrationKind::Sms),
            (
                "Finance",
                settings.finance_configured,
                IntegrationKind::Finance,
            ),
            (
                "Desktop",
                settings.desktop_configured,
                IntegrationKind::Desktop,
            ),
            (
                "DevTools",
                settings.devtools_configured,
                IntegrationKind::DevTools,
            ),
        ]
    }

    /// Field definitions for a given integration kind.
    /// Secret fields have `is_secret: true` and a non-empty `store_key`.
    pub fn integration_fields(kind: IntegrationKind) -> Vec<IntegrationField> {
        match kind {
            IntegrationKind::Email => vec![
                IntegrationField {
                    label: "Address".into(),
                    toml_key: "address".into(),
                    value: String::new(),
                    is_secret: false,
                    store_key: "",
                },
                IntegrationField {
                    label: "IMAP Host".into(),
                    toml_key: "imap_host".into(),
                    value: String::new(),
                    is_secret: false,
                    store_key: "",
                },
                IntegrationField {
                    label: "IMAP Port".into(),
                    toml_key: "imap_port".into(),
                    value: "993".into(),
                    is_secret: false,
                    store_key: "",
                },
                IntegrationField {
                    label: "SMTP Host".into(),
                    toml_key: "smtp_host".into(),
                    value: String::new(),
                    is_secret: false,
                    store_key: "",
                },
                IntegrationField {
                    label: "SMTP Port".into(),
                    toml_key: "smtp_port".into(),
                    value: "587".into(),
                    is_secret: false,
                    store_key: "",
                },
                IntegrationField {
                    label: "Username".into(),
                    toml_key: "username".into(),
                    value: String::new(),
                    is_secret: false,
                    store_key: "",
                },
                IntegrationField {
                    label: "Password".into(),
                    toml_key: "".into(),
                    value: String::new(),
                    is_secret: true,
                    store_key: "EMAIL_PASSWORD",
                },
            ],
            IntegrationKind::Telegram => vec![
                IntegrationField {
                    label: "Chat ID".into(),
                    toml_key: "default_chat_id".into(),
                    value: String::new(),
                    is_secret: false,
                    store_key: "",
                },
                IntegrationField {
                    label: "Bot Token".into(),
                    toml_key: "".into(),
                    value: String::new(),
                    is_secret: true,
                    store_key: "TELEGRAM_BOT_TOKEN",
                },
            ],
            IntegrationKind::Matrix => vec![
                IntegrationField {
                    label: "Homeserver".into(),
                    toml_key: "homeserver".into(),
                    value: String::new(),
                    is_secret: false,
                    store_key: "",
                },
                IntegrationField {
                    label: "Room ID".into(),
                    toml_key: "default_room_id".into(),
                    value: String::new(),
                    is_secret: false,
                    store_key: "",
                },
                IntegrationField {
                    label: "Access Token".into(),
                    toml_key: "".into(),
                    value: String::new(),
                    is_secret: true,
                    store_key: "MATRIX_ACCESS_TOKEN",
                },
            ],
            IntegrationKind::Calendar => vec![
                IntegrationField {
                    label: "CalDAV URL".into(),
                    toml_key: "url".into(),
                    value: String::new(),
                    is_secret: false,
                    store_key: "",
                },
                IntegrationField {
                    label: "Username".into(),
                    toml_key: "username".into(),
                    value: String::new(),
                    is_secret: false,
                    store_key: "",
                },
                IntegrationField {
                    label: "Password".into(),
                    toml_key: "".into(),
                    value: String::new(),
                    is_secret: true,
                    store_key: "CALENDAR_PASSWORD",
                },
            ],
            IntegrationKind::Contacts => vec![
                IntegrationField {
                    label: "CardDAV URL".into(),
                    toml_key: "url".into(),
                    value: String::new(),
                    is_secret: false,
                    store_key: "",
                },
                IntegrationField {
                    label: "Username".into(),
                    toml_key: "username".into(),
                    value: String::new(),
                    is_secret: false,
                    store_key: "",
                },
                IntegrationField {
                    label: "Password".into(),
                    toml_key: "".into(),
                    value: String::new(),
                    is_secret: true,
                    store_key: "CONTACTS_PASSWORD",
                },
            ],
            IntegrationKind::Vault => vec![
                IntegrationField {
                    label: "Path".into(),
                    toml_key: "path".into(),
                    value: String::new(),
                    is_secret: false,
                    store_key: "",
                },
                IntegrationField {
                    label: "Extensions".into(),
                    toml_key: "extensions".into(),
                    value: "md,txt,pdf".into(),
                    is_secret: false,
                    store_key: "",
                },
            ],
            IntegrationKind::Signal => vec![
                IntegrationField {
                    label: "Account".into(),
                    toml_key: "account".into(),
                    value: String::new(),
                    is_secret: false,
                    store_key: "",
                },
                IntegrationField {
                    label: "Socket Path".into(),
                    toml_key: "socket_path".into(),
                    value: String::new(),
                    is_secret: false,
                    store_key: "",
                },
            ],
            IntegrationKind::Sms => vec![
                IntegrationField {
                    label: "Provider".into(),
                    toml_key: "provider".into(),
                    value: String::new(),
                    is_secret: false,
                    store_key: "",
                },
                IntegrationField {
                    label: "Account ID".into(),
                    toml_key: "account_id".into(),
                    value: String::new(),
                    is_secret: false,
                    store_key: "",
                },
                IntegrationField {
                    label: "From Number".into(),
                    toml_key: "from_number".into(),
                    value: String::new(),
                    is_secret: false,
                    store_key: "",
                },
            ],
            IntegrationKind::Finance => vec![IntegrationField {
                label: "Receipt Folder".into(),
                toml_key: "receipt_folder".into(),
                value: String::new(),
                is_secret: false,
                store_key: "",
            }],
            IntegrationKind::Desktop => vec![
                IntegrationField {
                    label: "Clipboard".into(),
                    toml_key: "clipboard".into(),
                    value: "true".into(),
                    is_secret: false,
                    store_key: "",
                },
                IntegrationField {
                    label: "Windows".into(),
                    toml_key: "windows".into(),
                    value: "true".into(),
                    is_secret: false,
                    store_key: "",
                },
                IntegrationField {
                    label: "Notifications".into(),
                    toml_key: "notifications".into(),
                    value: "true".into(),
                    is_secret: false,
                    store_key: "",
                },
            ],
            IntegrationKind::DevTools => vec![
                IntegrationField {
                    label: "Repo Path".into(),
                    toml_key: "repo_path".into(),
                    value: String::new(),
                    is_secret: false,
                    store_key: "",
                },
                IntegrationField {
                    label: "Forge".into(),
                    toml_key: "forge".into(),
                    value: "github".into(),
                    is_secret: false,
                    store_key: "",
                },
                IntegrationField {
                    label: "Repo Owner".into(),
                    toml_key: "repo_owner".into(),
                    value: String::new(),
                    is_secret: false,
                    store_key: "",
                },
                IntegrationField {
                    label: "Repo Name".into(),
                    toml_key: "repo_name".into(),
                    value: String::new(),
                    is_secret: false,
                    store_key: "",
                },
            ],
        }
    }

    /// Number of interactive items in a settings card.
    pub fn settings_item_count(&self, card: usize) -> usize {
        match card {
            0 => 2,  // provider: model, base_url
            1 => 3,  // autonomy: tier, rate_limit, max_cost
            2 => 11, // heartbeat: enabled + 10 flags
            3 => self
                .settings
                .as_ref()
                .map(|s| s.schedules.len())
                .unwrap_or(0),
            4 => 3,  // agent: name, soul, skills
            5 => 11, // integrations: all 11 types
            7 => 5,  // persona: formality, verbosity, warmth, humor, confidence
            8 => 1,  // tools & extensions: discovery mode toggle
            9 => self
                .settings
                .as_ref()
                .map(|s| s.desktop_app_access.len().max(1))
                .unwrap_or(1), // applications
            _ => 0,
        }
    }

    /// Toggle the currently selected settings item.
    ///
    /// Maps (card_index, item_index) → (section, key) and calls
    /// `toggle_config_bool` to edit config.toml in place, then reloads.
    pub fn settings_toggle_current(&mut self) {
        let Some(ref state) = self.state else { return };
        let config_path = state.config_path.clone();

        let (section, key) = match (self.settings_card_index, self.settings_item_index) {
            (2, 0) => ("[heartbeat]", "enabled"),
            (2, 1) => ("[heartbeat]", "can_reflect"),
            (2, 2) => ("[heartbeat]", "can_consolidate_memory"),
            (2, 3) => ("[heartbeat]", "can_analyze_failures"),
            (2, 4) => ("[heartbeat]", "can_extract_knowledge"),
            (2, 5) => ("[heartbeat]", "can_plan_review"),
            (2, 6) => ("[heartbeat]", "can_strategy_review"),
            (2, 7) => ("[heartbeat]", "can_track_mood"),
            (2, 8) => ("[heartbeat]", "can_encourage"),
            (2, 9) => ("[heartbeat]", "can_track_milestones"),
            (2, 10) => ("[heartbeat]", "notification_pacing"),
            _ => return,
        };

        // Read current value from snapshot and toggle it
        let current = self
            .settings
            .as_ref()
            .map(
                |s| match (self.settings_card_index, self.settings_item_index) {
                    (2, 0) => s.heartbeat_enabled,
                    (2, 1) => s.heartbeat_can_reflect,
                    (2, 2) => s.heartbeat_can_consolidate,
                    (2, 3) => s.heartbeat_can_analyze_failures,
                    (2, 4) => s.heartbeat_can_extract_knowledge,
                    (2, 5) => s.heartbeat_can_plan_review,
                    (2, 6) => s.heartbeat_can_strategy_review,
                    (2, 7) => s.heartbeat_can_track_mood,
                    (2, 8) => s.heartbeat_can_encourage,
                    (2, 9) => s.heartbeat_can_track_milestones,
                    (2, 10) => s.heartbeat_notification_pacing,
                    _ => false,
                },
            )
            .unwrap_or(false);

        let _ = aivyx_pa::settings::toggle_config_bool(&config_path, section, key, !current);
        // Reload the snapshot to reflect the change
        match aivyx_pa::settings::reload_settings_snapshot(&config_path) {
            Ok(s) => {
                self.settings = Some(s);
                self.settings_error = None;
            }
            Err(e) => {
                self.settings = None;
                self.settings_error = Some(e);
            }
        }
    }

    /// Cycle the access level of the currently selected application (←/→ keys).
    ///
    /// `forward` = true cycles Blocked → ViewOnly → Interact → Full → Blocked,
    /// `forward` = false cycles in reverse.
    pub fn settings_cycle_app_access(&mut self, forward: bool) {
        let Some(ref state) = self.state else { return };
        let Some(ref settings) = self.settings else {
            return;
        };
        let idx = self.settings_item_index;
        if idx >= settings.desktop_app_access.len() {
            return;
        }

        let (ref binary, _, ref current_level) = settings.desktop_app_access[idx];

        // Cycle through access levels without depending on aivyx_actions.
        // Order: Blocked → View Only → Interact → Full → (wraps)
        const LEVELS: [&str; 4] = ["Blocked", "View Only", "Interact", "Full"];
        let cur = LEVELS
            .iter()
            .position(|l| *l == current_level.as_str())
            .unwrap_or(3);
        let next = if forward {
            LEVELS[(cur + 1) % 4]
        } else {
            LEVELS[(cur + 3) % 4] // +3 mod 4 = -1 mod 4
        };

        let config_path = state.config_path.clone();
        let _ = aivyx_pa::settings::write_app_access(&config_path, binary, next);
        match aivyx_pa::settings::reload_settings_snapshot(&config_path) {
            Ok(s) => {
                self.settings = Some(s);
                self.settings_error = None;
            }
            Err(e) => {
                self.settings = None;
                self.settings_error = Some(e);
            }
        }
    }

    /// Activate the currently selected settings item (Enter key).
    ///
    /// Opens a popup for editable fields, cycles values for enums,
    /// or delegates to `settings_toggle_current` for heartbeat bools.
    pub fn settings_activate_current(&mut self) {
        let Some(ref settings) = self.settings else {
            return;
        };
        match (self.settings_card_index, self.settings_item_index) {
            // Card 0: Provider
            (0, 0) => {
                self.settings_popup = Some(SettingsPopup::TextInput {
                    title: "Model Name".into(),
                    value: settings.model_name.clone(),
                    section: "[provider]",
                    key: "model",
                    kind: InputKind::String,
                });
            }
            (0, 1) => {
                self.settings_popup = Some(SettingsPopup::TextInput {
                    title: "Base URL".into(),
                    value: settings.provider_base_url.clone().unwrap_or_default(),
                    section: "[provider]",
                    key: "base_url",
                    kind: InputKind::String,
                });
            }
            // Card 1: Autonomy — tier cycles inline
            (1, 0) => {
                let Some(ref state) = self.state else { return };
                let tiers = ["Locked", "Leash", "Trust", "Free"];
                let cur = &settings.autonomy_tier;
                let idx = tiers
                    .iter()
                    .position(|t| t.eq_ignore_ascii_case(cur))
                    .unwrap_or(0);
                let next = tiers[(idx + 1) % 4];
                let _ = aivyx_pa::settings::write_toml_string(
                    &state.config_path,
                    "[autonomy]",
                    "default_tier",
                    next,
                );
                match aivyx_pa::settings::reload_settings_snapshot(&state.config_path) {
                    Ok(s) => {
                        self.settings = Some(s);
                        self.settings_error = None;
                    }
                    Err(e) => {
                        self.settings = None;
                        self.settings_error = Some(e);
                    }
                }
            }
            (1, 1) => {
                self.settings_popup = Some(SettingsPopup::TextInput {
                    title: "Rate Limit (calls/min)".into(),
                    value: settings.max_tool_calls_per_min.to_string(),
                    section: "[autonomy]",
                    key: "max_tool_calls_per_min",
                    kind: InputKind::UInt,
                });
            }
            (1, 2) => {
                self.settings_popup = Some(SettingsPopup::TextInput {
                    title: "Max Cost (USD)".into(),
                    value: format!("{:.2}", settings.max_cost_usd),
                    section: "[autonomy]",
                    key: "max_cost_usd",
                    kind: InputKind::Float,
                });
            }
            // Card 2: Heartbeat — delegate to toggle
            (2, _) => self.settings_toggle_current(),
            // Card 3: Schedules — open editor for selected schedule
            (3, idx) => {
                if idx < settings.schedules.len() {
                    let (ref name, ref cron, _, ref prompt) = settings.schedules[idx];
                    let prompt_lines: Vec<String> = prompt.lines().map(String::from).collect();
                    let prompt_lines = if prompt_lines.is_empty() {
                        vec![String::new()]
                    } else {
                        prompt_lines
                    };
                    self.settings_popup = Some(SettingsPopup::ScheduleEditor {
                        editing: Some(name.clone()),
                        name: name.clone(),
                        cron_builder: CronBuilder::from_cron(cron),
                        prompt: prompt_lines,
                        prompt_cursor_row: 0,
                        prompt_cursor_col: 0,
                        notify: true,
                        focused: 1,
                        error: None,
                    });
                }
            }
            // Card 4: Agent
            (4, 0) => {
                self.settings_popup = Some(SettingsPopup::TextInput {
                    title: "Agent Name".into(),
                    value: settings.agent_name.clone(),
                    section: "[agent]",
                    key: "name",
                    kind: InputKind::String,
                });
            }
            (4, 1) => {
                let soul_text = if settings.has_custom_soul {
                    // Read current soul from config
                    self.state
                        .as_ref()
                        .and_then(|s| {
                            let pa = aivyx_pa::config::PaConfig::load(&s.config_path);
                            pa.agent.and_then(|a| a.soul)
                        })
                        .unwrap_or_default()
                } else {
                    String::new()
                };
                let lines: Vec<String> = soul_text.lines().map(String::from).collect();
                let lines = if lines.is_empty() {
                    vec![String::new()]
                } else {
                    lines
                };
                self.settings_popup = Some(SettingsPopup::MultiLineInput {
                    title: "Soul (system prompt)".into(),
                    lines,
                    cursor_row: 0,
                    cursor_col: 0,
                    section: "[agent]",
                    key: "soul",
                });
            }
            (4, 2) => {
                self.settings_popup = Some(SettingsPopup::SkillManager {
                    input: String::new(),
                    selected: 0,
                    skills: settings.agent_skills.clone(),
                });
            }
            // Card 5: Integrations — open setup popup (works for both new and reconfigure)
            (5, idx) => {
                let list = Self::integrations_list(settings);
                if idx >= list.len() {
                    return;
                }
                let (_, configured, kind) = list[idx];
                let mut fields = Self::integration_fields(kind);
                // Pre-fill non-secret TOML values when reconfiguring
                if configured
                    && let Some(ref state) = self.state
                    && let Ok(content) = std::fs::read_to_string(&state.config_path)
                {
                    let section = aivyx_pa::settings::integration_section_name(kind);
                    let header = format!("[{section}]");
                    let mut in_section = false;
                    for line in content.lines() {
                        let trimmed = line.trim();
                        if trimmed == header {
                            in_section = true;
                            continue;
                        }
                        if trimmed.starts_with('[') {
                            if in_section {
                                break;
                            }
                            continue;
                        }
                        if in_section && let Some((k, v)) = trimmed.split_once('=') {
                            let k = k.trim();
                            let v = v.trim().trim_matches('"');
                            for f in &mut fields {
                                if !f.is_secret && f.toml_key == k {
                                    f.value = v.to_string();
                                }
                            }
                        }
                    }
                }
                self.settings_popup = Some(SettingsPopup::IntegrationSetup {
                    kind,
                    fields,
                    focused: 0,
                    is_configured: configured,
                });
            }
            // Card 7: Persona — no-op (uses Left/Right slider)
            // Card 8: Tools & Extensions — cycle discovery mode
            (8, 0) => {
                let Some(ref state) = self.state else { return };
                let modes = ["Off", "Embedding", "Hybrid"];
                let cur = settings.tool_discovery_mode.as_deref().unwrap_or("Off");
                let idx = modes
                    .iter()
                    .position(|m| m.eq_ignore_ascii_case(cur))
                    .unwrap_or(0);
                let next = modes[(idx + 1) % 3];
                let _ = aivyx_pa::settings::write_toml_string_create(
                    &state.config_path,
                    "[tool_discovery]",
                    "mode",
                    next,
                );
                match aivyx_pa::settings::reload_settings_snapshot(&state.config_path) {
                    Ok(s) => {
                        self.settings = Some(s);
                        self.settings_error = None;
                    }
                    Err(e) => {
                        self.settings = None;
                        self.settings_error = Some(e);
                    }
                }
            }
            // Card 9: Applications — trigger scan if empty, otherwise no-op as Left/Right cycles access
            (9, _) => {
                if settings.desktop_app_access.is_empty() {
                    let Some(ref state) = self.state else { return };
                    let path = state.config_path.clone();
                    // Must write the empty table first so [desktop] block knows it's configured
                    let _ = aivyx_pa::settings::write_toml_string_create(
                        &path,
                        "[desktop.app_access]",
                        "dummy",
                        "Interact",
                    );

                    tokio::spawn(async move {
                        let apps = aivyx_actions::desktop::scanner::scan_applications();
                        for (bin_name, _) in apps {
                            let _ =
                                aivyx_pa::settings::write_app_access(&path, &bin_name, "Interact");
                        }
                    });
                }
            }
            _ => {}
        }
    }

    /// Adjust a persona dimension by delta (±0.1), clamped to [0.0, 1.0].
    pub fn settings_adjust_persona(&mut self, delta: f32) {
        let Some(ref state) = self.state else { return };
        let Some(ref settings) = self.settings else {
            return;
        };
        let Some(ref persona) = settings.persona_dimensions else {
            return;
        };

        let dims = ["formality", "verbosity", "warmth", "humor", "confidence"];
        let vals = [
            persona.formality,
            persona.verbosity,
            persona.warmth,
            persona.humor,
            persona.confidence,
        ];
        let idx = self.settings_item_index;
        if idx >= dims.len() {
            return;
        }

        let new_val = (vals[idx] + delta).clamp(0.0, 1.0);
        let _ = aivyx_pa::settings::write_toml_number(
            &state.config_path,
            "[persona]",
            dims[idx],
            &format!("{:.1}", new_val),
        );
        match aivyx_pa::settings::reload_settings_snapshot(&state.config_path) {
            Ok(s) => {
                self.settings = Some(s);
                self.settings_error = None;
            }
            Err(e) => {
                self.settings = None;
                self.settings_error = Some(e);
            }
        }
    }

    /// Handle keystrokes when a settings popup is active.
    pub fn handle_settings_popup(&mut self, key: KeyEvent) {
        use crossterm::event::KeyCode;

        let Some(ref state) = self.state else { return };
        let config_path = state.config_path.clone();

        match self.settings_popup.take() {
            Some(SettingsPopup::TextInput {
                title,
                mut value,
                section,
                key: toml_key,
                kind,
            }) => {
                match key.code {
                    KeyCode::Esc => {} // closed — popup already taken
                    KeyCode::Enter => {
                        match kind {
                            InputKind::String => {
                                let _ = aivyx_pa::settings::write_toml_string(
                                    &config_path,
                                    section,
                                    toml_key,
                                    &value,
                                );
                            }
                            InputKind::UInt | InputKind::Float => {
                                let _ = aivyx_pa::settings::write_toml_number(
                                    &config_path,
                                    section,
                                    toml_key,
                                    &value,
                                );
                            }
                        }
                        match aivyx_pa::settings::reload_settings_snapshot(&config_path) {
                            Ok(s) => {
                                self.settings = Some(s);
                                self.settings_error = None;
                            }
                            Err(e) => {
                                self.settings = None;
                                self.settings_error = Some(e);
                            }
                        }
                    }
                    KeyCode::Backspace => {
                        value.pop();
                        self.settings_popup = Some(SettingsPopup::TextInput {
                            title,
                            value,
                            section,
                            key: toml_key,
                            kind,
                        });
                    }
                    KeyCode::Char(c) => {
                        let accept = match kind {
                            InputKind::String => true,
                            InputKind::UInt => c.is_ascii_digit(),
                            InputKind::Float => {
                                c.is_ascii_digit() || (c == '.' && !value.contains('.'))
                            }
                        };
                        if accept {
                            value.push(c);
                        }
                        self.settings_popup = Some(SettingsPopup::TextInput {
                            title,
                            value,
                            section,
                            key: toml_key,
                            kind,
                        });
                    }
                    _ => {
                        self.settings_popup = Some(SettingsPopup::TextInput {
                            title,
                            value,
                            section,
                            key: toml_key,
                            kind,
                        });
                    }
                }
            }
            Some(SettingsPopup::MultiLineInput {
                title,
                mut lines,
                mut cursor_row,
                mut cursor_col,
                section,
                key: toml_key,
            }) => {
                match key.code {
                    KeyCode::Esc => {} // close without saving
                    KeyCode::Char('s')
                        if key
                            .modifiers
                            .contains(crossterm::event::KeyModifiers::CONTROL) =>
                    {
                        let text = lines.join("\n");
                        let _ = aivyx_pa::settings::write_toml_multiline_string(
                            &config_path,
                            section,
                            toml_key,
                            &text,
                        );
                        match aivyx_pa::settings::reload_settings_snapshot(&config_path) {
                            Ok(s) => {
                                self.settings = Some(s);
                                self.settings_error = None;
                            }
                            Err(e) => {
                                self.settings = None;
                                self.settings_error = Some(e);
                            }
                        }
                    }
                    KeyCode::Enter => {
                        let rest = lines[cursor_row].split_off(cursor_col);
                        cursor_row += 1;
                        lines.insert(cursor_row, rest);
                        cursor_col = 0;
                        self.settings_popup = Some(SettingsPopup::MultiLineInput {
                            title,
                            lines,
                            cursor_row,
                            cursor_col,
                            section,
                            key: toml_key,
                        });
                    }
                    KeyCode::Backspace => {
                        if cursor_col > 0 {
                            cursor_col -= 1;
                            lines[cursor_row].remove(cursor_col);
                        } else if cursor_row > 0 {
                            let removed = lines.remove(cursor_row);
                            cursor_row -= 1;
                            cursor_col = lines[cursor_row].len();
                            lines[cursor_row].push_str(&removed);
                        }
                        self.settings_popup = Some(SettingsPopup::MultiLineInput {
                            title,
                            lines,
                            cursor_row,
                            cursor_col,
                            section,
                            key: toml_key,
                        });
                    }
                    KeyCode::Left => {
                        cursor_col = cursor_col.saturating_sub(1);
                        self.settings_popup = Some(SettingsPopup::MultiLineInput {
                            title,
                            lines,
                            cursor_row,
                            cursor_col,
                            section,
                            key: toml_key,
                        });
                    }
                    KeyCode::Right => {
                        if cursor_col < lines[cursor_row].len() {
                            cursor_col += 1;
                        }
                        self.settings_popup = Some(SettingsPopup::MultiLineInput {
                            title,
                            lines,
                            cursor_row,
                            cursor_col,
                            section,
                            key: toml_key,
                        });
                    }
                    KeyCode::Up => {
                        if cursor_row > 0 {
                            cursor_row -= 1;
                            cursor_col = cursor_col.min(lines[cursor_row].len());
                        }
                        self.settings_popup = Some(SettingsPopup::MultiLineInput {
                            title,
                            lines,
                            cursor_row,
                            cursor_col,
                            section,
                            key: toml_key,
                        });
                    }
                    KeyCode::Down => {
                        if cursor_row + 1 < lines.len() {
                            cursor_row += 1;
                            cursor_col = cursor_col.min(lines[cursor_row].len());
                        }
                        self.settings_popup = Some(SettingsPopup::MultiLineInput {
                            title,
                            lines,
                            cursor_row,
                            cursor_col,
                            section,
                            key: toml_key,
                        });
                    }
                    KeyCode::Char(c) => {
                        lines[cursor_row].insert(cursor_col, c);
                        cursor_col += 1;
                        self.settings_popup = Some(SettingsPopup::MultiLineInput {
                            title,
                            lines,
                            cursor_row,
                            cursor_col,
                            section,
                            key: toml_key,
                        });
                    }
                    _ => {
                        self.settings_popup = Some(SettingsPopup::MultiLineInput {
                            title,
                            lines,
                            cursor_row,
                            cursor_col,
                            section,
                            key: toml_key,
                        });
                    }
                }
            }
            Some(SettingsPopup::Confirm { message, action }) => {
                match key.code {
                    KeyCode::Char('y') | KeyCode::Enter => {
                        match action {
                            ConfirmAction::RemoveSkill(idx) => {
                                if let Some(ref s) = self.settings {
                                    let mut skills = s.agent_skills.clone();
                                    if idx < skills.len() {
                                        skills.remove(idx);
                                        aivyx_pa::settings::write_toml_string_array(
                                            &config_path,
                                            "[agent]",
                                            "skills",
                                            &skills,
                                        );
                                        match aivyx_pa::settings::reload_settings_snapshot(
                                            &config_path,
                                        ) {
                                            Ok(s) => {
                                                self.settings = Some(s);
                                                self.settings_error = None;
                                            }
                                            Err(e) => {
                                                self.settings = None;
                                                self.settings_error = Some(e);
                                            }
                                        }
                                    }
                                }
                            }
                            ConfirmAction::RemoveIntegration(kind) => {
                                let _ = aivyx_pa::settings::remove_integration_config(
                                    &config_path,
                                    kind,
                                );
                                // Delete associated secrets
                                if let Some(ref state) = self.state {
                                    let secret_keys: &[&str] = match kind {
                                        IntegrationKind::Email => &["EMAIL_PASSWORD"],
                                        IntegrationKind::Telegram => &["TELEGRAM_BOT_TOKEN"],
                                        IntegrationKind::Matrix => &["MATRIX_ACCESS_TOKEN"],
                                        IntegrationKind::Calendar => &["CALENDAR_PASSWORD"],
                                        IntegrationKind::Contacts => &["CONTACTS_PASSWORD"],
                                        _ => &[],
                                    };
                                    for key in secret_keys {
                                        let _ = state.store.delete(key);
                                    }
                                }
                                match aivyx_pa::settings::reload_settings_snapshot(&config_path) {
                                    Ok(s) => {
                                        self.settings = Some(s);
                                        self.settings_error = None;
                                    }
                                    Err(e) => {
                                        self.settings = None;
                                        self.settings_error = Some(e);
                                    }
                                }
                            }
                            ConfirmAction::RemoveSchedule(name) => {
                                let _ = aivyx_pa::settings::remove_schedule(
                                    &config_path,
                                    &name,
                                );
                                match aivyx_pa::settings::reload_settings_snapshot(&config_path) {
                                    Ok(s) => {
                                        self.settings = Some(s);
                                        self.settings_error = None;
                                    }
                                    Err(e) => {
                                        self.settings = None;
                                        self.settings_error = Some(e);
                                    }
                                }
                                // Reset selection so it doesn't point past the list
                                let max = self
                                    .settings
                                    .as_ref()
                                    .map(|s| s.schedules.len())
                                    .unwrap_or(0);
                                if self.settings_item_index >= max && max > 0 {
                                    self.settings_item_index = max - 1;
                                }
                            }
                        }
                    }
                    KeyCode::Char('n') | KeyCode::Esc => {} // cancelled
                    _ => {
                        self.settings_popup = Some(SettingsPopup::Confirm { message, action });
                    }
                }
            }
            Some(SettingsPopup::SkillManager {
                mut input,
                mut selected,
                mut skills,
            }) => {
                match key.code {
                    KeyCode::Esc => {}                       // close
                    KeyCode::Enter if input.is_empty() => {} // done — close popup
                    KeyCode::Enter => {
                        skills.push(input.clone());
                        aivyx_pa::settings::write_toml_string_array(
                            &config_path,
                            "[agent]",
                            "skills",
                            &skills,
                        );
                        match aivyx_pa::settings::reload_settings_snapshot(&config_path) {
                            Ok(s) => {
                                self.settings = Some(s);
                                self.settings_error = None;
                            }
                            Err(e) => {
                                self.settings = None;
                                self.settings_error = Some(e);
                            }
                        }
                        input.clear();
                        self.settings_popup = Some(SettingsPopup::SkillManager {
                            input,
                            selected,
                            skills,
                        });
                    }
                    KeyCode::Char('d') if input.is_empty() && !skills.is_empty() => {
                        let name = skills[selected].clone();
                        // Put skill manager back, then overlay confirm
                        self.settings_popup = Some(SettingsPopup::Confirm {
                            message: format!("Remove skill \"{name}\"?"),
                            action: ConfirmAction::RemoveSkill(selected),
                        });
                    }
                    KeyCode::Up => {
                        selected = selected.saturating_sub(1);
                        self.settings_popup = Some(SettingsPopup::SkillManager {
                            input,
                            selected,
                            skills,
                        });
                    }
                    KeyCode::Down => {
                        if selected + 1 < skills.len() {
                            selected += 1;
                        }
                        self.settings_popup = Some(SettingsPopup::SkillManager {
                            input,
                            selected,
                            skills,
                        });
                    }
                    KeyCode::Backspace => {
                        input.pop();
                        self.settings_popup = Some(SettingsPopup::SkillManager {
                            input,
                            selected,
                            skills,
                        });
                    }
                    KeyCode::Char(c) => {
                        input.push(c);
                        self.settings_popup = Some(SettingsPopup::SkillManager {
                            input,
                            selected,
                            skills,
                        });
                    }
                    _ => {
                        self.settings_popup = Some(SettingsPopup::SkillManager {
                            input,
                            selected,
                            skills,
                        });
                    }
                }
            }
            Some(SettingsPopup::IntegrationSetup {
                kind,
                mut fields,
                mut focused,
                is_configured,
            }) => {
                match key.code {
                    KeyCode::Esc => {} // close
                    KeyCode::Tab => {
                        focused = (focused + 1) % fields.len();
                        self.settings_popup = Some(SettingsPopup::IntegrationSetup {
                            kind,
                            fields,
                            focused,
                            is_configured,
                        });
                    }
                    KeyCode::BackTab => {
                        focused = if focused == 0 {
                            fields.len() - 1
                        } else {
                            focused - 1
                        };
                        self.settings_popup = Some(SettingsPopup::IntegrationSetup {
                            kind,
                            fields,
                            focused,
                            is_configured,
                        });
                    }
                    KeyCode::Enter => {
                        // Collect TOML fields (non-secret, non-empty toml_key)
                        let toml_fields: Vec<(String, String)> = fields
                            .iter()
                            .filter(|f| !f.is_secret && !f.toml_key.is_empty())
                            .map(|f| (f.toml_key.clone(), f.value.clone()))
                            .collect();
                        let _ = aivyx_pa::settings::write_integration_config(
                            &config_path,
                            kind,
                            &toml_fields,
                        );
                        // Store secrets in EncryptedStore
                        if let Some(ref state) = self.state {
                            for f in &fields {
                                if f.is_secret && !f.value.is_empty() {
                                    let _ = state.store.put(
                                        f.store_key,
                                        f.value.as_bytes(),
                                        &state.master_key,
                                    );
                                }
                            }
                        }
                        match aivyx_pa::settings::reload_settings_snapshot(&config_path) {
                            Ok(s) => {
                                self.settings = Some(s);
                                self.settings_error = None;
                            }
                            Err(e) => {
                                self.settings = None;
                                self.settings_error = Some(e);
                            }
                        }
                    }
                    KeyCode::Backspace => {
                        fields[focused].value.pop();
                        self.settings_popup = Some(SettingsPopup::IntegrationSetup {
                            kind,
                            fields,
                            focused,
                            is_configured,
                        });
                    }
                    KeyCode::Char(c) => {
                        fields[focused].value.push(c);
                        self.settings_popup = Some(SettingsPopup::IntegrationSetup {
                            kind,
                            fields,
                            focused,
                            is_configured,
                        });
                    }
                    _ => {
                        self.settings_popup = Some(SettingsPopup::IntegrationSetup {
                            kind,
                            fields,
                            focused,
                            is_configured,
                        });
                    }
                }
            }
            Some(SettingsPopup::ScheduleEditor {
                editing,
                mut name,
                mut cron_builder,
                mut prompt,
                mut prompt_cursor_row,
                mut prompt_cursor_col,
                mut notify,
                mut focused,
                error: _,
            }) => {
                // Clear validation error on any keystroke
                let mut error: Option<String> = None;

                // Ctrl+S: save
                let is_save = key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
                    && key.code == KeyCode::Char('s');

                let cron = cron_builder.build_cron();

                if key.code == KeyCode::Esc {
                    // Close without saving
                } else if is_save {
                    // ── Validate ──────────────────────────────────
                    if name.is_empty() || name.len() > 64 {
                        error = Some("Name must be 1-64 characters".into());
                    } else if !name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                    {
                        error = Some("Name: letters, numbers, hyphens, underscores only".into());
                    } else if editing.is_none()
                        && aivyx_pa::settings::schedule_exists(&config_path, &name)
                    {
                        error = Some(format!("Schedule '{}' already exists", name));
                    } else if aivyx_config::schedule::validate_cron(&cron).is_err() {
                        error = Some("Invalid cron expression".into());
                    } else {
                        let prompt_text = prompt.join("\n");
                        if prompt_text.trim().is_empty() {
                            error = Some("Prompt must not be empty".into());
                        } else {
                            // ── Save ─────────────────────────────────
                            let result = if editing.is_some() {
                                // Edit existing: update cron and prompt
                                let r1 = aivyx_pa::settings::edit_schedule_field(
                                    &config_path,
                                    &name,
                                    "cron",
                                    &format!("\"{cron}\""),
                                );
                                let escaped = prompt_text
                                    .replace('\\', "\\\\")
                                    .replace('"', "\\\"");
                                let r2 = aivyx_pa::settings::edit_schedule_field(
                                    &config_path,
                                    &name,
                                    "prompt",
                                    &format!("\"{escaped}\""),
                                );
                                r1.and(r2)
                            } else {
                                // Create new
                                aivyx_pa::settings::append_schedule(
                                    &config_path,
                                    &name,
                                    &cron,
                                    &prompt_text,
                                    notify,
                                )
                            };

                            match result {
                                Ok(()) => {
                                    match aivyx_pa::settings::reload_settings_snapshot(
                                        &config_path,
                                    ) {
                                        Ok(s) => {
                                            self.settings = Some(s);
                                            self.settings_error = None;
                                        }
                                        Err(e) => {
                                            self.settings = None;
                                            self.settings_error = Some(e);
                                        }
                                    }
                                    return; // close popup
                                }
                                Err(e) => {
                                    error = Some(format!("Write failed: {e}"));
                                }
                            }
                        }
                    }

                    // If we got here, there was a validation error — keep popup open
                    self.settings_popup = Some(SettingsPopup::ScheduleEditor {
                        editing,
                        name,
                        cron_builder,
                        prompt,
                        prompt_cursor_row,
                        prompt_cursor_col,
                        notify,
                        focused,
                        error,
                    });
                } else {
                    // ── Field navigation & input ─────────────────
                    match key.code {
                        KeyCode::Tab => {
                            if focused == 1 {
                                // Reset sub-focus when leaving the schedule builder
                                cron_builder.sub_focus = 0;
                            }
                            focused = (focused + 1) % 4;
                        }
                        KeyCode::BackTab => {
                            focused = (focused + 3) % 4;
                            if focused == 1 {
                                cron_builder.sub_focus = 0;
                            }
                        }
                        _ => match focused {
                            // Name field (0)
                            0 => {
                                if editing.is_some() {
                                    // Name is read-only when editing
                                } else {
                                    match key.code {
                                        KeyCode::Char(c) => name.push(c),
                                        KeyCode::Backspace => { name.pop(); }
                                        _ => {}
                                    }
                                }
                            }
                            // Schedule builder (1)
                            1 => {
                                let sf = cron_builder.sub_focus;
                                let sf_count = cron_builder.sub_field_count();
                                match key.code {
                                    KeyCode::Up => {
                                        if sf > 0 {
                                            cron_builder.sub_focus -= 1;
                                        }
                                    }
                                    KeyCode::Down => {
                                        if sf + 1 < sf_count {
                                            cron_builder.sub_focus += 1;
                                        }
                                    }
                                    KeyCode::Left | KeyCode::Right => {
                                        let delta: i8 = if key.code == KeyCode::Left { -1 } else { 1 };
                                        match (cron_builder.mode, sf) {
                                            // Sub-field 0: mode selector (all modes)
                                            (_, 0) => {
                                                let total = CRON_MODES.len();
                                                cron_builder.mode = ((cron_builder.mode as isize + delta as isize + total as isize) % total as isize) as usize;
                                                // Clamp sub_focus to new mode's field count
                                                let new_count = cron_builder.sub_field_count();
                                                if cron_builder.sub_focus >= new_count {
                                                    cron_builder.sub_focus = new_count - 1;
                                                }
                                            }
                                            // EveryNMin: sf=1 → interval
                                            (0, 1) => {
                                                let total = CRON_INTERVALS.len();
                                                cron_builder.interval_idx = ((cron_builder.interval_idx as isize + delta as isize + total as isize) % total as isize) as usize;
                                            }
                                            // Hourly: sf=1 → minute
                                            (1, 1) => {
                                                cron_builder.minute = ((cron_builder.minute as i16 + (delta as i16 * 5) + 60) % 60) as u8;
                                            }
                                            // Daily/Weekdays: sf=1 → hour, sf=2 → minute
                                            (2 | 3, 1) => {
                                                cron_builder.hour = ((cron_builder.hour as i16 + delta as i16 + 24) % 24) as u8;
                                            }
                                            (2 | 3, 2) => {
                                                cron_builder.minute = ((cron_builder.minute as i16 + (delta as i16 * 5) + 60) % 60) as u8;
                                            }
                                            // Weekly: sf=1 → weekday, sf=2 → hour, sf=3 → minute
                                            (4, 1) => {
                                                cron_builder.weekday = ((cron_builder.weekday as i16 + delta as i16 + 7) % 7) as u8;
                                            }
                                            (4, 2) => {
                                                cron_builder.hour = ((cron_builder.hour as i16 + delta as i16 + 24) % 24) as u8;
                                            }
                                            (4, 3) => {
                                                cron_builder.minute = ((cron_builder.minute as i16 + (delta as i16 * 5) + 60) % 60) as u8;
                                            }
                                            // Monthly: sf=1 → day, sf=2 → hour, sf=3 → minute
                                            (5, 1) => {
                                                cron_builder.month_day = ((cron_builder.month_day as i16 - 1 + delta as i16 + 28) % 28 + 1) as u8;
                                            }
                                            (5, 2) => {
                                                cron_builder.hour = ((cron_builder.hour as i16 + delta as i16 + 24) % 24) as u8;
                                            }
                                            (5, 3) => {
                                                cron_builder.minute = ((cron_builder.minute as i16 + (delta as i16 * 5) + 60) % 60) as u8;
                                            }
                                            // Custom: no Left/Right adjustment
                                            _ => {}
                                        }
                                    }
                                    // Custom mode typing
                                    KeyCode::Char(c) if cron_builder.mode == 6 && sf == 1 => {
                                        cron_builder.custom.push(c);
                                    }
                                    KeyCode::Backspace if cron_builder.mode == 6 && sf == 1 => {
                                        cron_builder.custom.pop();
                                    }
                                    _ => {}
                                }
                            }
                            // Prompt field (2) — multi-line
                            2 => match key.code {
                                KeyCode::Char(c) => {
                                    if prompt_cursor_row < prompt.len() {
                                        let col = prompt_cursor_col.min(prompt[prompt_cursor_row].len());
                                        prompt[prompt_cursor_row].insert(col, c);
                                        prompt_cursor_col = col + 1;
                                    }
                                }
                                KeyCode::Backspace => {
                                    if prompt_cursor_col > 0 && prompt_cursor_row < prompt.len() {
                                        let col = prompt_cursor_col.min(prompt[prompt_cursor_row].len());
                                        if col > 0 {
                                            prompt[prompt_cursor_row].remove(col - 1);
                                            prompt_cursor_col = col - 1;
                                        }
                                    } else if prompt_cursor_col == 0 && prompt_cursor_row > 0 {
                                        // Merge with previous line
                                        let current = prompt.remove(prompt_cursor_row);
                                        prompt_cursor_row -= 1;
                                        prompt_cursor_col = prompt[prompt_cursor_row].len();
                                        prompt[prompt_cursor_row].push_str(&current);
                                    }
                                }
                                KeyCode::Enter => {
                                    if prompt_cursor_row < prompt.len() {
                                        let col = prompt_cursor_col.min(prompt[prompt_cursor_row].len());
                                        let rest = prompt[prompt_cursor_row][col..].to_string();
                                        prompt[prompt_cursor_row].truncate(col);
                                        prompt.insert(prompt_cursor_row + 1, rest);
                                        prompt_cursor_row += 1;
                                        prompt_cursor_col = 0;
                                    }
                                }
                                KeyCode::Up => {
                                    if prompt_cursor_row > 0 {
                                        prompt_cursor_row -= 1;
                                        prompt_cursor_col = prompt_cursor_col
                                            .min(prompt[prompt_cursor_row].len());
                                    }
                                }
                                KeyCode::Down => {
                                    if prompt_cursor_row + 1 < prompt.len() {
                                        prompt_cursor_row += 1;
                                        prompt_cursor_col = prompt_cursor_col
                                            .min(prompt[prompt_cursor_row].len());
                                    }
                                }
                                KeyCode::Left => {
                                    if prompt_cursor_col > 0 {
                                        prompt_cursor_col -= 1;
                                    }
                                }
                                KeyCode::Right => {
                                    if prompt_cursor_row < prompt.len()
                                        && prompt_cursor_col < prompt[prompt_cursor_row].len()
                                    {
                                        prompt_cursor_col += 1;
                                    }
                                }
                                _ => {}
                            },
                            // Notify toggle (3)
                            3 => match key.code {
                                KeyCode::Char(' ') | KeyCode::Enter => {
                                    notify = !notify;
                                }
                                _ => {}
                            },
                            _ => {}
                        },
                    }

                    self.settings_popup = Some(SettingsPopup::ScheduleEditor {
                        editing,
                        name,
                        cron_builder,
                        prompt,
                        prompt_cursor_row,
                        prompt_cursor_col,
                        notify,
                        focused,
                        error,
                    });
                }
            }
            None => {}
        }
    }

    /// Poll for streamed chat tokens. Call from the event loop.
    pub fn poll_chat_tokens(&mut self) {
        let Some(ref mut rx) = self.chat_token_rx else {
            return;
        };

        while let Ok(token) = rx.try_recv() {
            if token.starts_with("\n[[DONE") {
                // If Voice is enabled, pass the fully generated response to Piper TTS
                if self
                    .settings
                    .as_ref()
                    .map(|s| s.voice_enabled)
                    .unwrap_or(true)
                    && let Some(last) = self.chat_messages.last()
                {
                    let text = last.content.clone();
                    // Resolve TTS model: explicit setting wins, otherwise
                    // look under the per-user models directory. Missing model
                    // means "skip TTS" — we never fall back to a developer
                    // machine path.
                    let tts_model = self
                        .settings
                        .as_ref()
                        .and_then(|s| s.tts_model_path.clone())
                        .or_else(|| {
                            let candidate = self
                                .state
                                .as_ref()?
                                .dirs
                                .root()
                                .join("models/en_US-lessac-medium.onnx");
                            candidate.exists().then(|| candidate.display().to_string())
                        });
                    let runtime_dir = self.voice_runtime_dir();

                    if let (Some(tts_model), Some(runtime_dir)) = (tts_model, runtime_dir) {
                        let tts_wav = runtime_dir.join("tts.wav");
                        tokio::spawn(async move {
                            use tokio::io::AsyncWriteExt;

                            // Spawn piper directly (no shell) and feed text
                            // through stdin. This eliminates the previous
                            // `sh -c "cat /tmp/aivyx-tts.txt | piper -m '{}' ..."`
                            // which was both a shell-injection sink (model
                            // path interpolated into the command string) and
                            // a disk leak of every LLM response through a
                            // world-readable /tmp file.
                            let mut child = match tokio::process::Command::new("piper")
                                .arg("-m")
                                .arg(&tts_model)
                                .arg("-f")
                                .arg(&tts_wav)
                                .stdin(std::process::Stdio::piped())
                                .stdout(std::process::Stdio::null())
                                .stderr(std::process::Stdio::null())
                                .spawn()
                            {
                                Ok(c) => c,
                                Err(e) => {
                                    tracing::error!("Failed to spawn piper: {e}");
                                    return;
                                }
                            };

                            if let Some(mut stdin) = child.stdin.take() {
                                let _ = stdin.write_all(text.as_bytes()).await;
                                let _ = stdin.shutdown().await;
                            }

                            let status = child.wait().await;
                            if status.map(|s| s.success()).unwrap_or(false) {
                                let _ = tokio::process::Command::new("aplay")
                                    .arg(&tts_wav)
                                    .stdout(std::process::Stdio::null())
                                    .stderr(std::process::Stdio::null())
                                    .status()
                                    .await;
                            }
                        });
                    } else {
                        tracing::warn!("TTS skipped: model or runtime dir unavailable");
                    }
                }

                // Auto-save: persist the full conversation (including tool results)
                // to the encrypted store so sessions survive restarts.
                if let Some(ref state) = self.state {
                    // Ensure we have a stable session ID for this conversation
                    let sid = self
                        .chat_session_id
                        .get_or_insert_with(|| format!("{}", chrono::Utc::now().timestamp_millis()))
                        .clone();

                    let agent = state.agent.clone();
                    let store = state.store.clone();
                    let conv_key = state.conversation_key.clone();
                    let chat_msgs = self.chat_messages.clone();

                    self.chat_save_tasks.spawn(async move {
                        let agent = agent.lock().await;
                        let conversation = agent.conversation();

                        // Extract the full conversation from the agent
                        let session_msgs =
                            aivyx_pa::sessions::conversation_to_session(conversation);
                        if session_msgs.is_empty() {
                            return;
                        }

                        // Derive title from first user message in chat view
                        let title = chat_msgs
                            .first()
                            .filter(|m| m.role == "user")
                            .map(|m| aivyx_pa::sessions::auto_title(&m.content))
                            .unwrap_or_else(|| {
                                // Fall back to existing session title if available
                                aivyx_pa::sessions::list_chat_sessions(&store, &conv_key)
                                    .into_iter()
                                    .find(|s| s.id == sid)
                                    .map(|s| s.title)
                                    .unwrap_or_else(|| "Untitled".into())
                            });

                        let mut meta = aivyx_pa::sessions::ChatSessionMeta::new(title);
                        meta.id = sid;
                        meta.turn_count = aivyx_pa::sessions::count_turns(&session_msgs);
                        meta.updated_at = chrono::Utc::now();
                        // NOTE: save_chat_session is now fallible and atomic —
                        // meta + payload land in a single redb txn so a partial
                        // write can no longer leave an orphaned meta pointing at
                        // a missing body. Log-and-continue here: we're in a
                        // detached task with no user-facing channel to surface
                        // the error, but tui.log will still capture it.
                        if let Err(e) = aivyx_pa::sessions::save_chat_session(
                            &store,
                            &conv_key,
                            &meta,
                            &session_msgs,
                        ) {
                            tracing::error!(
                                session_id = %meta.id,
                                "chat session save failed: {e}"
                            );
                            return;
                        }

                        // Persist the agent's ephemeral learned state alongside
                        // the conversation so it can be restored on session load.
                        // This is a second independent put — tracked separately
                        // under C1 (store namespace work).
                        let resume = aivyx_pa::sessions::ResumeToken::from_snapshot(
                            agent.export_resume_state(),
                        );
                        aivyx_pa::sessions::save_resume_token(&store, &conv_key, &meta.id, &resume);
                    });
                }

                self.chat_streaming = false;
                self.chat_token_rx = None;
                // Stream finished naturally — drop the cancellation
                // handle so the next `cancel_chat_stream` call is a no-op.
                self.chat_cancel = None;
                return;
            }
            // Append to the last (assistant) message
            if let Some(last) = self.chat_messages.last_mut() {
                last.content.push_str(&token);
            }
        }
    }

    /// Poll for asynchronous voice transcripts from the STT pipeline.
    pub fn poll_voice_transcripts(&mut self) {
        let mut text_to_send = None;
        let mut clear_flag = false;

        if let Some(rx) = &mut self.voice_transcript_rx {
            while let Ok(transcript) = rx.try_recv() {
                if transcript == "__CLEAR_TRANSCRIBING__" {
                    clear_flag = true;
                } else {
                    text_to_send = Some(transcript);
                }
            }
        }

        if clear_flag {
            self.voice_transcribing = false;
        }

        if let Some(text) = text_to_send {
            self.chat_input = text;
            self.chat_input_cursor = self.chat_input.chars().count();
            self.send_chat_message();
        }
    }

    /// Poll for agent lifecycle events. Call from the event loop alongside
    /// `poll_chat_tokens`.
    pub fn poll_agent_events(&mut self) {
        let Some(ref mut rx) = self.chat_event_rx else {
            return;
        };

        while let Ok(event) = rx.try_recv() {
            match event {
                AgentEvent::TurnStarted { .. } => {
                    self.chat_tool_status.clear();
                    self.chat_compacting = false;
                }
                AgentEvent::ToolCallStarted {
                    tool_name,
                    tool_call_id,
                    ..
                } => {
                    self.chat_tool_status.push(ToolStatusEntry {
                        tool_name,
                        tool_call_id,
                        duration_ms: None,
                        error: None,
                        denied: false,
                    });
                }
                AgentEvent::ToolCallCompleted {
                    tool_call_id,
                    duration_ms,
                    ..
                } => {
                    if let Some(entry) = self
                        .chat_tool_status
                        .iter_mut()
                        .find(|e| e.tool_call_id == tool_call_id)
                    {
                        entry.duration_ms = Some(duration_ms);
                    }
                }
                AgentEvent::ToolCallFailed {
                    tool_call_id,
                    error,
                    duration_ms,
                    ..
                } => {
                    if let Some(entry) = self
                        .chat_tool_status
                        .iter_mut()
                        .find(|e| e.tool_call_id == tool_call_id)
                    {
                        entry.duration_ms = Some(duration_ms);
                        entry.error = Some(error);
                    }
                }
                AgentEvent::ToolCallDenied {
                    tool_call_id,
                    reason,
                    ..
                } => {
                    if let Some(entry) = self
                        .chat_tool_status
                        .iter_mut()
                        .find(|e| e.tool_call_id == tool_call_id)
                    {
                        entry.error = Some(reason);
                        entry.denied = true;
                    } else {
                        // Denied before a Started event was emitted (e.g. Locked tier)
                        self.chat_tool_status.push(ToolStatusEntry {
                            tool_name: String::new(),
                            tool_call_id,
                            duration_ms: Some(0),
                            error: Some(reason),
                            denied: true,
                        });
                    }
                }
                AgentEvent::ConversationCompacting { .. } => {
                    self.chat_compacting = true;
                }
                AgentEvent::TurnCompleted {
                    tool_calls_made,
                    total_tokens,
                    ..
                } => {
                    self.turn_tool_calls = tool_calls_made;
                    self.turn_total_tokens = total_tokens;
                    self.turn_tool_log = self.chat_tool_status.clone();
                    self.chat_compacting = false;
                }
                AgentEvent::LoopIteration { .. } => {
                    // Could be used for a loop counter display; for now just
                    // clear compaction flag since we're past it.
                    self.chat_compacting = false;
                }

                // ── Team delegation events → agent monitor ───────
                AgentEvent::DelegationStarted {
                    specialist, task, ..
                } => {
                    let status = self
                        .agent_statuses
                        .iter_mut()
                        .find(|a| a.name == specialist);
                    if let Some(status) = status {
                        status.state = "running".into();
                        status.current_task = Some(task.clone());
                        status.started_at = Some(chrono::Utc::now());
                        status.tool_log.clear();
                    } else {
                        let mut s = AgentStatus::new(specialist.clone());
                        s.state = "running".into();
                        s.current_task = Some(task.clone());
                        s.started_at = Some(chrono::Utc::now());
                        self.agent_statuses.push(s);
                    }
                    // Push as notification for "All" timeline
                    self.notifications.insert(
                        0,
                        Notification {
                            id: uuid::Uuid::new_v4().to_string(),
                            kind: aivyx_loop::NotificationKind::Info,
                            title: format!("Delegated to {specialist}"),
                            body: task,
                            source: "agent".into(),
                            timestamp: chrono::Utc::now(),
                            requires_approval: false,
                            goal_id: None,
                        },
                    );
                }
                AgentEvent::DelegationCompleted {
                    specialist,
                    status,
                    duration_ms,
                    ..
                } => {
                    if let Some(agent) = self
                        .agent_statuses
                        .iter_mut()
                        .find(|a| a.name == specialist)
                    {
                        agent.state = status.clone();
                        agent.last_duration_ms = Some(duration_ms);
                        agent.current_task = None;
                        agent.started_at = None;
                    }
                    // Push completion notification
                    let icon = if status == "completed" { "✓" } else { "✗" };
                    self.notifications.insert(
                        0,
                        Notification {
                            id: uuid::Uuid::new_v4().to_string(),
                            kind: aivyx_loop::NotificationKind::Info,
                            title: format!("{icon} {specialist} {status}"),
                            body: format!("Finished in {duration_ms}ms"),
                            source: "agent".into(),
                            timestamp: chrono::Utc::now(),
                            requires_approval: false,
                            goal_id: None,
                        },
                    );
                }
                AgentEvent::SpecialistToolCall {
                    specialist,
                    tool_name,
                    status,
                    duration_ms,
                    error,
                    ..
                } => {
                    if let Some(agent) = self
                        .agent_statuses
                        .iter_mut()
                        .find(|a| a.name == specialist)
                    {
                        if status == "started" {
                            // Add new entry at front
                            agent.tool_log.insert(
                                0,
                                SpecialistToolEntry {
                                    tool_name,
                                    status,
                                    duration_ms,
                                    error,
                                },
                            );
                            agent.tool_log.truncate(MAX_SPECIALIST_TOOL_LOG);
                        } else {
                            // Update existing entry for this tool
                            if let Some(entry) = agent
                                .tool_log
                                .iter_mut()
                                .find(|e| e.tool_name == tool_name && e.status == "started")
                            {
                                entry.status = status;
                                entry.duration_ms = duration_ms;
                                entry.error = error;
                            }
                        }
                    }
                }
            }
        }

        // When streaming ends, also drop the event receiver
        if !self.chat_streaming && self.chat_token_rx.is_none() {
            self.chat_event_rx = None;
        }
    }

    // ── Goal CRUD ─────────────────────────────────────────────

    pub fn priority_from_index(idx: usize) -> Priority {
        match idx {
            0 => Priority::Background,
            1 => Priority::Low,
            3 => Priority::High,
            4 => Priority::Critical,
            _ => Priority::Medium,
        }
    }

    pub fn priority_to_index(p: &Priority) -> usize {
        match p {
            Priority::Background => 0,
            Priority::Low => 1,
            Priority::Medium => 2,
            Priority::High => 3,
            Priority::Critical => 4,
        }
    }

    /// Handle keystrokes when a goal popup is active.
    pub fn handle_goal_popup(&mut self, key: KeyEvent) {
        use crossterm::event::KeyCode;

        match self.goal_popup.take() {
            Some(GoalPopup::Create {
                mut description,
                mut criteria,
                mut priority,
                mut focused_field,
            }) => {
                match key.code {
                    KeyCode::Esc => {} // close
                    KeyCode::Tab => {
                        focused_field = (focused_field + 1) % 3;
                        self.goal_popup = Some(GoalPopup::Create {
                            description,
                            criteria,
                            priority,
                            focused_field,
                        });
                    }
                    KeyCode::Enter if !description.is_empty() => {
                        // Create the goal
                        if let (Some(store), Some(bk)) = (
                            self.state.as_ref().and_then(|s| s.brain_store.as_ref()),
                            self.state.as_ref().and_then(|s| s.brain_key.as_ref()),
                        ) {
                            let crit = if criteria.is_empty() {
                                description.clone()
                            } else {
                                criteria
                            };
                            let goal = Goal::new(description, crit)
                                .with_priority(Self::priority_from_index(priority));
                            let _ = store.upsert_goal(&goal, bk);
                        }
                    }
                    KeyCode::Left if focused_field == 2 => {
                        priority = priority.saturating_sub(1);
                        self.goal_popup = Some(GoalPopup::Create {
                            description,
                            criteria,
                            priority,
                            focused_field,
                        });
                    }
                    KeyCode::Right if focused_field == 2 => {
                        if priority < 4 {
                            priority += 1;
                        }
                        self.goal_popup = Some(GoalPopup::Create {
                            description,
                            criteria,
                            priority,
                            focused_field,
                        });
                    }
                    KeyCode::Backspace => {
                        match focused_field {
                            0 => {
                                description.pop();
                            }
                            1 => {
                                criteria.pop();
                            }
                            _ => {}
                        }
                        self.goal_popup = Some(GoalPopup::Create {
                            description,
                            criteria,
                            priority,
                            focused_field,
                        });
                    }
                    KeyCode::Char(c) if focused_field < 2 => {
                        match focused_field {
                            0 => description.push(c),
                            1 => criteria.push(c),
                            _ => {}
                        }
                        self.goal_popup = Some(GoalPopup::Create {
                            description,
                            criteria,
                            priority,
                            focused_field,
                        });
                    }
                    _ => {
                        self.goal_popup = Some(GoalPopup::Create {
                            description,
                            criteria,
                            priority,
                            focused_field,
                        });
                    }
                }
            }
            Some(GoalPopup::Edit {
                goal_id,
                mut description,
                mut criteria,
                mut priority,
                mut deadline,
                mut focused_field,
            }) => {
                match key.code {
                    KeyCode::Esc => {} // close
                    KeyCode::Tab => {
                        focused_field = (focused_field + 1) % 4;
                        self.goal_popup = Some(GoalPopup::Edit {
                            goal_id,
                            description,
                            criteria,
                            priority,
                            deadline,
                            focused_field,
                        });
                    }
                    KeyCode::Enter if !description.is_empty() => {
                        if let (Some(store), Some(bk)) = (
                            self.state.as_ref().and_then(|s| s.brain_store.as_ref()),
                            self.state.as_ref().and_then(|s| s.brain_key.as_ref()),
                        ) && let Ok(Some(mut goal)) = store.get_goal(goal_id, bk)
                        {
                            goal.description = description;
                            goal.success_criteria = if criteria.is_empty() {
                                goal.success_criteria
                            } else {
                                criteria
                            };
                            goal.priority = Self::priority_from_index(priority);
                            goal.deadline =
                                chrono::NaiveDate::parse_from_str(&deadline, "%Y-%m-%d")
                                    .ok()
                                    .and_then(|d| d.and_hms_opt(23, 59, 59))
                                    .map(|dt| dt.and_utc());
                            goal.updated_at = chrono::Utc::now();
                            let _ = store.upsert_goal(&goal, bk);
                        }
                    }
                    KeyCode::Left if focused_field == 2 => {
                        priority = priority.saturating_sub(1);
                        self.goal_popup = Some(GoalPopup::Edit {
                            goal_id,
                            description,
                            criteria,
                            priority,
                            deadline,
                            focused_field,
                        });
                    }
                    KeyCode::Right if focused_field == 2 => {
                        if priority < 4 {
                            priority += 1;
                        }
                        self.goal_popup = Some(GoalPopup::Edit {
                            goal_id,
                            description,
                            criteria,
                            priority,
                            deadline,
                            focused_field,
                        });
                    }
                    KeyCode::Backspace => {
                        match focused_field {
                            0 => {
                                description.pop();
                            }
                            1 => {
                                criteria.pop();
                            }
                            3 => {
                                deadline.pop();
                            }
                            _ => {}
                        }
                        self.goal_popup = Some(GoalPopup::Edit {
                            goal_id,
                            description,
                            criteria,
                            priority,
                            deadline,
                            focused_field,
                        });
                    }
                    KeyCode::Char(c) => {
                        match focused_field {
                            0 => description.push(c),
                            1 => criteria.push(c),
                            3 if c.is_ascii_digit() || c == '-' => deadline.push(c),
                            _ => {}
                        }
                        self.goal_popup = Some(GoalPopup::Edit {
                            goal_id,
                            description,
                            criteria,
                            priority,
                            deadline,
                            focused_field,
                        });
                    }
                    _ => {
                        self.goal_popup = Some(GoalPopup::Edit {
                            goal_id,
                            description,
                            criteria,
                            priority,
                            deadline,
                            focused_field,
                        });
                    }
                }
            }
            Some(GoalPopup::Confirm { message, action }) => {
                match key.code {
                    KeyCode::Char('y') | KeyCode::Enter => {
                        if let (Some(store), Some(bk)) = (
                            self.state.as_ref().and_then(|s| s.brain_store.as_ref()),
                            self.state.as_ref().and_then(|s| s.brain_key.as_ref()),
                        ) {
                            let goal_id = match &action {
                                GoalAction::Complete(id) | GoalAction::Abandon(id) => *id,
                            };
                            if let Ok(Some(mut goal)) = store.get_goal(goal_id, bk) {
                                match action {
                                    GoalAction::Complete(_) => {
                                        goal.set_status(GoalStatus::Completed)
                                    }
                                    GoalAction::Abandon(_) => {
                                        goal.set_status(GoalStatus::Abandoned)
                                    }
                                }
                                let _ = store.upsert_goal(&goal, bk);
                            }
                        }
                    }
                    KeyCode::Char('n') | KeyCode::Esc => {} // cancelled
                    _ => {
                        self.goal_popup = Some(GoalPopup::Confirm { message, action });
                    }
                }
            }
            None => {}
        }
    }

    // ── Chat popup handler ────────────────────────────────────

    /// Handle key events when a chat popup is open.
    pub fn handle_chat_popup(&mut self, key: KeyEvent) {
        match self.chat_popup.take() {
            Some(ChatPopup::ConversationHistory {
                sessions,
                mut selected,
                mut scroll_offset,
                mode,
            }) => {
                match mode {
                    HistoryMode::List => {
                        match key.code {
                            KeyCode::Esc => {} // close popup
                            KeyCode::Up | KeyCode::Char('k') => {
                                selected = selected.saturating_sub(1);
                                // Keep selection visible
                                if selected < scroll_offset {
                                    scroll_offset = selected;
                                }
                                self.chat_popup = Some(ChatPopup::ConversationHistory {
                                    sessions,
                                    selected,
                                    scroll_offset,
                                    mode: HistoryMode::List,
                                });
                            }
                            KeyCode::Down | KeyCode::Char('j') => {
                                if selected < sessions.len() {
                                    selected += 1;
                                }
                                self.chat_popup = Some(ChatPopup::ConversationHistory {
                                    sessions,
                                    selected,
                                    scroll_offset,
                                    mode: HistoryMode::List,
                                });
                            }
                            KeyCode::Enter => {
                                if selected == 0 {
                                    // "New conversation" — clear chat and agent history.
                                    // Cancel any in-flight stream first so the spawned
                                    // `restore_conversation` call doesn't queue behind
                                    // the 5-minute turn_stream lock.
                                    self.cancel_chat_stream();
                                    if let Some(ref state) = self.state {
                                        let agent = state.agent.clone();
                                        tokio::spawn(async move {
                                            let mut agent = agent.lock().await;
                                            agent.restore_conversation(vec![]);
                                        });
                                    }
                                    self.chat_messages.clear();
                                    self.chat_session_id = None;
                                    self.chat_scroll = 0;
                                } else if let Some(entry) = sessions.get(selected - 1) {
                                    self.load_session(&entry.id);
                                }
                            }
                            KeyCode::Char('d') if selected > 0 => {
                                // Ask for confirmation before deleting
                                self.chat_popup = Some(ChatPopup::ConversationHistory {
                                    sessions,
                                    selected,
                                    scroll_offset,
                                    mode: HistoryMode::ConfirmDelete,
                                });
                            }
                            KeyCode::Char('r') if selected > 0 => {
                                // Enter rename mode with current title pre-filled
                                let input = sessions
                                    .get(selected - 1)
                                    .map(|e| e.title.clone())
                                    .unwrap_or_default();
                                self.chat_popup = Some(ChatPopup::ConversationHistory {
                                    sessions,
                                    selected,
                                    scroll_offset,
                                    mode: HistoryMode::Rename { input },
                                });
                            }
                            KeyCode::Char('p') if selected > 0 => {
                                // Preview session messages (display pairs only)
                                if let Some(entry) = sessions.get(selected - 1) {
                                    // Preview is read-only: on load error we show
                                    // an empty preview and log the reason. The more
                                    // dangerous path (load_session, which re-saves
                                    // on the next turn) has stricter handling below.
                                    let lines = self
                                        .state
                                        .as_ref()
                                        .and_then(|s| match aivyx_pa::sessions::load_chat_messages(
                                            &s.store,
                                            &s.conversation_key,
                                            &entry.id,
                                        ) {
                                            Ok(opt) => opt,
                                            Err(e) => {
                                                tracing::warn!(
                                                    session_id = %entry.id,
                                                    error = %e,
                                                    "preview: session unreadable"
                                                );
                                                None
                                            }
                                        })
                                        .map(|msgs| aivyx_pa::sessions::to_display_pairs(&msgs))
                                        .unwrap_or_default();
                                    self.chat_popup = Some(ChatPopup::ConversationHistory {
                                        sessions,
                                        selected,
                                        scroll_offset,
                                        mode: HistoryMode::Preview { lines, scroll: 0 },
                                    });
                                } else {
                                    self.chat_popup = Some(ChatPopup::ConversationHistory {
                                        sessions,
                                        selected,
                                        scroll_offset,
                                        mode: HistoryMode::List,
                                    });
                                }
                            }
                            _ => {
                                self.chat_popup = Some(ChatPopup::ConversationHistory {
                                    sessions,
                                    selected,
                                    scroll_offset,
                                    mode: HistoryMode::List,
                                });
                            }
                        }
                    }
                    HistoryMode::Rename { mut input } => {
                        match key.code {
                            KeyCode::Esc => {
                                // Cancel rename, back to list
                                self.chat_popup = Some(ChatPopup::ConversationHistory {
                                    sessions,
                                    selected,
                                    scroll_offset,
                                    mode: HistoryMode::List,
                                });
                            }
                            KeyCode::Enter => {
                                let new_title = input.trim().to_string();
                                if !new_title.is_empty()
                                    && let Some(entry) = sessions.get(selected - 1)
                                    && let Some(ref state) = self.state
                                {
                                    aivyx_pa::sessions::rename_chat_session(
                                        &state.store,
                                        &state.conversation_key,
                                        &entry.id,
                                        &new_title,
                                    );
                                }
                                // Re-open with updated data
                                self.open_session_list();
                            }
                            KeyCode::Backspace => {
                                input.pop();
                                self.chat_popup = Some(ChatPopup::ConversationHistory {
                                    sessions,
                                    selected,
                                    scroll_offset,
                                    mode: HistoryMode::Rename { input },
                                });
                            }
                            KeyCode::Char(c) if input.len() < 80 => {
                                input.push(c);
                                self.chat_popup = Some(ChatPopup::ConversationHistory {
                                    sessions,
                                    selected,
                                    scroll_offset,
                                    mode: HistoryMode::Rename { input },
                                });
                            }
                            _ => {
                                self.chat_popup = Some(ChatPopup::ConversationHistory {
                                    sessions,
                                    selected,
                                    scroll_offset,
                                    mode: HistoryMode::Rename { input },
                                });
                            }
                        }
                    }
                    HistoryMode::ConfirmDelete => {
                        match key.code {
                            KeyCode::Char('y') | KeyCode::Enter => {
                                if let Some(entry) = sessions.get(selected - 1) {
                                    if let Some(ref state) = self.state {
                                        aivyx_pa::sessions::delete_chat_session(
                                            &state.store,
                                            &entry.id,
                                        );
                                    }
                                    // If deleting the active session, clear the chat
                                    if self.chat_session_id.as_deref() == Some(&entry.id) {
                                        self.chat_messages.clear();
                                        self.chat_session_id = None;
                                        self.chat_scroll = 0;
                                    }
                                }
                                self.open_session_list();
                            }
                            _ => {
                                // Any other key cancels
                                self.chat_popup = Some(ChatPopup::ConversationHistory {
                                    sessions,
                                    selected,
                                    scroll_offset,
                                    mode: HistoryMode::List,
                                });
                            }
                        }
                    }
                    HistoryMode::Preview { lines, mut scroll } => {
                        match key.code {
                            KeyCode::Esc | KeyCode::Char('q') => {
                                self.chat_popup = Some(ChatPopup::ConversationHistory {
                                    sessions,
                                    selected,
                                    scroll_offset,
                                    mode: HistoryMode::List,
                                });
                            }
                            KeyCode::Up | KeyCode::Char('k') => {
                                scroll = scroll.saturating_sub(1);
                                self.chat_popup = Some(ChatPopup::ConversationHistory {
                                    sessions,
                                    selected,
                                    scroll_offset,
                                    mode: HistoryMode::Preview { lines, scroll },
                                });
                            }
                            KeyCode::Down | KeyCode::Char('j') => {
                                scroll += 1;
                                self.chat_popup = Some(ChatPopup::ConversationHistory {
                                    sessions,
                                    selected,
                                    scroll_offset,
                                    mode: HistoryMode::Preview { lines, scroll },
                                });
                            }
                            KeyCode::Enter => {
                                // Load this session from preview
                                if let Some(entry) = sessions.get(selected - 1) {
                                    self.load_session(&entry.id);
                                }
                            }
                            _ => {
                                self.chat_popup = Some(ChatPopup::ConversationHistory {
                                    sessions,
                                    selected,
                                    scroll_offset,
                                    mode: HistoryMode::Preview { lines, scroll },
                                });
                            }
                        }
                    }
                }
            }
            Some(ChatPopup::SystemPrompt { lines, mut scroll }) => {
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => {} // close
                    KeyCode::Up | KeyCode::Char('k') => {
                        scroll = scroll.saturating_sub(1);
                        self.chat_popup = Some(ChatPopup::SystemPrompt { lines, scroll });
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        scroll += 1;
                        self.chat_popup = Some(ChatPopup::SystemPrompt { lines, scroll });
                    }
                    KeyCode::PageUp => {
                        scroll = scroll.saturating_sub(10);
                        self.chat_popup = Some(ChatPopup::SystemPrompt { lines, scroll });
                    }
                    KeyCode::PageDown => {
                        scroll += 10;
                        self.chat_popup = Some(ChatPopup::SystemPrompt { lines, scroll });
                    }
                    _ => {
                        self.chat_popup = Some(ChatPopup::SystemPrompt { lines, scroll });
                    }
                }
            }
            Some(ChatPopup::ExportDone { .. }) => {
                // Any key closes it
            }
            Some(ChatPopup::BranchManager {
                snapshots,
                mut selected,
                mut label_input,
                creating,
            }) => {
                if creating {
                    // Label input mode
                    match key.code {
                        KeyCode::Esc => {
                            // Cancel creation, go back to list
                            self.chat_popup = Some(ChatPopup::BranchManager {
                                snapshots,
                                selected,
                                label_input: String::new(),
                                creating: false,
                            });
                        }
                        KeyCode::Enter => {
                            // Create snapshot with label
                            let label = if label_input.is_empty() {
                                None
                            } else {
                                Some(label_input)
                            };
                            self.create_branch_snapshot(label);
                            // Re-open with updated list
                            self.open_branch_manager();
                        }
                        KeyCode::Backspace => {
                            label_input.pop();
                            self.chat_popup = Some(ChatPopup::BranchManager {
                                snapshots,
                                selected,
                                label_input,
                                creating: true,
                            });
                        }
                        KeyCode::Char(c) => {
                            label_input.push(c);
                            self.chat_popup = Some(ChatPopup::BranchManager {
                                snapshots,
                                selected,
                                label_input,
                                creating: true,
                            });
                        }
                        _ => {
                            self.chat_popup = Some(ChatPopup::BranchManager {
                                snapshots,
                                selected,
                                label_input,
                                creating: true,
                            });
                        }
                    }
                } else {
                    // List navigation mode
                    match key.code {
                        KeyCode::Esc => {} // close
                        KeyCode::Up | KeyCode::Char('k') => {
                            selected = selected.saturating_sub(1);
                            self.chat_popup = Some(ChatPopup::BranchManager {
                                snapshots,
                                selected,
                                label_input,
                                creating: false,
                            });
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            if selected < snapshots.len() {
                                selected += 1;
                            }
                            self.chat_popup = Some(ChatPopup::BranchManager {
                                snapshots,
                                selected,
                                label_input,
                                creating: false,
                            });
                        }
                        KeyCode::Char('n') => {
                            // New snapshot — switch to label input
                            self.chat_popup = Some(ChatPopup::BranchManager {
                                snapshots,
                                selected,
                                label_input: String::new(),
                                creating: true,
                            });
                        }
                        KeyCode::Enter => {
                            // Branch from selected snapshot
                            if let Some(snap) = snapshots.get(selected) {
                                self.branch_from_snapshot(snap.message_index);
                            }
                        }
                        KeyCode::Char('d') => {
                            // Delete selected snapshot
                            if let Some(snap) = snapshots.get(selected) {
                                self.delete_branch_snapshot(snap.message_index);
                                self.open_branch_manager();
                            } else {
                                self.chat_popup = Some(ChatPopup::BranchManager {
                                    snapshots,
                                    selected,
                                    label_input,
                                    creating: false,
                                });
                            }
                        }
                        _ => {
                            self.chat_popup = Some(ChatPopup::BranchManager {
                                snapshots,
                                selected,
                                label_input,
                                creating: false,
                            });
                        }
                    }
                }
            }
            None => {}
        }
    }

    /// Open the conversation history popup.
    pub fn open_session_list(&mut self) {
        let Some(ref state) = self.state else { return };
        let sessions: Vec<SessionEntry> =
            aivyx_pa::sessions::list_chat_sessions(&state.store, &state.conversation_key)
                .into_iter()
                .map(|s| SessionEntry {
                    id: s.id,
                    title: s.title,
                    turn_count: s.turn_count,
                    updated_at: s.updated_at.format("%Y-%m-%d %H:%M").to_string(),
                    created_at: s.created_at.format("%Y-%m-%d %H:%M").to_string(),
                })
                .collect();
        self.chat_popup = Some(ChatPopup::ConversationHistory {
            sessions,
            selected: 0,
            scroll_offset: 0,
            mode: HistoryMode::List,
        });
    }

    /// Load a session's messages into the chat view and restore
    /// the agent's conversation so subsequent turns have full context.
    fn load_session(&mut self, session_id: &str) {
        // Cancel any in-flight stream so the agent lock releases before
        // `restore_conversation` tries to acquire it. Without this, a
        // mid-stream session switch would freeze the TUI until the
        // previous turn finishes or hits its 5-minute timeout.
        self.cancel_chat_stream();
        let Some(ref state) = self.state else { return };
        // H2: distinguish "not found" from "unreadable". Only restore
        // on Ok(Some(..)). On Err we abort the load entirely — the
        // subsequent turn would otherwise re-save this session and
        // overwrite the unreadable-but-present bytes on disk.
        let messages = match aivyx_pa::sessions::load_chat_messages(
            &state.store,
            &state.conversation_key,
            session_id,
        ) {
            Ok(Some(m)) => m,
            Ok(None) => {
                tracing::info!(session_id = %session_id, "load_session: no messages persisted yet");
                return;
            }
            Err(e) => {
                tracing::error!(
                    session_id = %session_id,
                    error = %e,
                    "load_session: refusing to restore corrupted or unreadable session"
                );
                self.notifications.insert(
                    0,
                    Notification {
                        id: uuid::Uuid::new_v4().to_string(),
                        kind: aivyx_loop::NotificationKind::Urgent,
                        title: "Session load failed".into(),
                        body: format!(
                            "Session '{session_id}' exists on disk but could not be decoded. \
                             The record is preserved; do not create new turns under this ID."
                        ),
                        source: "sessions".into(),
                        timestamp: chrono::Utc::now(),
                        requires_approval: false,
                        goal_id: None,
                    },
                );
                return;
            }
        };

        // Restore agent conversation history (includes tool results)
        // and ephemeral learned state (resume token) if available.
        let agent = state.agent.clone();
        let history = aivyx_pa::sessions::to_chat_messages(&messages);
        let resume_token = aivyx_pa::sessions::load_resume_token(
            &state.store,
            &state.conversation_key,
            session_id,
        );
        tokio::spawn(async move {
            let mut agent = agent.lock().await;
            agent.restore_conversation(history);
            if let Some(token) = resume_token {
                agent.apply_resume_state(token.into_snapshot());
            }
        });

        // Display only user/assistant messages in the chat view
        let display_pairs = aivyx_pa::sessions::to_display_pairs(&messages);
        self.chat_messages = display_pairs
            .into_iter()
            .map(|(role, content)| {
                ChatMessage {
                    role: if role == "you" { "user".into() } else { role },
                    content,
                    timestamp: String::new(), // historical messages don't have timestamps
                }
            })
            .collect();
        self.chat_session_id = Some(session_id.to_string());
        self.chat_scroll = 0;
    }

    /// Open the system prompt preview popup.
    pub fn open_system_prompt_preview(&mut self) {
        let Some(ref state) = self.state else { return };
        let prompt = state.pa_config.effective_system_prompt();
        let lines: Vec<String> = prompt.lines().map(String::from).collect();
        self.chat_popup = Some(ChatPopup::SystemPrompt { lines, scroll: 0 });
    }

    /// Export the current chat as a markdown file.
    pub fn export_chat_markdown(&mut self) {
        if self.chat_messages.is_empty() {
            return;
        }

        let mut md = String::from("# Chat Export\n\n");
        md.push_str(&format!(
            "_Exported: {}_\n\n---\n\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
        ));

        for msg in &self.chat_messages {
            let role = if msg.role == "user" || msg.role == "you" {
                "You"
            } else {
                "Assistant"
            };
            md.push_str(&format!("### {role}"));
            if !msg.timestamp.is_empty() {
                md.push_str(&format!(" _{}_", msg.timestamp));
            }
            md.push_str("\n\n");
            md.push_str(&msg.content);
            md.push_str("\n\n---\n\n");
        }

        let filename = format!(
            "chat-export-{}.md",
            chrono::Local::now().format("%Y%m%d-%H%M%S")
        );
        let path = std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
            .join(&filename);

        match std::fs::write(&path, &md) {
            Ok(_) => {
                self.chat_popup = Some(ChatPopup::ExportDone {
                    path: path.display().to_string(),
                });
            }
            Err(e) => {
                tracing::warn!("Failed to export chat: {e}");
            }
        }
    }

    // ── Session Branching ─────────────────────────────────────

    /// Open the branch manager popup, listing snapshots for the current session.
    pub fn open_branch_manager(&mut self) {
        let Some(ref state) = self.state else { return };

        let snapshots = if let Ok(agent) = state.agent.try_lock() {
            let session_id = agent.session_id();
            // Route through the typed session_store API rather than
            // constructing raw redb keys here. Keeps snapshot key
            // formatting in exactly one place (aivyx-agent::session_store)
            // and prevents drift between writer (TUI) and reader (agent).
            let mut entries = match aivyx_agent::session_store::list_snapshots(
                &state.store,
                &state.conversation_key,
                &session_id,
            ) {
                Ok(snaps) => snaps
                    .into_iter()
                    .map(|snap| SnapshotEntry {
                        message_index: snap.message_index,
                        label: snap.label,
                        created_at: snap.created_at.format("%Y-%m-%d %H:%M").to_string(),
                    })
                    .collect::<Vec<_>>(),
                Err(e) => {
                    tracing::warn!("Failed to list snapshots: {e}");
                    Vec::new()
                }
            };
            entries.sort_by_key(|e| e.message_index);
            entries
        } else {
            Vec::new() // agent locked (turn in progress)
        };

        self.chat_popup = Some(ChatPopup::BranchManager {
            snapshots,
            selected: 0,
            label_input: String::new(),
            creating: false,
        });
    }

    /// Create a conversation snapshot with an optional label.
    fn create_branch_snapshot(&mut self, label: Option<String>) {
        let Some(ref state) = self.state else { return };

        if let Ok(agent) = state.agent.try_lock() {
            let snapshot = agent.create_snapshot(None, label);
            if let Err(e) = aivyx_agent::session_store::save_snapshot(
                &state.store,
                &state.conversation_key,
                &snapshot,
            ) {
                tracing::warn!("Failed to save snapshot: {e}");
            }
        }
    }

    /// Branch the conversation from a snapshot at the given message index.
    fn branch_from_snapshot(&mut self, message_index: usize) {
        // Cancel any in-flight stream so the branching call releases the
        // agent lock promptly. Branching is user-initiated, so the user
        // implicitly no longer wants the current turn's output.
        self.cancel_chat_stream();
        let Some(ref state) = self.state else { return };

        let agent = state.agent.clone();
        let store = state.store.clone();
        let conv_key = state.conversation_key.clone();

        tokio::spawn(async move {
            let mut agent = agent.lock().await;
            let session_id = agent.session_id();
            match aivyx_agent::session_store::load_snapshot(
                &store,
                &conv_key,
                &session_id,
                message_index,
            ) {
                Ok(Some(snapshot)) => {
                    let _parent_id = agent.branch_from_snapshot(&snapshot);
                }
                Ok(None) => {
                    tracing::warn!(
                        "branch_from_snapshot: no snapshot at index {message_index} for session {session_id}"
                    );
                }
                Err(e) => tracing::warn!("Failed to load snapshot: {e}"),
            }
        });

        // Clear popup immediately for responsiveness. The conversation
        // update takes effect on the next refresh_data cycle.
        self.chat_popup = None;
        self.chat_session_id = None;
        self.chat_scroll = 0;
    }

    /// Delete a snapshot at the given message index.
    fn delete_branch_snapshot(&mut self, message_index: usize) {
        let Some(ref state) = self.state else { return };

        if let Ok(agent) = state.agent.try_lock() {
            let session_id = agent.session_id();
            if let Err(e) = aivyx_agent::session_store::delete_snapshot(
                &state.store,
                &session_id,
                message_index,
            ) {
                tracing::warn!("Failed to delete snapshot: {e}");
            }
        }
    }
}

// ── Test-only constructor ─────────────────────────────────────

#[cfg(test)]
impl App {
    /// Create a minimal App with no backend for unit testing.
    /// All backend-dependent methods gracefully no-op when `state` is `None`.
    pub fn new_test() -> Self {
        Self {
            running: true,
            view: View::Home,
            nav_index: 0,
            focus: Focus::Sidebar,
            frame_count: 0,
            state: None,
            agent_name: "test-agent".into(),
            provider_label: "test".into(),
            model_name: "test-model".into(),
            autonomy_tier: "trust".into(),
            memory_count: 0,
            goal_count: 0,
            active_goals: 0,
            pending_approvals: 0,
            heartbeat_interval: 30,
            version: "0.0.0-test".into(),
            chat_input: String::new(),
            chat_messages: Vec::new(),
            chat_streaming: false,
            chat_cancel: None,
            chat_scroll: 0,
            chat_token_rx: None,
            chat_session_id: None,
            chat_input_tokens: 0,
            chat_output_tokens: 0,
            chat_context_window: 0,
            chat_cost_usd: 0.0,
            chat_popup: None,
            chat_event_rx: None,
            chat_tool_status: Vec::new(),
            chat_compacting: false,
            turn_tool_log: Vec::new(),
            turn_tool_calls: 0,
            turn_total_tokens: 0,
            goals: Vec::new(),
            goal_filter: 0,
            goal_selected: 0,
            goal_popup: None,
            approvals: Vec::new(),
            approval_selected: 0,
            approval_detail_open: false,
            approval_detail_scroll: 0,
            audit_entries: Vec::new(),
            audit_filter: 0,
            audit_selected: 0,
            audit_chain_valid: None,
            notifications: Vec::new(),
            activity_selected: 0,
            activity_filter: 0,
            agent_statuses: Vec::new(),
            agent_monitor_selected: 0,
            missions: Vec::new(),
            mission_selected: 0,
            mission_filter: 0,
            mission_detail: None,
            voice_recording: false,
            voice_transcribing: false,
            voice_process: None,
            voice_transcript_rx: None,
            voice_transcript_tx: None,
            memories: Vec::new(),
            memory_selected: 0,
            memory_total: 0,
            help_scroll: 0,
            settings: None,
            settings_error: None,
            settings_card_index: 0,
            settings_item_index: 0,
            settings_popup: None,
            tool_count: 0,
            health_provider: "n/a".into(),
            health_email: "n/a".into(),
            health_config: "n/a".into(),
            health_disk: "n/a".into(),
            health_provider_detail: None,
            health_email_detail: None,
            last_refresh: std::time::Instant::now(),

            chat_save_tasks: tokio::task::JoinSet::new(),
        }
    }
}

/// Classify an audit event into a simple type string for filtering.
/// Parse a priority prefix from user chat input.
///
/// Supported prefixes: `!critical:`, `!high:`, `!low:`, `!bg:`.
/// Returns `(Some(badge_label), stripped_message)` or `(None, original_message)`.
pub fn parse_priority_prefix(input: &str) -> (Option<&'static str>, String) {
    let trimmed = input.trim_start();
    let prefixes: &[(&str, &str)] = &[
        ("!critical:", "CRITICAL"),
        ("!urgent:", "CRITICAL"),
        ("!high:", "HIGH"),
        ("!low:", "LOW"),
        ("!bg:", "BG"),
        ("!background:", "BG"),
    ];
    for &(prefix, badge) in prefixes {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return (Some(badge), rest.trim_start().to_string());
        }
    }
    (None, input.to_string())
}

pub fn audit_event_type(event: &aivyx_audit::AuditEvent) -> &'static str {
    use aivyx_audit::AuditEvent;
    match event {
        AuditEvent::ToolExecuted { .. }
        | AuditEvent::ToolDenied { .. }
        | AuditEvent::ToolExecutionFailed { .. } => "tool",
        AuditEvent::HeartbeatFired { .. }
        | AuditEvent::HeartbeatCompleted { .. }
        | AuditEvent::HeartbeatSkipped { .. } => "heartbeat",
        AuditEvent::CapabilityGranted { .. } | AuditEvent::CapabilityRevoked { .. } => "security",
        _ => "other",
    }
}

/// Format an audit event into a brief description for display.
pub fn format_audit_event(event: &aivyx_audit::AuditEvent) -> String {
    use aivyx_audit::AuditEvent;
    match event {
        AuditEvent::SystemInit { .. } => "System initialized".into(),
        AuditEvent::ToolExecuted { action, .. } => format!("Tool: {action}"),
        AuditEvent::ToolDenied { action, reason, .. } => format!("Denied: {action} ({reason})"),
        AuditEvent::ToolExecutionFailed { action, error, .. } => {
            format!("Failed: {action} ({error})")
        }
        AuditEvent::AgentTurnStarted { .. } => "Turn started".into(),
        AuditEvent::AgentTurnCompleted { .. } => "Turn completed".into(),
        AuditEvent::ScheduleFired { schedule_name, .. } => format!("Schedule: {schedule_name}"),
        AuditEvent::ScheduleCompleted { schedule_name, .. } => {
            format!("Schedule done: {schedule_name}")
        }
        AuditEvent::MemoryStored { .. } => "Memory stored".into(),
        AuditEvent::CapabilityGranted { scope_summary, .. } => {
            format!("Cap granted: {scope_summary}")
        }
        AuditEvent::CapabilityRevoked { .. } => "Cap revoked".into(),
        AuditEvent::ConfigChanged { key, .. } => format!("Config: {key}"),
        AuditEvent::HeartbeatFired {
            context_sections, ..
        } => format!("Heartbeat ({context_sections} ctx)"),
        AuditEvent::HeartbeatCompleted {
            actions_dispatched, ..
        } => format!("Heartbeat done ({actions_dispatched} actions)"),
        AuditEvent::HeartbeatSkipped { reason } => format!("Heartbeat skip: {reason}"),
        AuditEvent::BriefingGenerated { item_count, .. } => {
            format!("Briefing ({item_count} items)")
        }
        other => {
            let debug = format!("{other:?}");
            let variant = debug.split(['{', '(']).next().unwrap_or(&debug);
            variant.trim().to_string()
        }
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Tests
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    /// Helper: create a KeyEvent with no modifiers.
    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// Helper: create a Char key event.
    fn char_key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    /// Helper: create a notification with a given source.
    fn notif(source: &str) -> Notification {
        Notification {
            id: uuid::Uuid::new_v4().to_string(),
            kind: aivyx_loop::NotificationKind::Info,
            title: format!("Test {source}"),
            body: String::new(),
            source: source.into(),
            timestamp: chrono::Utc::now(),
            requires_approval: false,
            goal_id: None,
        }
    }

    // ── Navigation ───────────────────────────────────────────

    #[test]
    fn nav_starts_at_home() {
        let app = App::new_test();
        assert_eq!(app.view, View::Home);
        assert_eq!(app.nav_index, 0);
    }

    #[test]
    fn nav_down_increments() {
        let mut app = App::new_test();
        app.nav_down();
        assert_eq!(app.view, View::Chat);
        assert_eq!(app.nav_index, 1);
    }

    #[test]
    fn nav_up_at_zero_stays() {
        let mut app = App::new_test();
        app.nav_up();
        assert_eq!(app.view, View::Home);
        assert_eq!(app.nav_index, 0);
    }

    #[test]
    fn nav_down_clamps_at_max() {
        let mut app = App::new_test();
        for _ in 0..20 {
            app.nav_down();
        }
        assert_eq!(app.nav_index, View::ALL.len() - 1);
        assert_eq!(app.view, View::Settings);
    }

    #[test]
    fn go_to_view_valid_indices() {
        let mut app = App::new_test();
        for (i, expected) in View::ALL.iter().enumerate() {
            app.go_to_view(i);
            assert_eq!(app.view, *expected);
            assert_eq!(app.nav_index, i);
        }
    }

    #[test]
    fn go_to_view_invalid_index_no_op() {
        let mut app = App::new_test();
        app.go_to_view(4); // Approvals
        app.go_to_view(100); // invalid — should not change
        assert_eq!(app.view, View::Approvals);
        assert_eq!(app.nav_index, 4);
    }

    // ── View enum ────────────────────────────────────────────

    #[test]
    fn view_all_has_ten_entries() {
        assert_eq!(View::ALL.len(), 10);
    }

    #[test]
    fn view_labels_non_empty() {
        for v in &View::ALL {
            assert!(!v.label().is_empty());
            assert!(!v.icon().is_empty());
        }
    }

    #[test]
    fn view_groups_cover_all() {
        for v in &View::ALL {
            assert!(v.group() <= 2, "group out of range for {:?}", v);
        }
    }

    // ── Priority conversion ──────────────────────────────────

    #[test]
    fn priority_round_trip() {
        use aivyx_brain::Priority::*;
        let priorities = [Background, Low, Medium, High, Critical];
        for p in &priorities {
            let idx = App::priority_to_index(p);
            let back = App::priority_from_index(idx);
            assert_eq!(*p, back, "round-trip failed for {p:?}");
        }
    }

    #[test]
    fn priority_from_index_defaults_to_medium() {
        assert_eq!(App::priority_from_index(2), Priority::Medium);
        assert_eq!(App::priority_from_index(99), Priority::Medium);
    }

    #[test]
    fn priority_to_index_range() {
        use aivyx_brain::Priority::*;
        for p in [Background, Low, Medium, High, Critical] {
            let idx = App::priority_to_index(&p);
            assert!(idx < 5, "index {idx} out of PRIORITIES range");
            assert_eq!(PRIORITIES[idx], format!("{p:?}"));
        }
    }

    // ── Settings item count ──────────────────────────────────

    #[test]
    fn settings_item_count_cards() {
        let app = App::new_test();
        assert_eq!(app.settings_item_count(0), 2); // provider
        assert_eq!(app.settings_item_count(1), 3); // autonomy
        assert_eq!(app.settings_item_count(2), 11); // heartbeat
        assert_eq!(app.settings_item_count(3), 0); // schedules — no settings loaded
        assert_eq!(app.settings_item_count(4), 3); // agent
        assert_eq!(app.settings_item_count(5), 11); // integrations
        assert_eq!(app.settings_item_count(7), 5); // persona
        assert_eq!(app.settings_item_count(8), 1); // tools & extensions: discovery
        assert_eq!(app.settings_item_count(6), 0); // unknown card
        assert_eq!(app.settings_item_count(99), 0); // out of range
    }

    #[test]
    fn integrations_list_has_eleven_entries() {
        let snapshot = SettingsSnapshot::default();
        let list = App::integrations_list(&snapshot);
        assert_eq!(list.len(), 11);
        // All should be unconfigured in default snapshot
        for (_, configured, _) in &list {
            assert!(!configured);
        }
    }

    #[test]
    fn integration_fields_all_kinds_have_fields() {
        use aivyx_pa::settings::IntegrationKind;
        let kinds = [
            IntegrationKind::Email,
            IntegrationKind::Telegram,
            IntegrationKind::Matrix,
            IntegrationKind::Calendar,
            IntegrationKind::Contacts,
            IntegrationKind::Vault,
            IntegrationKind::Signal,
            IntegrationKind::Sms,
            IntegrationKind::Finance,
            IntegrationKind::Desktop,
            IntegrationKind::DevTools,
        ];
        for kind in kinds {
            let fields = App::integration_fields(kind);
            assert!(
                !fields.is_empty(),
                "integration_fields({kind:?}) returned empty"
            );
        }
    }

    // ── Filtered goals ───────────────────────────────────────

    #[test]
    fn filtered_goals_all() {
        let mut app = App::new_test();
        app.goals = vec![
            Goal::new("g1", "c1"),
            {
                let mut g = Goal::new("g2", "c2");
                g.set_status(GoalStatus::Completed);
                g
            },
            {
                let mut g = Goal::new("g3", "c3");
                g.set_status(GoalStatus::Abandoned);
                g
            },
        ];
        app.goal_filter = 0;
        assert_eq!(app.filtered_goals().len(), 3);
    }

    #[test]
    fn filtered_goals_by_status() {
        let mut app = App::new_test();
        app.goals = vec![Goal::new("active1", "c"), Goal::new("active2", "c"), {
            let mut g = Goal::new("done", "c");
            g.set_status(GoalStatus::Completed);
            g
        }];

        app.goal_filter = 1; // Active
        assert_eq!(app.filtered_goals().len(), 2);

        app.goal_filter = 2; // Completed
        assert_eq!(app.filtered_goals().len(), 1);

        app.goal_filter = 3; // Abandoned
        assert_eq!(app.filtered_goals().len(), 0);
    }

    // ── Filtered notifications ───────────────────────────────

    #[test]
    fn filtered_notifications_all() {
        let mut app = App::new_test();
        app.notifications = vec![notif("schedule"), notif("heartbeat"), notif("other")];
        app.activity_filter = 0;
        assert_eq!(app.filtered_notifications().len(), 3);
    }

    #[test]
    fn filtered_notifications_agents() {
        let mut app = App::new_test();
        app.notifications = vec![
            notif("agent"),
            notif("schedule"),
            notif("agent"),
            notif("other"),
        ];
        app.activity_filter = 1;
        let filtered = app.filtered_notifications();
        assert_eq!(filtered.len(), 2);
        assert!(filtered.iter().all(|n| n.source == "agent"));
    }

    #[test]
    fn filtered_notifications_schedule() {
        let mut app = App::new_test();
        app.notifications = vec![
            notif("schedule"),
            notif("briefing"),
            notif("heartbeat"),
            notif("other"),
        ];
        app.activity_filter = 2;
        let filtered = app.filtered_notifications();
        assert_eq!(filtered.len(), 2);
        assert!(
            filtered
                .iter()
                .all(|n| n.source == "schedule" || n.source == "briefing")
        );
    }

    #[test]
    fn filtered_notifications_heartbeat() {
        let mut app = App::new_test();
        app.notifications = vec![
            notif("heartbeat"),
            notif("heartbeat-check"),
            notif("schedule"),
        ];
        app.activity_filter = 3;
        let filtered = app.filtered_notifications();
        assert_eq!(filtered.len(), 2);
        assert!(filtered.iter().all(|n| n.source.contains("heartbeat")));
    }

    // ── Chat popup: ConversationHistory ─────────────────────

    fn test_session_entry(id: &str, title: &str) -> SessionEntry {
        SessionEntry {
            id: id.into(),
            title: title.into(),
            turn_count: 1,
            updated_at: "2026-01-01".into(),
            created_at: "2026-01-01".into(),
        }
    }

    fn history_popup(sessions: Vec<SessionEntry>, selected: usize) -> ChatPopup {
        ChatPopup::ConversationHistory {
            sessions,
            selected,
            scroll_offset: 0,
            mode: HistoryMode::List,
        }
    }

    #[test]
    fn chat_popup_history_esc_closes() {
        let mut app = App::new_test();
        app.chat_popup = Some(history_popup(vec![], 0));
        app.handle_chat_popup(key(KeyCode::Esc));
        assert!(app.chat_popup.is_none());
    }

    #[test]
    fn chat_popup_history_navigate() {
        let mut app = App::new_test();
        let sessions = vec![
            test_session_entry("s1", "First"),
            test_session_entry("s2", "Second"),
        ];
        app.chat_popup = Some(history_popup(sessions, 0));

        // Down navigates
        app.handle_chat_popup(key(KeyCode::Down));
        match &app.chat_popup {
            Some(ChatPopup::ConversationHistory { selected, .. }) => assert_eq!(*selected, 1),
            other => panic!("Expected ConversationHistory, got {other:?}"),
        }

        // Up navigates back
        app.handle_chat_popup(key(KeyCode::Up));
        match &app.chat_popup {
            Some(ChatPopup::ConversationHistory { selected, .. }) => assert_eq!(*selected, 0),
            other => panic!("Expected ConversationHistory, got {other:?}"),
        }

        // Up at 0 stays at 0
        app.handle_chat_popup(key(KeyCode::Up));
        match &app.chat_popup {
            Some(ChatPopup::ConversationHistory { selected, .. }) => assert_eq!(*selected, 0),
            other => panic!("Expected ConversationHistory, got {other:?}"),
        }
    }

    #[test]
    fn chat_popup_history_vim_keys() {
        let mut app = App::new_test();
        app.chat_popup = Some(history_popup(vec![test_session_entry("s1", "A")], 0));

        app.handle_chat_popup(char_key('j'));
        match &app.chat_popup {
            Some(ChatPopup::ConversationHistory { selected, .. }) => assert_eq!(*selected, 1),
            other => panic!("Expected ConversationHistory, got {other:?}"),
        }
        app.handle_chat_popup(char_key('k'));
        match &app.chat_popup {
            Some(ChatPopup::ConversationHistory { selected, .. }) => assert_eq!(*selected, 0),
            other => panic!("Expected ConversationHistory, got {other:?}"),
        }
    }

    #[test]
    fn chat_popup_history_enter_new_clears_chat() {
        let mut app = App::new_test();
        app.chat_messages.push(ChatMessage {
            role: "user".into(),
            content: "old message".into(),
            timestamp: "12:00".into(),
        });
        app.chat_session_id = Some("old-session".into());
        app.chat_popup = Some(history_popup(vec![], 0));

        app.handle_chat_popup(key(KeyCode::Enter)); // selected=0 → "New conversation"
        assert!(app.chat_messages.is_empty());
        assert!(app.chat_session_id.is_none());
        assert_eq!(app.chat_scroll, 0);
    }

    #[test]
    fn chat_popup_history_d_opens_confirm_delete() {
        let mut app = App::new_test();
        let sessions = vec![test_session_entry("s1", "To delete")];
        app.chat_popup = Some(history_popup(sessions, 1));

        app.handle_chat_popup(char_key('d'));
        match &app.chat_popup {
            Some(ChatPopup::ConversationHistory {
                mode: HistoryMode::ConfirmDelete,
                ..
            }) => {}
            other => panic!("Expected ConfirmDelete mode, got {other:?}"),
        }
    }

    #[test]
    fn chat_popup_history_confirm_delete_cancel() {
        let mut app = App::new_test();
        let sessions = vec![test_session_entry("s1", "Keep me")];
        app.chat_popup = Some(ChatPopup::ConversationHistory {
            sessions,
            selected: 1,
            scroll_offset: 0,
            mode: HistoryMode::ConfirmDelete,
        });

        app.handle_chat_popup(key(KeyCode::Esc)); // cancel
        match &app.chat_popup {
            Some(ChatPopup::ConversationHistory {
                mode: HistoryMode::List,
                ..
            }) => {}
            other => panic!("Expected List mode after cancel, got {other:?}"),
        }
    }

    #[test]
    fn chat_popup_history_r_opens_rename() {
        let mut app = App::new_test();
        let sessions = vec![test_session_entry("s1", "Original")];
        app.chat_popup = Some(history_popup(sessions, 1));

        app.handle_chat_popup(char_key('r'));
        match &app.chat_popup {
            Some(ChatPopup::ConversationHistory {
                mode: HistoryMode::Rename { input },
                ..
            }) => {
                assert_eq!(input, "Original");
            }
            other => panic!("Expected Rename mode, got {other:?}"),
        }
    }

    #[test]
    fn chat_popup_history_rename_esc_cancels() {
        let mut app = App::new_test();
        let sessions = vec![test_session_entry("s1", "Orig")];
        app.chat_popup = Some(ChatPopup::ConversationHistory {
            sessions,
            selected: 1,
            scroll_offset: 0,
            mode: HistoryMode::Rename {
                input: "New name".into(),
            },
        });

        app.handle_chat_popup(key(KeyCode::Esc));
        match &app.chat_popup {
            Some(ChatPopup::ConversationHistory {
                mode: HistoryMode::List,
                ..
            }) => {}
            other => panic!("Expected List mode after rename cancel, got {other:?}"),
        }
    }

    // ── Chat popup: SystemPrompt ─────────────────────────────

    #[test]
    fn chat_popup_system_prompt_esc_closes() {
        let mut app = App::new_test();
        app.chat_popup = Some(ChatPopup::SystemPrompt {
            lines: vec!["Line 1".into()],
            scroll: 0,
        });
        app.handle_chat_popup(key(KeyCode::Esc));
        assert!(app.chat_popup.is_none());
    }

    #[test]
    fn chat_popup_system_prompt_q_closes() {
        let mut app = App::new_test();
        app.chat_popup = Some(ChatPopup::SystemPrompt {
            lines: vec!["Line 1".into()],
            scroll: 0,
        });
        app.handle_chat_popup(char_key('q'));
        assert!(app.chat_popup.is_none());
    }

    #[test]
    fn chat_popup_system_prompt_scroll() {
        let mut app = App::new_test();
        app.chat_popup = Some(ChatPopup::SystemPrompt {
            lines: (0..50).map(|i| format!("Line {i}")).collect(),
            scroll: 0,
        });

        app.handle_chat_popup(key(KeyCode::Down));
        match &app.chat_popup {
            Some(ChatPopup::SystemPrompt { scroll, .. }) => assert_eq!(*scroll, 1),
            _ => panic!("Expected SystemPrompt"),
        }

        app.handle_chat_popup(key(KeyCode::PageDown));
        match &app.chat_popup {
            Some(ChatPopup::SystemPrompt { scroll, .. }) => assert_eq!(*scroll, 11),
            _ => panic!("Expected SystemPrompt"),
        }

        app.handle_chat_popup(key(KeyCode::PageUp));
        match &app.chat_popup {
            Some(ChatPopup::SystemPrompt { scroll, .. }) => assert_eq!(*scroll, 1),
            _ => panic!("Expected SystemPrompt"),
        }

        app.handle_chat_popup(key(KeyCode::Up));
        match &app.chat_popup {
            Some(ChatPopup::SystemPrompt { scroll, .. }) => assert_eq!(*scroll, 0),
            _ => panic!("Expected SystemPrompt"),
        }

        // Up at 0 saturates to 0
        app.handle_chat_popup(key(KeyCode::Up));
        match &app.chat_popup {
            Some(ChatPopup::SystemPrompt { scroll, .. }) => assert_eq!(*scroll, 0),
            _ => panic!("Expected SystemPrompt"),
        }
    }

    // ── Chat popup: ExportDone ───────────────────────────────

    #[test]
    fn chat_popup_export_done_any_key_closes() {
        let mut app = App::new_test();
        app.chat_popup = Some(ChatPopup::ExportDone {
            path: "/tmp/test.md".into(),
        });
        app.handle_chat_popup(key(KeyCode::Enter));
        assert!(app.chat_popup.is_none());

        app.chat_popup = Some(ChatPopup::ExportDone {
            path: "/tmp/test.md".into(),
        });
        app.handle_chat_popup(char_key('x'));
        assert!(app.chat_popup.is_none());

        app.chat_popup = Some(ChatPopup::ExportDone {
            path: "/tmp/test.md".into(),
        });
        app.handle_chat_popup(key(KeyCode::Esc));
        assert!(app.chat_popup.is_none());
    }

    // ── Goal popup: Create ───────────────────────────────────

    #[test]
    fn goal_popup_create_esc_closes() {
        let mut app = App::new_test();
        app.goal_popup = Some(GoalPopup::Create {
            description: String::new(),
            criteria: String::new(),
            priority: 2,
            focused_field: 0,
        });
        app.handle_goal_popup(key(KeyCode::Esc));
        assert!(app.goal_popup.is_none());
    }

    #[test]
    fn goal_popup_create_tab_cycles_fields() {
        let mut app = App::new_test();
        app.goal_popup = Some(GoalPopup::Create {
            description: String::new(),
            criteria: String::new(),
            priority: 2,
            focused_field: 0,
        });

        app.handle_goal_popup(key(KeyCode::Tab));
        match &app.goal_popup {
            Some(GoalPopup::Create { focused_field, .. }) => assert_eq!(*focused_field, 1),
            _ => panic!("Expected Create"),
        }

        app.handle_goal_popup(key(KeyCode::Tab));
        match &app.goal_popup {
            Some(GoalPopup::Create { focused_field, .. }) => assert_eq!(*focused_field, 2),
            _ => panic!("Expected Create"),
        }

        app.handle_goal_popup(key(KeyCode::Tab));
        match &app.goal_popup {
            Some(GoalPopup::Create { focused_field, .. }) => assert_eq!(*focused_field, 0), // wraps
            _ => panic!("Expected Create"),
        }
    }

    #[test]
    fn goal_popup_create_type_description() {
        let mut app = App::new_test();
        app.goal_popup = Some(GoalPopup::Create {
            description: String::new(),
            criteria: String::new(),
            priority: 2,
            focused_field: 0,
        });

        for c in "Test Goal".chars() {
            app.handle_goal_popup(char_key(c));
        }
        match &app.goal_popup {
            Some(GoalPopup::Create { description, .. }) => assert_eq!(description, "Test Goal"),
            _ => panic!("Expected Create"),
        }

        // Backspace removes last char
        app.handle_goal_popup(key(KeyCode::Backspace));
        match &app.goal_popup {
            Some(GoalPopup::Create { description, .. }) => assert_eq!(description, "Test Goa"),
            _ => panic!("Expected Create"),
        }
    }

    #[test]
    fn goal_popup_create_priority_arrows() {
        let mut app = App::new_test();
        app.goal_popup = Some(GoalPopup::Create {
            description: String::new(),
            criteria: String::new(),
            priority: 2,
            focused_field: 2, // priority field focused
        });

        app.handle_goal_popup(key(KeyCode::Right));
        match &app.goal_popup {
            Some(GoalPopup::Create { priority, .. }) => assert_eq!(*priority, 3),
            _ => panic!("Expected Create"),
        }

        app.handle_goal_popup(key(KeyCode::Left));
        match &app.goal_popup {
            Some(GoalPopup::Create { priority, .. }) => assert_eq!(*priority, 2),
            _ => panic!("Expected Create"),
        }

        // Left at 0 saturates
        app.goal_popup = Some(GoalPopup::Create {
            description: String::new(),
            criteria: String::new(),
            priority: 0,
            focused_field: 2,
        });
        app.handle_goal_popup(key(KeyCode::Left));
        match &app.goal_popup {
            Some(GoalPopup::Create { priority, .. }) => assert_eq!(*priority, 0),
            _ => panic!("Expected Create"),
        }

        // Right at 4 clamps
        app.goal_popup = Some(GoalPopup::Create {
            description: String::new(),
            criteria: String::new(),
            priority: 4,
            focused_field: 2,
        });
        app.handle_goal_popup(key(KeyCode::Right));
        match &app.goal_popup {
            Some(GoalPopup::Create { priority, .. }) => assert_eq!(*priority, 4),
            _ => panic!("Expected Create"),
        }
    }

    #[test]
    fn goal_popup_create_enter_empty_desc_no_close() {
        let mut app = App::new_test();
        app.goal_popup = Some(GoalPopup::Create {
            description: String::new(),
            criteria: String::new(),
            priority: 2,
            focused_field: 0,
        });
        // Enter with empty description — does NOT match the `Enter if !description.is_empty()` arm,
        // falls to catch-all which keeps popup open
        app.handle_goal_popup(key(KeyCode::Enter));
        assert!(
            app.goal_popup.is_some(),
            "Popup should stay open with empty description"
        );
    }

    #[test]
    fn goal_popup_create_enter_with_desc_closes_no_backend() {
        let mut app = App::new_test();
        app.goal_popup = Some(GoalPopup::Create {
            description: "My goal".into(),
            criteria: String::new(),
            priority: 2,
            focused_field: 0,
        });
        // Enter with description but no backend — creates attempt no-ops, popup closes
        app.handle_goal_popup(key(KeyCode::Enter));
        assert!(app.goal_popup.is_none());
    }

    // ── Goal popup: Confirm ──────────────────────────────────

    #[test]
    fn goal_popup_confirm_esc_cancels() {
        let mut app = App::new_test();
        app.goal_popup = Some(GoalPopup::Confirm {
            message: "Complete this?".into(),
            action: GoalAction::Complete(GoalId::new()),
        });
        app.handle_goal_popup(key(KeyCode::Esc));
        assert!(app.goal_popup.is_none());
    }

    #[test]
    fn goal_popup_confirm_n_cancels() {
        let mut app = App::new_test();
        app.goal_popup = Some(GoalPopup::Confirm {
            message: "Abandon?".into(),
            action: GoalAction::Abandon(GoalId::new()),
        });
        app.handle_goal_popup(char_key('n'));
        assert!(app.goal_popup.is_none());
    }

    #[test]
    fn goal_popup_confirm_y_closes() {
        let mut app = App::new_test();
        app.goal_popup = Some(GoalPopup::Confirm {
            message: "Complete?".into(),
            action: GoalAction::Complete(GoalId::new()),
        });
        app.handle_goal_popup(char_key('y'));
        assert!(app.goal_popup.is_none()); // closes (no backend to act on)
    }

    #[test]
    fn goal_popup_confirm_other_key_keeps_open() {
        let mut app = App::new_test();
        app.goal_popup = Some(GoalPopup::Confirm {
            message: "Complete?".into(),
            action: GoalAction::Complete(GoalId::new()),
        });
        app.handle_goal_popup(char_key('x'));
        assert!(app.goal_popup.is_some());
    }

    // ── Goal popup: Edit ─────────────────────────────────────

    #[test]
    fn goal_popup_edit_tab_cycles_4_fields() {
        let mut app = App::new_test();
        app.goal_popup = Some(GoalPopup::Edit {
            goal_id: GoalId::new(),
            description: "desc".into(),
            criteria: "crit".into(),
            priority: 2,
            deadline: String::new(),
            focused_field: 0,
        });

        let mut fields = vec![];
        for _ in 0..5 {
            app.handle_goal_popup(key(KeyCode::Tab));
            match &app.goal_popup {
                Some(GoalPopup::Edit { focused_field, .. }) => fields.push(*focused_field),
                _ => panic!("Expected Edit"),
            }
        }
        assert_eq!(fields, vec![1, 2, 3, 0, 1]); // cycles through 4 fields
    }

    #[test]
    fn goal_popup_edit_deadline_only_digits_and_dash() {
        let mut app = App::new_test();
        app.goal_popup = Some(GoalPopup::Edit {
            goal_id: GoalId::new(),
            description: "d".into(),
            criteria: "c".into(),
            priority: 2,
            deadline: String::new(),
            focused_field: 3, // deadline field
        });

        // Valid chars
        for c in "2026-04-05".chars() {
            app.handle_goal_popup(char_key(c));
        }
        match &app.goal_popup {
            Some(GoalPopup::Edit { deadline, .. }) => assert_eq!(deadline, "2026-04-05"),
            _ => panic!("Expected Edit"),
        }

        // Letters are rejected on deadline field
        app.handle_goal_popup(char_key('a'));
        match &app.goal_popup {
            Some(GoalPopup::Edit { deadline, .. }) => assert_eq!(deadline, "2026-04-05"), // unchanged
            _ => panic!("Expected Edit"),
        }
    }

    // ── Settings popup: SkillManager ─────────────────────────

    #[test]
    fn settings_popup_skill_manager_esc_closes() {
        let mut app = App::new_test();
        // SkillManager requires state for config writes, but Esc path doesn't
        app.settings_popup = Some(SettingsPopup::SkillManager {
            input: String::new(),
            selected: 0,
            skills: vec!["email".into()],
        });
        // handle_settings_popup requires state — but Esc path just takes + drops.
        // However, the method guards `let Some(ref state) = self.state else { return }` at top.
        // So with state=None, it returns immediately without processing. That's a limitation.
        // We can test the popup was preserved (method no-ops entirely).
        app.handle_settings_popup(key(KeyCode::Esc));
        // With no state, handle_settings_popup returns early, so popup is still taken but
        // not re-inserted — it's None now because .take() happened then early return.
        // Wait, let me re-check: the method does `let Some(ref state) = self.state` BEFORE
        // the `match self.settings_popup.take()`. So it returns before take(). Popup stays.
        assert!(
            app.settings_popup.is_some(),
            "Settings popup untouched because state is None"
        );
    }

    // ── Chat export ──────────────────────────────────────────

    #[test]
    fn export_chat_markdown_empty_no_op() {
        let mut app = App::new_test();
        app.export_chat_markdown();
        assert!(app.chat_popup.is_none()); // no export done popup
    }

    #[test]
    fn export_chat_markdown_creates_file() {
        let mut app = App::new_test();
        app.chat_messages = vec![
            ChatMessage {
                role: "user".into(),
                content: "Hello".into(),
                timestamp: "10:00".into(),
            },
            ChatMessage {
                role: "assistant".into(),
                content: "Hi there!".into(),
                timestamp: "10:01".into(),
            },
        ];
        app.export_chat_markdown();

        match &app.chat_popup {
            Some(ChatPopup::ExportDone { path }) => {
                assert!(path.contains("chat-export-"));
                // Clean up
                let _ = std::fs::remove_file(path);
            }
            _ => panic!("Expected ExportDone popup"),
        }
    }

    // ── send_chat_message guards ─────────────────────────────

    #[test]
    fn send_chat_message_empty_input_no_op() {
        let mut app = App::new_test();
        app.send_chat_message();
        assert!(app.chat_messages.is_empty());
        assert!(!app.chat_streaming);
    }

    #[test]
    fn send_chat_message_no_backend_no_op() {
        let mut app = App::new_test();
        app.chat_input = "Hello".into();
        app.send_chat_message();
        // No state → returns early after checking state
        // But first it checks is_empty (false) and streaming (false), then checks state
        assert!(app.chat_messages.is_empty() || app.chat_token_rx.is_none());
    }

    #[test]
    fn send_chat_message_while_streaming_no_op() {
        let mut app = App::new_test();
        app.chat_input = "Hello".into();
        app.chat_streaming = true;
        let original_input = app.chat_input.clone();
        app.send_chat_message();
        assert_eq!(app.chat_input, original_input); // input not consumed
    }

    // ── poll_chat_tokens ─────────────────────────────────────

    #[test]
    fn poll_chat_tokens_no_rx_no_op() {
        let mut app = App::new_test();
        app.poll_chat_tokens(); // should not panic
    }

    #[tokio::test]
    async fn poll_chat_tokens_receives_and_appends() {
        let mut app = App::new_test();
        let (tx, rx) = mpsc::channel::<String>(16);
        app.chat_token_rx = Some(rx);
        app.chat_streaming = true;
        app.chat_messages.push(ChatMessage {
            role: "assistant".into(),
            content: String::new(),
            timestamp: "12:00".into(),
        });

        // Send tokens synchronously (channel is buffered)
        tx.try_send("Hello ".into()).unwrap();
        tx.try_send("world".into()).unwrap();
        app.poll_chat_tokens();
        assert_eq!(app.chat_messages.last().unwrap().content, "Hello world");
        assert!(app.chat_streaming); // not done yet

        // Send DONE signal
        tx.try_send("\n[[DONE]]".into()).unwrap();
        app.poll_chat_tokens();
        assert!(!app.chat_streaming);
        assert!(app.chat_token_rx.is_none());
    }

    // ── Audit event classification ───────────────────────────

    #[test]
    fn audit_event_type_heartbeat() {
        use aivyx_audit::AuditEvent;
        let event = AuditEvent::HeartbeatSkipped {
            reason: "quiet hours".into(),
        };
        assert_eq!(audit_event_type(&event), "heartbeat");
    }

    #[test]
    fn audit_event_type_other() {
        use aivyx_audit::AuditEvent;
        let event = AuditEvent::SystemInit {
            timestamp: chrono::Utc::now(),
        };
        assert_eq!(audit_event_type(&event), "other");
    }

    #[test]
    fn format_audit_event_heartbeat_skip() {
        use aivyx_audit::AuditEvent;
        let event = AuditEvent::HeartbeatSkipped {
            reason: "quiet hours".into(),
        };
        assert_eq!(format_audit_event(&event), "Heartbeat skip: quiet hours");
    }

    #[test]
    fn format_audit_event_system_init() {
        use aivyx_audit::AuditEvent;
        let event = AuditEvent::SystemInit {
            timestamp: chrono::Utc::now(),
        };
        assert_eq!(format_audit_event(&event), "System initialized");
    }

    // ── Focus and state basics ───────────────────────────────

    #[test]
    fn new_test_defaults() {
        let app = App::new_test();
        assert!(app.running);
        assert_eq!(app.focus, Focus::Sidebar);
        assert!(app.state.is_none());
        assert!(app.chat_popup.is_none());
        assert!(app.goal_popup.is_none());
        assert!(app.settings_popup.is_none());
        assert!(app.settings.is_none());
        assert_eq!(app.chat_messages.len(), 0);
        assert_eq!(app.goals.len(), 0);
        assert_eq!(app.notifications.len(), 0);
    }

    // ── Backend-dependent methods no-op safely ───────────────

    #[test]
    fn toggle_schedule_no_backend_no_panic() {
        let mut app = App::new_test();
        app.toggle_schedule(); // should not panic
    }

    #[test]
    fn open_session_list_no_backend_no_panic() {
        let mut app = App::new_test();
        app.open_session_list(); // should not panic, popup stays None
        assert!(app.chat_popup.is_none());
    }

    #[test]
    fn open_system_prompt_no_backend_no_panic() {
        let mut app = App::new_test();
        app.open_system_prompt_preview(); // should not panic
        assert!(app.chat_popup.is_none());
    }

    #[test]
    fn resolve_approval_no_backend_no_panic() {
        let mut app = App::new_test();
        app.resolve_approval(ApprovalStatus::Approved);
    }

    /// Build a synthetic pending `ApprovalItem` for tests.
    fn mk_approval(id: &str) -> ApprovalItem {
        ApprovalItem {
            notification: aivyx_loop::Notification {
                id: id.into(),
                kind: aivyx_loop::NotificationKind::ApprovalNeeded,
                title: format!("approval {id}"),
                body: "body".into(),
                source: "test".into(),
                timestamp: chrono::Utc::now(),
                requires_approval: true,
                goal_id: None,
            },
            status: ApprovalStatus::Pending,
            resolved_at: None,
            expires_at: Some(chrono::Utc::now() + chrono::Duration::seconds(120)),
        }
    }

    /// Regression test for the approval-resolution race.
    ///
    /// Scenario: the TUI snapshot has items [A, B, C] with user cursor on
    /// B. Between the user pressing Enter and the spawned task acquiring
    /// the shared queue lock, the notification bridge pushes a new item X
    /// at the front, shifting the shared queue to [X, A, B, C]. Under the
    /// old index-based resolution the spawned task would have updated
    /// index 1 (A) instead of B. With the ID-based fix, B is resolved
    /// correctly regardless of position.
    #[test]
    fn apply_resolution_matches_by_id_not_index() {
        // Shared queue as the bridge would see it after a new arrival.
        let mut shared = vec![
            mk_approval("X-new-arrival"),
            mk_approval("A"),
            mk_approval("B-target"),
            mk_approval("C"),
        ];

        // TUI snapshot captured *before* X arrived — user pressed Enter
        // on position 1, which in the snapshot was B.
        let now = chrono::Utc::now();
        let updated =
            App::apply_resolution_by_id(&mut shared, "B-target", ApprovalStatus::Approved, now);

        assert!(updated, "should have matched the target");
        // The correct item — B — was resolved.
        assert_eq!(shared[2].status, ApprovalStatus::Approved);
        assert_eq!(shared[2].resolved_at, Some(now));
        // And none of the bystanders were touched.
        assert_eq!(shared[0].status, ApprovalStatus::Pending);
        assert_eq!(shared[1].status, ApprovalStatus::Pending);
        assert_eq!(shared[3].status, ApprovalStatus::Pending);
    }

    #[test]
    fn apply_resolution_preserves_terminal_status() {
        // If another resolver already marked the item Expired in the
        // same tick, we must not clobber it with a late-arriving Approve.
        let mut shared = vec![mk_approval("X")];
        shared[0].status = ApprovalStatus::Expired;
        shared[0].resolved_at = Some(chrono::Utc::now());
        let prior_resolved_at = shared[0].resolved_at;

        let updated = App::apply_resolution_by_id(
            &mut shared,
            "X",
            ApprovalStatus::Approved,
            chrono::Utc::now(),
        );
        assert!(!updated, "should refuse to overwrite terminal status");
        assert_eq!(shared[0].status, ApprovalStatus::Expired);
        assert_eq!(shared[0].resolved_at, prior_resolved_at);
    }

    #[test]
    fn cancel_chat_stream_fires_token_and_clears_field() {
        let mut app = App::new_test();
        // Simulate an in-flight turn with a live cancellation token.
        let token = tokio_util::sync::CancellationToken::new();
        let observer = token.clone();
        app.chat_cancel = Some(token);
        app.chat_streaming = true;

        assert!(
            !observer.is_cancelled(),
            "precondition: token not yet cancelled"
        );
        app.cancel_chat_stream();
        assert!(
            observer.is_cancelled(),
            "cancel_chat_stream should fire the stored token"
        );
        assert!(
            app.chat_cancel.is_none(),
            "cancel_chat_stream should drop the handle"
        );

        // Second call is a no-op and must not panic.
        app.cancel_chat_stream();
    }

    #[test]
    fn cancel_chat_stream_no_active_stream_is_noop() {
        let mut app = App::new_test();
        assert!(app.chat_cancel.is_none());
        app.cancel_chat_stream(); // no-op, no panic
        assert!(app.chat_cancel.is_none());
    }

    #[test]
    fn apply_resolution_missing_id_is_noop() {
        // If the ID was drained from the shared queue (e.g. API consumer
        // popped it), we log and do nothing rather than panic.
        let mut shared = vec![mk_approval("A")];
        let updated = App::apply_resolution_by_id(
            &mut shared,
            "not-in-queue",
            ApprovalStatus::Approved,
            chrono::Utc::now(),
        );
        assert!(!updated);
        assert_eq!(shared[0].status, ApprovalStatus::Pending);
    }

    #[test]
    fn settings_toggle_current_no_backend_no_panic() {
        let mut app = App::new_test();
        app.settings_card_index = 2;
        app.settings_item_index = 0;
        app.settings_toggle_current(); // no state → returns early
    }

    #[test]
    fn settings_activate_current_no_backend_no_panic() {
        let mut app = App::new_test();
        app.settings_card_index = 0;
        app.settings_item_index = 0;
        app.settings_activate_current(); // needs settings snapshot
    }

    #[test]
    fn settings_cycle_app_access_no_backend_no_panic() {
        let mut app = App::new_test();
        app.settings_card_index = 9;
        app.settings_item_index = 0;
        app.settings_cycle_app_access(true); // no state → returns early
        app.settings_cycle_app_access(false); // same
    }

    #[test]
    fn integration_setup_popup_opens_on_card5_enter() {
        let mut app = App::new_test();
        // Give it a default snapshot so settings != None
        app.settings = Some(SettingsSnapshot::default());
        app.settings_card_index = 5;
        app.settings_item_index = 0; // Email
        app.settings_activate_current();
        assert!(
            matches!(
                app.settings_popup,
                Some(SettingsPopup::IntegrationSetup { .. })
            ),
            "Expected IntegrationSetup popup, got {:?}",
            app.settings_popup.is_some(),
        );
    }

    // ── Filtered missions ────────────────────────────────────

    fn test_mission_meta(status: MissionStatus) -> TaskMetadata {
        TaskMetadata {
            id: aivyx_core::TaskId::new(),
            goal: "test mission".into(),
            agent_name: "agent".into(),
            status,
            steps_completed: 0,
            steps_total: 3,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn filtered_missions_all() {
        let mut app = App::new_test();
        app.missions = vec![
            test_mission_meta(MissionStatus::Executing),
            test_mission_meta(MissionStatus::Completed),
            test_mission_meta(MissionStatus::Failed {
                reason: "oom".into(),
            }),
        ];
        app.mission_filter = 0;
        assert_eq!(app.filtered_missions().len(), 3);
    }

    #[test]
    fn filtered_missions_active() {
        let mut app = App::new_test();
        app.missions = vec![
            test_mission_meta(MissionStatus::Executing),
            test_mission_meta(MissionStatus::Planning),
            test_mission_meta(MissionStatus::Completed),
            test_mission_meta(MissionStatus::Cancelled),
        ];
        app.mission_filter = 1; // active
        assert_eq!(app.filtered_missions().len(), 2);
    }

    #[test]
    fn filtered_missions_completed() {
        let mut app = App::new_test();
        app.missions = vec![
            test_mission_meta(MissionStatus::Executing),
            test_mission_meta(MissionStatus::Completed),
            test_mission_meta(MissionStatus::Completed),
        ];
        app.mission_filter = 2; // completed
        assert_eq!(app.filtered_missions().len(), 2);
    }

    #[test]
    fn filtered_missions_failed() {
        let mut app = App::new_test();
        app.missions = vec![
            test_mission_meta(MissionStatus::Executing),
            test_mission_meta(MissionStatus::Failed { reason: "x".into() }),
            test_mission_meta(MissionStatus::Cancelled),
        ];
        app.mission_filter = 3; // failed/cancelled
        assert_eq!(app.filtered_missions().len(), 2);
    }

    #[test]
    fn cancel_mission_no_backend_no_panic() {
        let mut app = App::new_test();
        app.cancel_mission(); // should not panic
    }

    #[test]
    fn load_mission_detail_no_backend_no_panic() {
        let mut app = App::new_test();
        app.load_mission_detail(); // should not panic
        assert!(app.mission_detail.is_none());
    }

    #[test]
    fn load_mission_detail_clears_when_empty() {
        let mut app = App::new_test();
        app.mission_selected = 5; // out of range
        app.load_mission_detail();
        assert!(app.mission_detail.is_none());
    }
}
