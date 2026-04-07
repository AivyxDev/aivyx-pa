//! Approvals view — pending actions awaiting user confirmation.
//!
//! Renders real `ApprovalItem` structs from the PA's approval queue.

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, BorderType, Widget},
};

use aivyx_pa::api::ApprovalStatus;
use crate::app::App;
use crate::theme;

pub fn render(app: &App, area: Rect, buf: &mut Buffer) {
    let [header, body] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(5),
    ]).areas(area);

    let pending = app.approvals.iter()
        .filter(|a| a.status == ApprovalStatus::Pending)
        .count();
    let title = Line::from(vec![
        Span::styled("Approvals", theme::text_bold()),
        Span::styled(format!("  {pending} pending actions require your decision."), theme::dim()),
    ]);
    buf.set_line(header.x, header.y, &title, header.width);

    if app.approvals.is_empty() {
        let empty = Line::from(vec![
            Span::styled("  ◇  ", theme::dim()),
            Span::styled("No pending approvals. The agent will request decisions here.", theme::dim()),
        ]);
        buf.set_line(body.x + 1, body.y + 1, &empty, body.width - 2);
        return;
    }

    let mut y = body.y;
    for (i, item) in app.approvals.iter().enumerate() {
        let card_h: u16 = if i == app.approval_selected
            && item.status == ApprovalStatus::Pending { 6 } else { 5 };
        if y + card_h >= body.y + body.height {
            break;
        }

        let is_selected = i == app.approval_selected;
        let is_pending  = item.status == ApprovalStatus::Pending;

        let border_style = if is_selected && is_pending {
            // Pulsing amber border for the active pending approval
            theme::border_active()
        } else if is_selected {
            theme::border_active()
        } else {
            theme::border()
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Plain)
            .border_style(border_style);
        let card = Rect::new(body.x, y, body.width, card_h);
        let inner = block.inner(card);
        block.render(card, buf);

        // Status badge
        let (status_label, status_style) = match item.status {
            ApprovalStatus::Pending  => ("PENDING",  Style::default().fg(theme::ACCENT_GLOW).add_modifier(Modifier::BOLD)),
            ApprovalStatus::Approved => ("APPROVED", theme::sage()),
            ApprovalStatus::Denied   => ("DENIED",   theme::error()),
        };

        // Row 0: title + status badge
        let title_line = Line::from(vec![
            Span::styled(
                if is_selected && is_pending { "[■] " } else { "[ ] " },
                theme::primary(),
            ),
            Span::styled(&item.notification.title, theme::text_bold()),
            Span::styled("  ", Style::default()),
            Span::styled(format!("[{status_label}]"), status_style),
        ]);
        buf.set_line(inner.x + 1, inner.y, &title_line, inner.width - 2);

        // Row 1: source + elapsed wait time for pending items
        let elapsed_str = if is_pending {
            let secs = (chrono::Utc::now() - item.notification.timestamp).num_seconds().max(0);
            if secs < 60 {
                format!("  waiting {secs}s")
            } else {
                format!("  waiting {}m{}s", secs / 60, secs % 60)
            }
        } else {
            String::new()
        };
        let source_line = Line::from(vec![
            Span::styled("   Source: ", theme::muted()),
            Span::styled(&item.notification.source, theme::secondary()),
            Span::styled(elapsed_str, if is_pending { theme::warning() } else { theme::dim() }),
        ]);
        buf.set_line(inner.x + 1, inner.y + 1, &source_line, inner.width - 2);

        // Row 2: action context preview
        let body_preview: String = item.notification.body
            .lines().next().unwrap_or("")
            .chars().take((inner.width - 6) as usize).collect();
        let detail_line = Line::from(vec![
            Span::styled("   ", Style::default()),
            Span::styled(body_preview, theme::text()),
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
            ]);
            buf.set_line(inner.x + 1, inner.y + 3, &action_line, inner.width - 2);
        }

        y += card_h;
    }

    // ── Help bar ─────────────────────────────────────────────
    let hint_y = body.y + body.height.saturating_sub(1);
    let hint = Line::from(vec![
        Span::styled("[A]", theme::sage()),
        Span::styled(" APPROVE  ", theme::dim()),
        Span::styled("[D]", theme::error()),
        Span::styled(" DENY  ", theme::dim()),
        Span::styled("[↑↓]", theme::primary()),
        Span::styled(" NAVIGATE  ", theme::dim()),
        Span::styled("Tab", theme::primary()),
        Span::styled(" SIDEBAR", theme::dim()),
    ]);
    buf.set_line(body.x, hint_y, &hint, body.width);
}
