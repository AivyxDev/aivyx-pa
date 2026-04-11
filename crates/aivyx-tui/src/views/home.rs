//! Home view — live dashboard with stat cards, activity feed, and system status.

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Widget},
};

use crate::app::App;
use crate::theme;
use crate::widgets::telemetry::TelemetrySidebar;

pub fn render(app: &App, area: Rect, buf: &mut Buffer) {
    let [main_content, telemetry_area] =
        Layout::horizontal([Constraint::Min(40), Constraint::Length(32)]).areas(area);

    // Right: telemetry sidebar
    TelemetrySidebar::new(app).render(telemetry_area, buf);

    // Left: header + body
    let [header, body] =
        Layout::vertical([Constraint::Length(3), Constraint::Min(10)]).areas(main_content);

    // ── Header ────────────────────────────────────────────────
    let title = Line::from(vec![
        Span::styled("[ IDENTITY: ", theme::dim()),
        Span::styled(
            &app.agent_name,
            Style::default()
                .fg(theme::PRIMARY)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ]  [ CORE: ", theme::dim()),
        Span::styled(format!("v{}", app.version), theme::muted()),
        Span::styled(" ]", theme::dim()),
    ]);
    let tier_style = match app.autonomy_tier.to_lowercase().as_str() {
        "free" => theme::sage(),
        "trust" => theme::primary(),
        "leash" => theme::warning(),
        "locked" => theme::error(),
        _ => theme::dim(),
    };
    let subtitle = Line::from(vec![
        Span::styled("[ AUTONOMY: ", theme::dim()),
        Span::styled(&app.autonomy_tier, tier_style),
        Span::styled(" ]  |  ", theme::dim()),
        Span::styled("[ SKILLS: ", theme::dim()),
        Span::styled(
            app.settings
                .as_ref()
                .map(|s| s.agent_skills.len().to_string())
                .unwrap_or("0".into()),
            theme::text(),
        ),
        Span::styled(" ]  |  ", theme::dim()),
        Span::styled("[ SCHEDULES: ", theme::dim()),
        Span::styled(
            app.settings
                .as_ref()
                .map(|s| {
                    let active = s.schedules.iter().filter(|(_, _, e, _)| *e).count();
                    let total = s.schedules.len();
                    format!("{active}/{total}")
                })
                .unwrap_or("0/0".into()),
            theme::text(),
        ),
        Span::styled(" ]", theme::dim()),
    ]);
    buf.set_line(header.x, header.y, &title, header.width);
    buf.set_line(header.x, header.y + 1, &subtitle, header.width);

    // ── Body: stat cards + health bar + activity feed ──────────
    let [cards_area, health_area, activity_area] = Layout::vertical([
        Constraint::Length(5),
        Constraint::Length(1),
        Constraint::Min(5),
    ])
    .areas(body);

    render_stat_cards(app, cards_area, buf);
    render_health_bar(app, health_area, buf);
    render_activity_feed(app, activity_area, buf);
}

/// Render the 4 stat cards in a horizontal row.
fn render_stat_cards(app: &App, area: Rect, buf: &mut Buffer) {
    let [c1, c2, c3, c4] = Layout::horizontal([
        Constraint::Percentage(25),
        Constraint::Percentage(25),
        Constraint::Percentage(25),
        Constraint::Percentage(25),
    ])
    .areas(area);

    // Goals
    render_card(
        buf,
        c1,
        "Goals",
        &[
            (&format!("{}", app.active_goals), "active", theme::primary()),
            (&format!("{}", app.goal_count), "total", theme::dim()),
        ],
    );

    // Missions
    let active_missions = app
        .missions
        .iter()
        .filter(|m| !m.status.is_terminal())
        .count();
    let total_missions = app.missions.len();
    render_card(
        buf,
        c2,
        "Missions",
        &[
            (&format!("{active_missions}"), "active", theme::primary()),
            (&format!("{total_missions}"), "total", theme::dim()),
        ],
    );

    // Approvals
    let approval_style = if app.pending_approvals > 0 {
        theme::warning()
    } else {
        theme::dim()
    };
    render_card(
        buf,
        c3,
        "Approvals",
        &[(
            &format!("{}", app.pending_approvals),
            "pending",
            approval_style,
        )],
    );

    // Memories
    render_card(
        buf,
        c4,
        "Memories",
        &[(&format!("{}", app.memory_count), "stored", theme::primary())],
    );
}

/// Render a single stat card.
fn render_card(buf: &mut Buffer, area: Rect, title: &str, stats: &[(&str, &str, Style)]) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(theme::border())
        .title(Line::from(Span::styled(
            format!(" [ {title} ] "),
            theme::dim(),
        )));
    let inner = block.inner(area);
    block.render(area, buf);

    let mut y = inner.y;
    for (value, label, style) in stats {
        if y >= inner.y + inner.height {
            break;
        }
        let line = Line::from(vec![
            Span::styled(format!("  [ {}", label.to_uppercase()), theme::dim()),
            Span::styled(format!(": {} ]", value), style.add_modifier(Modifier::BOLD)),
        ]);
        buf.set_line(inner.x, y, &line, inner.width);
        y += 1;
    }
}

