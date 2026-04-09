//! Audit view — real HMAC-chained event log with selection and scroll.

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Widget},
};

use crate::app::{self, App};
use crate::theme;

const FILTERS: [&str; 4] = ["All", "Tool", "Heartbeat", "Security"];

pub fn render(app: &App, area: Rect, buf: &mut Buffer) {
    let [header, filter_row, body, help_bar] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Min(5),
        Constraint::Length(1),
    ])
    .areas(area);

    let count = app.audit_entries.len();
    let title = Line::from(vec![
        Span::styled("Audit Trail", theme::text_bold()),
        Span::styled(
            format!("  {count} entries — HMAC-chained immutable log."),
            theme::dim(),
        ),
    ]);
    buf.set_line(header.x, header.y, &title, header.width);

    // Filter tabs
    let mut filter_spans = Vec::new();
    for (i, f) in FILTERS.iter().enumerate() {
        let style = if i == app.audit_filter {
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

    // Split: log list + chain status / detail sidebar
    let [log_area, side_area] =
        Layout::horizontal([Constraint::Percentage(65), Constraint::Percentage(35)]).areas(body);

    // ── Event log with selection + scroll ─────────────────────
    let log_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(theme::border())
        .title(Line::from(Span::styled(
            "[ SECURE EVENT LOG ]",
            theme::primary_bold(),
        )));
    let log_inner = log_block.inner(log_area);
    log_block.render(log_area, buf);

    let entries = app.filtered_audit();

    if entries.is_empty() {
        let empty = Line::from(Span::styled("  No audit entries yet.", theme::dim()));
        buf.set_line(log_inner.x + 1, log_inner.y, &empty, log_inner.width - 2);
    } else {
        // Each entry takes 2 lines (event + gap)
        let rows_per_item = 2u16;
        let visible_count = log_inner.height / rows_per_item;
        let scroll_offset = if app.audit_selected as u16 >= visible_count {
            app.audit_selected - (visible_count as usize - 1)
        } else {
            0
        };

        let mut y = log_inner.y;
        for (i, entry) in entries.iter().enumerate().skip(scroll_offset) {
            if y + 1 >= log_inner.y + log_inner.height {
                break;
            }

            let is_selected = i == app.audit_selected;
            let event_type = app::audit_event_type(&entry.event);
            let type_style = match event_type {
                "tool" => theme::secondary(),
                "heartbeat" => theme::sage(),
                "security" => theme::warning(),
                _ => theme::dim(),
            };

            let marker = if is_selected { "[■]" } else { "[ ]" };
            let time = entry.timestamp.get(11..19).unwrap_or(&entry.timestamp);
            let desc = app::format_audit_event(&entry.event);

            // Short type tag in brackets for rapid scanning
            let type_tag = match event_type {
                "tool" => Span::styled(" [TOOL] ", type_style),
                "heartbeat" => Span::styled(" [HB] ", type_style),
                "security" => Span::styled(" [SEC] ", type_style),
                _ => Span::styled(" [EVENT] ", type_style),
            };

            let line1 = Line::from(vec![
                Span::styled(
                    marker,
                    if is_selected {
                        theme::primary()
                    } else {
                        theme::dim()
                    },
                ),
                Span::styled(time, theme::dim()),
                Span::styled(format!("  #{}", entry.sequence_number), theme::muted()),
                type_tag,
                Span::styled(
                    desc,
                    if is_selected {
                        theme::primary_bold()
                    } else {
                        theme::text()
                    },
                ),
            ]);
            buf.set_line(log_inner.x + 1, y, &line1, log_inner.width - 2);
            y += 2;
        }
    }

    // ── Sidebar: chain status + selected entry detail ─────────
    let [chain_area, detail_area] =
        Layout::vertical([Constraint::Length(7), Constraint::Min(3)]).areas(side_area);

    // Chain status
    let chain_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(theme::border())
        .title(Line::from(Span::styled(
            "[ HMAC CHAIN STATUS ]",
            theme::primary_bold(),
        )));
    let chain_inner = chain_block.inner(chain_area);
    chain_block.render(chain_area, buf);

    let (status_text, status_style, status_detail) = match app.audit_chain_valid {
        Some(true) => (
            "✓ CHAIN VERIFIED",
            theme::sage(),
            "HMAC-SHA256 • all seals intact",
        ),
        Some(false) => (
            "✗ CHAIN BROKEN",
            theme::error(),
            "⚠  one or more seals invalid",
        ),
        None => ("◇ CHECKING...", theme::dim(), "verifying HMAC chain..."),
    };
    // Full-width status line for maximum visibility
    buf.set_line(
        chain_inner.x + 1,
        chain_inner.y,
        &Line::from(Span::styled(status_text, status_style)),
        chain_inner.width - 2,
    );
    buf.set_line(
        chain_inner.x + 1,
        chain_inner.y + 1,
        &Line::from(Span::styled(status_detail, theme::dim())),
        chain_inner.width - 2,
    );

    let algo = Line::from(vec![
        Span::styled("Algorithm   ", theme::dim()),
        Span::styled("HMAC-SHA256", theme::text()),
    ]);
    buf.set_line(
        chain_inner.x + 1,
        chain_inner.y + 3,
        &algo,
        chain_inner.width - 2,
    );

    let entries_count = Line::from(vec![
        Span::styled("Entries     ", theme::dim()),
        Span::styled(app.audit_entries.len().to_string(), theme::text()),
    ]);
    buf.set_line(
        chain_inner.x + 1,
        chain_inner.y + 4,
        &entries_count,
        chain_inner.width - 2,
    );

    // Selected entry detail
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

    let filtered = app.filtered_audit();
    if let Some(entry) = filtered.get(app.audit_selected) {
        let mut y = detail_inner.y;

        let seq_line = Line::from(vec![
            Span::styled("Seq:   ", theme::muted()),
            Span::styled(format!("#{}", entry.sequence_number), theme::text()),
        ]);
        buf.set_line(detail_inner.x + 1, y, &seq_line, detail_inner.width - 2);
        y += 1;

        let time_line = Line::from(vec![
            Span::styled("Time:  ", theme::muted()),
            Span::styled(&entry.timestamp, theme::dim()),
        ]);
        buf.set_line(detail_inner.x + 1, y, &time_line, detail_inner.width - 2);
        y += 1;

        let type_line = Line::from(vec![
            Span::styled("Type:  ", theme::muted()),
            Span::styled(app::audit_event_type(&entry.event), theme::secondary()),
        ]);
        buf.set_line(detail_inner.x + 1, y, &type_line, detail_inner.width - 2);
        y += 2;

        // Full event description (wrapped)
        let desc = app::format_audit_event(&entry.event);
        let max_w = (detail_inner.width - 4) as usize;
        let mut pos = 0;
        while pos < desc.len() && y < detail_inner.y + detail_inner.height {
            let end = (pos + max_w).min(desc.len());
            buf.set_line(
                detail_inner.x + 1,
                y,
                &Line::from(Span::styled(&desc[pos..end], theme::text())),
                detail_inner.width - 2,
            );
            y += 1;
            pos = end;
        }
    } else {
        let empty = Line::from(Span::styled("  Select an entry.", theme::dim()));
        buf.set_line(
            detail_inner.x + 1,
            detail_inner.y,
            &empty,
            detail_inner.width - 2,
        );
    }

    // ── Help bar ──────────────────────────────────────────────
    let help = Line::from(vec![
        Span::styled("↑↓", theme::primary()),
        Span::styled(" navigate  ", theme::dim()),
        Span::styled("[]", theme::primary()),
        Span::styled(" filter  ", theme::dim()),
        Span::styled("Tab", theme::primary()),
        Span::styled(" sidebar", theme::dim()),
    ]);
    buf.set_line(help_bar.x, help_bar.y, &help, help_bar.width);
}
