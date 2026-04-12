//! Missions view — filterable list with step detail panel.
//!
//! Shows active and historical missions from the task engine, with a
//! detail panel displaying step-by-step progress for the selected mission.

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Widget},
};

use crate::app::App;
use crate::theme;
use aivyx_task_engine::{StepStatus, TaskMetadata, TaskStatus};

const FILTERS: [&str; 4] = ["All", "Active", "Completed", "Failed"];

pub fn render(app: &App, area: Rect, buf: &mut Buffer) {
    let [header, filter_row, body, help_bar] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Min(5),
        Constraint::Length(1),
    ])
    .areas(area);

    // Title
    let total = app.missions.len();
    let title = Line::from(vec![
        Span::styled("[ TACTICAL MISSIONS ]", theme::text_bold()),
        Span::styled(format!("  [ TRACKING: {total} ]"), theme::dim()),
    ]);
    buf.set_line(header.x, header.y, &title, header.width);

    // Filter tabs
    let mut filter_spans = Vec::new();
    for (i, f) in FILTERS.iter().enumerate() {
        let style = if i == app.mission_filter {
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

    // Split: mission list + detail panel
    let [list_area, detail_area] =
        Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(body);

    // ── Mission list ──────────────────────────────────────────
    let missions = app.filtered_missions();

    let list_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(theme::border())
        .title(Line::from(Span::styled(
            format!("[ INDEX: {} MISSIONS ]", missions.len()),
            theme::dim(),
        )));
    let list_inner = list_block.inner(list_area);
    list_block.render(list_area, buf);

    if missions.is_empty() {
        let empty = [
            Line::from(Span::styled("  No missions yet.", theme::dim())),
            Line::from(Span::styled("", theme::dim())),
            Line::from(Span::styled(
                "  Ask your assistant to start one in Chat,",
                theme::muted(),
            )),
            Line::from(Span::styled(
                "  or it will create them autonomously.",
                theme::muted(),
            )),
        ];
        for (i, line) in empty.iter().enumerate() {
            let y = list_inner.y + 1 + i as u16;
            if y >= list_inner.y + list_inner.height {
                break;
            }
            buf.set_line(list_inner.x + 1, y, line, list_inner.width - 2);
        }
    } else {
        let card_height = 5u16;
        let visible_count = list_inner.height / card_height;
        let scroll_offset = if app.mission_selected as u16 >= visible_count {
            app.mission_selected - (visible_count as usize - 1)
        } else {
            0
        };

        let mut y = list_inner.y;
        for (i, meta) in missions.iter().enumerate().skip(scroll_offset) {
            if y + card_height > list_inner.y + list_inner.height {
                break;
            }

            let is_selected = i == app.mission_selected;
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Plain)
                .border_style(if is_selected {
                    theme::border_active()
                } else {
                    theme::border()
                });
            let card = Rect::new(list_inner.x, y, list_inner.width, card_height);
            let inner = block.inner(card);
            block.render(card, buf);

            // Row 0: goal title (truncated) with selection marker
            let marker = if is_selected { "▸ " } else { "  " };
            let max_goal_len = inner.width.saturating_sub(4) as usize;
            let goal = truncate(&meta.goal, max_goal_len);
            let goal_style = if is_selected {
                theme::text_bold()
            } else {
                theme::text()
            };
            let goal_line = Line::from(vec![
                Span::styled(
                    marker,
                    if is_selected {
                        theme::primary()
                    } else {
                        theme::dim()
                    },
                ),
                Span::styled(format!("[ TARGET: {} ]", goal), goal_style),
            ]);
            buf.set_line(inner.x, inner.y, &goal_line, inner.width);

            // Row 1: status + elapsed time or error snippet
            if inner.height > 1 {
                let row1 = match &meta.status {
                    TaskStatus::Failed { reason } => Line::from(vec![
                        Span::styled("  [ FAILED ]  ", Style::default().fg(theme::ERROR)),
                        Span::styled(
                            truncate(reason, inner.width.saturating_sub(14) as usize),
                            Style::default().fg(theme::ERROR),
                        ),
                    ]),
                    _ => {
                        let elapsed = format_elapsed(meta);
                        let step_str = format!(
                            "  [ METRICS: {}/{} ]",
                            meta.steps_completed, meta.steps_total
                        );
                        Line::from(vec![
                            Span::styled("  [ STATUS: ", theme::muted()),
                            format_status(&meta.status),
                            Span::styled(" ]", theme::muted()),
                            Span::styled(step_str, theme::dim()),
                            if elapsed.is_empty() {
                                Span::raw("")
                            } else {
                                Span::styled(format!("  [ ELAPSED: {} ]", elapsed), theme::dim())
                            },
                        ])
                    }
                };
                buf.set_line(inner.x, inner.y + 1, &row1, inner.width);
            }

            // Row 2: step-progress dot string
            if inner.height > 2 {
                let dots = format!("  {}", format_progress(meta));
                buf.set_line(
                    inner.x,
                    inner.y + 2,
                    &Line::from(Span::styled(dots, theme::dim())),
                    inner.width,
                );
            }

            y += card_height;
        }
    }

    // ── Detail panel ──────────────────────────────────────────
    let detail_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(theme::border())
        .title(Line::from(Span::styled("[ DETAILS ]", theme::dim())));
    let detail_inner = detail_block.inner(detail_area);
    detail_block.render(detail_area, buf);

    if let Some(ref mission) = app.mission_detail {
        render_detail(mission, detail_inner, buf);
    } else if !missions.is_empty() {
        let hint = Line::from(Span::styled(
            "  Select a mission to view details.",
            theme::dim(),
        ));
        buf.set_line(
            detail_inner.x + 1,
            detail_inner.y + 1,
            &hint,
            detail_inner.width - 2,
        );
    }

    // ── Help bar ──────────────────────────────────────────────
    let help_text = if app.mission_popup.is_some() {
        "[ ENTER: DISPATCH ]  [ ESC: CANCEL ]"
    } else {
        "[ \u{2191}\u{2193}: NAVIGATE ]  [ []: FILTER ]  [ N: NEW ]  [ R: RESUME ]  [ A/D: APPROVE/DENY ]  [ X: CANCEL ]  [ TAB: SIDEBAR ]"
    };
    let help = Line::from(Span::styled(help_text, theme::dim()));
    buf.set_line(help_bar.x + 1, help_bar.y, &help, help_bar.width - 2);

    // ── Popup overlay ────────────────────────────────────────
    if let Some(ref popup) = app.mission_popup {
        render_popup(popup, app.frame_count, area, buf);
    }
}

