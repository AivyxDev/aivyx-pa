//! Goals view — filterable list with progress bars, selection, and detail panel.

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Widget},
};

use crate::app::{App, GoalPopup, PRIORITIES};
use crate::theme;

const FILTERS: [&str; 4] = ["All", "Active", "Completed", "Abandoned"];

pub fn render(app: &App, area: Rect, buf: &mut Buffer) {
    let [header, filter_row, body, help_bar] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Min(5),
        Constraint::Length(1),
    ])
    .areas(area);

    // Title
    let total = app.goals.len();
    let title = Line::from(vec![
        Span::styled("Goals", theme::text_bold()),
        Span::styled(
            format!("  {total} goals tracked by your assistant."),
            theme::dim(),
        ),
    ]);
    buf.set_line(header.x, header.y, &title, header.width);

    // Filter tabs
    let mut filter_spans = Vec::new();
    for (i, f) in FILTERS.iter().enumerate() {
        let style = if i == app.goal_filter {
            Style::default()
                .fg(theme::PRIMARY)
                .add_modifier(Modifier::BOLD)
        } else {
            theme::dim()
        };
        filter_spans.push(Span::styled(format!(" {f} "), style));
        if i < FILTERS.len() - 1 {
            filter_spans.push(Span::styled("│", theme::dim()));
        }
    }
    buf.set_line(
        filter_row.x,
        filter_row.y,
        &Line::from(filter_spans),
        filter_row.width,
    );

    // Split: goal list + detail panel
    let [list_area, detail_area] =
        Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(body);

    // ── Goal list with scroll ─────────────────────────────────
    let goals = app.filtered_goals();

    let list_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::border())
        .title(Line::from(Span::styled(
            format!(" {} Goals ", goals.len()),
            theme::dim(),
        )));
    let list_inner = list_block.inner(list_area);
    list_block.render(list_area, buf);

    if goals.is_empty() {
        let empty = Line::from(Span::styled("  No goals yet.", theme::dim()));
        buf.set_line(
            list_inner.x + 1,
            list_inner.y + 1,
            &empty,
            list_inner.width - 2,
        );
    } else {
        let card_height = 5u16;
        let visible_count = list_inner.height / card_height;
        let scroll_offset = if app.goal_selected as u16 >= visible_count {
            app.goal_selected - (visible_count as usize - 1)
        } else {
            0
        };

        let mut y = list_inner.y;
        for (i, goal) in goals.iter().enumerate().skip(scroll_offset) {
            if y + card_height > list_inner.y + list_inner.height {
                break;
            }

            let is_selected = i == app.goal_selected;
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(if is_selected {
                    theme::border_active()
                } else {
                    theme::border()
                });
            let card = Rect::new(list_inner.x, y, list_inner.width, card_height);
            let inner = block.inner(card);
            block.render(card, buf);

            // Title + priority + optional cooldown icon
            let priority_str = format!("{:?}", goal.priority).to_lowercase();
            let priority_style = match priority_str.as_str() {
                "high" | "critical" => theme::error(),
                "medium" => theme::warning(),
                _ => theme::dim(),
            };
            let status_str = format!("{:?}", goal.status).to_lowercase();
            let mut title_spans = vec![
                Span::styled(
                    &goal.description,
                    if is_selected {
                        theme::primary_bold()
                    } else {
                        theme::text_bold()
                    },
                ),
                Span::styled("  [", theme::dim()),
                Span::styled(&priority_str, priority_style),
                Span::styled("] [", theme::dim()),
                Span::styled(&status_str, theme::muted()),
                Span::styled("]", theme::dim()),
            ];
            if goal.cooldown_until.is_some() {
                title_spans.push(Span::styled(" ⏸", theme::warning()));
            }
            let title_line = Line::from(title_spans);
            buf.set_line(inner.x + 1, inner.y, &title_line, inner.width - 2);

            // Progress bar with high-contrast block chars
            let pct = goal.progress;
            let bar_width = (inner.width - 14) as usize;
            let filled = (pct as f64 * bar_width as f64) as usize;
            let empty = bar_width.saturating_sub(filled);
            let pct_display = (pct * 100.0) as u8;
            let bar = format!(
                "{}{}  {}%",
                "█".repeat(filled),
                "░".repeat(empty),
                pct_display
            );
            let bar_style = match goal.status {
                aivyx_brain::GoalStatus::Completed => theme::sage(),
                aivyx_brain::GoalStatus::Abandoned => theme::dim(),
                _ => theme::primary(),
            };
            buf.set_line(
                inner.x + 1,
                inner.y + 1,
                &Line::from(Span::styled(bar, bar_style)),
                inner.width - 2,
            );

            // Info line: deadline (if set) or last-updated timestamp
            if inner.height > 2 {
                let info_str = if let Some(dl) = goal.deadline {
                    format!("  deadline: {}", dl.format("%Y-%m-%d"))
                } else {
                    format!("  updated: {}", goal.updated_at.format("%H:%M"))
                };
                buf.set_line(
                    inner.x + 1,
                    inner.y + 2,
                    &Line::from(Span::styled(info_str, theme::dim())),
                    inner.width - 2,
                );
            }

            y += card_height;
        }
    }

    // ── Detail panel for selected goal ────────────────────────
    let detail_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::border())
        .title(Line::from(Span::styled(" Goal Detail ", theme::dim())));
    let detail_inner = detail_block.inner(detail_area);
    detail_block.render(detail_area, buf);

    let filtered = app.filtered_goals();
    if let Some(goal) = filtered.get(app.goal_selected) {
        let mut y = detail_inner.y;

        // Description
        let desc_line = Line::from(vec![Span::styled(&goal.description, theme::primary_bold())]);
        buf.set_line(detail_inner.x + 1, y, &desc_line, detail_inner.width - 2);
        y += 2;

        // Status + Priority
        let status_str = format!("{:?}", goal.status);
        let priority_str = format!("{:?}", goal.priority);
        let meta_line = Line::from(vec![
            Span::styled("Status:   ", theme::muted()),
            Span::styled(&status_str, theme::text()),
            Span::styled("    Priority: ", theme::muted()),
            Span::styled(&priority_str, theme::text()),
        ]);
        buf.set_line(detail_inner.x + 1, y, &meta_line, detail_inner.width - 2);
        y += 1;

        // Progress
        let pct_line = Line::from(vec![
            Span::styled("Progress: ", theme::muted()),
            Span::styled(format!("{:.0}%", goal.progress * 100.0), theme::primary()),
        ]);
        buf.set_line(detail_inner.x + 1, y, &pct_line, detail_inner.width - 2);
        y += 2;

        // Success criteria (wrapped)
        if !goal.success_criteria.is_empty() {
            let label = Line::from(Span::styled("Success Criteria:", theme::muted()));
            buf.set_line(detail_inner.x + 1, y, &label, detail_inner.width - 2);
            y += 1;

            let max_w = (detail_inner.width - 4) as usize;
            for line in goal.success_criteria.lines() {
                if y >= detail_inner.y + detail_inner.height {
                    break;
                }
                if line.len() <= max_w {
                    buf.set_line(
                        detail_inner.x + 2,
                        y,
                        &Line::from(Span::styled(line, theme::text())),
                        detail_inner.width - 3,
                    );
                    y += 1;
                } else {
                    let mut pos = 0;
                    while pos < line.len() && y < detail_inner.y + detail_inner.height {
                        let end = (pos + max_w).min(line.len());
                        buf.set_line(
                            detail_inner.x + 2,
                            y,
                            &Line::from(Span::styled(&line[pos..end], theme::text())),
                            detail_inner.width - 3,
                        );
                        y += 1;
                        pos = end;
                    }
                }
            }
            y += 1;
        }

        // Tags — self-development goals rendered in lavender to distinguish
        // the agent's own growth goals from user-created goals
        if !goal.tags.is_empty() && y < detail_inner.y + detail_inner.height {
            let tags: Vec<Span> = goal
                .tags
                .iter()
                .map(|t| {
                    let color = if t == "self-development" {
                        theme::SECONDARY
                    } else {
                        theme::PRIMARY
                    };
                    Span::styled(
                        format!(" [{t}] "),
                        Style::default().fg(color).add_modifier(Modifier::BOLD),
                    )
                })
                .collect();
            buf.set_line(
                detail_inner.x + 1,
                y,
                &Line::from(tags),
                detail_inner.width - 2,
            );
            y += 2;
        }

        // Deadline
        if y < detail_inner.y + detail_inner.height {
            let dl_str = goal
                .deadline
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_else(|| "none".into());
            let dl_line = Line::from(vec![
                Span::styled("Deadline: ", theme::muted()),
                Span::styled(&dl_str, theme::text()),
            ]);
            buf.set_line(detail_inner.x + 1, y, &dl_line, detail_inner.width - 2);
            y += 1;
        }

        // Timestamps
        if y + 1 < detail_inner.y + detail_inner.height {
            let created = Line::from(vec![
                Span::styled("Created:  ", theme::muted()),
                Span::styled(
                    goal.created_at.format("%Y-%m-%d %H:%M").to_string(),
                    theme::dim(),
                ),
            ]);
            buf.set_line(detail_inner.x + 1, y, &created, detail_inner.width - 2);
            y += 1;

            if y < detail_inner.y + detail_inner.height {
                let updated = Line::from(vec![
                    Span::styled("Updated:  ", theme::muted()),
                    Span::styled(
                        goal.updated_at.format("%Y-%m-%d %H:%M").to_string(),
                        theme::dim(),
                    ),
                ]);
                buf.set_line(detail_inner.x + 1, y, &updated, detail_inner.width - 2);
                y += 1;
            }
        }

        // Failure info (if any)
        if goal.failure_count > 0 && y < detail_inner.y + detail_inner.height {
            y += 1;
            let fail_line = Line::from(vec![
                Span::styled("Failures: ", theme::muted()),
                Span::styled(
                    format!(
                        "{} total, {} consecutive",
                        goal.failure_count, goal.consecutive_failures
                    ),
                    theme::error(),
                ),
            ]);
            buf.set_line(detail_inner.x + 1, y, &fail_line, detail_inner.width - 2);
            y += 1;

            if let Some(ref cooldown) = goal.cooldown_until {
                if y < detail_inner.y + detail_inner.height {
                    let cd = Line::from(vec![
                        Span::styled("Cooldown: ", theme::muted()),
                        Span::styled(
                            format!("until {}", cooldown.format("%H:%M")),
                            theme::warning(),
                        ),
                    ]);
                    buf.set_line(detail_inner.x + 1, y, &cd, detail_inner.width - 2);
                }
            }
        }

        // Sub-goals (goals whose parent matches this goal's id)
        let sub_goals: Vec<_> = app
            .goals
            .iter()
            .filter(|g| g.parent == Some(goal.id))
            .collect();
        if !sub_goals.is_empty() && y + 2 < detail_inner.y + detail_inner.height {
            y += 1;
            let label = Line::from(Span::styled("Sub-goals:", theme::muted()));
            buf.set_line(detail_inner.x + 1, y, &label, detail_inner.width - 2);
            y += 1;
            for sg in &sub_goals {
                if y >= detail_inner.y + detail_inner.height {
                    break;
                }
                let marker = match sg.status {
                    aivyx_brain::GoalStatus::Completed => "✓",
                    aivyx_brain::GoalStatus::Abandoned => "✗",
                    _ => "○",
                };
                let style = match sg.status {
                    aivyx_brain::GoalStatus::Completed => theme::sage(),
                    aivyx_brain::GoalStatus::Abandoned => theme::dim(),
                    _ => theme::text(),
                };
                let line = Line::from(vec![
                    Span::styled(format!("  {marker} "), style),
                    Span::styled(&sg.description, style),
                ]);
                buf.set_line(detail_inner.x + 1, y, &line, detail_inner.width - 2);
                y += 1;
            }
        }

        let _ = y; // suppress unused
    } else {
        let empty = Line::from(Span::styled(
            "  Select a goal to view details.",
            theme::dim(),
        ));
        buf.set_line(
            detail_inner.x + 1,
            detail_inner.y,
            &empty,
            detail_inner.width - 2,
        );
    }

    // ── Help bar ─────────────────────────────────────────────
    let help_text = if app.goal_popup.is_some() {
        match &app.goal_popup {
            Some(GoalPopup::Confirm { .. }) => "y confirm  n/Esc cancel",
            _ => "Tab next field  ←→ priority  Enter save  Esc cancel",
        }
    } else {
        "↑↓ navigate  [] filter  n new  e edit  c complete  x abandon  Tab sidebar"
    };
    let help = Line::from(Span::styled(help_text, theme::dim()));
    buf.set_line(help_bar.x + 1, help_bar.y, &help, help_bar.width - 2);

    // ── Popup overlay ────────────────────────────────────────
    if let Some(ref popup) = app.goal_popup {
        render_popup(popup, app.frame_count, area, buf);
    }
}

