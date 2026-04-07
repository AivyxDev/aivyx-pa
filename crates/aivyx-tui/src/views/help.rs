//! Help view — scrollable user manual rendered from docs/manual.md.

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    text::{Line, Span},
    widgets::{Block, Borders, BorderType, Widget},
};

use crate::app::App;
use crate::theme;

const MANUAL: &str = include_str!("../../../../docs/manual.md");

pub fn render(app: &App, area: Rect, buf: &mut Buffer) {
    let [header, body, help_bar] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(5),
        Constraint::Length(1),
    ]).areas(area);

    let title = Line::from(vec![
        Span::styled("Help", theme::text_bold()),
        Span::styled("  User manual — scroll with ↑↓ or j/k.", theme::dim()),
    ]);
    buf.set_line(header.x, header.y, &title, header.width);

    // ── Manual content ───────────────────────────────────────
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(theme::border())
        .title(Line::from(Span::styled("[ USER MANUAL ]", theme::primary_bold())));
    let inner = block.inner(body);
    block.render(body, buf);

    let max_w = (inner.width.saturating_sub(4)) as usize;
    let lines = render_manual_lines(max_w);
    let total = lines.len();
    let visible = inner.height as usize;
    let scroll = app.help_scroll.min(total.saturating_sub(visible));

    for (i, line) in lines.iter().enumerate().skip(scroll).take(visible) {
        let y = inner.y + (i - scroll) as u16;
        buf.set_line(inner.x + 1, y, line, inner.width - 2);
    }

    // ── Help bar ─────────────────────────────────────────────
    let pos = if total > 0 {
        let pct = if total <= visible { 100 } else { (scroll * 100) / (total - visible) };
        format!("  {pct}%  ({}/{})", scroll + 1, total)
    } else {
        String::new()
    };
    let help = Line::from(vec![
        Span::styled("↑↓/j k", theme::primary()),
        Span::styled(" scroll  ", theme::dim()),
        Span::styled("Tab", theme::primary()),
        Span::styled(" sidebar", theme::dim()),
        Span::styled(pos, theme::muted()),
    ]);
    buf.set_line(help_bar.x, help_bar.y, &help, help_bar.width);
}

