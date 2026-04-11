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

        // Bail out entirely on pathologically narrow sidebars — below 5
        // columns there isn't room for the "▌ " active marker + a single
        // glyph, and the various `width - N` offsets below would underflow
        // (usize wraps to a huge positive value and ratatui panics).
        if inner.width < 5 || inner.height == 0 {
            return;
        }

        let mut y = inner.y;

        // ── Brand header ───────────────────────────────────────
        if inner.height >= 4 {
            y += 1; // Top padding
            buf.set_line(
                inner.x + 3,
                y,
                &Line::from(vec![
                    Span::styled("[ ", theme::dim()),
                    Span::styled("NODE: ", theme::muted()),
                    Span::styled("AIVYX-PA ", theme::primary_bold()),
                    Span::styled("]", theme::dim()),
                ]),
                inner.width.saturating_sub(3),
            );
            y += 1;
            buf.set_line(
                inner.x + 3,
                y,
                &Line::from(vec![
                    Span::styled("[ ", theme::dim()),
                    Span::styled("CORE: ", theme::muted()),
                    Span::styled(format!("v{} ", self.app.version), theme::text()),
                    Span::styled("]", theme::dim()),
                ]),
                inner.width.saturating_sub(3),
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
            if let Some(pg) = prev_group
                && pg != group
                && y < inner.y + inner.height
            {
                let sep = "\u{2500}".repeat(inner.width.saturating_sub(6) as usize);
                buf.set_line(
                    inner.x + 3,
                    y,
                    &Line::from(Span::styled(sep, theme::dim())),
                    inner.width.saturating_sub(6),
                );
                y += 1;
            }
            prev_group = Some(group);

            if y >= inner.y + inner.height {
                break;
            }

            let is_active = idx == self.app.nav_index;

            // Active indicator bar
            if is_active {
                buf.set_line(
                    inner.x + 1, // 1 col padding inward for the gutter line
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

            // Build label
            let label = format!("{} {}", view.icon(), view.label().to_uppercase());
            buf.set_line(
                inner.x + 3, // 3 cols left padding for the text
                y,
                &Line::from(Span::styled(label, style)),
                inner.width.saturating_sub(3),
            );

            // Set badge perfectly right-aligned
            let badge = self.badge_for(*view);
            if let Some((count, badge_style)) = badge {
                let badge_str = format!("[{}]", count);
                let badge_x = inner.x + inner.width.saturating_sub(badge_str.len() as u16 + 2); // 2 cols right padding
                buf.set_line(
                    badge_x,
                    y,
                    &Line::from(Span::styled(badge_str.clone(), badge_style)),
                    badge_str.len() as u16,
                );
            }

            y += 1;
        }

        // ── Agent footer ───────────────────────────────────────
        if inner.height > 15 {
            let footer_y = inner.y + inner.height.saturating_sub(5); // Anchor perfectly back up from the bottom edge

            // Divider cleanly tracking sidebar widths
            let sep = "\u{2500}".repeat(inner.width.saturating_sub(6) as usize);
            buf.set_line(
                inner.x + 3,
                footer_y,
                &Line::from(Span::styled(sep, theme::dim())),
                inner.width.saturating_sub(6),
            );

            // Agent name + streaming indicator
            let status_icon = if self.app.chat_streaming {
                Span::styled("◆ ", theme::warning()) // pulsing
            } else {
                Span::styled("  ", theme::secondary())
            };

            buf.set_line(
                inner.x + 3,
                footer_y + 1,
                &Line::from(vec![
                    status_icon,
                    Span::styled("ID: ", theme::muted()),
                    Span::styled(self.app.agent_name.to_uppercase(), theme::text_bold()),
                ]),
                inner.width.saturating_sub(3),
            );

            // Persona & Autonomy Details
            if footer_y + 3 < inner.y + inner.height {
                let persona = self
                    .app
                    .settings
                    .as_ref()
                    .map(|s| s.agent_persona.as_str())
                    .unwrap_or("assistant");

                buf.set_line(
                    inner.x + 3,
                    footer_y + 2,
                    &Line::from(vec![
                        Span::styled("  RO: ", theme::muted()),
                        Span::styled(persona.to_uppercase(), theme::dim()),
                    ]),
                    inner.width.saturating_sub(3),
                );

                buf.set_line(
                    inner.x + 3,
                    footer_y + 3,
                    &Line::from(vec![
                        Span::styled("  TR: ", theme::muted()),
                        Span::styled(self.app.autonomy_tier.to_uppercase(), theme::dim()),
                    ]),
                    inner.width.saturating_sub(3),
                );
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
