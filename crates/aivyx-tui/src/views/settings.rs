//! Settings view — interactive configuration display from SettingsSnapshot.

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Widget},
};

use crate::app::{App, InputKind, SettingsPopup};
use crate::theme;

pub fn render(app: &App, area: Rect, buf: &mut Buffer) {
    let [header, body, help_bar] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(5),
        Constraint::Length(1),
    ])
    .areas(area);

    let title = Line::from(vec![
        Span::styled("Settings", theme::text_bold()),
        Span::styled("  Configure your personal assistant.", theme::dim()),
    ]);
    buf.set_line(header.x, header.y, &title, header.width);

    let Some(ref settings) = app.settings else {
        let msg = app.settings_error.as_deref().unwrap_or("Unknown error");
        // Truncate error to fit body width, wrapping to multiple lines if needed
        let max_w = (body.width as usize).saturating_sub(4);
        let full_msg = format!("Could not load settings: {msg}");
        let mut row = 1u16;
        for chunk in full_msg.as_bytes().chunks(max_w) {
            if row >= body.height {
                break;
            }
            let text = String::from_utf8_lossy(chunk);
            buf.set_line(
                body.x + 2,
                body.y + row,
                &Line::from(Span::styled(text.to_string(), theme::error())),
                body.width - 4,
            );
            row += 1;
        }
        row += 1;
        if row < body.height {
            let hint = Line::from(Span::styled(
                "Press 'e' to open config.toml in $EDITOR, or 'r' to reload after fixing.",
                theme::dim(),
            ));
            buf.set_line(body.x + 2, body.y + row, &hint, body.width - 4);
        }
        let help = Line::from(Span::styled(
            "e open editor  r reload  Tab sidebar",
            theme::dim(),
        ));
        buf.set_line(help_bar.x + 1, help_bar.y, &help, help_bar.width - 2);
        return;
    };

    // Two-column layout for settings cards
    let [left_col, right_col] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(body);

    let mut ly = left_col.y;
    let mut ry = right_col.y;
    // Only highlight cards when content area is focused — prevents confusion
    // when the sidebar is focused and the user hasn't entered the card area.
    let ci = if app.focus == crate::app::Focus::Content {
        app.settings_card_index
    } else {
        usize::MAX
    };

    // ── LLM Provider (left, card 0) ──────────────────────────
    ly = render_card(
        "LLM Provider",
        left_col.x,
        ly,
        left_col.width,
        6,
        ci == 0,
        buf,
        |inner, buf| {
            let sel = |row: usize| {
                if ci == 0 && app.settings_item_index == row {
                    theme::highlight()
                } else {
                    theme::text_bold()
                }
            };
            kv(
                buf,
                inner,
                0,
                "PROVIDER",
                &settings.provider_label,
                theme::text_bold(),
            );
            kv(buf, inner, 1, "MODEL", &settings.model_name, sel(0));
            let url_str = settings.provider_base_url.as_deref().unwrap_or("—");
            kv(
                buf,
                inner,
                2,
                "URL",
                url_str,
                if ci == 0 && app.settings_item_index == 1 {
                    theme::highlight()
                } else {
                    theme::dim()
                },
            );
            if let Some(ref embed) = settings.embedding_model {
                let dim_str = settings
                    .embedding_dimensions
                    .map(|d| format!(" ({d}d)"))
                    .unwrap_or_default();
                kv(
                    buf,
                    inner,
                    3,
                    "EMBEDDING",
                    &format!("{embed}{dim_str}"),
                    theme::dim(),
                );
            }
        },
    );

    // ── Autonomy (left, card 1) ──────────────────────────────
    ly = render_card(
        "Autonomy",
        left_col.x,
        ly,
        left_col.width,
        6,
        ci == 1,
        buf,
        |inner, buf| {
            let sel = |row: usize| {
                if ci == 1 && app.settings_item_index == row {
                    theme::highlight()
                } else {
                    theme::text()
                }
            };
            let tier_style = if ci == 1 && app.settings_item_index == 0 {
                theme::highlight()
            } else {
                theme::primary_bold()
            };
            let tier_display = if ci == 1 && app.settings_item_index == 0 {
                format!("◄ {} ►", settings.autonomy_tier)
            } else {
                settings.autonomy_tier.clone()
            };
            kv(buf, inner, 0, "TIER", &tier_display, tier_style);
            kv(
                buf,
                inner,
                1,
                "RATE LIMIT",
                &format!("{}/min", settings.max_tool_calls_per_min),
                sel(1),
            );
            kv(
                buf,
                inner,
                2,
                "MAX COST",
                &format!("${:.2}", settings.max_cost_usd),
                sel(2),
            );
            let approval_str = if settings.require_approval_destructive {
                "Required for destructive"
            } else {
                "Not required"
            };
            kv(buf, inner, 3, "APPROVAL", approval_str, theme::text());
        },
    );

    // ── Heartbeat (left, card 2) — interactive toggles ───────
    let hb_height = 14u16;
    ly = render_card(
        "Heartbeat",
        left_col.x,
        ly,
        left_col.width,
        hb_height,
        ci == 2,
        buf,
        |inner, buf| {
            let status = if settings.heartbeat_enabled {
                "Enabled"
            } else {
                "Disabled"
            };
            let status_style = if settings.heartbeat_enabled {
                theme::sage()
            } else {
                theme::dim()
            };

            let flags: &[(&str, bool)] = &[
                (status, settings.heartbeat_enabled),
                ("reflect", settings.heartbeat_can_reflect),
                ("consolidate", settings.heartbeat_can_consolidate),
                ("analyze failures", settings.heartbeat_can_analyze_failures),
                (
                    "extract knowledge",
                    settings.heartbeat_can_extract_knowledge,
                ),
                ("plan review", settings.heartbeat_can_plan_review),
                ("strategy review", settings.heartbeat_can_strategy_review),
                ("mood tracking", settings.heartbeat_can_track_mood),
                ("encouragement", settings.heartbeat_can_encourage),
                ("milestones", settings.heartbeat_can_track_milestones),
                (
                    "notification pacing",
                    settings.heartbeat_notification_pacing,
                ),
            ];

            for (i, (label, enabled)) in flags.iter().enumerate() {
                if i as u16 >= inner.height {
                    break;
                }
                let is_selected = ci == 2 && app.settings_item_index == i;
                // Use Stitch [■]/[○] style matching Genesis wizard
                let check = if *enabled { "[■]" } else { "[○]" };
                let marker_style = if *enabled {
                    theme::sage()
                } else {
                    theme::dim()
                };

                // First row is the master status toggle — render differently
                if i == 0 {
                    let line = if is_selected {
                        Line::from(vec![
                            Span::styled(format!("{check} "), marker_style),
                            Span::styled(format!("STATUS  {label}"), theme::highlight()),
                        ])
                    } else {
                        Line::from(vec![
                            Span::styled(format!("{check} "), marker_style),
                            Span::styled("STATUS  ", theme::muted()),
                            Span::styled(*label, status_style),
                        ])
                    };
                    buf.set_line(inner.x + 1, inner.y + i as u16, &line, inner.width - 2);
                } else {
                    let text_style = if is_selected {
                        theme::highlight()
                    } else {
                        theme::text()
                    };
                    let line = Line::from(vec![
                        Span::styled(format!("{check} "), marker_style),
                        Span::styled(*label, text_style),
                    ]);
                    buf.set_line(inner.x + 1, inner.y + i as u16, &line, inner.width - 2);
                }
            }

            // Interval on a separate line if room
            if 11 < inner.height {
                let interval_line = Line::from(vec![
                    Span::styled("    interval  ", theme::muted()),
                    Span::styled(format!("{}m", settings.heartbeat_interval), theme::dim()),
                ]);
                buf.set_line(inner.x + 1, inner.y + 11, &interval_line, inner.width - 2);
            }
        },
    );

    // ── Schedules (left, card 3) ─────────────────────────────
    if !settings.schedules.is_empty() && ly + 4 < left_col.y + left_col.height {
        let height = (settings.schedules.len() as u16 + 2).min(10);
        ly = render_card(
            "Schedules",
            left_col.x,
            ly,
            left_col.width,
            height,
            ci == 3,
            buf,
            |inner, buf| {
                let now = chrono::Utc::now();
                for (i, (name, cron_expr, enabled)) in settings.schedules.iter().enumerate() {
                    if i as u16 >= inner.height {
                        break;
                    }
                    let is_sel = ci == 3 && app.settings_item_index == i;
                    let marker = if *enabled { "●" } else { "○" };
                    let marker_style = if *enabled {
                        theme::sage()
                    } else {
                        theme::dim()
                    };
                    let name_style = if is_sel {
                        theme::highlight()
                    } else {
                        theme::text()
                    };

                    let next_fire = if *enabled {
                        croner::Cron::new(cron_expr)
                            .parse()
                            .ok()
                            .and_then(|c| c.find_next_occurrence(&now, false).ok())
                            .map(|t| {
                                let local = t.with_timezone(&chrono::Local);
                                if (t - now).num_hours() < 24 {
                                    format!("→ {}", local.format("%H:%M"))
                                } else {
                                    format!("→ {}", local.format("%a %H:%M"))
                                }
                            })
                            .unwrap_or_default()
                    } else {
                        "(disabled)".into()
                    };

                    let line = Line::from(vec![
                        Span::styled(format!("{marker} "), marker_style),
                        Span::styled(name, name_style),
                        Span::styled(format!("  {cron_expr}  "), theme::dim()),
                        Span::styled(
                            next_fire,
                            if *enabled {
                                theme::muted()
                            } else {
                                theme::dim()
                            },
                        ),
                    ]);
                    buf.set_line(inner.x + 1, inner.y + i as u16, &line, inner.width - 2);
                }
            },
        );
    }

    // ── Agent (right, card 4) ────────────────────────────────
    ry = render_card(
        "Agent",
        right_col.x,
        ry,
        right_col.width,
        6,
        ci == 4,
        buf,
        |inner, buf| {
            let sel = |row: usize| {
                if ci == 4 && app.settings_item_index == row {
                    theme::highlight()
                } else {
                    theme::text_bold()
                }
            };
            kv(buf, inner, 0, "NAME", &settings.agent_name, sel(0));
            let soul_label = if settings.has_custom_soul {
                "custom"
            } else {
                "default"
            };
            kv(
                buf,
                inner,
                1,
                "SOUL",
                soul_label,
                if ci == 4 && app.settings_item_index == 1 {
                    theme::highlight()
                } else {
                    theme::text()
                },
            );
            let skills_str = if settings.agent_skills.is_empty() {
                "none".into()
            } else {
                settings.agent_skills.join(", ")
            };
            kv(
                buf,
                inner,
                2,
                "SKILLS",
                &skills_str,
                if ci == 4 && app.settings_item_index == 2 {
                    theme::highlight()
                } else {
                    theme::dim()
                },
            );
            kv(
                buf,
                inner,
                3,
                "PERSONA",
                &settings.agent_persona,
                theme::dim(),
            );
        },
    );

    // ── Integrations (right, card 5) ─────────────────────────
    let integrations = crate::app::App::integrations_list(settings);
    let integ_height = if ci == 5 {
        (integrations.len() as u16 + 2).min(15)
    } else {
        6u16
    };
    ry = render_card(
        "Integrations",
        right_col.x,
        ry,
        right_col.width,
        integ_height,
        ci == 5,
        buf,
        |inner, buf| {
            if ci == 5 {
                // Vertical list with selection when card is focused
                for (i, (name, configured, _kind)) in integrations.iter().enumerate() {
                    if i as u16 >= inner.height {
                        break;
                    }
                    let is_sel = app.settings_item_index == i;
                    let check = if *configured { "[■]" } else { "[○]" };
                    let marker_style = if *configured {
                        theme::sage()
                    } else {
                        theme::dim()
                    };
                    let text_style = if is_sel {
                        theme::highlight()
                    } else {
                        theme::text()
                    };
                    let line = Line::from(vec![
                        Span::styled(format!("{check} "), marker_style),
                        Span::styled(*name, text_style),
                    ]);
                    buf.set_line(inner.x + 1, inner.y + i as u16, &line, inner.width - 2);
                }
            } else {
                // Compact multi-row display when not focused — wrap items to fit card width
                let usable = (inner.width as usize).saturating_sub(2);
                let mut row = 0u16;
                let mut col = 0usize;
                for (name, configured, _kind) in &integrations {
                    let (marker, style) = if *configured {
                        ("●", theme::sage())
                    } else {
                        ("○", theme::dim())
                    };
                    let item = format!("{marker} {name}");
                    let item_width = item.len() + 2; // +2 for spacing
                    if col > 0 && col + item_width > usable {
                        row += 1;
                        col = 0;
                    }
                    if row >= inner.height {
                        break;
                    }
                    let span = Span::styled(format!("{item}  "), style);
                    buf.set_line(
                        inner.x + 1 + col as u16,
                        inner.y + row,
                        &Line::from(vec![span]),
                        (usable - col) as u16,
                    );
                    col += item_width;
                }
            }
        },
    );

    // ── Memory Config (right, card 6) ────────────────────────
    ry = render_card(
        "Memory",
        right_col.x,
        ry,
        right_col.width,
        5,
        ci == 6,
        buf,
        |inner, buf| {
            kv(
                buf,
                inner,
                0,
                "MAX MEMORIES",
                &settings.max_memories.to_string(),
                theme::text(),
            );
            kv(
                buf,
                inner,
                1,
                "SESSION AGE",
                &format!("{}h", settings.session_max_age_hours),
                theme::text(),
            );
            kv(
                buf,
                inner,
                2,
                "GRAPH RECALL",
                &if settings.use_graph_recall {
                    "Enabled"
                } else {
                    "Disabled"
                }
                .to_string(),
                theme::text(),
            );
        },
    );

    // ── Persona (right, card 7) ──────────────────────────────
    if let Some(ref persona) = settings.persona_dimensions {
        if ry + 6 < right_col.y + right_col.height {
            ry = render_card(
                "Persona",
                right_col.x,
                ry,
                right_col.width,
                7,
                ci == 7,
                buf,
                |inner, buf| {
                    let dims = [
                        ("Formality", persona.formality),
                        ("Verbosity", persona.verbosity),
                        ("Warmth", persona.warmth),
                        ("Humor", persona.humor),
                        ("Confidence", persona.confidence),
                    ];
                    for (i, (name, val)) in dims.iter().enumerate() {
                        if i as u16 >= inner.height {
                            break;
                        }
                        let is_sel = ci == 7 && app.settings_item_index == i;
                        let bar_w = 10;
                        let filled = (*val * bar_w as f32) as usize;
                        // High-contrast █/░ matching goals/missions
                        let bar = format!("{}{}", "█".repeat(filled), "░".repeat(bar_w - filled));
                        let name_style = if is_sel {
                            theme::highlight()
                        } else {
                            theme::muted()
                        };
                        let line = if is_sel {
                            Line::from(vec![
                                Span::styled(format!("{name:<12}"), name_style),
                                Span::styled("◄ ", theme::primary()),
                                Span::styled(&bar, theme::primary()),
                                Span::styled(" ►", theme::primary()),
                                Span::styled(format!(" {:.1}", val), theme::text_bold()),
                            ])
                        } else {
                            Line::from(vec![
                                Span::styled(format!("{name:<12}"), name_style),
                                Span::styled(bar, theme::primary()),
                                Span::styled(format!(" {:.1}", val), theme::dim()),
                            ])
                        };
                        buf.set_line(inner.x + 1, inner.y + i as u16, &line, inner.width - 2);
                    }
                },
            );
        }
    }

    // ── Tools & Extensions (right, card 8) ────────────────────
    if ry + 4 < right_col.y + right_col.height {
        let mcp_count = settings.mcp_servers.len();
        let tool_disc = settings.tool_discovery_mode.as_deref().unwrap_or("off");
        let card_height = (3 + mcp_count as u16).max(4).min(10);
        ry = render_card(
            "Tools & Extensions",
            right_col.x,
            ry,
            right_col.width,
            card_height,
            ci == 8,
            buf,
            |inner, buf| {
                kv(
                    buf,
                    inner,
                    0,
                    "TOOLS",
                    &format!("{} registered", app.tool_count),
                    theme::text_bold(),
                );
                let disc_style = if ci == 8 && app.settings_item_index == 0 {
                    theme::highlight()
                } else {
                    theme::text()
                };
                kv(buf, inner, 1, "DISCOVERY", tool_disc, disc_style);

                if mcp_count == 0 {
                    kv(buf, inner, 2, "MCP", "no servers configured", theme::dim());
                } else {
                    for (i, name) in settings.mcp_servers.iter().enumerate() {
                        let row = 2 + i;
                        if row as u16 >= inner.height {
                            break;
                        }
                        let label = if i == 0 { "MCP" } else { "" };
                        kv(buf, inner, row as u16, label, name, theme::text());
                    }
                }
            },
        );
    }

    if ly + 4 < left_col.y + left_col.height {
        let app_count = settings.desktop_app_access.len();
        let visible = if ci == 9 { app_count.min(12) } else { 3 };
        let card_height = (visible as u16 + 2).max(4);
        ly = render_card(
            "Applications",
            left_col.x,
            ly,
            left_col.width,
            card_height,
            ci == 9,
            buf,
            |inner, buf| {
                if app_count == 0 {
                    let line = Line::from(Span::styled(
                        "  No apps detected. Enter to scan.",
                        theme::dim(),
                    ));
                    buf.set_line(inner.x, inner.y, &line, inner.width);
                } else if ci == 9 {
                    // Scrollable list with access level display
                    let scroll_offset = app
                        .settings_item_index
                        .saturating_sub(inner.height.saturating_sub(1) as usize);
                    for (i, (_bin, display_name, access)) in settings
                        .desktop_app_access
                        .iter()
                        .enumerate()
                        .skip(scroll_offset)
                    {
                        let row = (i - scroll_offset) as u16;
                        if row >= inner.height {
                            break;
                        }
                        let is_sel = app.settings_item_index == i;

                        let access_style = match access.as_str() {
                            "Blocked" => theme::error(),
                            "View Only" => Style::default().fg(theme::ACCENT_GLOW),
                            "Interact" => theme::sage(),
                            "Full" => theme::primary_bold(),
                            _ => theme::text(),
                        };
                        let name_style = if is_sel {
                            theme::highlight()
                        } else {
                            theme::text()
                        };

                        // Truncate name to fit
                        let max_name = (inner.width as usize).saturating_sub(14);
                        let name_display = if display_name.len() > max_name {
                            format!("{}…", &display_name[..max_name.saturating_sub(1)])
                        } else {
                            display_name.clone()
                        };

                        let line = if is_sel {
                            Line::from(vec![
                                Span::styled("◄ ", theme::primary()),
                                Span::styled(
                                    format!("{:<width$}", name_display, width = max_name),
                                    name_style,
                                ),
                                Span::styled(format!(" {access} "), access_style),
                                Span::styled("►", theme::primary()),
                            ])
                        } else {
                            let marker = match access.as_str() {
                                "Blocked" => "⊘",
                                "View Only" => "◐",
                                "Interact" => "◉",
                                "Full" | _ => "●",
                            };
                            Line::from(vec![
                                Span::styled(format!("{marker} "), access_style),
                                Span::styled(
                                    format!("{:<width$}", name_display, width = max_name),
                                    name_style,
                                ),
                                Span::styled(format!(" {access}"), theme::dim()),
                            ])
                        };
                        buf.set_line(inner.x + 1, inner.y + row, &line, inner.width - 2);
                    }
                } else {
                    // Compact summary when not focused
                    let counts: std::collections::HashMap<&str, usize> =
                        settings.desktop_app_access.iter().fold(
                            std::collections::HashMap::new(),
                            |mut acc, (_, _, access)| {
                                *acc.entry(access.as_str()).or_insert(0) += 1;
                                acc
                            },
                        );
                    let summary = format!(
                        "{} apps  {} full  {} interact  {} view  {} blocked",
                        app_count,
                        counts.get("Full").unwrap_or(&0),
                        counts.get("Interact").unwrap_or(&0),
                        counts.get("View Only").unwrap_or(&0),
                        counts.get("Blocked").unwrap_or(&0),
                    );
                    let line = Line::from(Span::styled(summary, theme::text()));
                    buf.set_line(inner.x + 1, inner.y, &line, inner.width - 2);
                }
            },
        );
    }

    // Suppress unused variable warnings
    let _ = (ly, ry);

    // ── Help bar ─────────────────────────────────────────────
    let content_focused = app.focus == crate::app::Focus::Content;
    let help_text = if app.settings_popup.is_some() {
        match &app.settings_popup {
            Some(SettingsPopup::MultiLineInput { .. }) => "Ctrl+S save  Esc cancel",
            Some(SettingsPopup::SkillManager { .. }) => {
                "Enter add/done  d remove  ↑↓ select  Esc close"
            }
            Some(SettingsPopup::IntegrationSetup { .. }) => {
                "Tab next field  Enter save  Esc cancel"
            }
            Some(SettingsPopup::Confirm { .. }) => "y confirm  n/Esc cancel",
            _ => "Enter confirm  Esc cancel",
        }
    } else if !content_focused {
        "Tab or → to edit settings cards  ↑↓ sidebar"
    } else if ci == 5 && app.settings_item_count(5) > 0 {
        "Enter setup  d remove  ↑↓ navigate"
    } else if ci == 9 && app.settings_item_count(9) > 0 {
        "←→ change access  ↑↓ navigate  Tab sidebar"
    } else if ci == 7 && app.settings_item_count(7) > 0 {
        "←→ adjust  ↑↓ navigate  Tab sidebar"
    } else if app.settings_item_count(ci) > 0 {
        "↑↓ navigate  Enter edit  Tab sidebar"
    } else {
        "↑↓ cards  Tab sidebar"
    };
    let help = Line::from(vec![Span::styled(help_text, theme::dim())]);
    buf.set_line(help_bar.x + 1, help_bar.y, &help, help_bar.width - 2);

    // ── Popup overlay ────────────────────────────────────────
    if let Some(ref popup) = app.settings_popup {
        render_popup(popup, app.frame_count, area, buf);
    }
}