/// Parse the markdown manual into styled ratatui Lines.
///
/// This is intentionally simple — it handles:
/// - `## Heading` → primary bold
/// - `### Sub-heading` → text bold
/// - ``` code blocks → dim monospace
/// - `---` horizontal rules → dim separator
/// - `- bullet` lists → primary bullet marker
/// - Numbered lists `1.` → primary number
/// - Inline `backtick` → highlighted spans
/// - Everything else → normal text (word-wrapped)
fn render_manual_lines(max_w: usize) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::with_capacity(800);
    let mut in_code_block = false;

    for raw in MANUAL.lines() {
        // Code fence toggle
        if raw.starts_with("```") {
            in_code_block = !in_code_block;
            // Render the fence itself as a dim separator
            out.push(Line::from(Span::styled(
                "─".repeat(max_w.min(40)),
                theme::dim(),
            )));
            continue;
        }

        if in_code_block {
            // Code lines: dim, indented, no wrapping (truncate)
            let display: String = raw.chars().take(max_w).collect();
            out.push(Line::from(Span::styled(
                format!("  {display}"),
                theme::dim(),
            )));
            continue;
        }

        let trimmed = raw.trim();

        // Blank lines → empty line
        if trimmed.is_empty() {
            out.push(Line::default());
            continue;
        }

        // Horizontal rule
        if trimmed == "---" || trimmed == "***" || trimmed == "___" {
            out.push(Line::from(Span::styled(
                "─".repeat(max_w.min(60)),
                theme::dim(),
            )));
            continue;
        }

        // H1: # Title
        if let Some(heading) = trimmed.strip_prefix("# ") {
            out.push(Line::default());
            out.push(Line::from(Span::styled(
                heading.to_uppercase().to_string(),
                theme::primary_bold(),
            )));
            out.push(Line::from(Span::styled(
                "═".repeat(heading.len().min(max_w)),
                theme::primary(),
            )));
            continue;
        }

        // H2: ## Section
        if let Some(heading) = trimmed.strip_prefix("## ") {
            out.push(Line::default());
            out.push(Line::from(Span::styled(
                heading.to_string(),
                theme::primary_bold(),
            )));
            out.push(Line::from(Span::styled(
                "─".repeat(heading.len().min(max_w)),
                theme::primary(),
            )));
            continue;
        }

        // H3: ### Subsection
        if let Some(heading) = trimmed.strip_prefix("### ") {
            out.push(Line::default());
            out.push(Line::from(Span::styled(
                heading.to_string(),
                theme::text_bold(),
            )));
            continue;
        }

        // H4+: #### etc
        if trimmed.starts_with("####") {
            let heading = trimmed.trim_start_matches('#').trim();
            out.push(Line::from(Span::styled(
                heading.to_string(),
                theme::text_bold(),
            )));
            continue;
        }

        // Bullet list
        if let Some(rest) = trimmed.strip_prefix("- ") {
            let spans = inline_spans(rest);
            let mut full = vec![Span::styled("  • ", theme::primary())];
            full.extend(spans);
            wrap_spans(&full, max_w, &mut out);
            continue;
        }

        // Numbered list: "1. text" or "10. text"
        if let Some(dot_pos) = trimmed.find(". ") {
            if dot_pos <= 3 && trimmed[..dot_pos].chars().all(|c| c.is_ascii_digit()) {
                let num = &trimmed[..dot_pos];
                let rest = &trimmed[dot_pos + 2..];
                let spans = inline_spans(rest);
                let mut full = vec![Span::styled(format!("  {num}. "), theme::primary())];
                full.extend(spans);
                wrap_spans(&full, max_w, &mut out);
                continue;
            }
        }

        // Regular paragraph — word-wrap with inline code highlighting
        let spans = inline_spans(trimmed);
        wrap_spans(&spans, max_w, &mut out);
    }

    out
}

/// Parse inline `backtick` code spans and **bold** into styled Spans.
fn inline_spans(text: &str) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut rest = text;

    while let Some(start) = rest.find('`') {
        // Text before the backtick
        if start > 0 {
            spans.push(Span::styled(rest[..start].to_string(), theme::text()));
        }
        rest = &rest[start + 1..];

        // Find closing backtick
        if let Some(end) = rest.find('`') {
            spans.push(Span::styled(
                rest[..end].to_string(),
                theme::highlight(),
            ));
            rest = &rest[end + 1..];
        } else {
            // No closing backtick — treat as normal text
            spans.push(Span::styled(format!("`{rest}"), theme::text()));
            return spans;
        }
    }

    // Remaining text
    if !rest.is_empty() {
        spans.push(Span::styled(rest.to_string(), theme::text()));
    }

    spans
}

/// Wrap a sequence of spans across multiple lines at `max_w` characters.
fn wrap_spans(spans: &[Span<'static>], max_w: usize, out: &mut Vec<Line<'static>>) {
    if max_w == 0 {
        return;
    }

    // Simple approach: concatenate all text, measure, and break lines.
    // We preserve styles by tracking span boundaries.
    let total_len: usize = spans.iter().map(|s| s.content.len()).sum();
    if total_len <= max_w {
        out.push(Line::from(spans.to_vec()));
        return;
    }

    // For longer content, break at word boundaries
    let full: String = spans.iter().map(|s| s.content.as_ref()).collect();
    let mut pos = 0;
    while pos < full.len() {
        let remaining = &full[pos..];
        if remaining.len() <= max_w {
            out.push(Line::from(Span::styled(remaining.to_string(), theme::text())));
            break;
        }
        // Find last space within max_w
        let chunk = &remaining[..max_w];
        let break_at = chunk.rfind(' ').unwrap_or(max_w);
        out.push(Line::from(Span::styled(remaining[..break_at].to_string(), theme::text())));
        pos += break_at;
        // Skip the space
        if pos < full.len() && full.as_bytes()[pos] == b' ' {
            pos += 1;
        }
    }
}
