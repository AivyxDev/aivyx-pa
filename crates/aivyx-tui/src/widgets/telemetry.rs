//! Telemetry sidebar for the Home view — live agent stats.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Widget},
};

use crate::app::App;
use crate::theme;

pub struct TelemetrySidebar<'a> {
    app: &'a App,
}

impl<'a> TelemetrySidebar<'a> {
    pub fn new(app: &'a App) -> Self {
        Self { app }
    }
}

impl Widget for TelemetrySidebar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .borders(Borders::LEFT)
            .border_type(BorderType::Rounded)
            .border_style(theme::border());
        let inner = block.inner(area);
        block.render(area, buf);

        let mut y = inner.y;
        let padding = 3;
        let w = inner.width.saturating_sub(6) as usize; // Double the padding for width inset
        
        y += 1; // Top margin

        // Title
        let title = Line::from(vec![
            Span::styled("[ ", theme::dim()),
            Span::styled("SYSTEM TELEMETRY", theme::muted().add_modifier(Modifier::BOLD)),
            Span::styled(" ]", theme::dim()),
        ]);
        buf.set_line(inner.x + padding, y, &title, inner.width.saturating_sub(padding));
        y += 2;

        // ── Provider ───────────────────────────────────────────
        render_label_value(buf, inner.x + padding, y, w, "PROVIDER", &self.app.provider_label);
        y += 1;
        render_label_value(buf, inner.x + padding, y, w, "MODEL", &self.app.model_name);
        y += 2;

        // ── Token Usage ────────────────────────────────────────
        let section = Line::from(vec![
            Span::styled("[ ", theme::dim()),
            Span::styled("TOKEN USAGE", theme::muted().add_modifier(Modifier::BOLD)),
            Span::styled(" ]", theme::dim()),
        ]);
        buf.set_line(inner.x + padding, y, &section, inner.width.saturating_sub(padding));
        y += 1;

        let input_str = format_tokens(self.app.chat_input_tokens);
        let output_str = format_tokens(self.app.chat_output_tokens);
        let total = self.app.chat_input_tokens + self.app.chat_output_tokens;
        let total_str = format_tokens(total);

        render_label_value(
            buf,
            inner.x + padding,
            y,
            w,
            "INPUT",
            &format!("{input_str} TOKENS"),
        );
        y += 1;
        render_label_value(
            buf,
            inner.x + padding,
            y,
            w,
            "OUTPUT",
            &format!("{output_str} TOKENS"),
        );
        y += 1;
        render_label_value(buf, inner.x + padding, y, w, "TOTAL", &total_str);
        y += 1;

        // Token bar
        if y < inner.y + inner.height && total > 0 {
            let bar_w = w.max(5).min(25); // Set reliable min and cap bounds
            let input_ratio = self.app.chat_input_tokens as f64 / total as f64;
            let input_bars = (input_ratio * bar_w as f64).round() as usize;
            let output_bars = bar_w.saturating_sub(input_bars);
            let bar_line = Line::from(vec![
                Span::styled("█".repeat(input_bars), theme::primary()),
                Span::styled("█".repeat(output_bars), theme::secondary()),
            ]);
            buf.set_line(inner.x + padding, y, &bar_line, inner.width.saturating_sub(padding));
            y += 1;
            let legend = Line::from(vec![
                Span::styled("■ IN  ", theme::primary()),
                Span::styled("■ OUT", theme::secondary()),
            ]);
            buf.set_line(inner.x + padding, y, &legend, inner.width.saturating_sub(padding));
        }
        y += 2;

        // ── Cost ───────────────────────────────────────────────
        if y < inner.y + inner.height {
            let cost = if self.app.chat_cost_usd < 0.01 && self.app.chat_cost_usd > 0.0 {
                format!("${:.4}", self.app.chat_cost_usd)
            } else {
                format!("${:.2}", self.app.chat_cost_usd)
            };
            render_label_value(buf, inner.x + padding, y, w, "SESSION COST", &cost);
            y += 1;

            let msgs = self.app.chat_context_window;
            render_label_value(buf, inner.x + padding, y, w, "MESSAGES", &format!("{msgs}"));
            y += 2;
        }

        // ── Last Turn Stats ────────────────────────────────────
        if y < inner.y + inner.height
            && (self.app.turn_tool_calls > 0 || !self.app.turn_tool_log.is_empty())
        {
            let section = Line::from(vec![
                Span::styled("[ ", theme::dim()),
                Span::styled("LAST TURN", theme::muted().add_modifier(Modifier::BOLD)),
                Span::styled(" ]", theme::dim()),
            ]);
            buf.set_line(inner.x + padding, y, &section, inner.width.saturating_sub(padding));
            y += 1;

            render_label_value(
                buf,
                inner.x + padding,
                y,
                w,
                "TOOL CALLS",
                &format!("{}", self.app.turn_tool_calls),
            );
            y += 1;

            let turn_tok = format_tokens(self.app.turn_total_tokens);
            render_label_value(buf, inner.x + padding, y, w, "TURN TOKENS", &turn_tok);
            y += 1;

            // Mini tool log (most recent 5)
            let max_entries = 5.min(self.app.turn_tool_log.len());
            for entry in self.app.turn_tool_log.iter().rev().take(max_entries) {
                if y >= inner.y + inner.height {
                    break;
                }
                let (icon, style) = if entry.denied {
                    ("⊘", Style::default().fg(theme::ERROR))
                } else if entry.error.is_some() {
                    ("✗", Style::default().fg(theme::ERROR))
                } else {
                    ("✓", Style::default().fg(theme::SAGE))
                };
                let ms_str = entry
                    .duration_ms
                    .map(|ms| format!(" {:.1}s", ms as f64 / 1000.0))
                    .unwrap_or_default();
                // Truncate tool name to fit sidebar
                let name = if entry.tool_name.len() > (w.saturating_sub(8)) {
                    format!("{}…", &entry.tool_name[..w.saturating_sub(9)])
                } else {
                    entry.tool_name.clone()
                };
                let tool_line = Line::from(Span::styled(format!("  {icon} {name}{ms_str}"), style));
                buf.set_line(inner.x + padding, y, &tool_line, inner.width.saturating_sub(padding));
                y += 1;
            }
            y += 1;
        }

        // ── Heartbeat ──────────────────────────────────────────
        if y < inner.y + inner.height {
            let section = Line::from(vec![
                Span::styled("[ ", theme::dim()),
                Span::styled("HEARTBEAT", theme::muted().add_modifier(Modifier::BOLD)),
                Span::styled(" ]", theme::dim()),
            ]);
            buf.set_line(inner.x + padding, y, &section, inner.width.saturating_sub(padding));
            y += 1;

            let hb_enabled = self
                .app
                .settings
                .as_ref()
                .map(|s| s.heartbeat_enabled)
                .unwrap_or(false);
            let status = if hb_enabled { "ACTIVE" } else { "DISABLED" };
            let status_style = if hb_enabled {
                theme::sage()
            } else {
                theme::dim()
            };
            let status_line = Line::from(vec![
                Span::styled(format!("{:<15}", "STATUS"), theme::muted()),
                Span::styled(status, status_style),
            ]);
            buf.set_line(inner.x + padding, y, &status_line, inner.width.saturating_sub(padding));
            y += 1;

            if hb_enabled {
                render_label_value(
                    buf,
                    inner.x + padding,
                    y,
                    w,
                    "INTERVAL",
                    &format!("{} MIN", self.app.heartbeat_interval),
                );
            }
        }
    }
}

/// Render a left-aligned label with a right-aligned value.
fn render_label_value(buf: &mut Buffer, x: u16, y: u16, width: usize, label: &str, value: &str) {
    let line = Line::from(vec![
        Span::styled(format!("{:<15}", label), theme::muted()),
        Span::styled(value, theme::text()),
    ]);
    buf.set_line(x, y, &line, width as u16);
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
