//! Chat view — conversational interface with streaming responses, scroll,
//! context usage display, session switching, and system prompt preview.

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, BorderType, Clear, Widget},
};

use crate::app::{App, ChatPopup};
use crate::theme;

/// A pre-rendered chat line ready to paint into the buffer.
struct ChatLine<'a> {
    spans: Vec<Span<'a>>,
    indent: u16,
}

pub fn render(app: &App, area: Rect, buf: &mut Buffer) {
    let [messages_area, context_bar, input_area, help_bar] = Layout::vertical([
        Constraint::Min(5),
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Length(1),
    ]).areas(area);

    // ── Messages ───────────────────────────────────────────────
    let msg_block = Block::default()
        .borders(Borders::BOTTOM)
        .border_type(BorderType::Rounded)
        .border_style(theme::border());
    let msg_inner = msg_block.inner(messages_area);
    msg_block.render(messages_area, buf);

    if app.chat_messages.is_empty() {
        let empty = Line::from(Span::styled(
            "  Start a conversation with your assistant...",
            theme::dim(),
        ));
        buf.set_line(msg_inner.x + 1, msg_inner.y + 1, &empty, msg_inner.width - 2);
    } else {
        // Pre-compute all visual lines
        let max_width = (msg_inner.width - 4) as usize;
        let mut all_lines: Vec<ChatLine> = Vec::new();

        for msg in &app.chat_messages {
            let (role_label, role_style) = if msg.role == "assistant" {
                ("◇ AI", theme::secondary())
            } else {
                ("● You", theme::primary())
            };
            let bubble_style = Style::default().fg(theme::ON_SURFACE);

            // Typing indicator + live tool status: streaming + empty assistant message
            if msg.role == "assistant" && msg.content.is_empty() && app.chat_streaming {
                all_lines.push(ChatLine {
                    spans: vec![
                        Span::styled(role_label, role_style),
                        Span::styled(format!("  {}", msg.timestamp), theme::dim()),
                    ],
                    indent: 1,
                });

                // Show compaction notice
                if app.chat_compacting {
                    let spinner = match (app.frame_count / 10) % 4 {
                        0 => "◐", 1 => "◓", 2 => "◑", _ => "◒",
                    };
                    all_lines.push(ChatLine {
                        spans: vec![Span::styled(
                            format!("{spinner} Compacting conversation..."),
                            theme::dim(),
                        )],
                        indent: 2,
                    });
                }

                // Show tool call status entries
                for entry in &app.chat_tool_status {
                    let line = if entry.denied {
                        let reason = entry.error.as_deref().unwrap_or("denied");
                        Span::styled(
                            format!("⊘ {} ({})", entry.tool_name, reason),
                            Style::default().fg(theme::ERROR),
                        )
                    } else if let Some(ref err) = entry.error {
                        let ms = entry.duration_ms.unwrap_or(0);
                        Span::styled(
                            format!("✗ {} failed: {} ({:.1}s)", entry.tool_name, err, ms as f64 / 1000.0),
                            Style::default().fg(theme::ERROR),
                        )
                    } else if let Some(ms) = entry.duration_ms {
                        Span::styled(
                            format!("✓ {} ({:.1}s)", entry.tool_name, ms as f64 / 1000.0),
                            Style::default().fg(theme::SAGE),
                        )
                    } else {
                        let spinner = match (app.frame_count / 10) % 4 {
                            0 => "◐", 1 => "◓", 2 => "◑", _ => "◒",
                        };
                        Span::styled(
                            format!("{spinner} Calling {}...", entry.tool_name),
                            theme::muted(),
                        )
                    };
                    all_lines.push(ChatLine { spans: vec![line], indent: 2 });
                }

                // Typing dots (show when no active tool call is running)
                let has_active_tool = app.chat_tool_status.iter().any(|e| e.duration_ms.is_none());
                if !has_active_tool && !app.chat_compacting {
                    let dot_phase = (app.frame_count / 15) % 3;
                    let dots = match dot_phase {
                        0 => "·",
                        1 => "· ·",
                        _ => "· · ·",
                    };
                    all_lines.push(ChatLine {
                        spans: vec![Span::styled(dots, role_style)],
                        indent: 2,
                    });
                }

                all_lines.push(ChatLine { spans: vec![], indent: 0 });
                continue;
            }

            // Role header line (with optional priority badge for user messages)
            let mut header_spans = vec![
                Span::styled(role_label, role_style),
                Span::styled(format!("  {}", msg.timestamp), theme::dim()),
            ];
            if msg.role == "user" {
                if msg.content.starts_with("[CRITICAL]") || msg.content.starts_with("[URGENT]") {
                    header_spans.push(Span::styled("  ▲ CRITICAL", Style::default().fg(theme::ERROR)));
                } else if msg.content.starts_with("[HIGH]") {
                    header_spans.push(Span::styled("  ▲ HIGH", Style::default().fg(theme::ACCENT_GLOW)));
                } else if msg.content.starts_with("[LOW]") {
                    header_spans.push(Span::styled("  ▽ LOW", theme::dim()));
                } else if msg.content.starts_with("[BG]") {
                    header_spans.push(Span::styled("  ◌ BG", theme::dim()));
                }
            }
            all_lines.push(ChatLine {
                spans: header_spans,
                indent: 1,
            });

            // Word-wrapped content lines — ✓/✗ lines get a colored ▌ left border
            for line in msg.content.lines() {
                let result_style: Option<Style> = if line.trim_start().starts_with('✓') {
                    Some(Style::default().fg(theme::SAGE))
                } else if line.trim_start().starts_with('✗') {
                    Some(Style::default().fg(theme::ERROR))
                } else {
                    None
                };

                let words: Vec<&str> = line.split_whitespace().collect();
                let mut current = String::new();
                for word in &words {
                    if current.len() + word.len() + 1 > max_width && !current.is_empty() {
                        let line_spans = build_content_spans(&current, bubble_style, result_style);
                        all_lines.push(ChatLine { spans: line_spans, indent: 2 });
                        current.clear();
                    }
                    if !current.is_empty() { current.push(' '); }
                    current.push_str(word);
                }
                if !current.is_empty() {
                    let line_spans = build_content_spans(&current, bubble_style, result_style);
                    all_lines.push(ChatLine { spans: line_spans, indent: 2 });
                }
            }

            // Gap between messages
            all_lines.push(ChatLine { spans: vec![], indent: 0 });
        }

        // Scroll: chat_scroll=0 means "show bottom", higher means scroll up
        let viewport_h = msg_inner.height as usize;
        let total = all_lines.len();
        let max_scroll = total.saturating_sub(viewport_h);
        let effective_scroll = app.chat_scroll.min(max_scroll);
        let start = total.saturating_sub(viewport_h + effective_scroll);

        let mut y = msg_inner.y;
        for line in all_lines.iter().skip(start).take(viewport_h) {
            if !line.spans.is_empty() {
                buf.set_line(
                    msg_inner.x + line.indent,
                    y,
                    &Line::from(line.spans.clone()),
                    msg_inner.width - line.indent - 1,
                );
            }
            y += 1;
        }

        // Scroll indicator
        if effective_scroll > 0 {
            let indicator = Line::from(Span::styled(
                format!("  ↑ {} more lines", effective_scroll),
                theme::dim(),
            ));
            buf.set_line(msg_inner.x, msg_inner.y, &indicator, msg_inner.width);
        }
    }

    // ── Context / token usage bar ──────────────────────────────
    let total_tokens = app.chat_input_tokens + app.chat_output_tokens;
    let session_label = if let Some(ref sid) = app.chat_session_id {
        format!("Session: {}…", &sid[..sid.len().min(8)])
    } else {
        "Unsaved".into()
    };
    let cost_str = if app.chat_cost_usd > 0.0 {
        format!("${:.4}", app.chat_cost_usd)
    } else {
        "$0".into()
    };
    let ctx_line = Line::from(vec![
        Span::styled(format!(" {session_label}"), theme::dim()),
        Span::styled("  │  ", theme::dim()),
        Span::styled(format!("{}↑ {}↓", format_tokens(app.chat_input_tokens), format_tokens(app.chat_output_tokens)), theme::muted()),
        Span::styled(format!("  ({} tok)", format_tokens(total_tokens)), theme::dim()),
        Span::styled(format!("  │  {cost_str}"), theme::dim()),
        Span::styled(format!("  │  {} msgs", app.chat_context_window), theme::dim()),
    ]);
    buf.set_line(context_bar.x, context_bar.y, &ctx_line, context_bar.width);

    // ── Input field ────────────────────────────────────────────
    let input_title = if app.chat_streaming {
        " Streaming... "
    } else {
        " Message (Esc to leave) "
    };
    let input_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(if app.chat_streaming { theme::border_active() } else { theme::border() })
        .title(Line::from(Span::styled(input_title, if app.chat_streaming { theme::primary() } else { theme::dim() })));
    let input_inner = input_block.inner(input_area);
    input_block.render(input_area, buf);

    let display = if app.chat_input.is_empty() {
        Span::styled("Type a message...", theme::dim())
    } else {
        Span::styled(&app.chat_input, theme::text())
    };
    buf.set_line(input_inner.x, input_inner.y, &Line::from(display), input_inner.width);

    // ── Help bar ───────────────────────────────────────────────
    let help_text = if let Some(ChatPopup::BranchManager { creating, .. }) = &app.chat_popup {
        if *creating {
            "Enter save  Esc cancel"
        } else {
            "n new snapshot  Enter branch  d delete  Esc close"
        }
    } else if app.chat_popup.is_some() {
        "Esc close  ↑↓ navigate  Enter select  d delete"
    } else {
        "^S sessions  ^P prompt  ^E export  ^B branches  ↑↓ scroll  Esc sidebar"
    };
    let help = Line::from(Span::styled(help_text, theme::dim()));
    buf.set_line(help_bar.x + 1, help_bar.y, &help, help_bar.width - 2);

    // ── Popup overlay ──────────────────────────────────────────
    if app.chat_popup.is_some() {
        render_popup(app, area, buf);
    }
}

