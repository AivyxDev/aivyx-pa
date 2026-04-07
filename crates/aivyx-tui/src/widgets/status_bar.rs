//! Status bar widget — bottom of the screen.
//!
//! Mirrors the GUI's `StatusBar.svelte` with engine version,
//! active agent, connection status, and latency.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Widget,
};

use crate::theme;

pub struct StatusBar<'a> {
    agent: &'a str,
    provider: &'a str,
    model: &'a str,
    version: &'a str,
    connected: bool,
}

impl<'a> StatusBar<'a> {
    pub fn new(agent: &'a str, provider: &'a str, model: &'a str, version: &'a str, connected: bool) -> Self {
        Self {
            agent,
            provider,
            model,
            version,
            connected,
        }
    }
}

impl Widget for StatusBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Background
        for x in area.x..area.x + area.width {
            buf[(x, area.y)]
                .set_style(Style::default().bg(theme::SURFACE));
        }

        // Left side: version + agent + provider
        let left = Line::from(vec![
            Span::styled(" PA ", theme::dim()),
            Span::styled(format!("v{}", self.version), theme::muted()),
            Span::styled(" │ ", theme::dim()),
            Span::styled("AGENT: ", theme::dim()),
            Span::styled(self.agent.to_uppercase(), theme::text()),
            Span::styled(" │ ", theme::dim()),
            Span::styled(
                format!("{} · {}", self.provider, self.model),
                theme::muted(),
            ),
        ]);
        buf.set_line(area.x, area.y, &left, area.width / 2 + 10);

        // Right side: connection status
        let (dot, label) = if self.connected {
            ("●", "IN-PROCESS")
        } else {
            ("○", "OFFLINE")
        };
        let dot_style = if self.connected {
            theme::sage()
        } else {
            theme::error()
        };

        let right = Line::from(vec![
            Span::styled(dot, dot_style),
            Span::styled(format!(" {label}"), theme::dim()),
            Span::raw(" "),
        ]);
        let right_width = right.width() as u16;
        let rx = area.x + area.width.saturating_sub(right_width);
        buf.set_line(rx, area.y, &right, right_width);
    }
}
