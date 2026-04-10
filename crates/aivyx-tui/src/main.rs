//! Aivyx Personal Assistant — Terminal UI
//!
//! Usage:
//!   aivyx-tui              Launch TUI (requires initialized ~/.aivyx)
//!   aivyx-tui --port 3100  Also start the HTTP API server on the given port

mod app;
#[allow(dead_code)]
mod theme;
mod views;
mod widgets;

use aivyx_pa::api::ApprovalStatus;
use app::{App, View};
use crossterm::{
    ExecutableCommand,
    event::{self, Event, KeyCode, KeyModifiers},
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{prelude::*, widgets::Block};
use std::io::{self, BufRead, Write as _, stdout};
use std::sync::Arc;
use std::time::Duration;

use aivyx_config::{AivyxConfig, AivyxDirs};
use aivyx_crypto::{EncryptedStore, MasterKey, MasterKeyEnvelope};
use aivyx_llm::create_provider;
use aivyx_loop::AgentLoop;
use aivyx_pa::{agent, api, config, runtime};
use clap::Parser;
use tokio::sync::broadcast;
use zeroize::Zeroizing;

#[derive(Parser)]
#[command(name = "aivyx-tui", about = "Aivyx PA — Terminal Interface", version)]
struct Cli {
    /// Also start the HTTP API server on this port
    #[arg(long)]
    port: Option<u16>,
}

/// Read passphrase from terminal with echo suppressed.
fn read_passphrase(msg: &str) -> Zeroizing<String> {
    eprint!("{msg}");
    let _ = io::stderr().flush();
    match rpassword::read_password() {
        Ok(p) => Zeroizing::new(p),
        Err(_) => {
            let mut input = String::new();
            if io::stdin().lock().read_line(&mut input).is_err() {
                eprintln!("\nError reading input.");
                std::process::exit(1);
            }
            Zeroizing::new(input.trim().to_string())
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let dirs = AivyxDirs::from_default()?;

    // Initialize logging to file (don't pollute the TUI)
    let _log_guard = init_logging(&dirs)?;

    // First launch — run Genesis wizard in the TUI before entering the main app.
    // The wizard returns the passphrase so we don't re-prompt.
    let genesis_passphrase = if !dirs.is_initialized() {
        enable_raw_mode()?;
        stdout().execute(EnterAlternateScreen)?;
        let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;

        let result = views::genesis::run_wizard(&mut terminal, &dirs).await;

        disable_raw_mode()?;
        stdout().execute(LeaveAlternateScreen)?;

        match result? {
            Some(p) => Some(p),
            None => {
                eprintln!("Setup cancelled.");
                std::process::exit(1);
            }
        }
    } else {
        None
    };

    // Unlock master key — reuse genesis passphrase or prompt
    let passphrase: Zeroizing<String> = if let Some(p) = genesis_passphrase {
        p
    } else {
        match std::env::var("AIVYX_PASSPHRASE") {
            Ok(p) => Zeroizing::new(p),
            Err(_) => read_passphrase("Passphrase: "),
        }
    };

    let envelope_json = std::fs::read_to_string(dirs.master_key_path())?;
    let envelope: MasterKeyEnvelope = serde_json::from_str(&envelope_json)?;
    let master_key = MasterKey::decrypt_from_envelope(passphrase.as_bytes(), &envelope)
        .map_err(|_| anyhow::anyhow!("Wrong passphrase."))?;

    eprintln!("Unlocked. Starting...");

    // Load configs
    let config = AivyxConfig::load(dirs.config_path())?;
    let pa_config = config::PaConfig::load(dirs.config_path());
    let store = EncryptedStore::open(dirs.store_path())?;

    // Lint config for common issues — logged but not shown in TUI (no terminal yet).
    for warning in config::PaConfig::lint(&dirs.config_path(), Some(&store), Some(&master_key)) {
        tracing::warn!("Config lint: {warning}");
    }

    // Create providers
    let provider = create_provider(&config.provider, &store, &master_key)?;
    let loop_provider = create_provider(&config.provider, &store, &master_key)?;
    let health_provider = create_provider(&config.provider, &store, &master_key)?;

    // Resolve services and derive keys
    let services = agent::ServiceConfigs {
        email: pa_config.resolve_email_config(&store, &master_key),
        calendar: pa_config.resolve_calendar_config(&store, &master_key),
        contacts: pa_config.resolve_contacts_config(&store, &master_key),
        vault: pa_config.resolve_vault_config(),
        telegram: pa_config.resolve_telegram_config(&store, &master_key),
        matrix: pa_config.resolve_matrix_config(&store, &master_key),
        devtools: pa_config.resolve_devtools_config(&store, &master_key),
        signal: pa_config.resolve_signal_config(),
        sms: pa_config.resolve_sms_config(&store, &master_key),
    };

    // Copy master key bytes before build_agent consumes it (MasterKey is
    // not Clone). Wrap in `Zeroizing` so the stack buffer is wiped when the
    // binding is dropped — otherwise the bytes would linger on the main
    // thread's stack frame until `main()` returns. `MasterKey::from_bytes`
    // also zeroizes the [u8; 32] it receives, so the only window of
    // exposure is between this line and the `from_bytes` call below.
    let master_key_bytes: Zeroizing<[u8; 32]> =
        Zeroizing::new(master_key.expose_secret().try_into().unwrap());

    let mut keys = runtime::derive_all_keys(
        &master_key,
        &pa_config,
        services.vault.is_some(),
        services.contacts.is_some(),
    );
    let loop_inputs = runtime::LoopInputs::from_services(&services, &pa_config);
    let email_config_for_health = loop_inputs.email_config.clone();

    let store = Arc::new(store);
    let agent_audit_log = aivyx_audit::AuditLog::new(dirs.audit_path(), &keys.audit_key);
    let api_audit_log = aivyx_audit::AuditLog::new(dirs.audit_path(), &keys.ui_audit_key);

    let mut built = agent::build_agent(
        &dirs,
        &config,
        &pa_config,
        services,
        Arc::clone(&store),
        master_key,
        provider,
        Some(agent_audit_log),
    )
    .await?;

    // Clone brain store for UI before loop takes it
    let brain_store_for_ui = built.brain_store.as_ref().map(Arc::clone);

    // Build loop
    let schedule_tools = runtime::build_schedule_tools(&loop_inputs, built.imap_pool.clone());
    let loop_context = runtime::build_loop_context(
        &mut built,
        &mut keys,
        loop_inputs,
        loop_provider,
        schedule_tools,
        Arc::clone(&store),
        &dirs,
        &pa_config,
    );
    let loop_config = runtime::build_loop_config(&pa_config);

    let mission_ctx_for_ui = built.mission_ctx.clone();
    let memory_manager_for_ui = built.memory_manager.as_ref().map(Arc::clone);
    let is_first_launch = built.is_first_launch;
    let agent_name = pa_config
        .agent
        .as_ref()
        .map(|a| a.name.clone())
        .unwrap_or_else(|| "assistant".into());
    let persona = pa_config
        .agent
        .as_ref()
        .map(|a| a.persona.clone())
        .unwrap_or_else(|| "assistant".into());

    let (_agent_loop, mut notification_rx) = AgentLoop::start(loop_config, loop_context);
    // Extract the approval sender so the TUI can feed decisions back to the loop.
    // _agent_loop must remain alive until end of main() to keep the loop running.
    let approval_tx = _agent_loop.approval_tx.clone();

    // Shared approval queue and notification history
    let approvals: Arc<tokio::sync::Mutex<Vec<api::ApprovalItem>>> =
        Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let notification_history: Arc<tokio::sync::Mutex<Vec<aivyx_loop::Notification>>> =
        Arc::new(tokio::sync::Mutex::new(Vec::new()));

    // Bridge notifications
    let (notification_tx, _) = broadcast::channel::<aivyx_loop::Notification>(256);
    let broadcast_tx = notification_tx.clone();
    let bridge_approvals = Arc::clone(&approvals);
    let bridge_history = Arc::clone(&notification_history);
    tokio::spawn(async move {
        while let Some(notif) = notification_rx.recv().await {
            if notif.requires_approval {
                // 120s is well within i64 range, so `Duration::seconds` is
                // infallible — prefer it over `try_seconds(...).unwrap()`.
                bridge_approvals.lock().await.push(api::ApprovalItem {
                    expires_at: Some(notif.timestamp + chrono::Duration::seconds(120)),
                    notification: notif.clone(),
                    status: api::ApprovalStatus::Pending,
                    resolved_at: None,
                });
            }
            let mut hist = bridge_history.lock().await;
            hist.push(notif.clone());
            if hist.len() > 500 {
                let excess = hist.len() - 500;
                hist.drain(..excess);
            }
            let _ = broadcast_tx.send(notif);
        }
    });

    let config_path = dirs.config_path();

    // Run startup health checks (non-blocking — updates shared state asynchronously).
    let health = Arc::new(tokio::sync::RwLock::new(api::HealthStatus::default()));
    let health_for_check = Arc::clone(&health);
    let health_config_path = config_path.clone();
    let health_data_dir = dirs.root().to_path_buf();
    tokio::spawn(async move {
        let result = api::run_health_checks(
            health_provider.as_ref(),
            email_config_for_health.as_ref(),
            &health_config_path,
            &health_data_dir,
        )
        .await;
        tracing::info!(
            provider = %result.provider.label(),
            email = %result.email.label(),
            config = %result.config.label(),
            disk = %result.disk.label(),
            "Startup health check complete",
        );
        *health_for_check.write().await = result;
    });

    let app_state = Arc::new(api::AppState {
        agent: Arc::new(tokio::sync::Mutex::new(built.agent)),
        brain_store: brain_store_for_ui,
        brain_key: Some(Arc::new(keys.ui_brain_key)),
        audit_log: Arc::new(api_audit_log),
        notification_tx,
        pa_config: Arc::new(pa_config),
        dirs: Arc::new(dirs.clone()),
        store,
        conversation_key: Arc::new(keys.conversation_key),
        // `*master_key_bytes` copies the array out of the `Zeroizing`
        // wrapper for `from_bytes` to consume; the original `Zeroizing`
        // binding is still dropped at end-of-scope, zeroing its storage.
        master_key: Arc::new(MasterKey::from_bytes(*master_key_bytes)),
        memory_manager: memory_manager_for_ui,
        approvals,
        notification_history,
        config_path,
        agent_name: agent_name.clone(),
        mission_ctx: mission_ctx_for_ui,
        health,
        approval_tx: Some(approval_tx),
    });

    // Optionally start HTTP API server alongside TUI
    let _api_handle = if let Some(port) = cli.port {
        let (handle, _cancel) = api::spawn_api_server((*app_state).clone(), port).await?;
        Some(handle)
    } else {
        None
    };

    // Create TUI app with live backend
    let mut app = App::new_live(Arc::clone(&app_state));

    // Initial data load
    app.refresh_data().await;

    // First launch onboarding — agent introduces itself
    if is_first_launch {
        let greeting =
            aivyx_pa::config::onboarding_message(&agent_name, &persona, &app_state.pa_config);
        app.chat_messages.push(app::ChatMessage {
            role: "assistant".into(),
            content: greeting,
            timestamp: chrono::Local::now().format("%H:%M").to_string(),
        });
        // Start on Chat view so the user sees the welcome message
        app.go_to(View::Chat);
        app.focus = app::Focus::Content;
    }

    // Initialize terminal
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;

    // Main event loop
    let result = run_loop(&mut terminal, &mut app).await;

    // Restore terminal
    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;

    result
}

/// Initialize logging to a file so it doesn't interfere with the TUI.
fn init_logging(
    dirs: &AivyxDirs,
) -> anyhow::Result<Option<tracing_appender::non_blocking::WorkerGuard>> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));

    let _ = std::fs::create_dir_all(dirs.root());
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dirs.root().join("tui.log"))?;
    let (non_blocking, guard) = tracing_appender::non_blocking(log_file);
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(non_blocking)
        .with_ansi(false)
        .init();
    Ok(Some(guard))
}

