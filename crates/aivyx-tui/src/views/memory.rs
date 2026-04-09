//! Memory view — real memory entries from the MemoryManager.

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Widget},
};

use crate::app::App;
use crate::theme;

pub fn render(app: &App, area: Rect, buf: &mut Buffer) {
    let [header, body, help_bar] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(5),
        Constraint::Length(1),
    ])
    .areas(area);

    let title = Line::from(vec![
        Span::styled("Memory", theme::text_bold()),
        Span::styled(
            format!("  {} memories stored.", app.memory_total),
            theme::dim(),
        ),
    ]);
    buf.set_line(header.x, header.y, &title, header.width);

    let [list_area, detail_area] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(body);

    // ── Memory list ───────────────────────────────────────────
    let list_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(theme::border())
        .title(Line::from(Span::styled(
            "[ MEMORY BANK ]",
            theme::primary_bold(),
        )));
    let list_inner = list_block.inner(list_area);
    list_block.render(list_area, buf);

    if app.memories.is_empty() {
        let empty = Line::from(Span::styled("  No memories yet.", theme::dim()));
        buf.set_line(list_inner.x + 1, list_inner.y, &empty, list_inner.width - 2);
    } else {
        // Each memory takes 2 lines (content preview + tags)
        let rows_per_item = 2u16;
        let visible_count = list_inner.height / rows_per_item;
        let scroll_offset = if app.memory_selected as u16 >= visible_count {
            app.memory_selected - (visible_count as usize - 1)
        } else {
            0
        };

        let mut y = list_inner.y;
        for (i, mem) in app.memories.iter().enumerate().skip(scroll_offset) {
            if y + 2 >= list_inner.y + list_inner.height {
                break;
            }

            let is_selected = i == app.memory_selected;
            let style = if is_selected {
                theme::primary_bold()
            } else {
                theme::text()
            };
            let marker = if is_selected { "[■] " } else { "[ ] " };

            // Content preview (first line, truncated)
            let preview: String = mem
                .content
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take((list_inner.width - 6) as usize)
                .collect();
            let line = Line::from(vec![
                Span::styled(marker, style),
                Span::styled(preview, style),
            ]);
            buf.set_line(list_inner.x + 1, y, &line, list_inner.width - 2);
            y += 1;

            // Tags in amber brackets matching goals/chat style
            if !mem.tags.is_empty() {
                let tags: Vec<Span> = mem
                    .tags
                    .iter()
                    .take(4)
                    .map(|t| Span::styled(format!("[{t}]"), Style::default().fg(theme::PRIMARY)))
                    .collect();
                buf.set_line(list_inner.x + 4, y, &Line::from(tags), list_inner.width - 5);
            }
            y += 1;
        }
    }

    // ── Detail panel ──────────────────────────────────────────
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

    if let Some(mem) = app.memories.get(app.memory_selected) {
        let mut y = detail_inner.y;

        // ID — abbreviated to first 8 chars to save space
        let id_short: String = mem.id.chars().take(8).collect();
        let id_line = Line::from(vec![
            Span::styled("ID:      ", theme::muted()),
            Span::styled(format!("{id_short}…"), theme::dim()),
        ]);
        buf.set_line(detail_inner.x + 1, y, &id_line, detail_inner.width - 2);
        y += 1;

        // Updated timestamp
        let time_line = Line::from(vec![
            Span::styled("Updated: ", theme::muted()),
            Span::styled(&mem.updated_at, theme::dim()),
        ]);
        buf.set_line(detail_inner.x + 1, y, &time_line, detail_inner.width - 2);
        y += 2;

        // Content (word-wrapped)
        let max_w = (detail_inner.width - 4) as usize;
        for line in mem.content.lines() {
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
                // Simple wrap
                let mut pos = 0;
                while pos < line.len() && y < detail_inner.y + detail_inner.height {
                    let end = (pos + max_w).min(line.len());
                    let chunk = &line[pos..end];
                    buf.set_line(
                        detail_inner.x + 1,
                        y,
                        &Line::from(Span::styled(chunk, theme::text())),
                        detail_inner.width - 2,
                    );
                    y += 1;
                    pos = end;
                }
            }
        }

        // Tags in amber brackets (matching list view)
        if y + 1 < detail_inner.y + detail_inner.height && !mem.tags.is_empty() {
            y += 1;
            let tags: Vec<Span> = mem
                .tags
                .iter()
                .map(|t| {
                    Span::styled(
                        format!(" [{t}] "),
                        Style::default()
                            .fg(theme::PRIMARY)
                            .add_modifier(Modifier::BOLD),
                    )
                })
                .collect();
            buf.set_line(
                detail_inner.x + 1,
                y,
                &Line::from(tags),
                detail_inner.width - 2,
            );
        }
    } else {
        let empty = Line::from(Span::styled(
            "  Select a memory to view details.",
            theme::dim(),
        ));
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
        Span::styled("Tab", theme::primary()),
        Span::styled(" sidebar", theme::dim()),
    ]);
    buf.set_line(help_bar.x, help_bar.y, &help, help_bar.width);
}