/// Render the mission detail panel.
fn render_detail(mission: &aivyx_task_engine::Mission, area: Rect, buf: &mut Buffer) {
    let max_w = area.width.saturating_sub(2) as usize;
    let mut y = area.y;

    // Goal
    let goal_label = Line::from(vec![
        Span::styled("[ CORE     ]  ", theme::muted()),
        Span::styled(
            truncate(&mission.goal, max_w.saturating_sub(14)),
            theme::text(),
        ),
    ]);
    buf.set_line(area.x + 1, y, &goal_label, area.width - 2);
    y += 1;

    // Agent + mode
    let mode = if mission.execution_mode() == aivyx_task_engine::ExecutionMode::Dag {
        "DAG"
    } else {
        "Sequential"
    };
    let agent_line = Line::from(vec![
        Span::styled("[ AGENT    ]  ", theme::muted()),
        Span::styled(&mission.agent_name, theme::text()),
        Span::styled("      [ MODE ]  ", theme::muted()),
        Span::styled(mode, theme::text()),
    ]);
    buf.set_line(area.x + 1, y, &agent_line, area.width - 2);
    y += 1;

    // Status + progress
    let completed = mission.steps_completed();
    let total = mission.steps.len();
    let status_line = Line::from(vec![
        Span::styled("[ STATUS   ]  ", theme::muted()),
        format_status(&mission.status),
        Span::styled(
            format!("      [ METRICS ]  (step {completed}/{total})"),
            theme::dim(),
        ),
    ]);
    buf.set_line(area.x + 1, y, &status_line, area.width - 2);
    y += 1;

    // Timestamps
    let created = mission.created_at.format("%Y-%m-%d %H:%M").to_string();
    let updated = format_age(mission.updated_at);
    let time_line = Line::from(vec![
        Span::styled("[ CREATED  ]  ", theme::muted()),
        Span::styled(created, theme::text()),
        Span::styled("   [ UPDATED ]  ", theme::muted()),
        Span::styled(updated, theme::text()),
    ]);
    buf.set_line(area.x + 1, y, &time_line, area.width - 2);
    y += 2; // blank line

    // Recipe name if present
    if let Some(ref recipe) = mission.recipe_name {
        let recipe_line = Line::from(vec![
            Span::styled("[ RECIPE   ]  ", theme::muted()),
            Span::styled(recipe, theme::text()),
        ]);
        buf.set_line(area.x + 1, y, &recipe_line, area.width - 2);
        y += 2;
    }

    // Steps header
    if y < area.y + area.height {
        let steps_hdr = Line::from(Span::styled("[ TIMELINE ]", theme::muted()));
        buf.set_line(area.x + 1, y, &steps_hdr, area.width - 2);
        y += 1;
    }

    // Step list
    for step in &mission.steps {
        if y >= area.y + area.height {
            break;
        }

        let mut icon_str = match &step.status {
            StepStatus::Completed => "[⚙ OK]".to_string(),
            StepStatus::Running => "[ACTIVE]".to_string(),
            StepStatus::Failed { .. } => "[FAILED]".to_string(),
            StepStatus::Pending => "[ WAIT ]".to_string(),
            StepStatus::Skipped => "[ SKIP ]".to_string(),
        };

        // Append retry counter if active/failed
        if step.retries > 0
            && matches!(step.status, StepStatus::Running | StepStatus::Failed { .. })
        {
            icon_str = format!(
                "{} (Attempt {})]",
                icon_str.strip_suffix(']').unwrap_or(&icon_str),
                step.retries + 1
            );
        }

        let icon_style = match &step.status {
            StepStatus::Completed => Style::default().fg(theme::SAGE),
            StepStatus::Running => Style::default().fg(theme::PRIMARY),
            StepStatus::Failed { .. } => Style::default().fg(theme::ERROR),
            StepStatus::Pending | StepStatus::Skipped => theme::dim(),
        };

        let desc = truncate(&step.description, max_w.saturating_sub(40));
        let duration = match (&step.started_at, &step.completed_at) {
            (Some(start), Some(end)) => {
                let secs = (*end - *start).num_seconds();
                if secs >= 60 {
                    format!("  [ {:.0}m{:02}s ]", secs / 60, secs % 60)
                } else {
                    format!("  [ {secs}s ]")
                }
            }
            (Some(_), None) if matches!(step.status, StepStatus::Running) => "  [ RUNNING ]".into(),
            _ => String::new(),
        };

        let mut step_spans = vec![
            Span::styled(format!("  {icon_str}  "), icon_style),
            Span::styled(format!("{}. {desc}", step.index + 1), theme::text()),
            Span::styled(duration, theme::dim()),
        ];

        // Indicate if the step passes through an acceptance contract gate
        if !step.acceptance.is_empty() {
            step_spans.push(Span::styled(
                format!("  [ CONTRACT CHECK: {} ]", step.acceptance.len()),
                theme::dim(),
            ));
        }

        let step_line = Line::from(step_spans);
        buf.set_line(area.x + 1, y, &step_line, area.width - 2);
        y += 1;

        // Show failure reason inline
        if let StepStatus::Failed { ref reason } = step.status
            && y < area.y + area.height
        {
            let reason_line = Line::from(Span::styled(
                format!("      {}", truncate(reason, max_w.saturating_sub(8))),
                Style::default().fg(theme::ERROR),
            ));
            buf.set_line(area.x + 1, y, &reason_line, area.width - 2);
            y += 1;
        }
    }
}

