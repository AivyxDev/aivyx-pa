#![allow(clippy::if_same_then_else, clippy::collapsible_if)]

//! Activity view — real-time notification timeline with agent monitoring.

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Widget},
};

use crate::app::App;
use crate::theme;

const FILTERS: [&str; 5] = ["All", "Agents", "Schedule", "Heartbeat", "Triage"];

pub fn render(app: &App, area: Rect, buf: &mut Buffer) {
    let [header, filter_row, body, help_bar] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Min(5),
        Constraint::Length(1),
    ])
    .areas(area);

    let count = app.notifications.len();
    let title = Paragraph::new(Line::from(vec![
        Span::styled("[ ACTIVITY FEED ]", theme::text_bold()),
        Span::styled(format!("  [ EVENTS: {count} ]"), theme::dim()),
    ]));
    title.render(header, buf);

    // ── Filter tabs ──────────────────────────────────────────
    let mut filter_spans = Vec::new();
    for (i, f) in FILTERS.iter().enumerate() {
        let style = if i == app.activity_filter {
            Style::default()
                .fg(theme::PRIMARY)
                .add_modifier(Modifier::BOLD)
        } else {
            theme::dim()
        };
        filter_spans.push(Span::styled(format!("[ {} ] ", f.to_uppercase()), style));
    }
    buf.set_line(
        filter_row.x,
        filter_row.y,
        &Line::from(filter_spans),
        filter_row.width,
    );

    let [list_area, detail_area] =
        Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)]).areas(body);

    // Agents tab gets its own rendering path
    if app.activity_filter == 1 {
        render_agent_list(app, list_area, buf);
        render_agent_detail(app, detail_area, buf);
    } else {
        render_notification_list(app, list_area, buf);
        render_notification_detail(app, detail_area, buf);
    }

    // ── Help bar ─────────────────────────────────────────────
    let help = if app.activity_filter == 1 {
        Line::from(vec![
            Span::styled("↑↓", theme::primary()),
            Span::styled(" navigate  ", theme::dim()),
            Span::styled("[]", theme::primary()),
            Span::styled(" filter  ", theme::dim()),
            Span::styled("Tab", theme::primary()),
            Span::styled(" sidebar", theme::dim()),
        ])
    } else {
        Line::from(vec![
            Span::styled("↑↓", theme::primary()),
            Span::styled(" navigate  ", theme::dim()),
            Span::styled("[]", theme::primary()),
            Span::styled(" filter  ", theme::dim()),
            Span::styled("Tab", theme::primary()),
            Span::styled(" sidebar", theme::dim()),
        ])
    };
    buf.set_line(help_bar.x + 1, help_bar.y, &help, help_bar.width - 2);
}

// ── Agent list panel ─────────────────────────────────────────────