async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> anyhow::Result<()> {
    while app.running {
        // Render
        terminal.draw(|frame| render(app, frame))?;

        // Tick animation counter
        app.frame_count = app.frame_count.wrapping_add(1);

        // Poll for streamed chat tokens and agent lifecycle events
        app.poll_chat_tokens();
        app.poll_voice_transcripts();
        app.poll_agent_events();
        app.poll_approval_expiries();

        // Periodic data refresh (every 2 seconds)
        if app.last_refresh.elapsed() > Duration::from_secs(2) {
            app.refresh_data().await;
        }

        // Poll for keyboard events (60fps for smooth animations)
        if event::poll(Duration::from_millis(16))?
            && let Event::Key(key) = event::read()?
        {
            handle_key(app, key);
        }
    }
    Ok(())
}

// ── Rendering ──────────────────────────────────────────────────

fn render(app: &App, frame: &mut Frame) {
    let area = frame.area();

    // Background fill
    frame.render_widget(Block::default().style(Style::default().bg(theme::BG)), area);

    // ── Authenticated: shell layout ────────────────────────────
    // Sidebar (22 cols) | Header + Content + StatusBar
    let sidebar_width = 22u16.min(area.width / 4);

    let [sidebar_area, main_area] =
        Layout::horizontal([Constraint::Length(sidebar_width), Constraint::Min(30)]).areas(area);

    let [header_area, content_area] =
        Layout::vertical([Constraint::Length(2), Constraint::Min(10)]).areas(main_area);

    // Sidebar
    frame.render_widget(widgets::sidebar::Sidebar::new(app), sidebar_area);

    // Header
    frame.render_widget(widgets::header::Header::new(app), header_area);

    // Content area (with 1-cell padding)
    let padded = Rect {
        x: content_area.x + 1,
        y: content_area.y,
        width: content_area.width.saturating_sub(2),
        height: content_area.height,
    };

    match app.view {
        View::Home => views::home::render(app, padded, frame.buffer_mut()),
        View::Chat => views::chat::render(app, padded, frame.buffer_mut()),
        View::Activity => views::activity::render(app, padded, frame.buffer_mut()),
        View::Goals => views::goals::render(app, padded, frame.buffer_mut()),
        View::Approvals => views::approvals::render(app, padded, frame.buffer_mut()),
        View::Missions => views::missions::render(app, padded, frame.buffer_mut()),
        View::Audit => views::audit::render(app, padded, frame.buffer_mut()),
        View::Memory => views::memory::render(app, padded, frame.buffer_mut()),
        View::Help => views::help::render(app, padded, frame.buffer_mut()),
        View::Settings => views::settings::render(app, padded, frame.buffer_mut()),
    }
}

