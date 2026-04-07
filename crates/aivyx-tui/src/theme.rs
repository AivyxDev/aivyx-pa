//! Kinetic/Stitch design system — terminal color palette.
//!
//! These colors are the ratatui equivalents of the CSS tokens defined in
//! `layout.css` for the SvelteKit GUI. Both UIs share the same visual identity.


use ratatui::style::{Color, Modifier, Style};

// ── Primary Palette ────────────────────────────────────────────

pub const PRIMARY: Color = Color::Rgb(255, 183, 125); // #FFB77D amber
pub const PRIMARY_DIM: Color = Color::Rgb(138, 80, 0); // #8A5000
pub const SECONDARY: Color = Color::Rgb(204, 193, 230); // #CCC1E6 lavender
pub const SAGE: Color = Color::Rgb(77, 139, 106); // #4D8B6A green
pub const ERROR: Color = Color::Rgb(255, 180, 171); // #FFB4AB soft red
pub const ACCENT_GLOW: Color = Color::Rgb(255, 214, 102); // #FFD666

// ── Surface / Background ───────────────────────────────────────

pub const BG: Color = Color::Rgb(19, 19, 25); // #131319 midnight
pub const SURFACE: Color = Color::Rgb(30, 30, 36); // #1E1E24
pub const SURFACE_HIGH: Color = Color::Rgb(53, 52, 59); // #35343B
pub const SURFACE_HIGHEST: Color = Color::Rgb(72, 70, 78); // #48464E

// ── Text ───────────────────────────────────────────────────────

pub const ON_SURFACE: Color = Color::Rgb(229, 225, 235); // #E5E1EB
pub const ON_SURFACE_DIM: Color = Color::Rgb(150, 143, 160); // muted text
pub const MUTED: Color = Color::Rgb(100, 95, 110); // very dim
pub const BORDER: Color = Color::Rgb(84, 67, 55); // #544337

// ── Semantic Styles ────────────────────────────────────────────

/// Primary accent text (amber).
pub fn primary() -> Style {
    Style::default().fg(PRIMARY)
}

/// Bold primary text.
pub fn primary_bold() -> Style {
    Style::default().fg(PRIMARY).add_modifier(Modifier::BOLD)
}

/// Secondary accent text (lavender).
pub fn secondary() -> Style {
    Style::default().fg(SECONDARY)
}

/// Normal body text.
pub fn text() -> Style {
    Style::default().fg(ON_SURFACE)
}

/// Bold body text.
pub fn text_bold() -> Style {
    Style::default().fg(ON_SURFACE).add_modifier(Modifier::BOLD)
}

/// Muted label text.
pub fn muted() -> Style {
    Style::default().fg(ON_SURFACE_DIM)
}

/// Very dim text (timestamps, decorative).
pub fn dim() -> Style {
    Style::default().fg(MUTED)
}

/// Success / healthy status.
pub fn sage() -> Style {
    Style::default().fg(SAGE)
}

/// Error / danger.
pub fn error() -> Style {
    Style::default().fg(ERROR)
}

/// Warning / attention.
pub fn warning() -> Style {
    Style::default().fg(ACCENT_GLOW)
}

/// Default block border.
pub fn border() -> Style {
    Style::default().fg(BORDER)
}

/// Active/highlighted border.
pub fn border_active() -> Style {
    Style::default().fg(PRIMARY)
}

/// Highlighted/selected item — inverted primary on surface.
pub fn highlight() -> Style {
    Style::default().fg(PRIMARY).add_modifier(Modifier::BOLD)
}

/// Status bar background.
pub fn status_bar() -> Style {
    Style::default().bg(SURFACE).fg(MUTED)
}
