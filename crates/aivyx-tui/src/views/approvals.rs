//! Approvals view — pending actions awaiting user confirmation.
//!
//! Renders real `ApprovalItem` structs from the PA's approval queue.
//! Press [V] on any item to expand its full body in a detail pane.

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Widget},
};

use crate::app::App;
use crate::theme;
use aivyx_pa::api::ApprovalStatus;

pub fn render(app: &App, area: Rect, buf: &mut Buffer) {
    let [header, body] = Layout::vertical([Constraint::Length(2), Constraint::Min(5)]).areas(area);

    let pending = app
        .approvals
        .iter()
        .filter(|a| a.status == ApprovalStatus::Pending)
        .count();
    let title = Line::from(vec![
        Span::styled("Approvals", theme::text_bold()),
        Span::styled(
            format!("  {pending} pending actions require your decision."),
            theme::dim(),
        ),
    ]);
    buf.set_line(header.x, header.y, &title, header.width);

    if app.approvals.is_empty() {
        let empty = Line::from(vec![
            Span::styled("  ◇  ", theme::dim()),
            Span::styled(
                "No pending approvals. The agent will request decisions here.",
                theme::dim(),
            ),
        ]);
        buf.set_line(body.x + 1, body.y + 1, &empty, body.width - 2);
        render_hint(app, body, buf);
        return;
    }

    // Split layout when detail pane is open
    let (list_area, detail_area) = if app.approval_detail_open && !app.approvals.is_empty() {
        let [top, bottom] =
            Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(body);
        (top, Some(bottom))
    } else {
        (body, None)
    };

    // ── Card list ─────────────────────────────────────────────────
    let mut y = list_area.y;
    for (i, item) in app.approvals.iter().enumerate() {
        let card_h: u16 = if i == app.approval_selected && item.status == ApprovalStatus::Pending {
            6
        } else {
            5
        };
        if y + card_h >= list_area.y + list_area.height {
            break;
        }

        let is_selected = i == app.approval_selected;
        let is_pending = item.status == ApprovalStatus::Pending;

        let border_style = if is_selected {
            theme::border_active()
        } else {
            theme::border()
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Plain)
            .border_style(border_style);
        let card = Rect::new(list_area.x, y, list_area.width, card_h);
        let inner = block.inner(card);
        block.render(card, buf);

        // Status badge
        let (status_label, status_style) = match item.status {
            ApprovalStatus::Pending => (
                "PENDING",
                Style::default()
                    .fg(theme::ACCENT_GLOW)
                    .add_modifier(Modifier::BOLD),
            ),
            ApprovalStatus::Approved => ("APPROVED", theme::sage()),
            ApprovalStatus::Denied => ("DENIED", theme::error()),
            ApprovalStatus::Expired => ("EXPIRED", theme::dim()),
        };

        // Row 0: title + status badge
        let title_line = Line::from(vec![
            Span::styled(
                if is_selected && is_pending {
                    "[■] "
                } else {
                    "[ ] "
                },
                theme::primary(),
            ),
            Span::styled(&item.notification.title, theme::text_bold()),
            Span::styled("  ", Style::default()),
            Span::styled(format!("[{status_label}]"), status_style),
        ]);
        buf.set_line(inner.x + 1, inner.y, &title_line, inner.width - 2);

        // Row 1: source + elapsed wait time for pending items
        let elapsed_str = if is_pending {
            if let Some(expires) = item.expires_at {
                let remaining = (expires - chrono::Utc::now()).num_seconds().max(0);
                if remaining < 60 {
                    format!("  expires in {remaining}s")
                } else {
                    format!("  expires in {}m{}s", remaining / 60, remaining % 60)
                }
            } else {
                let secs = (chrono::Utc::now() - item.notification.timestamp)
                    .num_seconds()
                    .max(0);
                if secs < 60 {
                    format!("  waiting {secs}s")
                } else {
                    format!("  waiting {}m{}s", secs / 60, secs % 60)
                }
            }
        } else {
            String::new()
        };
        let source_line = Line::from(vec![
            Span::styled("   Source: ", theme::muted()),
            Span::styled(&item.notification.source, theme::secondary()),
            Span::styled(
                elapsed_str,
                if is_pending {
                    theme::warning()
                } else {
                    theme::dim()
                },
            ),
        ]);
        buf.set_line(inner.x + 1, inner.y + 1, &source_line, inner.width - 2);

        // Row 2: action context preview (first line of body)
        let body_preview: String = item
            .notification
            .body
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take((inner.width.saturating_sub(6)) as usize)
            .collect();
        let detail_line = Line::from(vec![
            Span::styled("   ", Style::default()),
            Span::styled(body_preview, theme::text()),
            if item.notification.body.lines().count() > 1 {
                Span::styled("  [V] expand", theme::dim())
            } else {
                Span::styled("", Style::default())
            },
        ]);
        buf.set_line(inner.x + 1, inner.y + 2, &detail_line, inner.width - 2);

        // Row 3 (extra): inline approve/deny for selected pending card
        if is_selected && is_pending && inner.height > 3 {
            let action_line = Line::from(vec![
                Span::styled("   ", Style::default()),
                Span::styled("[A]", theme::sage()),
                Span::styled(" APPROVE", Style::default().fg(theme::SAGE)),
                Span::styled("    ", Style::default()),
                Span::styled("[D]", theme::error()),
                Span::styled(" DENY", Style::default().fg(theme::ERROR)),
                Span::styled("    ", Style::default()),
                Span::styled("[V]", theme::primary()),
                Span::styled(" DETAIL", theme::dim()),
            ]);
            buf.set_line(inner.x + 1, inner.y + 3, &action_line, inner.width - 2);
        }

        y += card_h;
    }

    // ── Detail pane ───────────────────────────────────────────────
    if let Some(detail) = detail_area
        && let Some(item) = app.approvals.get(app.approval_selected)
    {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Plain)
            .border_style(theme::border_active())
            .title(Line::from(vec![
                Span::styled(" Detail — ", theme::muted()),
                Span::styled(&item.notification.title, theme::text_bold()),
                Span::styled(" ", Style::default()),
            ]));
        let inner = block.inner(detail);
        block.render(detail, buf);

        // Word-wrap the body to the inner width
        let max_width = inner.width.saturating_sub(2) as usize;
        let lines: Vec<String> = item
            .notification
            .body
            .lines()
            .flat_map(|line| {
                if line.is_empty() {
                    return vec!["".to_string()];
                }
                // Simple word-wrap
                let mut wrapped = Vec::new();
                let mut current = String::new();
                for word in line.split_whitespace() {
                    if current.is_empty() {
                        current.push_str(word);
                    } else if current.len() + 1 + word.len() <= max_width {
                        current.push(' ');
                        current.push_str(word);
                    } else {
                        wrapped.push(current.clone());
                        current = word.to_string();
                    }
                }
                if !current.is_empty() {
                    wrapped.push(current);
                }
                wrapped
            })
            .collect();

        let total = lines.len();
        let visible = inner.height.saturating_sub(1) as usize;
        let scroll = (app.approval_detail_scroll as usize).min(total.saturating_sub(visible));

        for (row, line_text) in lines.iter().skip(scroll).take(visible).enumerate() {
            let rendered = Line::from(vec![
                Span::styled(" ", Style::default()),
                Span::styled(line_text.as_str(), theme::text()),
            ]);
            buf.set_line(inner.x, inner.y + row as u16, &rendered, inner.width);
        }

        // Scroll position hint
        if total > visible {
            let hint = Line::from(vec![Span::styled(
                format!(
                    " {}/{} lines  [↑↓] scroll",
                    scroll + visible.min(total),
                    total
                ),
                theme::dim(),
            )]);
            let hint_y = inner.y + inner.height.saturating_sub(1);
            buf.set_line(inner.x, hint_y, &hint, inner.width);
        }
    }

    render_hint(app, body, buf);
}

fn render_hint(app: &App, body: Rect, buf: &mut Buffer) {
    let hint_y = body.y + body.height.saturating_sub(1);
    let detail_label = if app.approval_detail_open {
        "CLOSE"
    } else {
        "DETAIL"
    };
    let hint = Line::from(vec![
        Span::styled("[A]", theme::sage()),
        Span::styled(" APPROVE  ", theme::dim()),
        Span::styled("[D]", theme::error()),
        Span::styled(" DENY  ", theme::dim()),
        Span::styled("[V]", theme::primary()),
        Span::styled(format!(" {detail_label}  "), theme::dim()),
        Span::styled("[↑↓]", theme::primary()),
        Span::styled(" NAVIGATE  ", theme::dim()),
        Span::styled("Tab", theme::primary()),
        Span::styled(" SIDEBAR", theme::dim()),
    ]);
    buf.set_line(body.x, hint_y, &hint, body.width);
}