// ── Key handling ───────────────────────────────────────────────

fn handle_key(app: &mut App, key: event::KeyEvent) {
    use app::Focus;

    // Global: Ctrl+C always quits
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        app.running = false;
        return;
    }

    // ── Modal popup intercepts (must run before global Esc) ──
    if app.view == View::Chat && app.chat_popup.is_some() {
        app.handle_chat_popup(key);
        return;
    }
    if app.view == View::Settings && app.settings_popup.is_some() {
        app.handle_settings_popup(key);
        return;
    }
    if app.view == View::Goals && app.goal_popup.is_some() {
        app.handle_goal_popup(key);
        return;
    }

    // Esc: from content → sidebar, from sidebar → Home, from Chat → sidebar
    if key.code == KeyCode::Esc {
        if app.view == View::Chat {
            app.focus = Focus::Sidebar;
            return;
        }
        match app.focus {
            Focus::Content => {
                app.focus = Focus::Sidebar;
            }
            Focus::Sidebar => {
                app.go_to(View::Home);
            }
        }
        return;
    }

    // ── Chat view with content focus: capture all input ───────
    if app.view == View::Chat && app.focus == Focus::Content {
        // Ctrl+key shortcuts (must check before generic Char handler)
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('s') => {
                    app.open_session_list();
                    return;
                }
                KeyCode::Char('p') => {
                    app.open_system_prompt_preview();
                    return;
                }
                KeyCode::Char('e') => {
                    app.export_chat_markdown();
                    return;
                }
                KeyCode::Char('b') => {
                    app.open_branch_manager();
                    return;
                }
                KeyCode::Char('r') => {
                    app.toggle_voice_recording();
                    return;
                }
                _ => {}
            }
        }
        match key.code {
            KeyCode::Char(c) => app.chat_input.push(c),
            KeyCode::Backspace => {
                app.chat_input.pop();
            }
            KeyCode::Enter => app.send_chat_message(),
            KeyCode::Up => {
                app.chat_scroll = app.chat_scroll.saturating_add(1);
            }
            KeyCode::Down => {
                app.chat_scroll = app.chat_scroll.saturating_sub(1);
            }
            KeyCode::PageUp => {
                app.chat_scroll = app.chat_scroll.saturating_add(10);
            }
            KeyCode::PageDown => {
                app.chat_scroll = app.chat_scroll.saturating_sub(10);
            }
            KeyCode::Tab => {
                app.focus = Focus::Sidebar;
            }
            _ => {}
        }
        return;
    }

    // ── Tab: toggle focus between sidebar and content ─────────
    if key.code == KeyCode::Tab {
        app.focus = match app.focus {
            Focus::Sidebar => Focus::Content,
            Focus::Content => Focus::Sidebar,
        };
        // When entering Chat content, reset scroll to bottom
        if app.focus == Focus::Content && app.view == View::Chat {
            app.chat_scroll = 0;
        }
        return;
    }

    // ── Left/Right: spatial focus switching ───────────────────
    // In Settings view with content focused, Left/Right are used for
    // column navigation and persona slider adjustment — don't steal them.
    let settings_content = app.view == View::Settings && app.focus == Focus::Content;
    if key.code == KeyCode::Left && !settings_content {
        app.focus = Focus::Sidebar;
        return;
    }
    if key.code == KeyCode::Right && !settings_content {
        app.focus = Focus::Content;
        return;
    }

    // ── Number keys: direct view switch (always works) ───────
    // 1–9 → views 0–8, 0 → view 9 (Settings)
    if let KeyCode::Char(c @ '1'..='9') = key.code {
        let idx = (c as usize) - ('1' as usize);
        app.go_to_view(idx);
        app.focus = Focus::Content;
        return;
    }
    if key.code == KeyCode::Char('0') {
        app.go_to(View::Settings);
        app.focus = Focus::Content;
        return;
    }

    // ── Quit ──────────────────────────────────────────────────
    if key.code == KeyCode::Char('q') {
        app.running = false;
        return;
    }

    match app.focus {
        // ── Sidebar focused: Up/Down navigate views ──────────
        Focus::Sidebar => match key.code {
            KeyCode::Up | KeyCode::Char('k') => app.nav_up(),
            KeyCode::Down | KeyCode::Char('j') => app.nav_down(),
            KeyCode::Enter => {
                app.focus = Focus::Content;
            }
            _ => {}
        },

        // ── Content focused: per-view navigation ─────────────
        Focus::Content => {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => content_up(app),
                KeyCode::Down | KeyCode::Char('j') => content_down(app),

                // Filter cycling for Goals / Audit
                KeyCode::Char('[') => {
                    match app.view {
                        View::Goals => {
                            app.goal_filter = (app.goal_filter + 3) % 4; // prev
                            app.goal_selected = 0;
                        }
                        View::Missions => {
                            app.mission_filter = (app.mission_filter + 3) % 4;
                            app.mission_selected = 0;
                            app.load_mission_detail();
                        }
                        View::Audit => {
                            app.audit_filter = (app.audit_filter + 3) % 4;
                            app.audit_selected = 0;
                        }
                        View::Activity => {
                            app.activity_filter = (app.activity_filter + 4) % 5;
                            app.activity_selected = 0;
                            app.agent_monitor_selected = 0;
                        }
                        _ => {}
                    }
                }
                KeyCode::Char(']') => match app.view {
                    View::Goals => {
                        app.goal_filter = (app.goal_filter + 1) % 4;
                        app.goal_selected = 0;
                    }
                    View::Missions => {
                        app.mission_filter = (app.mission_filter + 1) % 4;
                        app.mission_selected = 0;
                        app.load_mission_detail();
                    }
                    View::Audit => {
                        app.audit_filter = (app.audit_filter + 1) % 4;
                        app.audit_selected = 0;
                    }
                    View::Activity => {
                        app.activity_filter = (app.activity_filter + 1) % 5;
                        app.activity_selected = 0;
                        app.agent_monitor_selected = 0;
                    }
                    _ => {}
                },

                // Approve / Deny / Detail in Approvals view
                KeyCode::Char('a') if app.view == View::Approvals => {
                    app.resolve_approval(ApprovalStatus::Approved);
                }
                KeyCode::Char('d') if app.view == View::Approvals => {
                    app.resolve_approval(ApprovalStatus::Denied);
                }
                // [V] toggles the body detail pane for the selected approval
                KeyCode::Char('v') if app.view == View::Approvals => {
                    app.approval_detail_open = !app.approval_detail_open;
                    app.approval_detail_scroll = 0;
                }

                // Goals: create / edit / complete / abandon
                KeyCode::Char('n') if app.view == View::Goals => {
                    app.goal_popup = Some(crate::app::GoalPopup::Create {
                        description: String::new(),
                        criteria: String::new(),
                        priority: 2, // Medium
                        focused_field: 0,
                    });
                }
                KeyCode::Char('e') if app.view == View::Goals => {
                    let goals = app.filtered_goals();
                    if let Some(goal) = goals.get(app.goal_selected) {
                        let dl = goal
                            .deadline
                            .map(|d| d.format("%Y-%m-%d").to_string())
                            .unwrap_or_default();
                        app.goal_popup = Some(crate::app::GoalPopup::Edit {
                            goal_id: goal.id,
                            description: goal.description.clone(),
                            criteria: goal.success_criteria.clone(),
                            priority: crate::app::App::priority_to_index(&goal.priority),
                            deadline: dl,
                            focused_field: 0,
                        });
                    }
                }
                KeyCode::Char('c') if app.view == View::Goals => {
                    let goals = app.filtered_goals();
                    if let Some(goal) = goals.get(app.goal_selected)
                        && matches!(
                            goal.status,
                            aivyx_brain::GoalStatus::Active | aivyx_brain::GoalStatus::Dormant
                        )
                    {
                        let msg = format!("Complete \"{}\"?", truncate(&goal.description, 35));
                        app.goal_popup = Some(crate::app::GoalPopup::Confirm {
                            message: msg,
                            action: crate::app::GoalAction::Complete(goal.id),
                        });
                    }
                }
                KeyCode::Char('x') if app.view == View::Missions => {
                    app.cancel_mission();
                }
                KeyCode::Char('r') if app.view == View::Missions => {
                    app.resume_mission();
                }
                KeyCode::Char('a') if app.view == View::Missions => {
                    app.approve_mission();
                }
                KeyCode::Char('d') if app.view == View::Missions => {
                    app.deny_mission();
                }
                KeyCode::Char('x') if app.view == View::Goals => {
                    let goals = app.filtered_goals();
                    if let Some(goal) = goals.get(app.goal_selected)
                        && matches!(
                            goal.status,
                            aivyx_brain::GoalStatus::Active | aivyx_brain::GoalStatus::Dormant
                        )
                    {
                        let msg = format!("Abandon \"{}\"?", truncate(&goal.description, 35));
                        app.goal_popup = Some(crate::app::GoalPopup::Confirm {
                            message: msg,
                            action: crate::app::GoalAction::Abandon(goal.id),
                        });
                    }
                }

                // Settings error recovery: 'e' opens config in editor, 'r' reloads
                KeyCode::Char('e') if app.view == View::Settings && app.settings.is_none() => {
                    if let Some(ref state) = app.state {
                        let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".into());
                        let path = state.config_path.clone();
                        // Suspend TUI, open editor, resume
                        let _ = crossterm::terminal::disable_raw_mode();
                        let _ = crossterm::execute!(
                            std::io::stdout(),
                            crossterm::terminal::LeaveAlternateScreen
                        );
                        let _ = std::process::Command::new(&editor).arg(&path).status();
                        let _ = crossterm::terminal::enable_raw_mode();
                        let _ = crossterm::execute!(
                            std::io::stdout(),
                            crossterm::terminal::EnterAlternateScreen
                        );
                        // Reload after editing
                        match aivyx_pa::settings::reload_settings_snapshot(&path) {
                            Ok(s) => {
                                app.settings = Some(s);
                                app.settings_error = None;
                            }
                            Err(e) => {
                                app.settings = None;
                                app.settings_error = Some(e);
                            }
                        }
                    }
                }
                KeyCode::Char('r') if app.view == View::Settings && app.settings.is_none() => {
                    if let Some(ref state) = app.state {
                        match aivyx_pa::settings::reload_settings_snapshot(&state.config_path) {
                            Ok(s) => {
                                app.settings = Some(s);
                                app.settings_error = None;
                            }
                            Err(e) => {
                                app.settings = None;
                                app.settings_error = Some(e);
                            }
                        }
                    }
                }

                // Settings: 'd' to remove a configured integration
                KeyCode::Char('d')
                    if app.view == View::Settings
                        && app.settings_card_index == 5
                        && app.settings_popup.is_none() =>
                {
                    if let Some(ref settings) = app.settings {
                        let list = crate::app::App::integrations_list(settings);
                        let idx = app.settings_item_index;
                        if idx < list.len() {
                            let (label, configured, kind) = list[idx];
                            if configured {
                                app.settings_popup = Some(crate::app::SettingsPopup::Confirm {
                                    message: format!("Remove {label} integration?"),
                                    action: crate::app::ConfirmAction::RemoveIntegration(kind),
                                });
                            }
                        }
                    }
                }

                // Settings: Left/Right to switch columns (or adjust persona/app access)
                KeyCode::Left if app.view == View::Settings => {
                    if app.settings_card_index == 7 && app.settings_item_count(7) > 0 {
                        app.settings_adjust_persona(-0.1);
                    } else if app.settings_card_index == 9 && app.settings_item_count(9) > 0 {
                        app.settings_cycle_app_access(false);
                    } else if app.settings_card_index >= 4 {
                        app.settings_card_index -= 4;
                        app.settings_item_index = 0;
                    }
                }
                KeyCode::Right if app.view == View::Settings => {
                    if app.settings_card_index == 7 && app.settings_item_count(7) > 0 {
                        app.settings_adjust_persona(0.1);
                    } else if app.settings_card_index == 9 && app.settings_item_count(9) > 0 {
                        app.settings_cycle_app_access(true);
                    } else if app.settings_card_index < 4 {
                        app.settings_card_index += 4;
                        app.settings_item_index = 0;
                    }
                }
                // Settings: Enter to activate (edit/toggle/cycle)
                KeyCode::Enter if app.view == View::Settings => {
                    app.settings_activate_current();
                }

                _ => {}
            }
        }
    }
}