// ── Helpers ───────────────────────────────────────────────────

/// Render a bordered card. When `selected`, the border uses the primary color.
fn render_card(
    title: &str,
    x: u16,
    y: u16,
    width: u16,
    height: u16,
    selected: bool,
    buf: &mut Buffer,
    body_fn: impl FnOnce(Rect, &mut Buffer),
) -> u16 {
    let border_style = if selected {
        theme::primary()
    } else {
        theme::border()
    };
    let title_style = if selected {
        theme::primary_bold()
    } else {
        theme::text_bold()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(border_style)
        .title(Line::from(Span::styled(
            format!("[ {} ]", title.to_uppercase()),
            title_style,
        )));
    let card = Rect::new(x, y, width, height);
    let inner = block.inner(card);
    block.render(card, buf);
    body_fn(inner, buf);
    y + height + 1
}

/// Render a key-value line inside a card inner area at the given row offset.
fn kv(
    buf: &mut Buffer,
    inner: Rect,
    row: u16,
    label: &str,
    value: &str,
    style: ratatui::style::Style,
) {
    if row >= inner.height {
        return;
    }
    let line = Line::from(vec![
        Span::styled(format!("{label:<14}"), theme::muted()),
        Span::styled(value, style),
    ]);
    buf.set_line(inner.x + 1, inner.y + row, &line, inner.width - 2);
}

/// Centered popup rect within the given area.
fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect::new(x, y, width.min(area.width), height.min(area.height))
}

