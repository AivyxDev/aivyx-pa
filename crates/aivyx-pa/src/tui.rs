//! TUI — the primary interface for the personal assistant.
//!
//! Launches a ratatui terminal UI with:
//! - Home: morning briefing + recent notifications
//! - Chat: conversational interface with streaming
//! - Activity: audit log of what the assistant has been doing
//! - Settings: configuration display

use aivyx_agent::agent::Agent;
use aivyx_agent::cost_tracker::CostTracker;
use aivyx_agent::rate_limiter::RateLimiter;
use aivyx_capability::CapabilitySet;
use aivyx_config::{AivyxConfig, AivyxDirs};
use aivyx_core::{AgentId, AutonomyTier, ToolRegistry};
use aivyx_crypto::{EncryptedStore, MasterKey};
use aivyx_llm::{LlmProvider, create_embedding_provider};
use aivyx_memory::{MemoryManager, MemoryStore};
use aivyx_loop::{AgentLoop, LoopConfig, Notification, NotificationKind};

use aivyx_actions::ActionRegistry;
use aivyx_actions::bridge::register_default_actions;

use chrono::Timelike;
use std::sync::Arc;
use tokio::sync::Mutex;
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::{prelude::*, widgets::*};
use std::io::stdout;
use tokio::sync::mpsc;

// ── Agent construction ──────────────────────────────────────────

/// Build an Agent from the unlocked key + provider. Shared by TUI and one-shot chat.
pub fn build_agent(
    dirs: &AivyxDirs,
    config: &AivyxConfig,
    store: &EncryptedStore,
    master_key: MasterKey,
    provider: Box<dyn LlmProvider>,
) -> anyhow::Result<Agent> {
    let system_prompt = concat!(
        "You are Aivyx, a private AI personal assistant. ",
        "You help the user manage their life and business: reading email, ",
        "setting reminders, managing files, searching the web, and taking ",
        "actions on their behalf. Be concise, helpful, and proactive. ",
        "When you take an action, explain what you did briefly.",
    );

    // Register action tools (files, shell, web, reminders)
    let mut registry = ToolRegistry::new();
    register_default_actions(&mut registry);

    let agent_id = AgentId::new();

    // Wire memory if an embedding provider is available
    let memory_manager = match wire_memory(dirs, config, store, &master_key, agent_id) {
        Ok(mgr) => Some(mgr),
        Err(e) => {
            tracing::warn!("Memory system unavailable: {e}");
            None
        }
    };

    // Register memory tools into the tool registry
    if let Some(ref mgr) = memory_manager {
        aivyx_memory::register_memory_tools(&mut registry, Arc::clone(mgr), agent_id);
    }

    let mut agent = Agent::new(
        agent_id,
        "assistant".to_string(),
        system_prompt.to_string(),
        4096,
        AutonomyTier::Trust,
        provider,
        registry,
        CapabilitySet::new(),
        RateLimiter::new(60),
        CostTracker::new(0.0, 0.0, 0.0),
        None, // audit log — TODO: wire up
        3,
        100,
    );

    // Attach memory manager to the agent for memory-augmented turns
    if let Some(mgr) = memory_manager {
        agent.set_memory_manager(mgr);
    }

    Ok(agent)
}

/// Create embedding provider + MemoryStore + MemoryManager.
fn wire_memory(
    dirs: &AivyxDirs,
    config: &AivyxConfig,
    store: &EncryptedStore,
    master_key: &MasterKey,
    _agent_id: AgentId,
) -> anyhow::Result<Arc<Mutex<MemoryManager>>> {
    let embed_config = config.embedding.clone().unwrap_or_default();
    let embedding_provider = create_embedding_provider(&embed_config, store, master_key)?;

    let memory_key = aivyx_crypto::derive_memory_key(master_key);
    let memory_store = MemoryStore::open(dirs.memory_dir().join("assistant"))?;
    let manager = MemoryManager::new(
        memory_store,
        Arc::from(embedding_provider),
        memory_key,
        0, // unlimited memories
    )?;

    Ok(Arc::new(Mutex::new(manager)))
}

// ── TUI state ───────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    Home,
    Chat,
    Activity,
    Settings,
}

struct App {
    view: View,
    running: bool,
    chat_input: String,
    chat_messages: Vec<(String, String)>,
    notifications: Vec<String>,
    streaming: bool,
    config: AivyxConfig,
}

impl App {
    fn new(config: AivyxConfig) -> Self {
        Self {
            view: View::Home,
            running: true,
            chat_input: String::new(),
            chat_messages: Vec::new(),
            notifications: vec!["Your assistant is ready.".into()],
            streaming: false,
            config,
        }
    }
}

// ── Main TUI loop ───────────────────────────────────────────────