fn render_agent_list(app: &App, area: Rect, buf: &mut Buffer) {
    let active_count = app
        .agent_statuses
        .iter()
        .filter(|a| a.state == "running")
        .count();
    let title_text = if active_count > 0 {
        format!("[ AGENTS ({active_count} active) ]")
    } else {
        "[ AGENTS ]".into()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(theme::border())
        .title(Line::from(Span::styled(&title_text, theme::primary_bold())));
    let inner = block.inner(area);
    block.render(area, buf);

    if app.agent_statuses.is_empty() {
        let empty = Line::from(Span::styled("  No agent activity yet.", theme::dim()));
        buf.set_line(inner.x + 1, inner.y + 1, &empty, inner.width - 2);
        return;
    }

    // Sort: running agents first, then by name
    let mut sorted_indices: Vec<usize> = (0..app.agent_statuses.len()).collect();
    sorted_indices.sort_by(|&a, &b| {
        let a_running = app.agent_statuses[a].state == "running";
        let b_running = app.agent_statuses[b].state == "running";
        b_running
            .cmp(&a_running)
            .then_with(|| app.agent_statuses[a].name.cmp(&app.agent_statuses[b].name))
    });

    let rows_per_item = 2u16;
    let visible_count = inner.height / rows_per_item;
    let scroll_offset = if app.agent_monitor_selected as u16 >= visible_count {
        app.agent_monitor_selected - (visible_count as usize - 1)
    } else {
        0
    };

    let mut y = inner.y;
    for (display_idx, &real_idx) in sorted_indices.iter().enumerate().skip(scroll_offset) {
        if y + 1 >= inner.y + inner.height {
            break;
        }
        let agent = &app.agent_statuses[real_idx];
        let is_selected = display_idx == app.agent_monitor_selected;

        let (status_tag, icon_style) = match agent.state.as_str() {
            "running" => ("[ RUNNING ]", theme::sage()),
            "completed" => ("[ SUCCESS ]", theme::secondary()),
            "error" => ("[ FAILURE ]", Style::default().fg(theme::ERROR)),
            _ => ("[ IDLE    ]", theme::muted()),
        };

        let marker = if is_selected { "[■]" } else { "[ ]" };
        let name_style = if is_selected {
            theme::primary_bold()
        } else if agent.state == "running" {
            theme::text_bold()
        } else {
            theme::dim()
        };

        // Elapsed time for running agents
        let elapsed = if agent.state == "running" {
            agent
                .started_at
                .map(|start| {
                    let secs = chrono::Utc::now()
                        .signed_duration_since(start)
                        .num_seconds();
                    format!(" [ {secs}s ]")
                })
                .unwrap_or_default()
        } else {
            agent
                .last_duration_ms
                .map(|ms| format!(" [ {:.1}s ]", ms as f64 / 1000.0))
                .unwrap_or_default()
        };

        let line = Line::from(vec![
            Span::styled(
                marker,
                if is_selected {
                    theme::primary()
                } else {
                    icon_style
                },
            ),
            Span::styled(format!(" {} ", status_tag), icon_style),
            Span::styled(&agent.name, name_style),
            Span::styled(&elapsed, theme::dim()),
        ]);
        buf.set_line(inner.x + 1, y, &line, inner.width - 2);
        y += 1;

        // Second row: current task or last tool
        if y < inner.y + inner.height {
            let detail_text = if let Some(ref task) = agent.current_task {
                let max = (inner.width - 15) as usize;
                let truncated: String = task.chars().take(max).collect();
                truncated
            } else if let Some(entry) = agent.tool_log.first() {
                let icon = match entry.status.as_str() {
                    "completed" => "✓",
                    "failed" => "✗",
                    "denied" => "⊘",
                    _ => "◐",
                };
                let dur = entry
                    .duration_ms
                    .map(|ms| format!(" {ms}ms"))
                    .unwrap_or_default();
                format!("tool: {} {icon}{dur}", entry.tool_name)
            } else {
                String::new()
            };

            let detail_line = Line::from(vec![
                Span::styled("│  ", icon_style),
                Span::styled("         ", Style::default()),
                Span::styled(&detail_text, theme::muted()),
            ]);
            buf.set_line(inner.x + 1, y, &detail_line, inner.width - 2);
            y += 1;
        }
    }
}

// ── Agent detail panel ───────────────────────────────────────────

fn render_agent_detail(app: &App, area: Rect, buf: &mut Buffer) {
    let detail_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(theme::border())
        .title(Line::from(Span::styled(
            "[ DETAIL ]",
            theme::primary_bold(),
        )));
    let detail_inner = detail_block.inner(area);
    detail_block.render(area, buf);

    // Find selected agent (with same sort order as list)
    let mut sorted_indices: Vec<usize> = (0..app.agent_statuses.len()).collect();
    sorted_indices.sort_by(|&a, &b| {
        let a_running = app.agent_statuses[a].state == "running";
        let b_running = app.agent_statuses[b].state == "running";
        b_running
            .cmp(&a_running)
            .then_with(|| app.agent_statuses[a].name.cmp(&app.agent_statuses[b].name))
    });

    let Some(&real_idx) = sorted_indices.get(app.agent_monitor_selected) else {
        let empty = Line::from(Span::styled("  No agent selected.", theme::dim()));
        buf.set_line(
            detail_inner.x + 1,
            detail_inner.y,
            &empty,
            detail_inner.width - 2,
        );
        return;
    };

    let agent = &app.agent_statuses[real_idx];
    let mut y = detail_inner.y;
    let w = detail_inner.width.saturating_sub(2);

    // Agent name
    let name_line = Line::from(vec![
        Span::styled("[ CORE     ]  ", theme::muted()),
        Span::styled(&agent.name, theme::primary_bold()),
    ]);
    buf.set_line(detail_inner.x + 1, y, &name_line, w);
    y += 1;

    // Status
    let (status_label, status_style) = match agent.state.as_str() {
        "running" => ("RUNNING", theme::sage()),
        "completed" => ("COMPLETED", theme::secondary()),
        "error" => ("ERROR", Style::default().fg(theme::ERROR)),
        _ => ("IDLE", theme::muted()),
    };
    let status_line = Line::from(vec![
        Span::styled("[ STATUS   ]  ", theme::muted()),
        Span::styled(status_label, status_style),
    ]);
    buf.set_line(detail_inner.x + 1, y, &status_line, w);
    y += 1;

    // Task
    if let Some(ref task) = agent.current_task {
        let task_line = Line::from(vec![
            Span::styled("[ ASSIGNED ]  ", theme::muted()),
            Span::styled(task.as_str(), theme::text()),
        ]);
        buf.set_line(detail_inner.x + 1, y, &task_line, w);
        y += 1;
    }

    // Duration
    if agent.state == "running" {
        if let Some(start) = agent.started_at {
            let secs = chrono::Utc::now()
                .signed_duration_since(start)
                .num_seconds();
            let dur_line = Line::from(vec![
                Span::styled("[ ELAPSED  ]  ", theme::muted()),
                Span::styled(format!("{secs}s"), theme::dim()),
            ]);
            buf.set_line(detail_inner.x + 1, y, &dur_line, w);
            y += 1;
        }
    } else if let Some(ms) = agent.last_duration_ms {
        let dur_line = Line::from(vec![
            Span::styled("[ ELAPSED  ]  ", theme::muted()),
            Span::styled(format!("{:.1}s", ms as f64 / 1000.0), theme::dim()),
        ]);
        buf.set_line(detail_inner.x + 1, y, &dur_line, w);
        y += 1;
    }

    y += 1; // blank separator

    // Tool call log
    if !agent.tool_log.is_empty() && y < detail_inner.y + detail_inner.height {
        let tools_header = Line::from(Span::styled("Tools:", theme::muted()));
        buf.set_line(detail_inner.x + 1, y, &tools_header, w);
        y += 1;

        for entry in &agent.tool_log {
            if y >= detail_inner.y + detail_inner.height {
                break;
            }
            let (icon, icon_style) = match entry.status.as_str() {
                "completed" => ("[⚙ OK]", theme::sage()),
                "failed" => ("[⚙ FAIL]", Style::default().fg(theme::ERROR)),
                "denied" => ("[⚙ DENIED]", theme::warning()),
                _ => ("[⚙ RUN]", theme::secondary()),
            };
            let dur = entry
                .duration_ms
                .map(|ms| format!(" [ {}ms ]", ms))
                .unwrap_or_else(|| " [ ... ]".into());
            let tool_line = Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled(icon, icon_style),
                Span::styled(" ", Style::default()),
                Span::styled(&entry.tool_name, theme::text()),
                Span::styled(&dur, theme::dim()),
            ]);
            buf.set_line(detail_inner.x + 1, y, &tool_line, w);
            y += 1;

            // Show error on next line if present
            if let Some(ref err) = entry.error {
                if y < detail_inner.y + detail_inner.height {
                    let max = (w - 4) as usize;
                    let truncated: String = err.chars().take(max).collect();
                    let err_line = Line::from(vec![
                        Span::styled("    ", Style::default()),
                        Span::styled(truncated, Style::default().fg(theme::ERROR)),
                    ]);
                    buf.set_line(detail_inner.x + 1, y, &err_line, w);
                    y += 1;
                }
            }
        }
    }
}