/// Format progress dots: ●●●▶─── with ▶ marking the current executing step.
fn format_progress(meta: &TaskMetadata) -> String {
    if meta.steps_total == 0 {
        return "no steps".into();
    }
    let max_display = 14usize;
    let total_display = meta.steps_total.min(max_display);
    let filled = meta.steps_completed.min(total_display);
    let is_active = matches!(
        meta.status,
        TaskStatus::Executing
            | TaskStatus::Verifying
            | TaskStatus::Planning
            | TaskStatus::Planned
            | TaskStatus::AwaitingApproval { .. }
    );

    let mut dots = String::new();
    for i in 0..total_display {
        if i < filled {
            dots.push('●');
        } else if i == filled && is_active && filled < total_display {
            dots.push('▶');
        } else {
            dots.push('─');
        }
    }
    if meta.steps_total > max_display {
        dots.push('…');
    }
    format!("{dots}  {}/{}", meta.steps_completed, meta.steps_total)
}

/// Format elapsed time for an active mission (empty string if terminal).
fn format_elapsed(meta: &TaskMetadata) -> String {
    if meta.status.is_terminal() {
        return String::new();
    }
    let elapsed = chrono::Utc::now() - meta.created_at;
    let secs = elapsed.num_seconds().max(0);
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    }
}

/// Format a TaskStatus as a styled span.
fn format_status(status: &TaskStatus) -> Span<'static> {
    match status {
        TaskStatus::Planning => {
            Span::styled("Planning...", Style::default().fg(theme::ACCENT_GLOW))
        }
        TaskStatus::Planned => Span::styled("Planned", Style::default().fg(theme::ACCENT_GLOW)),
        TaskStatus::Executing => Span::styled("Executing", Style::default().fg(theme::PRIMARY)),
        TaskStatus::Verifying => Span::styled("Verifying", Style::default().fg(theme::PRIMARY)),
        TaskStatus::AwaitingApproval { .. } => {
            Span::styled("⊕ Approval", Style::default().fg(theme::ACCENT_GLOW))
        }
        TaskStatus::Completed => Span::styled("✓ Completed", Style::default().fg(theme::SAGE)),
        TaskStatus::Failed { .. } => Span::styled("✗ Failed", Style::default().fg(theme::ERROR)),
        TaskStatus::Cancelled => Span::styled("Cancelled", theme::dim()),
    }
}