pub async fn run(
    dirs: &AivyxDirs,
    config: AivyxConfig,
    store: EncryptedStore,
    master_key: MasterKey,
    provider: Box<dyn LlmProvider>,
) -> anyhow::Result<()> {
    // Build agent
    let mut agent = build_agent(dirs, &config, &store, master_key, provider)?;

    // Start the background agent loop
    let actions = ActionRegistry::new();
    let loop_config = LoopConfig::default();
    let (_agent_loop, mut notification_rx) = AgentLoop::start(loop_config, actions);

    // Channel for streaming tokens from agent turns
    let (token_tx, mut token_rx) = mpsc::channel::<String>(256);
    // Channel for sending user messages to the agent task
    let (msg_tx, mut msg_rx) = mpsc::channel::<String>(16);

    // Spawn agent turn handler in background
    tokio::spawn(async move {
        while let Some(user_msg) = msg_rx.recv().await {
            let tx = token_tx.clone();

            // Use streaming turn
            let cancel = tokio_util::sync::CancellationToken::new();
            match agent.turn_stream(&user_msg, None, tx.clone(), Some(cancel)).await {
                Ok(_final_text) => {
                    // Send a sentinel so TUI knows the turn is done
                    let _ = tx.send("\n[[DONE]]".to_string()).await;
                }
                Err(e) => {
                    let _ = tx.send(format!("\n[Error: {e}]")).await;
                    let _ = tx.send("\n[[DONE]]".to_string()).await;
                }
            }
        }
    });

    // Setup terminal
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(config);

    // Main event loop
    while app.running {
        // Drain notifications from the agent loop
        while let Ok(notif) = notification_rx.try_recv() {
            app.notifications.push(format_notification(&notif));
        }

        // Drain streaming tokens
        while let Ok(token) = token_rx.try_recv() {
            if token.contains("[[DONE]]") {
                app.streaming = false;
            } else if let Some(last) = app.chat_messages.last_mut() {
                if last.0 == "assistant" {
                    last.1.push_str(&token);
                }
            }
        }

        terminal.draw(|frame| render(&app, frame))?;

        if event::poll(std::time::Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                handle_key(&mut app, &msg_tx);
                handle_key_event(&mut app, key, &msg_tx);
            }
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;

    Ok(())
}

fn format_notification(notif: &Notification) -> String {
    let icon = match notif.kind {
        NotificationKind::Info => " ",
        NotificationKind::ActionTaken => " ",
        NotificationKind::ApprovalNeeded => " ",
        NotificationKind::Urgent => " ",
    };
    format!("{icon} {}", notif.title)
}

// ── Key handling ────────────────────────────────────────────────

fn handle_key(_app: &mut App, _msg_tx: &mpsc::Sender<String>) {
    // Placeholder — actual handling is in handle_key_event
}

fn handle_key_event(app: &mut App, key: event::KeyEvent, msg_tx: &mpsc::Sender<String>) {
    // Global keys
    match (key.modifiers, key.code) {
        (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
            app.running = false;
            return;
        }
        (_, KeyCode::Char('q')) if app.view != View::Chat => {
            app.running = false;
            return;
        }
        (KeyModifiers::CONTROL, KeyCode::Char('1')) => { app.view = View::Home; return; }
        (KeyModifiers::CONTROL, KeyCode::Char('2')) => { app.view = View::Chat; return; }
        (KeyModifiers::CONTROL, KeyCode::Char('3')) => { app.view = View::Activity; return; }
        (KeyModifiers::CONTROL, KeyCode::Char('4')) => { app.view = View::Settings; return; }
        _ => {}
    }

    // View-specific keys
    match app.view {
        View::Chat => match key.code {
            KeyCode::Char(c) if !app.streaming => {
                if app.chat_input.len() < 8192 {
                    app.chat_input.push(c);
                }
            }
            KeyCode::Backspace if !app.streaming => { app.chat_input.pop(); }
            KeyCode::Enter if !app.chat_input.is_empty() && !app.streaming => {
                let msg = std::mem::take(&mut app.chat_input);
                app.chat_messages.push(("you".into(), msg.clone()));
                app.chat_messages.push(("assistant".into(), String::new()));
                app.streaming = true;
                // Send to agent task (non-blocking)
                let _ = msg_tx.try_send(msg);
            }
            KeyCode::Esc => { app.view = View::Home; }
            _ => {}
        },
        View::Home => match key.code {
            KeyCode::Char('c') => { app.view = View::Chat; }
            _ => {}
        },
        _ => {}
    }
}

// ── Rendering ───────────────────────────────────────────────────

fn render(app: &App, frame: &mut Frame) {
    let area = frame.area();

    let layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(22), Constraint::Min(40)])
        .split(area);

    render_sidebar(app, frame, layout[0]);

    match app.view {
        View::Home => render_home(app, frame, layout[1]),
        View::Chat => render_chat(app, frame, layout[1]),
        View::Activity => render_activity(app, frame, layout[1]),
        View::Settings => render_settings(app, frame, layout[1]),
    }
}

