//! Header bar widget.
//!
//! Replaces the old Header and StatusBar, forming a persistent context bar.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Widget},
};

use crate::app::App;
use crate::theme;

pub struct Header<'a> {
    app: &'a App,
}

impl<'a> Header<'a> {
    pub fn new(app: &'a App) -> Self {
        Self { app }
    }
}

impl Widget for Header<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .borders(Borders::BOTTOM)
            .border_style(theme::border())
            .style(Style::default().bg(theme::BG));
        let inner = block.inner(area);
        block.render(area, buf);

        // Left side: Brand + View Context
        let title = Line::from(vec![
            Span::styled("[ ", theme::dim()),
            Span::styled(
                "AIVYX_OS ",
                Style::default()
                    .fg(theme::PRIMARY)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    "// {} {} ",
                    self.app.view.icon(),
                    self.app.view.label().to_uppercase()
                ),
                theme::secondary(),
            ),
            Span::styled("]", theme::dim()),
        ]);
        // 2 cols of padding on the left
        buf.set_line(inner.x + 2, inner.y, &title, inner.width / 2);

        // Right side: Agent metrics & Status
        let status_spans = if self.app.chat_streaming {
            vec![
                Span::styled("[ ", theme::dim()),
                Span::styled("STATUS: ", theme::muted()),
                Span::styled(
                    "STREAMING ",
                    Style::default()
                        .fg(theme::PRIMARY)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("]", theme::dim()),
            ]
        } else {
            vec![
                Span::styled("[ ", theme::dim()),
                Span::styled("STATUS: ", theme::muted()),
                Span::styled("IDLE ", theme::text()),
                Span::styled("]", theme::dim()),
            ]
        };

        let mut right_spans = vec![
            Span::styled("[ ", theme::dim()),
            Span::styled("AGENT: ", theme::muted()),
            Span::styled(
                format!("{} ", self.app.agent_name.to_uppercase()),
                theme::text(),
            ),
            Span::styled("]  ", theme::dim()),
            Span::styled("[ ", theme::dim()),
            Span::styled("SYS: ", theme::muted()),
            Span::styled(format!("v{} ", self.app.version), theme::text()),
            Span::styled("]  ", theme::dim()),
            Span::styled("[ ", theme::dim()),
            Span::styled("LLM: ", theme::muted()),
            Span::styled(
                format!("{} ", self.app.model_name.to_uppercase()),
                theme::text(),
            ),
            Span::styled("]  ", theme::dim()),
        ];

        right_spans.extend(status_spans);

        // Compute exact visual width of right side spans
        let right_width = right_spans.iter().map(|s| s.width() as u16).sum::<u16>();
        let padding_right = 2; // 2 cols padding on the right edge

        let right_x = inner.x + inner.width.saturating_sub(right_width + padding_right);
        let rs_line = Line::from(right_spans);
        buf.set_line(
            right_x,
            inner.y,
            &rs_line,
            inner.width.saturating_sub(right_x) + 1,
        );
    }
}