/// Format a timestamp as a human-readable age string.
fn format_age(ts: chrono::DateTime<chrono::Utc>) -> String {
    let now = chrono::Utc::now();
    let delta = now - ts;
    if delta.num_seconds() < 60 {
        "just now".into()
    } else if delta.num_minutes() < 60 {
        format!("{}m ago", delta.num_minutes())
    } else if delta.num_hours() < 24 {
        format!("{}h ago", delta.num_hours())
    } else {
        format!("{}d ago", delta.num_days())
    }
}

/// Truncate a string to `max` **characters** (not bytes) with an ellipsis.
///
/// Operates on `chars()` rather than byte indices so multi-byte UTF-8
/// sequences (emoji, accented characters, CJK) don't panic at a non-char
/// boundary when the slice falls mid-codepoint.
fn truncate(s: &str, max: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max {
        s.to_string()
    } else if max > 1 {
        let prefix: String = s.chars().take(max - 1).collect();
        format!("{prefix}…")
    } else {
        String::new()
    }
}

/// Centered popup rect within the given area.
fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect::new(x, y, width.min(area.width), height.min(area.height))
}

/// Render the mission creator popup overlay.
fn render_popup(popup: &crate::app::MissionPopup, frame_count: u64, area: Rect, buf: &mut Buffer) {
    let cursor_char = if frame_count % 60 < 30 { "█" } else { " " };

    let rect = centered_rect(60, 5, area);
    Clear.render(rect, buf);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(theme::primary())
        .title(Line::from(Span::styled(
            "[ CREATE MISSION ]",
            theme::primary_bold(),
        )));
    let inner = block.inner(rect);
    block.render(rect, buf);

    let y = inner.y + 1;

    let input_line = Line::from(vec![
        Span::styled("[ ", theme::primary()),
        Span::styled(&popup.input, theme::text_bold()),
        Span::styled(cursor_char, theme::primary()),
        Span::styled(" ]", theme::primary()),
    ]);
    buf.set_line(inner.x + 1, y, &input_line, inner.width - 2);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_progress_empty() {
        let meta = TaskMetadata {
            id: aivyx_core::TaskId::new(),
            goal: "test".into(),
            agent_name: "a".into(),
            status: TaskStatus::Planning,
            steps_completed: 0,
            steps_total: 0,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        assert_eq!(format_progress(&meta), "no steps");
    }

    #[test]
    fn format_progress_partial() {
        let meta = TaskMetadata {
            id: aivyx_core::TaskId::new(),
            goal: "test".into(),
            agent_name: "a".into(),
            status: TaskStatus::Executing,
            steps_completed: 3,
            steps_total: 5,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        // ●●● = completed, ▶ = current, ─ = pending
        assert_eq!(format_progress(&meta), "●●●▶─  3/5");
    }

    #[test]
    fn format_status_variants() {
        // Just verify they don't panic and produce non-empty text
        let statuses = [
            TaskStatus::Planning,
            TaskStatus::Planned,
            TaskStatus::Executing,
            TaskStatus::Verifying,
            TaskStatus::AwaitingApproval {
                step_index: 0,
                context: "test".into(),
            },
            TaskStatus::Completed,
            TaskStatus::Failed {
                reason: "oom".into(),
            },
            TaskStatus::Cancelled,
        ];
        for s in &statuses {
            let span = format_status(s);
            assert!(!span.content.is_empty(), "Empty status for {s:?}");
        }
    }

    #[test]
    fn format_age_recent() {
        let now = chrono::Utc::now();
        assert_eq!(format_age(now), "just now");
    }

    #[test]
    fn truncate_short() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_long() {
        assert_eq!(truncate("hello world", 6), "hello…");
    }

    #[test]
    fn truncate_multibyte_boundary() {
        // Emoji are 4-byte UTF-8 sequences; slicing at byte index 3 would
        // panic with "byte index 3 is not a char boundary". This test locks
        // in the chars()-based implementation.
        assert_eq!(truncate("🎉🎉🎉🎉", 3), "🎉🎉…");
    }

    #[test]
    fn truncate_accented_chars() {
        // "é" is 2 bytes in UTF-8 but 1 char — make sure we count chars.
        assert_eq!(truncate("café-bar", 5), "café…");
    }
}