/// Build spans for a single content line. If `result_style` is Some, renders
/// a colored `▌ ` left border + text in that style (for ✓/✗ tool results).
fn build_content_spans(text: &str, bubble_style: Style, result_style: Option<Style>) -> Vec<Span<'static>> {
    let text = text.to_owned();
    if let Some(rs) = result_style {
        vec![
            Span::styled("▌ ", rs),
            Span::styled(text, rs),
        ]
    } else {
        vec![Span::styled(text, bubble_style)]
    }
}

/// Format token count with K/M suffixes.
fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        format!("{n}")
    }
}

/// Render the chat popup overlay.
fn render_popup(app: &App, area: Rect, buf: &mut Buffer) {
    let Some(ref popup) = app.chat_popup else { return };

    let popup_area = centered_rect(area, 60, 70);
    Clear.render(popup_area, buf);

    match popup {
        ChatPopup::SessionList { sessions, selected } => {
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(theme::border_active())
                .title(Line::from(Span::styled(
                    format!(" Sessions ({}) ", sessions.len()),
                    theme::primary(),
                )));
            let inner = block.inner(popup_area);
            block.render(popup_area, buf);

            let mut y = inner.y;

            // "New conversation" option
            let new_style = if *selected == 0 { theme::primary_bold() } else { theme::text() };
            let marker = if *selected == 0 { "▸" } else { " " };
            let new_line = Line::from(vec![
                Span::styled(marker, if *selected == 0 { theme::primary() } else { theme::dim() }),
                Span::styled(" + New conversation", new_style),
            ]);
            buf.set_line(inner.x + 1, y, &new_line, inner.width - 2);
            y += 2;

            // Session entries
            for (i, entry) in sessions.iter().enumerate() {
                if y >= inner.y + inner.height { break; }
                let idx = i + 1;
                let is_sel = idx == *selected;
                let marker = if is_sel { "▸" } else { " " };
                let title_style = if is_sel { theme::primary_bold() } else { theme::text() };

                let title: String = if entry.title.len() > (inner.width as usize - 25) {
                    format!("{}…", &entry.title[..entry.title.len().min(inner.width as usize - 26)])
                } else {
                    entry.title.clone()
                };

                let line = Line::from(vec![
                    Span::styled(marker, if is_sel { theme::primary() } else { theme::dim() }),
                    Span::styled(format!(" {title}"), title_style),
                    Span::styled(format!("  {} turns", entry.turn_count), theme::dim()),
                ]);
                buf.set_line(inner.x + 1, y, &line, inner.width - 2);
                y += 1;

                // Subtitle: date
                let date_line = Line::from(vec![
                    Span::styled("   ", Style::default()),
                    Span::styled(&entry.updated_at, theme::dim()),
                ]);
                buf.set_line(inner.x + 1, y, &date_line, inner.width - 2);
                y += 1;
            }
        }

        ChatPopup::SystemPrompt { lines, scroll } => {
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(theme::border_active())
                .title(Line::from(Span::styled(" System Prompt Preview ", theme::primary())));
            let inner = block.inner(popup_area);
            block.render(popup_area, buf);

            let max_scroll = lines.len().saturating_sub(inner.height as usize);
            let eff_scroll = (*scroll).min(max_scroll);

            let mut y = inner.y;
            for line in lines.iter().skip(eff_scroll).take(inner.height as usize) {
                let display: String = line.chars().take((inner.width - 2) as usize).collect();
                buf.set_line(inner.x + 1, y, &Line::from(Span::styled(display, theme::text())), inner.width - 2);
                y += 1;
            }

            // Scroll indicator
            if eff_scroll > 0 || eff_scroll < max_scroll {
                let indicator = format!(" Line {}/{} ", eff_scroll + 1, lines.len());
                let ind_line = Line::from(Span::styled(indicator, theme::dim()));
                let ind_x = popup_area.x + popup_area.width.saturating_sub(20);
                buf.set_line(ind_x, popup_area.y, &ind_line, 20);
            }
        }

        ChatPopup::ExportDone { path } => {
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(theme::border_active())
                .title(Line::from(Span::styled(" Export Complete ", theme::sage())));
            let inner = block.inner(popup_area);
            block.render(popup_area, buf);

            let msg = Line::from(Span::styled("Chat exported to:", theme::text()));
            buf.set_line(inner.x + 2, inner.y + 1, &msg, inner.width - 4);

            // Word-wrap the path
            let max_w = (inner.width - 4) as usize;
            let mut y = inner.y + 3;
            let mut pos = 0;
            while pos < path.len() && y < inner.y + inner.height {
                let end = (pos + max_w).min(path.len());
                buf.set_line(inner.x + 2, y, &Line::from(Span::styled(&path[pos..end], theme::primary())), inner.width - 4);
                y += 1;
                pos = end;
            }

            let dismiss = Line::from(Span::styled("Press any key to close.", theme::dim()));
            buf.set_line(inner.x + 2, inner.y + inner.height.saturating_sub(2), &dismiss, inner.width - 4);
        }

        ChatPopup::BranchManager { snapshots, selected, label_input, creating } => {
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(theme::border_active())
                .title(Line::from(Span::styled(
                    format!(" Snapshots ({}) ", snapshots.len()),
                    theme::primary(),
                )));
            let inner = block.inner(popup_area);
            block.render(popup_area, buf);

            let mut y = inner.y;

            if *creating {
                // Label input mode
                let prompt = Line::from(Span::styled("Snapshot label (optional):", theme::text()));
                buf.set_line(inner.x + 2, y, &prompt, inner.width - 4);
                y += 2;

                let input_display = if label_input.is_empty() {
                    Span::styled("(press Enter for no label)", theme::dim())
                } else {
                    Span::styled(label_input.as_str(), theme::primary_bold())
                };
                let cursor = Span::styled("▎", theme::primary());
                buf.set_line(
                    inner.x + 2,
                    y,
                    &Line::from(vec![input_display, cursor]),
                    inner.width - 4,
                );
            } else {
                if snapshots.is_empty() {
                    let empty = Line::from(Span::styled(
                        "  No snapshots yet. Press n to create one.",
                        theme::dim(),
                    ));
                    buf.set_line(inner.x + 1, y, &empty, inner.width - 2);
                } else {
                    for (i, snap) in snapshots.iter().enumerate() {
                        if y >= inner.y + inner.height { break; }
                        let is_sel = i == *selected;
                        let marker = if is_sel { "▸" } else { " " };
                        let label = snap.label.as_deref().unwrap_or("(unlabeled)");
                        let title_style = if is_sel { theme::primary_bold() } else { theme::text() };

                        let line = Line::from(vec![
                            Span::styled(marker, if is_sel { theme::primary() } else { theme::dim() }),
                            Span::styled(
                                format!(" {label}  at message #{}", snap.message_index),
                                title_style,
                            ),
                        ]);
                        buf.set_line(inner.x + 1, y, &line, inner.width - 2);
                        y += 1;

                        let date_line = Line::from(vec![
                            Span::styled("   ", Style::default()),
                            Span::styled(&snap.created_at, theme::dim()),
                        ]);
                        buf.set_line(inner.x + 1, y, &date_line, inner.width - 2);
                        y += 1;
                    }
                }
            }
        }
    }
}