/// Centered popup rect within the given area.
fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect::new(x, y, width.min(area.width), height.min(area.height))
}

/// Render a goal popup overlay.
fn render_popup(popup: &GoalPopup, frame_count: u64, area: Rect, buf: &mut Buffer) {
    let cursor_char = if frame_count % 60 < 30 { "█" } else { " " };

    match popup {
        GoalPopup::Create {
            description,
            criteria,
            priority,
            focused_field,
        }
        | GoalPopup::Edit {
            description,
            criteria,
            priority,
            focused_field,
            ..
        } => {
            let is_edit = matches!(popup, GoalPopup::Edit { .. });
            let deadline_str = if let GoalPopup::Edit { deadline, .. } = popup {
                deadline.as_str()
            } else {
                ""
            };
            let h = if is_edit { 13u16 } else { 11 };
            let title = if is_edit {
                " Edit Goal "
            } else {
                " Create Goal "
            };

            let rect = centered_rect(55, h, area);
            Clear.render(rect, buf);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(theme::primary())
                .title(Line::from(Span::styled(title, theme::primary_bold())));
            let inner = block.inner(rect);
            block.render(rect, buf);

            let mut y = inner.y;

            // Description
            let desc_label_style = if *focused_field == 0 {
                theme::highlight()
            } else {
                theme::muted()
            };
            buf.set_line(
                inner.x + 1,
                y,
                &Line::from(Span::styled("Description:", desc_label_style)),
                inner.width - 2,
            );
            y += 1;
            let desc_cursor = if *focused_field == 0 { cursor_char } else { "" };
            let desc_line = Line::from(vec![
                Span::styled("> ", theme::primary()),
                Span::styled(description, theme::text_bold()),
                Span::styled(desc_cursor, theme::primary()),
            ]);
            buf.set_line(inner.x + 1, y, &desc_line, inner.width - 2);
            y += 2;

            // Criteria
            let crit_label_style = if *focused_field == 1 {
                theme::highlight()
            } else {
                theme::muted()
            };
            buf.set_line(
                inner.x + 1,
                y,
                &Line::from(Span::styled("Success Criteria:", crit_label_style)),
                inner.width - 2,
            );
            y += 1;
            let crit_cursor = if *focused_field == 1 { cursor_char } else { "" };
            let crit_line = Line::from(vec![
                Span::styled("> ", theme::primary()),
                Span::styled(criteria, theme::text_bold()),
                Span::styled(crit_cursor, theme::primary()),
            ]);
            buf.set_line(inner.x + 1, y, &crit_line, inner.width - 2);
            y += 2;

            // Priority
            let pri_label_style = if *focused_field == 2 {
                theme::highlight()
            } else {
                theme::muted()
            };
            let pri_name = PRIORITIES.get(*priority).unwrap_or(&"Medium");
            let pri_display = if *focused_field == 2 {
                format!("◄ {pri_name} ►")
            } else {
                pri_name.to_string()
            };
            let pri_line = Line::from(vec![
                Span::styled("Priority:     ", pri_label_style),
                Span::styled(pri_display, theme::primary_bold()),
            ]);
            buf.set_line(inner.x + 1, y, &pri_line, inner.width - 2);
            y += 1;

            // Deadline (edit only)
            if is_edit {
                y += 1;
                let dl_label_style = if *focused_field == 3 {
                    theme::highlight()
                } else {
                    theme::muted()
                };
                let dl_cursor = if *focused_field == 3 { cursor_char } else { "" };
                let dl_line = Line::from(vec![
                    Span::styled("Deadline:     ", dl_label_style),
                    Span::styled(deadline_str, theme::text()),
                    Span::styled(dl_cursor, theme::primary()),
                    Span::styled(" (YYYY-MM-DD)", theme::dim()),
                ]);
                buf.set_line(inner.x + 1, y, &dl_line, inner.width - 2);
            }
        }
        GoalPopup::Confirm { message, .. } => {
            let rect = centered_rect(50, 5, area);
            Clear.render(rect, buf);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(theme::primary())
                .title(Line::from(Span::styled(" Confirm ", theme::primary_bold())));
            let inner = block.inner(rect);
            block.render(rect, buf);

            let msg = Line::from(Span::styled(message, theme::text()));
            buf.set_line(inner.x + 1, inner.y, &msg, inner.width - 2);
            let hint = Line::from(vec![
                Span::styled("[y]", theme::primary_bold()),
                Span::styled(" Yes  ", theme::text()),
                Span::styled("[n]", theme::primary_bold()),
                Span::styled(" No", theme::text()),
            ]);
            buf.set_line(inner.x + 1, inner.y + 2, &hint, inner.width - 2);
        }
    }
}
