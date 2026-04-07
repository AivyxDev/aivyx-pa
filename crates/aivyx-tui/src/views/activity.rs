//! Activity view — real-time notification timeline with selection.

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, BorderType, Paragraph, Widget},
};

use crate::app::App;
use crate::theme;

const FILTERS: [&str; 4] = ["All", "Schedule", "Heartbeat", "Triage"];

pub fn render(app: &App, area: Rect, buf: &mut Buffer) {
    let [header, filter_row, body, help_bar] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Min(5),
        Constraint::Length(1),
    ]).areas(area);

    let count = app.notifications.len();
    let title = Paragraph::new(Line::from(vec![
        Span::styled("Activity", theme::text_bold()),
        Span::styled(format!("  {count} events from your assistant."), theme::dim()),
    ]));
    title.render(header, buf);

    // ── Filter tabs ──────────────────────────────────────────
    let mut filter_spans = Vec::new();
    for (i, f) in FILTERS.iter().enumerate() {
        let style = if i == app.activity_filter {
            Style::default().fg(theme::PRIMARY).add_modifier(Modifier::BOLD)
        } else {
            theme::dim()
        };
        filter_spans.push(Span::styled(format!(" {f} "), style));
        if i < FILTERS.len() - 1 {
            filter_spans.push(Span::styled("│", theme::dim()));
        }
    }
    buf.set_line(filter_row.x, filter_row.y, &Line::from(filter_spans), filter_row.width);

    let [list_area, detail_area] = Layout::horizontal([
        Constraint::Percentage(60),
        Constraint::Percentage(40),
    ]).areas(body);

    // ── Notification list ─────────────────────────────────────
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
                "schedule" | "briefing"       => theme::secondary(),
                "triage" | "email"            => theme::warning(),
                "goal"                        => theme::primary(),
                "mission"                     => theme::primary(),
                _ if notif.requires_approval  => theme::warning(),
                _                             => theme::muted(),
            };

            let marker = if is_selected { "[■]" } else { "[ ]" };
            let title_style = if is_selected { theme::primary_bold() } else { theme::text_bold() };
            let time = notif.timestamp.format("%H:%M:%S").to_string();
            let timeline = Line::from(vec![
                Span::styled(marker, if is_selected { theme::primary() } else { icon_style }),
                Span::styled("  ", Style::default()),
                Span::styled(&time, theme::dim()),
                Span::styled("  ", Style::default()),
                Span::styled(&notif.title, title_style),
            ]);
            buf.set_line(inner.x + 1, y, &timeline, inner.width - 2);
            y += 1;

            if y < inner.y + inner.height && !notif.body.is_empty() {
                let body_preview: String = notif.body.lines().next().unwrap_or("").chars().take((inner.width - 15) as usize).collect();
                let detail = Line::from(vec![
                    Span::styled("│  ", icon_style),  // connector adopts source color
                    Span::styled("         ", Style::default()),
                    Span::styled(body_preview, theme::muted()),
                ]);
                buf.set_line(inner.x + 1, y, &detail, inner.width - 2);
                y += 1;
            }
        }
    }

    // ── Detail panel for selected notification ────────────────
    let detail_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(theme::border())
        .title(Line::from(Span::styled("[ DETAIL ]", theme::primary_bold())));
    let detail_inner = detail_block.inner(detail_area);
    detail_block.render(detail_area, buf);

    let filtered_detail = app.filtered_notifications();
    if let Some(notif) = filtered_detail.get(app.activity_selected) {
        let mut y = detail_inner.y;

        // Source
        let source_line = Line::from(vec![
            Span::styled("Source:  ", theme::muted()),
            Span::styled(&notif.source, theme::secondary()),
        ]);
        buf.set_line(detail_inner.x + 1, y, &source_line, detail_inner.width - 2);
        y += 1;

        // Time
        let time_line = Line::from(vec![
            Span::styled("Time:    ", theme::muted()),
            Span::styled(notif.timestamp.format("%Y-%m-%d %H:%M:%S").to_string(), theme::dim()),
        ]);
        buf.set_line(detail_inner.x + 1, y, &time_line, detail_inner.width - 2);
        y += 1;

        // Approval status
        if notif.requires_approval {
            let approval = Line::from(vec![
                Span::styled("Approval: ", theme::muted()),
                Span::styled("Required", theme::warning()),
            ]);
            buf.set_line(detail_inner.x + 1, y, &approval, detail_inner.width - 2);
            y += 1;
        }
        y += 1;

        // Full body (word-wrapped)
        let max_w = (detail_inner.width - 4) as usize;
        for line in notif.body.lines() {
            if y >= detail_inner.y + detail_inner.height { break; }
            if line.len() <= max_w {
                buf.set_line(detail_inner.x + 1, y, &Line::from(Span::styled(line, theme::text())), detail_inner.width - 2);
                y += 1;
            } else {
                let mut pos = 0;
                while pos < line.len() && y < detail_inner.y + detail_inner.height {
                    let end = (pos + max_w).min(line.len());
                    buf.set_line(detail_inner.x + 1, y, &Line::from(Span::styled(&line[pos..end], theme::text())), detail_inner.width - 2);
                    y += 1;
                    pos = end;
                }
            }
        }
    } else {
        let empty = Line::from(Span::styled("  No event selected.", theme::dim()));
        buf.set_line(detail_inner.x + 1, detail_inner.y, &empty, detail_inner.width - 2);
    }

    // ── Help bar ─────────────────────────────────────────────
    let help = Line::from(vec![
        Span::styled("↑↓", theme::primary()),
        Span::styled(" navigate  ", theme::dim()),
        Span::styled("[]", theme::primary()),
        Span::styled(" filter  ", theme::dim()),
        Span::styled("Tab", theme::primary()),
        Span::styled(" sidebar", theme::dim()),
    ]);
    buf.set_line(help_bar.x + 1, help_bar.y, &help, help_bar.width - 2);
}