/// Move content selection up for the current view.
fn content_up(app: &mut App) {
    match app.view {
        View::Goals => {
            if app.goal_selected > 0 {
                app.goal_selected -= 1;
            }
        }
        View::Missions => {
            if app.mission_selected > 0 {
                app.mission_selected -= 1;
                app.load_mission_detail();
            }
        }
        View::Approvals => {
            if app.approval_detail_open {
                // Scroll the detail pane body
                app.approval_detail_scroll = app.approval_detail_scroll.saturating_sub(1);
            } else if app.approval_selected > 0 {
                app.approval_selected -= 1;
            }
        }
        View::Activity => {
            if app.activity_filter == 1 {
                // Agents tab
                if app.agent_monitor_selected > 0 {
                    app.agent_monitor_selected -= 1;
                }
            } else if app.activity_selected > 0 {
                app.activity_selected -= 1;
            }
        }
        View::Audit => {
            if app.audit_selected > 0 {
                app.audit_selected -= 1;
            }
        }
        View::Memory => {
            if app.memory_selected > 0 {
                app.memory_selected -= 1;
            }
        }
        View::Help => {
            app.help_scroll = app.help_scroll.saturating_sub(1);
        }
        View::Chat => {
            app.chat_scroll = app.chat_scroll.saturating_add(1);
        }
        View::Settings => {
            if app.settings_item_index > 0 {
                app.settings_item_index -= 1;
            } else if app.settings_card_index > 0 {
                // Move to previous card, select last item
                app.settings_card_index -= 1;
                app.settings_item_index = app
                    .settings_item_count(app.settings_card_index)
                    .saturating_sub(1);
            }
        }
        _ => {}
    }
}