/// Render the system health status bar.
fn render_health_bar(app: &App, area: Rect, buf: &mut Buffer) {
    fn status_style(label: &str) -> Style {
        match label {
            "healthy" => theme::sage(),
            "degraded" => theme::error(),
            "n/a" => theme::dim(),
            _ => theme::warning(), // "..." = checking
        }
    }

    fn status_text(label: &str) -> &'static str {
        match label {
            "healthy" => "OK",
            "degraded" => "FAIL",
            "n/a" => "--",
            _ => "...",
        }
    }

    let mut spans = vec![Span::styled("[ SUBSYSTEMS ]  ", theme::dim())];

    for (name, label) in [
        ("LLM", &app.health_provider),
        ("EMAIL", &app.health_email),
        ("CFG", &app.health_config),
        ("DSK", &app.health_disk),
    ] {
        spans.push(Span::styled(format!("[ {name}: "), theme::dim()));
        spans.push(Span::styled(
            format!("{} ", status_text(label)),
            status_style(label),
        ));
        spans.push(Span::styled("]  ", theme::dim()));
    }

    // Append detail for degraded subsystems
    if let Some(ref detail) = app.health_provider_detail {
        spans.push(Span::styled("| ", theme::dim()));
        spans.push(Span::styled(
            format!("[ ERR: {} ]", detail.as_str()),
            theme::error(),
        ));
    } else if let Some(ref detail) = app.health_email_detail {
        spans.push(Span::styled("| ", theme::dim()));
        spans.push(Span::styled(
            format!("[ ERR: {} ]", detail.as_str()),
            theme::error(),
        ));
    }

    buf.set_line(area.x, area.y, &Line::from(spans), area.width);
}

/// Render the live activity feed (recent notifications).
fn render_activity_feed(app: &App, area: Rect, buf: &mut Buffer) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(theme::border())
        .title(Line::from(vec![
            Span::styled(" [ ACTIVITY FEED ]  ", theme::dim()),
            Span::styled(
                format!("[ LOGS: {} ] ", app.notifications.len()),
                theme::muted(),
            ),
        ]));
    let inner = block.inner(area);
    block.render(area, buf);

    if app.notifications.is_empty() {
        let empty = Line::from(Span::styled("  No recent activity.", theme::dim()));
        buf.set_line(inner.x + 1, inner.y + 1, &empty, inner.width - 2);
        return;
    }

    let max_rows = inner.height as usize;
    for (i, notif) in app.notifications.iter().take(max_rows).enumerate() {
        let y = inner.y + i as u16;
        if y >= inner.y + inner.height {
            break;
        }

        let source_color = match notif.source.as_str() {
            s if s.contains("heartbeat") => theme::SAGE,
            "schedule" | "briefing" => theme::SECONDARY,
            "triage" | "email" => theme::ACCENT_GLOW,
            "goal" => theme::ACCENT_GLOW,
            "mission" => theme::PRIMARY,
            _ => theme::PRIMARY,
        };

        let timestamp = notif.timestamp.format("%H:%M:%S").to_string();
        let source_tag = notif.source.to_uppercase();

        // Truncate title to fit
        let prefix_len = timestamp.len() + source_tag.len() + 7; // "[HH:MM:SS] | TAG | "
        let max_title = (inner.width as usize).saturating_sub(prefix_len + 2);
        let title = if notif.title.len() > max_title && max_title > 1 {
            format!("{}…", &notif.title[..max_title - 1])
        } else {
            notif.title.clone()
        };

        let line = Line::from(vec![
            Span::styled(format!("[{timestamp}] | "), theme::dim()),
            Span::styled(
                format!("{source_tag} | "),
                Style::default()
                    .fg(source_color)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(title, theme::text()),
        ]);
        buf.set_line(inner.x + 1, y, &line, inner.width - 2);
    }
}
