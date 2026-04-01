//! TUI — the primary interface for the personal assistant.
//!
//! Launches a ratatui terminal UI with:
//! - Home: morning briefing + recent activity
//! - Chat: conversational interface
//! - Activity: audit log of what the assistant has been doing
//! - Settings: configuration

use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::{
    prelude::*,
    widgets::*,
};
use std::io::stdout;
use std::path::Path;

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
    chat_messages: Vec<(String, String)>, // (role, content)
    notifications: Vec<String>,
}

impl App {
    fn new() -> Self {
        Self {
            view: View::Home,
            running: true,
            chat_input: String::new(),
            chat_messages: Vec::new(),
            notifications: vec![
                "Welcome to Aivyx. Your assistant is ready.".into(),
            ],
        }
    }
}

pub async fn run(home: &Path) -> anyhow::Result<()> {
    let _ = home; // Will be used to load config, connect to agent, etc.

    // Setup terminal
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();

    // Main event loop
    while app.running {
        terminal.draw(|frame| render(&app, frame))?;

        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                handle_key(&mut app, key);
            }
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;

    Ok(())
}

fn handle_key(app: &mut App, key: event::KeyEvent) {
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
            KeyCode::Char(c) => {
                if app.chat_input.len() < 8192 {
                    app.chat_input.push(c);
                }
            }
            KeyCode::Backspace => { app.chat_input.pop(); }
            KeyCode::Enter if !app.chat_input.is_empty() => {
                let msg = std::mem::take(&mut app.chat_input);
                app.chat_messages.push(("you".into(), msg));
                app.chat_messages.push(("assistant".into(), "(thinking...)".into()));
                // TODO: send to agent, stream response
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

fn render(app: &App, frame: &mut Frame) {
    let area = frame.area();

    // Split: sidebar (20 cols) | main content
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

    // Messages
    let messages: Vec<Line> = app.chat_messages
        .iter()
        .map(|(role, content)| {
            let style = if role == "you" {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::White)
            };
            Line::from(Span::styled(format!("  {role}: {content}"), style))
        })
        .collect();

    let messages_widget = if messages.is_empty() {
        Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled("  Start a conversation.", Style::default().fg(Color::DarkGray))),
            Line::from(Span::styled("  Type below and press Enter.", Style::default().fg(Color::DarkGray))),
        ])
    } else {
        Paragraph::new(messages).scroll((
            app.chat_messages.len().saturating_sub(layout[0].height as usize) as u16,
            0,
        ))
    };

    frame.render_widget(
        messages_widget.block(Block::default().borders(Borders::BOTTOM).border_style(Style::default().fg(Color::DarkGray))),
        layout[0],
    );

    // Input
    let input = Paragraph::new(format!("  > {}", app.chat_input))
        .style(Style::default().fg(Color::White));
    frame.render_widget(input, layout[1]);

    // Cursor
    frame.set_cursor_position((
        (3 + app.chat_input.len()) as u16 + layout[1].x,
        layout[1].y,
    ));
}

fn render_activity(_app: &App, frame: &mut Frame, area: Rect) {
    let content = Paragraph::new(vec![
        Line::from(Span::styled("  Activity Log", Style::default().fg(Color::White).bold())),
        Line::from(""),
        Line::from(Span::styled("  No activity recorded yet.", Style::default().fg(Color::DarkGray))),
        Line::from(Span::styled("  Your assistant's actions will appear here.", Style::default().fg(Color::DarkGray))),
    ])
    .block(Block::default().padding(Padding::new(1, 1, 1, 1)));

    frame.render_widget(content, area);
}

fn render_settings(_app: &App, frame: &mut Frame, area: Rect) {
    let content = Paragraph::new(vec![
        Line::from(Span::styled("  Settings", Style::default().fg(Color::White).bold())),
        Line::from(""),
        Line::from(Span::styled("  (settings editor coming soon)", Style::default().fg(Color::DarkGray))),
    ])
    .block(Block::default().padding(Padding::new(1, 1, 1, 1)));

    frame.render_widget(content, area);
}

use chrono::Timelike;