/// Truncate a string to a max **character** length, adding "…" if truncated.
///
/// Uses `chars()` rather than byte indexing so multi-byte UTF-8 sequences
/// (emoji, accented characters, CJK) don't panic at a non-char boundary.
fn truncate(s: &str, max: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max {
        s.to_string()
    } else {
        let prefix: String = s.chars().take(max).collect();
        format!("{prefix}…")
    }
}

/// Move content selection down for the current view.
fn content_down(app: &mut App) {
    match app.view {
        View::Goals => {
            let max = app.filtered_goals().len().saturating_sub(1);
            if app.goal_selected < max {
                app.goal_selected += 1;
            }
        }
        View::Missions => {
            let max = app.filtered_missions().len().saturating_sub(1);
            if app.mission_selected < max {
                app.mission_selected += 1;
                app.load_mission_detail();
            }
        }
        View::Approvals => {
            if app.approval_detail_open {
                app.approval_detail_scroll = app.approval_detail_scroll.saturating_add(1);
            } else {
                let max = app.approvals.len().saturating_sub(1);
                if app.approval_selected < max {
                    app.approval_selected += 1;
                }
            }
        }
        View::Activity => {
            if app.activity_filter == 1 {
                // Agents tab
                let max = app.agent_statuses.len().saturating_sub(1);
                if app.agent_monitor_selected < max {
                    app.agent_monitor_selected += 1;
                }
            } else {
                let max = app.filtered_notifications().len().saturating_sub(1);
                if app.activity_selected < max {
                    app.activity_selected += 1;
                }
            }
        }
        View::Audit => {
            let max = app.filtered_audit().len().saturating_sub(1);
            if app.audit_selected < max {
                app.audit_selected += 1;
            }
        }
        View::Memory => {
            let max = app.memories.len().saturating_sub(1);
            if app.memory_selected < max {
                app.memory_selected += 1;
            }
        }
        View::Help => {
            app.help_scroll += 1;
        }
        View::Chat => {
            app.chat_scroll = app.chat_scroll.saturating_sub(1);
        }
        View::Settings => {
            let max = app.settings_item_count(app.settings_card_index);
            if max > 0 && app.settings_item_index < max - 1 {
                app.settings_item_index += 1;
            } else {
                // Move to next card, select first item
                let next = app.settings_card_index + 1;
                if next <= 9 {
                    app.settings_card_index = next;
                    app.settings_item_index = 0;
                }
            }
        }
        _ => {}
    }
}
