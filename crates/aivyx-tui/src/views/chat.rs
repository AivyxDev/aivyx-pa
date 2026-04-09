//! Chat view — conversational interface with streaming responses, scroll,
//! context usage display, session switching, and system prompt preview.
//!
//! Design: Stitch operative aesthetic with role-differentiated gutter bars,
//! Markdown-lite inline formatting, and refined streaming indicators.

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Widget},
};

use crate::app::{App, ChatPopup, HistoryMode};
use crate::theme;

/// A pre-rendered chat line ready to paint into the buffer.
struct ChatLine<'a> {
    spans: Vec<Span<'a>>,
    indent: u16,
}

/// Gutter style for a message role — defines the accent bar color.
#[derive(Clone, Copy)]
struct GutterStyle {
    bar_style: Style,
}

impl GutterStyle {
    fn user() -> Self {
        Self {
            bar_style: Style::default().fg(theme::PRIMARY),
        }
    }
    fn assistant() -> Self {
        Self {
            bar_style: Style::default().fg(theme::SECONDARY),
        }
    }
}

pub fn render(app: &App, area: Rect, buf: &mut Buffer) {
    // Calculate input height: border (2) + wrapped lines (at least 1, capped at 6)
    let input_content_width = area.width.saturating_sub(4) as usize; // 2 border + 2 padding
    let input_char_count = app.chat_input.chars().count();
    let input_lines = if input_content_width == 0 || input_char_count == 0 {
        1
    } else {
        ((input_char_count + input_content_width - 1) / input_content_width).clamp(1, 6)
    };
    let input_height = (input_lines as u16) + 2; // +2 for top/bottom border

    let [messages_area, context_bar, input_area, help_bar] = Layout::vertical([
        Constraint::Min(5),
        Constraint::Length(1),
        Constraint::Length(input_height),
        Constraint::Length(1),
    ])
    .areas(area);

    // ── Messages ───────────────────────────────────────────────
    let msg_block = Block::default()
        .borders(Borders::BOTTOM)
        .border_type(BorderType::Rounded)
        .border_style(theme::border());
    let msg_inner = msg_block.inner(messages_area);
    msg_block.render(messages_area, buf);

    if app.chat_messages.is_empty() {
        // Empty state — centered prompt
        let empty_y = msg_inner.y + msg_inner.height / 2;
        let empty = Line::from(vec![
            Span::styled("  ◇ ", theme::secondary()),
            Span::styled("Start a conversation with your assistant...", theme::dim()),
        ]);
        buf.set_line(msg_inner.x + 1, empty_y, &empty, msg_inner.width - 2);
    } else {
        // Pre-compute all visual lines
        let max_width = (msg_inner.width.saturating_sub(6)) as usize; // gutter (3) + padding
        let mut all_lines: Vec<ChatLine> = Vec::new();
        let msg_count = app.chat_messages.len();

        for (msg_idx, msg) in app.chat_messages.iter().enumerate() {
            let is_assistant = msg.role == "assistant";
            let gutter = if is_assistant {
                GutterStyle::assistant()
            } else {
                GutterStyle::user()
            };
            let (role_label, role_style) = if is_assistant {
                (
                    "◇ AIVYX",
                    Style::default()
                        .fg(theme::SECONDARY)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                (
                    "● YOU",
                    Style::default()
                        .fg(theme::PRIMARY)
                        .add_modifier(Modifier::BOLD),
                )
            };
            let bubble_style = Style::default().fg(theme::ON_SURFACE);

            // ── Streaming indicator (empty assistant message while streaming) ──
            if is_assistant && msg.content.is_empty() && app.chat_streaming {
                // Header
                all_lines.push(ChatLine {
                    spans: vec![
                        Span::styled("▎ ", gutter.bar_style),
                        Span::styled(role_label, role_style),
                        Span::styled(format!("  {}", msg.timestamp), theme::dim()),
                    ],
                    indent: 1,
                });

                // Compaction notice
                if app.chat_compacting {
                    let spinner = match (app.frame_count / 10) % 4 {
                        0 => "◐",
                        1 => "◓",
                        2 => "◑",
                        _ => "◒",
                    };
                    all_lines.push(ChatLine {
                        spans: vec![
                            Span::styled("▎ ", gutter.bar_style),
                            Span::styled(
                                format!("{spinner} Compacting memory..."),
                                theme::secondary(),
                            ),
                        ],
                        indent: 1,
                    });
                }

                // Tool call status entries
                for entry in &app.chat_tool_status {
                    let tool_span = if entry.denied {
                        let reason = entry.error.as_deref().unwrap_or("denied");
                        Span::styled(
                            format!("⊘ {} — {}", entry.tool_name, reason),
                            Style::default().fg(theme::ERROR),
                        )
                    } else if let Some(ref err) = entry.error {
                        let ms = entry.duration_ms.unwrap_or(0);
                        Span::styled(
                            format!(
                                "✗ {} — {} ({:.1}s)",
                                entry.tool_name,
                                err,
                                ms as f64 / 1000.0
                            ),
                            Style::default().fg(theme::ERROR),
                        )
                    } else if let Some(ms) = entry.duration_ms {
                        Span::styled(
                            format!("✓ {} ({:.1}s)", entry.tool_name, ms as f64 / 1000.0),
                            Style::default().fg(theme::SAGE),
                        )
                    } else {
                        let spinner = match (app.frame_count / 10) % 4 {
                            0 => "◐",
                            1 => "◓",
                            2 => "◑",
                            _ => "◒",
                        };
                        Span::styled(
                            format!("{spinner} {}...", entry.tool_name),
                            Style::default().fg(theme::ACCENT_GLOW),
                        )
                    };
                    all_lines.push(ChatLine {
                        spans: vec![
                            Span::styled("▎ ", gutter.bar_style),
                            Span::styled("  ", Style::default()),
                            tool_span,
                        ],
                        indent: 1,
                    });
                }

                // Pulsing block cursor (no active tool, not compacting)
                let has_active_tool = app
                    .chat_tool_status
                    .iter()
                    .any(|e| e.duration_ms.is_none() && !e.denied);
                if !has_active_tool && !app.chat_compacting {
                    let pulse_phase = (app.frame_count / 20) % 3;
                    let cursor_char = match pulse_phase {
                        0 => "█",
                        1 => "▓",
                        _ => "▒",
                    };
                    all_lines.push(ChatLine {
                        spans: vec![
                            Span::styled("▎ ", gutter.bar_style),
                            Span::styled(
                                format!("  {cursor_char}"),
                                Style::default().fg(theme::SECONDARY),
                            ),
                        ],
                        indent: 1,
                    });
                }

                // Separator after streaming block
                push_separator(&mut all_lines, max_width);
                continue;
            }

            // ── Role header ───────────────────────────────────
            let mut header_spans = vec![
                Span::styled("▎ ", gutter.bar_style),
                Span::styled(role_label, role_style),
                Span::styled(format!("  {}", msg.timestamp), theme::dim()),
            ];
            if msg.role == "user" {
                if msg.content.starts_with("[CRITICAL]") || msg.content.starts_with("[URGENT]") {
                    header_spans.push(Span::styled(
                        "  ▲ CRITICAL",
                        Style::default().fg(theme::ERROR),
                    ));
                } else if msg.content.starts_with("[HIGH]") {
                    header_spans.push(Span::styled(
                        "  ▲ HIGH",
                        Style::default().fg(theme::ACCENT_GLOW),
                    ));
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

            // ── Content lines with gutter + Markdown-lite ─────
            let mut in_tool_block = false;
            for line in msg.content.lines() {
                let trimmed = line.trim_start();

                // Hide raw system boundaries to prevent UI bleed
                if trimmed.starts_with("[TOOL_OUTPUT]") || trimmed.starts_with("[SYSTEM INFO]") {
                    continue;
                }

                // Collapse raw JSON tool blocks entirely into a clean UI indicator
                if trimmed.starts_with("```json") || trimmed.starts_with("`json") {
                    in_tool_block = true;
                    all_lines.push(ChatLine {
                        spans: vec![
                            Span::styled("▎ ", gutter.bar_style),
                            Span::styled(
                                "  [⚙ System execution context generated]",
                                Style::default()
                                    .fg(theme::SURFACE_HIGHEST)
                                    .add_modifier(Modifier::ITALIC),
                            ),
                        ],
                        indent: 1,
                    });
                    continue;
                }
                if in_tool_block {
                    if trimmed.starts_with("```") || trimmed == "`" {
                        in_tool_block = false;
                    }
                    continue;
                }

                // Tool result lines: ✓/✗ get colored gutter
                let result_style: Option<Style> = if trimmed.starts_with('✓') {
                    Some(Style::default().fg(theme::SAGE))
                } else if trimmed.starts_with('✗') {
                    Some(Style::default().fg(theme::ERROR))
                } else {
                    None
                };

                // Markdown heading: # line → bold primary
                if trimmed.starts_with("# ") {
                    let heading_text = trimmed.trim_start_matches('#').trim();
                    all_lines.push(ChatLine {
                        spans: vec![
                            Span::styled("▎ ", gutter.bar_style),
                            Span::styled(
                                heading_text.to_uppercase(),
                                Style::default()
                                    .fg(theme::PRIMARY)
                                    .add_modifier(Modifier::BOLD),
                            ),
                        ],
                        indent: 1,
                    });
                    continue;
                }

                // Markdown sub-heading: ## / ### → bold text
                if trimmed.starts_with("## ") {
                    let heading_text = trimmed.trim_start_matches('#').trim();
                    all_lines.push(ChatLine {
                        spans: vec![
                            Span::styled("▎ ", gutter.bar_style),
                            Span::styled(
                                heading_text.to_owned(),
                                Style::default()
                                    .fg(theme::ON_SURFACE)
                                    .add_modifier(Modifier::BOLD),
                            ),
                        ],
                        indent: 1,
                    });
                    continue;
                }

                // Markdown blockquote: > text → sage gutter + italic
                if trimmed.starts_with("> ") {
                    let quote_text = trimmed.trim_start_matches('>').trim();
                    all_lines.push(ChatLine {
                        spans: vec![
                            Span::styled("▎ ", Style::default().fg(theme::SAGE)),
                            Span::styled(
                                quote_text.to_owned(),
                                Style::default()
                                    .fg(theme::ON_SURFACE_DIM)
                                    .add_modifier(Modifier::ITALIC),
                            ),
                        ],
                        indent: 1,
                    });
                    continue;
                }

                // Markdown list item: - or * prefix → bullet glyph
                let (is_list, list_text) = if (trimmed.starts_with("- ")
                    || trimmed.starts_with("* "))
                    && trimmed.len() > 2
                {
                    (true, &trimmed[2..])
                } else {
                    (false, line)
                };

                // Word-wrap content
                let effective_text = if is_list { list_text } else { line };
                let words: Vec<&str> = effective_text.split_whitespace().collect();
                let mut current = String::new();
                let mut is_first_wrap = true;

                for word in &words {
                    if current.len() + word.len() + 1 > max_width && !current.is_empty() {
                        let spans = build_guttered_spans(
                            &current,
                            gutter,
                            bubble_style,
                            result_style,
                            is_list && is_first_wrap,
                        );
                        all_lines.push(ChatLine { spans, indent: 1 });
                        current.clear();
                        is_first_wrap = false;
                    }
                    if !current.is_empty() {
                        current.push(' ');
                    }
                    current.push_str(word);
                }
                if !current.is_empty() {
                    let spans = build_guttered_spans(
                        &current,
                        gutter,
                        bubble_style,
                        result_style,
                        is_list && is_first_wrap,
                    );
                    all_lines.push(ChatLine { spans, indent: 1 });
                }
            }

            // Separator between messages (thin rule, not blank line)
            if msg_idx < msg_count - 1 {
                push_separator(&mut all_lines, max_width);
            } else {
                // Final message: small gap
                all_lines.push(ChatLine {
                    spans: vec![],
                    indent: 0,
                });
            }
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
            let indicator = Line::from(vec![
                Span::styled("  ↑ ", Style::default().fg(theme::PRIMARY)),
                Span::styled(format!("{} more lines", effective_scroll), theme::dim()),
            ]);
            buf.set_line(msg_inner.x, msg_inner.y, &indicator, msg_inner.width);
        }
    }

    // ── Context / token usage bar ──────────────────────────────
    render_context_bar(app, context_bar, buf);

    // ── Input field ────────────────────────────────────────────
    render_input_field(app, input_area, buf);

    // ── Help bar ───────────────────────────────────────────────
    render_help_bar(app, help_bar, buf);

    // ── Popup overlay ──────────────────────────────────────────
    if app.chat_popup.is_some() {
        render_popup(app, area, buf);
    }
}

// ── Context bar ────────────────────────────────────────────────

fn render_context_bar(app: &App, area: Rect, buf: &mut Buffer) {
    let total_tokens = app.chat_input_tokens + app.chat_output_tokens;

    // Session indicator: ● saved, ○ unsaved
    let (session_icon, session_label) = if let Some(ref sid) = app.chat_session_id {
        ("●", format!("{}…", &sid[..sid.len().min(8)]))
    } else {
        ("○", "unsaved".into())
    };

    let cost_str = if app.chat_cost_usd > 0.0 {
        format!("${:.4}", app.chat_cost_usd)
    } else {
        "$0".into()
    };

    let mut spans = vec![
        Span::styled(
            format!(" {session_icon} "),
            if app.chat_session_id.is_some() {
                theme::sage()
            } else {
                theme::dim()
            },
        ),
        Span::styled(session_label, theme::dim()),
        Span::styled("  ▸  ", Style::default().fg(theme::SURFACE_HIGHEST)),
        Span::styled(
            format!("{}↑", format_tokens(app.chat_input_tokens)),
            theme::primary(),
        ),
        Span::styled(" ", Style::default()),
        Span::styled(
            format!("{}↓", format_tokens(app.chat_output_tokens)),
            theme::secondary(),
        ),
        Span::styled(
            format!("  ({} tok)", format_tokens(total_tokens)),
            theme::dim(),
        ),
        Span::styled("  ▸  ", Style::default().fg(theme::SURFACE_HIGHEST)),
        Span::styled(format!("{} msgs", app.chat_context_window), theme::muted()),
        Span::styled("  ▸  ", Style::default().fg(theme::SURFACE_HIGHEST)),
        Span::styled(cost_str, theme::dim()),
    ];

    // Mini token gauge (if terminal is wide enough)
    if area.width > 85 && total_tokens > 0 {
        let gauge_w = 12usize;
        let input_ratio = app.chat_input_tokens as f64 / total_tokens as f64;
        let input_bars = (input_ratio * gauge_w as f64).round() as usize;
        let output_bars = gauge_w.saturating_sub(input_bars);
        spans.push(Span::styled("  ", Style::default()));
        spans.push(Span::styled("▮".repeat(input_bars), theme::primary()));
        spans.push(Span::styled("▮".repeat(output_bars), theme::secondary()));
    }

    buf.set_line(area.x, area.y, &Line::from(spans), area.width);
}

// ── Input field ────────────────────────────────────────────────

fn render_input_field(app: &App, area: Rect, buf: &mut Buffer) {
    let input_title = if app.voice_recording {
        " 🔴 RECORDING — Ctrl+R to send "
    } else if app.voice_transcribing {
        " ⚙ TRANSCRIBING "
    } else if app.chat_streaming {
        " ◐ Streaming... "
    } else {
        " Message "
    };

    let border_style = if app.voice_recording {
        Style::default().fg(theme::ERROR)
    } else if app.voice_transcribing || app.chat_streaming {
        theme::border_active()
    } else {
        theme::border()
    };
    let text_style = if app.voice_recording {
        Style::default().fg(theme::ERROR)
    } else if app.voice_transcribing || app.chat_streaming {
        theme::primary()
    } else {
        theme::dim()
    };

    let input_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title(Line::from(Span::styled(input_title, text_style)));
    let input_inner = input_block.inner(area);
    input_block.render(area, buf);

    if app.voice_recording {
        let display = Span::styled("Listening... speak now.", Style::default().fg(theme::ERROR));
        buf.set_line(
            input_inner.x,
            input_inner.y,
            &Line::from(display),
            input_inner.width,
        );
    } else if app.voice_transcribing {
        let display = Span::styled("Transcribing audio...", theme::primary());
        buf.set_line(
            input_inner.x,
            input_inner.y,
            &Line::from(display),
            input_inner.width,
        );
    } else if app.chat_input.is_empty() {
        let display = Span::styled("Type a message or Ctrl+R to talk...", theme::dim());
        buf.set_line(
            input_inner.x,
            input_inner.y,
            &Line::from(display),
            input_inner.width,
        );
    } else {
        // Wrap input text across available lines using char boundaries
        let w = input_inner.width as usize;
        let chars: Vec<char> = app.chat_input.chars().collect();
        let mut y = input_inner.y;
        let mut pos = 0;
        while pos < chars.len() && y < input_inner.y + input_inner.height {
            let end = chars.len().min(pos + w);
            let chunk: String = chars[pos..end].iter().collect();
            buf.set_line(
                input_inner.x,
                y,
                &Line::from(Span::styled(chunk, theme::text())),
                input_inner.width,
            );
            pos = end;
            y += 1;
        }
    }

    // Character count in bottom-right corner of input border
    if !app.chat_input.is_empty() && !app.chat_streaming {
        let count_str = format!(" {} chars ", app.chat_input.chars().count());
        let count_len = count_str.len() as u16;
        let count_x = area.x + area.width.saturating_sub(count_len + 2);
        let count_y = area.y + area.height - 1; // bottom border line
        buf.set_line(
            count_x,
            count_y,
            &Line::from(Span::styled(count_str, theme::dim())),
            count_len + 2,
        );
    }
}

// ── Help bar ───────────────────────────────────────────────────

fn render_help_bar(app: &App, area: Rect, buf: &mut Buffer) {
    let help_text = match &app.chat_popup {
        Some(ChatPopup::BranchManager { creating, .. }) => {
            if *creating {
                "Enter save  Esc cancel"
            } else {
                "n new snapshot  Enter branch  d delete  Esc close"
            }
        }
        Some(ChatPopup::ConversationHistory { mode, .. }) => match mode {
            HistoryMode::List => "Enter open  r rename  d delete  p preview  Esc close",
            HistoryMode::Rename { .. } => "Enter save  Esc cancel",
            HistoryMode::ConfirmDelete => "y confirm delete  any key cancel",
            HistoryMode::Preview { .. } => "Enter load  ↑↓ scroll  Esc back",
        },
        Some(_) => "Esc close  ↑↓ navigate",
        None => "^S history  ^P prompt  ^E export  ^B branches  ↑↓ scroll  Esc sidebar",
    };

    let mut spans = vec![
        Span::styled(" ", Style::default()),
        Span::styled(help_text, theme::dim()),
    ];

    // Send hint when input has content
    if app.chat_popup.is_none() && !app.chat_input.is_empty() && !app.chat_streaming {
        let hint = "  ↵ send";
        let pad = (area.width as usize).saturating_sub(help_text.len() + hint.len() + 3);
        if pad > 0 {
            spans.push(Span::styled(" ".repeat(pad), Style::default()));
            spans.push(Span::styled(hint, theme::primary()));
        }
    }

    buf.set_line(area.x, area.y, &Line::from(spans), area.width);
}

// ── Content rendering helpers ──────────────────────────────────

/// Push a thin separator line between messages.
fn push_separator(lines: &mut Vec<ChatLine<'_>>, max_width: usize) {
    let rule_width = max_width.min(60);
    let rule: String = "─".repeat(rule_width);
    lines.push(ChatLine {
        spans: vec![Span::styled(rule, Style::default().fg(theme::SURFACE_HIGH))],
        indent: 2,
    });
}

/// Build spans for a content line with gutter bar, optional result styling,
/// optional list bullet, and inline Markdown formatting.
fn build_guttered_spans(
    text: &str,
    gutter: GutterStyle,
    bubble_style: Style,
    result_style: Option<Style>,
    is_list_item: bool,
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();

    // Gutter bar
    if let Some(rs) = result_style {
        spans.push(Span::styled("▌ ", rs));
    } else {
        spans.push(Span::styled("▎ ", gutter.bar_style));
    }

    // List bullet
    if is_list_item {
        spans.push(Span::styled("◦ ", theme::dim()));
    }

    // Content with inline Markdown
    if let Some(rs) = result_style {
        spans.push(Span::styled(text.to_owned(), rs));
    } else {
        let inline_spans = parse_inline_spans(text, bubble_style);
        spans.extend(inline_spans);
    }

    spans
}

/// Parse inline Markdown formatting: **bold** and `code`.
///
/// Scans the text for `**...**` and `` `...` `` boundaries and emits
/// styled spans. Everything outside these markers uses `base_style`.
fn parse_inline_spans(text: &str, base_style: Style) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() {
        // Find the nearest marker
        let bold_pos = remaining.find("**");
        let code_pos = remaining.find('`');

        let next_marker = match (bold_pos, code_pos) {
            (Some(b), Some(c)) => {
                if b <= c {
                    Some(("**", b))
                } else {
                    Some(("`", c))
                }
            }
            (Some(b), None) => Some(("**", b)),
            (None, Some(c)) => Some(("`", c)),
            (None, None) => None,
        };

        match next_marker {
            None => {
                // No more markers — emit the rest as plain text
                if !remaining.is_empty() {
                    spans.push(Span::styled(remaining.to_owned(), base_style));
                }
                break;
            }
            Some(("**", pos)) => {
                // Emit text before the marker
                if pos > 0 {
                    spans.push(Span::styled(remaining[..pos].to_owned(), base_style));
                }
                // Find closing **
                let after = &remaining[pos + 2..];
                if let Some(end) = after.find("**") {
                    let bold_text = &after[..end];
                    spans.push(Span::styled(
                        bold_text.to_owned(),
                        base_style.add_modifier(Modifier::BOLD),
                    ));
                    remaining = &after[end + 2..];
                } else {
                    // No closing ** — emit as-is
                    spans.push(Span::styled(remaining[pos..].to_owned(), base_style));
                    break;
                }
            }
            Some(("`", pos)) => {
                // Emit text before the marker
                if pos > 0 {
                    spans.push(Span::styled(remaining[..pos].to_owned(), base_style));
                }
                // Find closing `
                let after = &remaining[pos + 1..];
                if let Some(end) = after.find('`') {
                    let code_text = &after[..end];
                    spans.push(Span::styled(
                        code_text.to_owned(),
                        Style::default().fg(theme::ACCENT_GLOW),
                    ));
                    remaining = &after[end + 1..];
                } else {
                    // No closing ` — emit as-is
                    spans.push(Span::styled(remaining[pos..].to_owned(), base_style));
                    break;
                }
            }
            _ => {
                spans.push(Span::styled(remaining.to_owned(), base_style));
                break;
            }
        }
    }

    if spans.is_empty() {
        spans.push(Span::styled(String::new(), base_style));
    }
    spans
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
    let Some(ref popup) = app.chat_popup else {
        return;
    };

    let popup_area = centered_rect(area, 60, 70);
    Clear.render(popup_area, buf);

    match popup {
        ChatPopup::ConversationHistory {
            sessions,
            selected,
            scroll_offset,
            mode,
        } => {
            let title_text = match mode {
                HistoryMode::ConfirmDelete => " Delete Conversation? ".to_string(),
                HistoryMode::Rename { .. } => " Rename Conversation ".to_string(),
                HistoryMode::Preview { .. } => {
                    let name = sessions
                        .get(selected.wrapping_sub(1))
                        .map(|e| e.title.as_str())
                        .unwrap_or("Preview");
                    format!(" {name} ")
                }
                HistoryMode::List => format!(" Conversations ({}) ", sessions.len()),
            };
            let border_style = match mode {
                HistoryMode::ConfirmDelete => Style::default().fg(theme::ERROR),
                _ => theme::border_active(),
            };
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(border_style)
                .title(Line::from(Span::styled(&title_text, theme::primary())));
            let inner = block.inner(popup_area);
            block.render(popup_area, buf);

            match mode {
                HistoryMode::List => {
                    let mut y = inner.y;
                    let active_id = app.chat_session_id.as_deref();

                    // "New conversation" option
                    let new_style = if *selected == 0 {
                        theme::primary_bold()
                    } else {
                        theme::text()
                    };
                    let marker = if *selected == 0 { "▸" } else { " " };
                    let new_line = Line::from(vec![
                        Span::styled(
                            marker,
                            if *selected == 0 {
                                theme::primary()
                            } else {
                                theme::dim()
                            },
                        ),
                        Span::styled(" + New conversation", new_style),
                    ]);
                    buf.set_line(inner.x + 1, y, &new_line, inner.width - 2);
                    y += 2;

                    // Session entries (with scroll support)
                    let visible_height = (inner.height as usize).saturating_sub(2); // minus header
                    let entry_height = 2usize; // title + date row
                    let max_visible = visible_height / entry_height;

                    for (i, entry) in sessions
                        .iter()
                        .enumerate()
                        .skip(*scroll_offset)
                        .take(max_visible)
                    {
                        if y >= inner.y + inner.height {
                            break;
                        }
                        let idx = i + 1;
                        let is_sel = idx == *selected;
                        let is_active = active_id == Some(entry.id.as_str());
                        let marker = if is_sel { "▸" } else { " " };
                        let title_style = if is_sel {
                            theme::primary_bold()
                        } else if is_active {
                            theme::sage()
                        } else {
                            theme::text()
                        };

                        let max_title = (inner.width as usize).saturating_sub(25);
                        let title: String = if entry.title.len() > max_title {
                            let end = entry.title.ceil_char_boundary(max_title.saturating_sub(1));
                            format!("{}…", &entry.title[..end])
                        } else {
                            entry.title.clone()
                        };

                        let mut spans = vec![
                            Span::styled(
                                marker,
                                if is_sel {
                                    theme::primary()
                                } else {
                                    theme::dim()
                                },
                            ),
                            Span::styled(format!(" {title}"), title_style),
                        ];
                        if is_active {
                            spans.push(Span::styled("  ●", theme::sage()));
                        }
                        spans.push(Span::styled(
                            format!("  {} turns", entry.turn_count),
                            theme::dim(),
                        ));
                        buf.set_line(inner.x + 1, y, &Line::from(spans), inner.width - 2);
                        y += 1;

                        // Subtitle: dates
                        let date_line = Line::from(vec![
                            Span::styled("   ", Style::default()),
                            Span::styled(&entry.updated_at, theme::dim()),
                            Span::styled("  created ", theme::dim()),
                            Span::styled(&entry.created_at, theme::dim()),
                        ]);
                        buf.set_line(inner.x + 1, y, &date_line, inner.width - 2);
                        y += 1;
                    }

                    // Scroll indicators
                    if *scroll_offset > 0 {
                        let ind = Line::from(Span::styled(
                            format!("  ↑ {} more", scroll_offset),
                            theme::dim(),
                        ));
                        buf.set_line(inner.x, inner.y, &ind, inner.width);
                    }
                    let remaining = sessions.len().saturating_sub(*scroll_offset + max_visible);
                    if remaining > 0 {
                        let ind = Line::from(Span::styled(
                            format!("  ↓ {} more", remaining),
                            theme::dim(),
                        ));
                        buf.set_line(inner.x, inner.y + inner.height - 1, &ind, inner.width);
                    }
                }
                HistoryMode::Rename { input } => {
                    let entry_title = sessions
                        .get(selected.wrapping_sub(1))
                        .map(|e| e.title.as_str())
                        .unwrap_or("");
                    let label = Line::from(Span::styled(
                        format!("Current: {entry_title}"),
                        theme::dim(),
                    ));
                    buf.set_line(inner.x + 2, inner.y + 1, &label, inner.width - 4);

                    let prompt = Line::from(Span::styled("New title:", theme::text()));
                    buf.set_line(inner.x + 2, inner.y + 3, &prompt, inner.width - 4);

                    let input_display = if input.is_empty() {
                        Span::styled("(type new name)", theme::dim())
                    } else {
                        Span::styled(input.as_str(), theme::primary_bold())
                    };
                    let cursor = Span::styled("▎", theme::primary());
                    buf.set_line(
                        inner.x + 2,
                        inner.y + 4,
                        &Line::from(vec![input_display, cursor]),
                        inner.width - 4,
                    );
                }
                HistoryMode::ConfirmDelete => {
                    let entry_title = sessions
                        .get(selected.wrapping_sub(1))
                        .map(|e| e.title.as_str())
                        .unwrap_or("this session");
                    let warning = Line::from(Span::styled(
                        "This will permanently delete:",
                        Style::default().fg(theme::ERROR),
                    ));
                    buf.set_line(inner.x + 2, inner.y + 1, &warning, inner.width - 4);

                    let name = Line::from(Span::styled(
                        format!("  \"{entry_title}\""),
                        theme::text_bold(),
                    ));
                    buf.set_line(inner.x + 2, inner.y + 3, &name, inner.width - 4);

                    let confirm = Line::from(vec![
                        Span::styled("Press ", theme::dim()),
                        Span::styled("y", theme::primary_bold()),
                        Span::styled(" to confirm, any other key to cancel.", theme::dim()),
                    ]);
                    buf.set_line(inner.x + 2, inner.y + 5, &confirm, inner.width - 4);
                }
                HistoryMode::Preview { lines, scroll } => {
                    if lines.is_empty() {
                        let empty =
                            Line::from(Span::styled("  (empty conversation)", theme::dim()));
                        buf.set_line(inner.x + 1, inner.y + 1, &empty, inner.width - 2);
                    } else {
                        let max_w = (inner.width - 6) as usize;
                        let mut rendered: Vec<(Span, Span)> = Vec::new();
                        for (role, content) in lines.iter() {
                            let label = if role == "you" || role == "user" {
                                "You"
                            } else {
                                "AI"
                            };
                            let preview: String = content.chars().take(max_w).collect();
                            let preview = preview.replace('\n', " ");
                            rendered.push((
                                Span::styled(
                                    format!("{label:>3}"),
                                    if label == "You" {
                                        theme::primary()
                                    } else {
                                        theme::secondary()
                                    },
                                ),
                                Span::styled(format!("  {preview}"), theme::text()),
                            ));
                        }
                        let max_scroll = rendered.len().saturating_sub(inner.height as usize);
                        let eff_scroll = (*scroll).min(max_scroll);
                        let mut y = inner.y;
                        for (label, content) in
                            rendered.iter().skip(eff_scroll).take(inner.height as usize)
                        {
                            buf.set_line(
                                inner.x + 1,
                                y,
                                &Line::from(vec![label.clone(), content.clone()]),
                                inner.width - 2,
                            );
                            y += 1;
                        }

                        if eff_scroll > 0 || eff_scroll < max_scroll {
                            let indicator = format!(" {}/{} ", eff_scroll + 1, rendered.len());
                            let ind = Line::from(Span::styled(indicator, theme::dim()));
                            let ind_x = popup_area.x + popup_area.width.saturating_sub(15);
                            buf.set_line(ind_x, popup_area.y, &ind, 15);
                        }
                    }
                }
            }
        }

        ChatPopup::SystemPrompt { lines, scroll } => {
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(theme::border_active())
                .title(Line::from(Span::styled(
                    " System Prompt Preview ",
                    theme::primary(),
                )));
            let inner = block.inner(popup_area);
            block.render(popup_area, buf);

            let max_scroll = lines.len().saturating_sub(inner.height as usize);
            let eff_scroll = (*scroll).min(max_scroll);

            let mut y = inner.y;
            for line in lines.iter().skip(eff_scroll).take(inner.height as usize) {
                let display: String = line.chars().take((inner.width - 2) as usize).collect();
                buf.set_line(
                    inner.x + 1,
                    y,
                    &Line::from(Span::styled(display, theme::text())),
                    inner.width - 2,
                );
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
                buf.set_line(
                    inner.x + 2,
                    y,
                    &Line::from(Span::styled(&path[pos..end], theme::primary())),
                    inner.width - 4,
                );
                y += 1;
                pos = end;
            }

            let dismiss = Line::from(Span::styled("Press any key to close.", theme::dim()));
            buf.set_line(
                inner.x + 2,
                inner.y + inner.height.saturating_sub(2),
                &dismiss,
                inner.width - 4,
            );
        }

        ChatPopup::BranchManager {
            snapshots,
            selected,
            label_input,
            creating,
        } => {
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
                        if y >= inner.y + inner.height {
                            break;
                        }
                        let is_sel = i == *selected;
                        let marker = if is_sel { "▸" } else { " " };
                        let label = snap.label.as_deref().unwrap_or("(unlabeled)");
                        let title_style = if is_sel {
                            theme::primary_bold()
                        } else {
                            theme::text()
                        };

                        let line = Line::from(vec![
                            Span::styled(
                                marker,
                                if is_sel {
                                    theme::primary()
                                } else {
                                    theme::dim()
                                },
                            ),
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
        let area = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 50,
        };
        let r = centered_rect(area, 60, 70);
        assert_eq!(r.width, 60);
        assert_eq!(r.height, 35);
        assert_eq!(r.x, 20); // (100 - 60) / 2
        assert_eq!(r.y, 7); // (50 - 35) / 2
    }

    #[test]
    fn centered_rect_full_size() {
        let area = Rect {
            x: 10,
            y: 5,
            width: 80,
            height: 40,
        };
        let r = centered_rect(area, 100, 100);
        assert_eq!(r.x, area.x);
        assert_eq!(r.y, area.y);
        assert_eq!(r.width, area.width);
        assert_eq!(r.height, area.height);
    }

    // ── parse_inline_spans tests ────────────────────────────────

    #[test]
    fn inline_plain_text() {
        let base = Style::default().fg(theme::ON_SURFACE);
        let spans = parse_inline_spans("Hello world", base);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content, "Hello world");
    }

    #[test]
    fn inline_bold() {
        let base = Style::default().fg(theme::ON_SURFACE);
        let spans = parse_inline_spans("Hello **bold** world", base);
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0].content, "Hello ");
        assert_eq!(spans[1].content, "bold");
        assert!(
            spans[1].style.add_modifier == Modifier::BOLD
                || format!("{:?}", spans[1].style).contains("BOLD")
        );
        assert_eq!(spans[2].content, " world");
    }

    #[test]
    fn inline_code() {
        let base = Style::default().fg(theme::ON_SURFACE);
        let spans = parse_inline_spans("Use `cargo build` here", base);
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0].content, "Use ");
        assert_eq!(spans[1].content, "cargo build");
        assert_eq!(spans[1].style.fg, Some(theme::ACCENT_GLOW));
        assert_eq!(spans[2].content, " here");
    }

    #[test]
    fn inline_mixed_bold_and_code() {
        let base = Style::default().fg(theme::ON_SURFACE);
        let spans = parse_inline_spans("Run **`cmd`** now", base);
        // **`cmd`** — the bold markers wrap the backticks
        assert!(spans.len() >= 3, "should have multiple spans: {:?}", spans);
    }

    #[test]
    fn inline_unclosed_bold() {
        let base = Style::default().fg(theme::ON_SURFACE);
        let spans = parse_inline_spans("Hello **unclosed", base);
        // Should not panic, just emit as-is
        assert!(!spans.is_empty());
    }

    #[test]
    fn inline_unclosed_code() {
        let base = Style::default().fg(theme::ON_SURFACE);
        let spans = parse_inline_spans("Use `unclosed", base);
        assert!(!spans.is_empty());
    }

    #[test]
    fn inline_empty() {
        let base = Style::default().fg(theme::ON_SURFACE);
        let spans = parse_inline_spans("", base);
        assert_eq!(spans.len(), 1); // empty span
        assert_eq!(spans[0].content, "");
    }

    // ── push_separator test ─────────────────────────────────────

    #[test]
    fn separator_width_capped_at_60() {
        let mut lines: Vec<ChatLine> = Vec::new();
        push_separator(&mut lines, 120);
        assert_eq!(lines.len(), 1);
        // The rule should be at most 60 chars wide
        let content = &lines[0].spans[0].content;
        let char_count = content.chars().count();
        assert_eq!(char_count, 60, "separator should be capped at 60 chars");
    }

    #[test]
    fn separator_short_width() {
        let mut lines: Vec<ChatLine> = Vec::new();
        push_separator(&mut lines, 20);
        assert_eq!(lines.len(), 1);
        let content = &lines[0].spans[0].content;
        let char_count = content.chars().count();
        assert_eq!(char_count, 20, "separator should respect width if < 60");
    }
}