// ── Notification list panel (existing) ───────────────────────────

fn render_notification_list(app: &App, list_area: Rect, buf: &mut Buffer) {
    let filtered = app.filtered_notifications();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(theme::border())
        .title(Line::from(Span::styled(
            format!("[ TIMELINE ({}) ]", filtered.len()),
            theme::primary_bold(),
        )));
    let inner = block.inner(list_area);
    block.render(list_area, buf);

    if filtered.is_empty() {
        let empty = Line::from(Span::styled("  Waiting for events...", theme::dim()));
        buf.set_line(inner.x + 1, inner.y + 1, &empty, inner.width - 2);
    } else {
        let rows_per_item = 2u16;
        let visible_count = inner.height / rows_per_item;
        let scroll_offset = if app.activity_selected as u16 >= visible_count {
            app.activity_selected - (visible_count as usize - 1)
        } else {
            0
        };

        let mut y = inner.y;
        for (i, notif) in filtered.iter().enumerate().skip(scroll_offset) {
            if y + 1 >= inner.y + inner.height {
                break;
            }

            let is_selected = i == app.activity_selected;
            let icon_style = match notif.source.as_str() {
                s if s.contains("heartbeat") => theme::sage(),
                "schedule" | "briefing" => theme::secondary(),
                "triage" | "email" => theme::warning(),
                "goal" => theme::primary(),
                "mission" => theme::primary(),
                "agent" => theme::sage(),
                _ if notif.requires_approval => theme::warning(),
                _ => theme::muted(),
            };

            let marker = if is_selected { "[■]" } else { "[ ]" };
            let title_style = if is_selected {
                theme::primary_bold()
            } else {
                theme::text_bold()
            };
            let time = notif.timestamp.format("%H:%M:%S").to_string();
            let source_tag = notif.source.to_uppercase();
            let timeline = Line::from(vec![
                Span::styled(
                    marker,
                    if is_selected {
                        theme::primary()
                    } else {
                        icon_style
                    },
                ),
                Span::styled(format!(" [ {time} ] "), theme::dim()),
                Span::styled(format!("[ {source_tag} ] "), icon_style),
                Span::styled(&notif.title, title_style),
            ]);
            buf.set_line(inner.x + 1, y, &timeline, inner.width - 2);
            y += 1;

            if y < inner.y + inner.height && !notif.body.is_empty() {
                let body_preview: String = notif
                    .body
                    .lines()
                    .next()
                    .unwrap_or("")
                    .chars()
                    .take((inner.width - 15) as usize)
                    .collect();
                let detail = Line::from(vec![
                    Span::styled("│  ", icon_style), // connector adopts source color
                    Span::styled("         ", Style::default()),
                    Span::styled(body_preview, theme::muted()),
                ]);
                buf.set_line(inner.x + 1, y, &detail, inner.width - 2);
                y += 1;
            }
        }
    }
}