/// Render a settings popup overlay.
fn render_popup(popup: &SettingsPopup, frame_count: u64, area: Rect, buf: &mut Buffer) {
    let cursor_visible = frame_count % 60 < 30;
    let cursor_char = if cursor_visible { "█" } else { " " };

    match popup {
        SettingsPopup::TextInput {
            title, value, kind, ..
        } => {
            let rect = centered_rect(50, 5, area);
            Clear.render(rect, buf);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Plain)
                .border_style(theme::primary())
                .title(Line::from(Span::styled(
                    format!("[ {} ]", title.to_uppercase()),
                    theme::primary_bold(),
                )));
            let inner = block.inner(rect);
            block.render(rect, buf);

            let hint = match kind {
                InputKind::UInt => " (number)",
                InputKind::Float => " (decimal)",
                InputKind::String => "",
            };
            let input_line = Line::from(vec![
                Span::styled("> ", theme::primary()),
                Span::styled(value, theme::text_bold()),
                Span::styled(cursor_char, theme::primary()),
                Span::styled(hint, theme::dim()),
            ]);
            buf.set_line(inner.x + 1, inner.y + 1, &input_line, inner.width - 2);
        }
        SettingsPopup::MultiLineInput {
            title,
            lines,
            cursor_row,
            cursor_col,
            ..
        } => {
            let h = (lines.len() as u16 + 4).min(20).max(8);
            let rect = centered_rect(60, h, area);
            Clear.render(rect, buf);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Plain)
                .border_style(theme::primary())
                .title(Line::from(Span::styled(
                    format!("[ {} ]", title.to_uppercase()),
                    theme::primary_bold(),
                )));
            let inner = block.inner(rect);
            block.render(rect, buf);

            for (i, line_text) in lines.iter().enumerate() {
                if i as u16 >= inner.height {
                    break;
                }
                let is_cursor_line = i == *cursor_row;
                if is_cursor_line && cursor_visible {
                    let (before, after) = if *cursor_col <= line_text.len() {
                        (&line_text[..*cursor_col], &line_text[*cursor_col..])
                    } else {
                        (line_text.as_str(), "")
                    };
                    let line = Line::from(vec![
                        Span::styled(before, theme::text()),
                        Span::styled("█", theme::primary()),
                        Span::styled(after, theme::text()),
                    ]);
                    buf.set_line(inner.x + 1, inner.y + i as u16, &line, inner.width - 2);
                } else {
                    let line = Line::from(Span::styled(line_text, theme::text()));
                    buf.set_line(inner.x + 1, inner.y + i as u16, &line, inner.width - 2);
                }
            }
        }
        SettingsPopup::Confirm { message, .. } => {
            let rect = centered_rect(50, 5, area);
            Clear.render(rect, buf);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Plain)
                .border_style(theme::primary())
                .title(Line::from(Span::styled(
                    "[ CONFIRM ]",
                    theme::primary_bold(),
                )));
            let inner = block.inner(rect);
            block.render(rect, buf);

            let msg = Line::from(Span::styled(message, theme::text()));
            buf.set_line(inner.x + 1, inner.y, &msg, inner.width - 2);
            let hint = Line::from(vec![
                Span::styled("[Y]", theme::primary_bold()),
                Span::styled(" YES  ", theme::text()),
                Span::styled("[N]", theme::primary_bold()),
                Span::styled(" NO", theme::text()),
            ]);
            buf.set_line(inner.x + 1, inner.y + 2, &hint, inner.width - 2);
        }
        SettingsPopup::SkillManager {
            input,
            selected,
            skills,
        } => {
            let h = (skills.len() as u16 + 5).min(16).max(7);
            let rect = centered_rect(50, h, area);
            Clear.render(rect, buf);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Plain)
                .border_style(theme::primary())
                .title(Line::from(Span::styled(
                    "[ SKILLS ]",
                    theme::primary_bold(),
                )));
            let inner = block.inner(rect);
            block.render(rect, buf);

            let input_line = Line::from(vec![
                Span::styled("Add: ", theme::muted()),
                Span::styled(input, theme::text_bold()),
                Span::styled(cursor_char, theme::primary()),
            ]);
            buf.set_line(inner.x + 1, inner.y, &input_line, inner.width - 2);

            for (i, skill) in skills.iter().enumerate() {
                if (i + 1) as u16 >= inner.height {
                    break;
                }
                let is_sel = *selected == i;
                let marker = if is_sel { ">" } else { " " };
                let style = if is_sel {
                    theme::highlight()
                } else {
                    theme::text()
                };
                let line = Line::from(vec![
                    Span::styled(format!("{marker} "), theme::primary()),
                    Span::styled(skill, style),
                ]);
                buf.set_line(inner.x + 1, inner.y + 1 + i as u16, &line, inner.width - 2);
            }
        }
        SettingsPopup::IntegrationSetup {
            fields,
            focused,
            is_configured,
            ..
        } => {
            let h = (fields.len() as u16 + 3).min(14);
            let rect = centered_rect(55, h, area);
            Clear.render(rect, buf);
            let title_text = if *is_configured {
                "[ RECONFIGURE ]"
            } else {
                "[ SETUP INTEGRATION ]"
            };
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Plain)
                .border_style(theme::primary())
                .title(Line::from(Span::styled(title_text, theme::primary_bold())));
            let inner = block.inner(rect);
            block.render(rect, buf);

            for (i, field) in fields.iter().enumerate() {
                if i as u16 >= inner.height {
                    break;
                }
                let is_focused = *focused == i;
                let label_style = if is_focused {
                    theme::highlight()
                } else {
                    theme::muted()
                };
                let val_display = if field.is_secret {
                    // Mask secret fields with dots
                    if field.value.is_empty() {
                        if is_focused {
                            cursor_char.to_string()
                        } else {
                            "(enter to set)".into()
                        }
                    } else if is_focused {
                        format!("{}{cursor_char}", "●".repeat(field.value.len()))
                    } else {
                        "●".repeat(field.value.len())
                    }
                } else if is_focused {
                    format!("{}{cursor_char}", field.value)
                } else {
                    field.value.clone()
                };
                let val_style = if field.is_secret && field.value.is_empty() && !is_focused {
                    theme::dim()
                } else {
                    theme::text()
                };
                let line = Line::from(vec![
                    Span::styled(format!("{:<14}", field.label), label_style),
                    Span::styled(val_display, val_style),
                ]);
                buf.set_line(inner.x + 1, inner.y + i as u16, &line, inner.width - 2);
            }
        }
    }
}