fn render_sidebar(app: &App, frame: &mut Frame, area: Rect) {
    let items = [
        ("^1 Home", View::Home),
        ("^2 Chat", View::Chat),
        ("^3 Activity", View::Activity),
        ("^4 Settings", View::Settings),
    ];

    let nav: Vec<Line> = items
        .iter()
        .map(|(label, view)| {
            let style = if *view == app.view {
                Style::default().fg(Color::Cyan).bold()
            } else {
                Style::default().fg(Color::DarkGray)
            };
            Line::from(Span::styled(format!("  {label}"), style))
        })
        .collect();

    let mut lines = vec![
        Line::from(Span::styled("  AIVYX", Style::default().fg(Color::Cyan).bold())),
        Line::from(""),
    ];
    lines.extend(nav);
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("  q quit", Style::default().fg(Color::DarkGray))));

    let sidebar = Paragraph::new(lines)
        .block(Block::default().borders(Borders::RIGHT).border_style(Style::default().fg(Color::DarkGray)));

    frame.render_widget(sidebar, area);
}

fn render_home(app: &App, frame: &mut Frame, area: Rect) {
    let now = chrono::Local::now();
    let greeting = match now.hour() {
        5..=11 => "Good morning",
        12..=17 => "Good afternoon",
        _ => "Good evening",
    };

    let mut lines = vec![
        Line::from(Span::styled(
            format!("  {greeting}."),
            Style::default().fg(Color::White).bold(),
        )),
        Line::from(""),
    ];

    if app.notifications.is_empty() {
        lines.push(Line::from(Span::styled(
            "  No new notifications.",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            "  Today",
            Style::default().fg(Color::Cyan).bold(),
        )));
        lines.push(Line::from(""));
        for note in &app.notifications {
            lines.push(Line::from(format!("  - {note}")));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Press 'c' to start chatting",
        Style::default().fg(Color::DarkGray),
    )));

    let home = Paragraph::new(lines)
        .block(Block::default().padding(Padding::new(1, 1, 1, 1)));

    frame.render_widget(home, area);
}

fn render_chat(app: &App, frame: &mut Frame, area: Rect) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(4), Constraint::Length(3)])
        .split(area);

    let messages: Vec<Line> = app.chat_messages
        .iter()
        .flat_map(|(role, content)| {
            let style = if role == "you" {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::White)
            };
            let prefix = if role == "you" { "  you: " } else { "  ai:  " };
            // Wrap long messages into multiple lines
            let wrapped: Vec<Line> = content
                .lines()
                .map(|line| Line::from(Span::styled(format!("{prefix}{line}"), style)))
                .collect();
            if wrapped.is_empty() {
                vec![Line::from(Span::styled(
                    format!("{prefix}..."),
                    style.add_modifier(Modifier::DIM),
                ))]
            } else {
                wrapped
            }
        })
        .collect();

    let msg_count = messages.len();
    let visible_height = layout[0].height as usize;
    let scroll = msg_count.saturating_sub(visible_height);

    let messages_widget = if app.chat_messages.is_empty() {
        Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled("  Start a conversation.", Style::default().fg(Color::DarkGray))),
            Line::from(Span::styled("  Type below and press Enter.", Style::default().fg(Color::DarkGray))),
        ])
    } else {
        Paragraph::new(messages).scroll((scroll as u16, 0))
    };

    frame.render_widget(
        messages_widget.block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(Color::DarkGray)),
        ),
        layout[0],
    );

    // Input line
    let input_style = if app.streaming {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default().fg(Color::White)
    };

    let input_text = if app.streaming {
        "  > (streaming...)".to_string()
    } else {
        format!("  > {}", app.chat_input)
    };

    let input = Paragraph::new(input_text).style(input_style);
    frame.render_widget(input, layout[1]);

    // Cursor
    if !app.streaming {
        frame.set_cursor_position((
            (4 + app.chat_input.len()) as u16 + layout[1].x,
            layout[1].y,
        ));
    }
}

fn render_activity(app: &App, frame: &mut Frame, area: Rect) {
    let mut lines = vec![
        Line::from(Span::styled("  Activity Log", Style::default().fg(Color::White).bold())),
        Line::from(""),
    ];

    if app.notifications.len() <= 1 {
        lines.push(Line::from(Span::styled(
            "  No activity recorded yet.",
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from(Span::styled(
            "  Your assistant's actions will appear here.",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for note in app.notifications.iter().rev() {
            lines.push(Line::from(format!("  {note}")));
        }
    }

    let content = Paragraph::new(lines)
        .block(Block::default().padding(Padding::new(1, 1, 1, 1)));

    frame.render_widget(content, area);
}

fn render_settings(app: &App, frame: &mut Frame, area: Rect) {
    let provider_info = format!("{:?}", app.config.provider);

    let content = Paragraph::new(vec![
        Line::from(Span::styled("  Settings", Style::default().fg(Color::White).bold())),
        Line::from(""),
        Line::from(Span::styled("  Provider", Style::default().fg(Color::Cyan))),
        Line::from(format!("  {provider_info}")),
        Line::from(""),
        Line::from(Span::styled("  Autonomy", Style::default().fg(Color::Cyan))),
        Line::from(format!("  {:?}", app.config.autonomy.default_tier)),
        Line::from(""),
        Line::from(Span::styled(
            "  Edit ~/.aivyx/config.toml to change settings.",
            Style::default().fg(Color::DarkGray),
        )),
    ])
    .block(Block::default().padding(Padding::new(1, 1, 1, 1)));

    frame.render_widget(content, area);
}