// ── Notification detail panel (existing) ─────────────────────────

fn render_notification_detail(app: &App, detail_area: Rect, buf: &mut Buffer) {
    let detail_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(theme::border())
        .title(Line::from(Span::styled(
            "[ DETAIL ]",
            theme::primary_bold(),
        )));
    let detail_inner = detail_block.inner(detail_area);
    detail_block.render(detail_area, buf);

    let filtered_detail = app.filtered_notifications();
    if let Some(notif) = filtered_detail.get(app.activity_selected) {
        let mut y = detail_inner.y;

        // Source
        let source_line = Line::from(vec![
            Span::styled("[ ROUTE    ]  ", theme::muted()),
            Span::styled(notif.source.to_uppercase(), theme::secondary()),
        ]);
        buf.set_line(detail_inner.x + 1, y, &source_line, detail_inner.width - 2);
        y += 1;

        // Time
        let time_line = Line::from(vec![
            Span::styled("[ DISPATCH ]  ", theme::muted()),
            Span::styled(
                notif.timestamp.format("%Y-%m-%d %H:%M:%S").to_string(),
                theme::dim(),
            ),
        ]);
        buf.set_line(detail_inner.x + 1, y, &time_line, detail_inner.width - 2);
        y += 1;

        // Approval status
        if notif.requires_approval {
            let approval = Line::from(vec![
                Span::styled("[ APPROVAL ]  ", theme::muted()),
                Span::styled("REQUIRED", theme::warning()),
            ]);
            buf.set_line(detail_inner.x + 1, y, &approval, detail_inner.width - 2);
            y += 1;
        }
        y += 1;

        // Full body (word-wrapped)
        let max_w = (detail_inner.width - 4) as usize;
        for line in notif.body.lines() {
            if y >= detail_inner.y + detail_inner.height {
                break;
            }
            if line.len() <= max_w {
                buf.set_line(
                    detail_inner.x + 1,
                    y,
                    &Line::from(Span::styled(line, theme::text())),
                    detail_inner.width - 2,
                );
                y += 1;
            } else {
                let mut pos = 0;
                while pos < line.len() && y < detail_inner.y + detail_inner.height {
                    let end = (pos + max_w).min(line.len());
                    buf.set_line(
                        detail_inner.x + 1,
                        y,
                        &Line::from(Span::styled(&line[pos..end], theme::text())),
                        detail_inner.width - 2,
                    );
                    y += 1;
                    pos = end;
                }
            }
        }
    } else {
        let empty = Line::from(Span::styled("  No event selected.", theme::dim()));
        buf.set_line(
            detail_inner.x + 1,
            detail_inner.y,
            &empty,
            detail_inner.width - 2,
        );
    }
}