/// Centered rectangle helper.
fn centered_rect(area: Rect, percent_x: u16, percent_y: u16) -> Rect {
    let w = area.width * percent_x / 100;
    let h = area.height * percent_y / 100;
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_tokens_small() {
        assert_eq!(format_tokens(0), "0");
        assert_eq!(format_tokens(1), "1");
        assert_eq!(format_tokens(999), "999");
    }

    #[test]
    fn format_tokens_thousands() {
        assert_eq!(format_tokens(1_000), "1.0K");
        assert_eq!(format_tokens(1_500), "1.5K");
        assert_eq!(format_tokens(999_999), "1000.0K");
    }

    #[test]
    fn format_tokens_millions() {
        assert_eq!(format_tokens(1_000_000), "1.0M");
        assert_eq!(format_tokens(2_500_000), "2.5M");
    }

    #[test]
    fn centered_rect_basics() {
        let area = Rect { x: 0, y: 0, width: 100, height: 50 };
        let r = centered_rect(area, 60, 70);
        assert_eq!(r.width, 60);
        assert_eq!(r.height, 35);
        assert_eq!(r.x, 20); // (100 - 60) / 2
        assert_eq!(r.y, 7);  // (50 - 35) / 2
    }

    #[test]
    fn centered_rect_full_size() {
        let area = Rect { x: 10, y: 5, width: 80, height: 40 };
        let r = centered_rect(area, 100, 100);
        assert_eq!(r.x, area.x);
        assert_eq!(r.y, area.y);
        assert_eq!(r.width, area.width);
        assert_eq!(r.height, area.height);
    }
}
