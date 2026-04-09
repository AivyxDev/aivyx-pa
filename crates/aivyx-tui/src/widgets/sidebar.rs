//! Sidebar navigation widget.
//!
//! Renders grouped nav items with live badge counts, the AIVYX brand
//! header, and an agent footer with name, persona, and tier from config.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Widget},
};

use crate::app::{App, Focus, View};
use crate::theme;

pub struct Sidebar<'a> {
    app: &'a App,
}

impl<'a> Sidebar<'a> {
    pub fn new(app: &'a App) -> Self {
        Self { app }
    }
}

impl Widget for Sidebar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let sidebar_focused = self.app.focus == Focus::Sidebar;
        let block = Block::default()
            .borders(Borders::RIGHT)
            .border_type(BorderType::Rounded)
            .border_style(if sidebar_focused {
                theme::border_active()
            } else {
                theme::border()
            })
            .style(Style::default().bg(theme::BG));
        let inner = block.inner(area);
        block.render(area, buf);

        let mut y = inner.y;

        // ── Brand header ───────────────────────────────────────
        if inner.height >= 4 {
            buf.set_line(
                inner.x + 2,
                y,
                &Line::from(Span::styled("AIVYX_OS", theme::primary_bold())),
                inner.width - 2,
            );
            y += 1;
            buf.set_line(
                inner.x + 2,
                y,
                &Line::from(vec![
                    Span::styled("v", theme::dim()),
                    Span::styled(self.app.version.clone(), theme::muted()),
                    Span::styled(" // PA", theme::dim()),
                ]),
                inner.width - 2,
            );
            y += 2;
        }

        // ── Nav items with live badges ─────────────────────────
        let mut prev_group: Option<u8> = None;
        for (idx, view) in View::ALL.iter().enumerate() {
            if y >= inner.y + inner.height {
                break;
            }

            // Group separator
            let group = view.group();
            if let Some(pg) = prev_group {
                if pg != group && y < inner.y + inner.height {
                    let sep = "─".repeat((inner.width - 4) as usize);
                    buf.set_line(
                        inner.x + 2,
                        y,
                        &Line::from(Span::styled(sep, theme::dim())),
                        inner.width - 4,
                    );
                    y += 1;
                }
            }
            prev_group = Some(group);

            if y >= inner.y + inner.height {
                break;
            }

            let is_active = idx == self.app.nav_index;

            // Active indicator bar
            if is_active {
                buf.set_line(
                    inner.x,
                    y,
                    &Line::from(Span::styled("▌", theme::primary())),
                    1,
                );
            }

            let style = if is_active {
                Style::default()
                    .fg(theme::PRIMARY)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::ON_SURFACE_DIM)
            };

            // Build label with optional badge
            let badge = self.badge_for(*view);
            let label = format!("{} {}", view.icon(), view.label().to_uppercase());

            let mut spans = vec![Span::styled(label, style)];
            if let Some((count, badge_style)) = badge {
                // Right-align the badge
                let badge_text = format!(" {count}");
                spans.push(Span::styled(badge_text, badge_style));
            }

            buf.set_line(inner.x + 2, y, &Line::from(spans), inner.width - 3);
            y += 1;
        }

        // ── Agent footer ───────────────────────────────────────
        if inner.height > 14 {
            let footer_y = inner.y + inner.height - 3;

            // Separator
            let sep = "─".repeat((inner.width - 4) as usize);
            buf.set_line(
                inner.x + 2,
                footer_y,
                &Line::from(Span::styled(sep, theme::dim())),
                inner.width - 4,
            );

            // Agent name + streaming indicator
            let status_icon = if self.app.chat_streaming {
                Span::styled("◆ ", theme::warning()) // pulsing when active
            } else {
                Span::styled("⊕ ", theme::secondary())
            };
            let agent_line = Line::from(vec![
                status_icon,
                Span::styled(self.app.agent_name.to_uppercase(), theme::text()),
            ]);
            buf.set_line(inner.x + 2, footer_y + 1, &agent_line, inner.width - 3);

            // Persona + tier
            if footer_y + 2 < inner.y + inner.height {
                let persona = self
                    .app
                    .settings
                    .as_ref()
                    .map(|s| s.agent_persona.as_str())
                    .unwrap_or("assistant");
                let tier_line = Line::from(vec![
                    Span::styled("  ", Style::default()),
                    Span::styled(persona.to_uppercase(), theme::dim()),
                    Span::styled(" · ", theme::dim()),
                    Span::styled(self.app.autonomy_tier.to_uppercase(), theme::dim()),
                ]);
                buf.set_line(inner.x + 2, footer_y + 2, &tier_line, inner.width - 3);
            }
        }
    }
}

impl Sidebar<'_> {
    /// Return a badge count + style for views that have live indicators.
    fn badge_for(&self, view: View) -> Option<(usize, Style)> {
        match view {
            View::Goals => {
                let count = self.app.active_goals;
                if count > 0 {
                    Some((count, theme::dim()))
                } else {
                    None
                }
            }
            View::Approvals => {
                let count = self.app.pending_approvals;
                if count > 0 {
                    // Pending approvals get an attention-grabbing badge
                    Some((
                        count,
                        Style::default()
                            .fg(theme::ACCENT_GLOW)
                            .add_modifier(Modifier::BOLD),
                    ))
                } else {
                    None
                }
            }
            View::Missions => {
                // Show count of active (non-terminal) missions
                let count = self
                    .app
                    .missions
                    .iter()
                    .filter(|m| !m.status.is_terminal())
                    .count();
                if count > 0 {
                    Some((count, Style::default().fg(theme::PRIMARY)))
                } else {
                    None
                }
            }
            View::Activity => {
                let count = self.app.notifications.len();
                if count > 0 {
                    Some((count, theme::dim()))
                } else {
                    None
                }
            }
            View::Memory => {
                let count = self.app.memory_total;
                if count > 0 {
                    Some((count, theme::dim()))
                } else {
                    None
                }
            }
            View::Audit => {
                let count = self.app.audit_entries.len();
                if count > 0 {
                    Some((count, theme::dim()))
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}
