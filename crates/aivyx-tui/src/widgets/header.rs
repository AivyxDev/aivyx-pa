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
            Span::styled(
                "AIVYX.STUDIO ",
                Style::default()
                    .fg(theme::PRIMARY)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    "// {} {}",
                    self.app.view.icon(),
                    self.app.view.label().to_uppercase()
                ),
                theme::secondary(),
            ),
        ]);
        buf.set_line(inner.x + 1, inner.y, &title, inner.width / 2);

        // Right side: Agent metrics & Status
        let status = if self.app.chat_streaming {
            Span::styled("● STREAMING", theme::primary())
        } else {
            Span::styled("● IDLE", theme::dim())
        };

        // Combine agent info and provider into right side
        let info = format!(
            "{} // {} // v{}",
            self.app.agent_name.to_uppercase(),
            self.app.model_name.to_uppercase(),
            self.app.version
        );

        let right_side_str = format!("{} // ", info);
        let right_x = inner.x + inner.width.saturating_sub(right_side_str.len() as u16 + 11); // 11 is "● STREAMING" max length
        let rs_line = Line::from(vec![Span::styled(right_side_str, theme::dim()), status]);
        buf.set_line(
            right_x,
            inner.y,
            &rs_line,
            inner.width.saturating_sub(right_x) + 1,
        );
    }
}
