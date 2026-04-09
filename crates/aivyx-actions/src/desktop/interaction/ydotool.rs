#![allow(
    unsafe_op_in_unsafe_fn,
    unused_imports,
    unreachable_code,
    unused_variables,
    dead_code,
    clippy::all
)]
//! ydotool backend — universal Wayland-compatible input injection.
//!
//! Uses the `ydotool` CLI (requires `ydotoold` daemon) for coordinate-based
//! clicks, keyboard input, and key combos. Works on both X11 and Wayland.
//! This is the fallback backend when AT-SPI2/CDP are unavailable.

use aivyx_core::{AivyxError, Result};

use super::{
    ElementQuery, InputBackend, ScrollDirection, UiBackend, UiElement, UiTreeNode, WindowRef,
};

/// ydotool-based input backend. Always available (subprocess, no crate deps).
pub struct YdotoolBackend;

impl Default for YdotoolBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl YdotoolBackend {
    pub fn new() -> Self {
        Self
    }

    /// Send a key combination via ydotool (e.g., "ctrl+s", "alt+F4").
    ///
    /// Parses modifier+key combos and translates to ydotool key codes.
    pub async fn key_combo(&self, keys: &str) -> Result<()> {
        let ydotool_keys = parse_key_combo(keys)?;
        run_ydotool(&["key", &ydotool_keys]).await?;
        Ok(())
    }

    /// Click at absolute screen coordinates.
    pub async fn click_at(&self, x: i32, y: i32) -> Result<()> {
        run_ydotool(&[
            "click",
            "--point",
            &format!("{x}:{y}"),
            "0xC0", // left button click (down+up)
        ])
        .await?;
        Ok(())
    }

    /// Type a string via ydotool keyboard simulation.
    pub async fn type_string(&self, text: &str) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        run_ydotool(&["type", "--", text]).await?;
        Ok(())
    }

    /// Double-click at absolute screen coordinates.
    pub async fn double_click_at(&self, x: i32, y: i32) -> Result<()> {
        // Two rapid left clicks. ydotool supports --repeat but using two
        // explicit clicks with a short delay is more reliable across compositors.
        run_ydotool(&["click", "--point", &format!("{x}:{y}"), "0xC0"]).await?;
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        run_ydotool(&["click", "--point", &format!("{x}:{y}"), "0xC0"]).await?;
        Ok(())
    }

    /// Middle-click at absolute screen coordinates.
    pub async fn middle_click_at(&self, x: i32, y: i32) -> Result<()> {
        run_ydotool(&[
            "click",
            "--point",
            &format!("{x}:{y}"),
            "0xC2", // middle button click (down+up)
        ])
        .await?;
        Ok(())
    }

    /// Ctrl+click at multiple coordinates (for multi-select in file managers, lists).
    ///
    /// Holds Ctrl down, clicks each position in sequence, then releases Ctrl.
    pub async fn multi_click_at(&self, positions: &[(i32, i32)]) -> Result<()> {
        if positions.is_empty() {
            return Err(AivyxError::Validation(
                "At least one position is required for multi-click".into(),
            ));
        }

        // Press Ctrl (key code 29 = KEY_LEFTCTRL).
        // ydotool key uses format: "keycode:state" where state 1=down, 0=up.
        run_ydotool(&["key", "29:1"]).await?;
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;

        for (x, y) in positions {
            self.click_at(*x, *y).await?;
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        // Release Ctrl.
        run_ydotool(&["key", "29:0"]).await?;
        Ok(())
    }

    /// Right-click at absolute screen coordinates.
    pub async fn right_click_at(&self, x: i32, y: i32) -> Result<()> {
        run_ydotool(&[
            "click",
            "--point",
            &format!("{x}:{y}"),
            "0xC1", // right button click (down+up)
        ])
        .await?;
        Ok(())
    }

    /// Scroll the mouse wheel.
    ///
    /// ydotool uses `mousemove --wheel` with positive=down, negative=up.
    /// For horizontal scrolling, we use `--hwheel`.
    pub async fn scroll(&self, direction: ScrollDirection, amount: u32) -> Result<()> {
        let amount_i = amount as i32;
        match direction {
            ScrollDirection::Up => {
                run_ydotool(&["mousemove", "--wheel", &format!("-{amount_i}")]).await?;
            }
            ScrollDirection::Down => {
                run_ydotool(&["mousemove", "--wheel", &format!("{amount_i}")]).await?;
            }
            ScrollDirection::Left => {
                run_ydotool(&["mousemove", "--hwheel", &format!("-{amount_i}")]).await?;
            }
            ScrollDirection::Right => {
                run_ydotool(&["mousemove", "--hwheel", &format!("{amount_i}")]).await?;
            }
        }
        Ok(())
    }

    /// Move the mouse to absolute screen coordinates.
    pub async fn mouse_move_to(&self, x: i32, y: i32) -> Result<()> {
        run_ydotool(&["mousemove", "--absolute", &format!("{x}"), &format!("{y}")]).await?;
        Ok(())
    }

    /// Drag from one position to another (left-button hold + move + release).
    pub async fn drag(&self, from_x: i32, from_y: i32, to_x: i32, to_y: i32) -> Result<()> {
        // Move to start position.
        self.mouse_move_to(from_x, from_y).await?;
        // Small delay for position to register.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        // Press left button down (0x40 = left button down only).
        run_ydotool(&["click", "0x40"]).await?;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        // Move to destination.
        self.mouse_move_to(to_x, to_y).await?;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        // Release left button (0x80 = left button up only).
        run_ydotool(&["click", "0x80"]).await?;
        Ok(())
    }
}

// ── InputBackend impl (cross-platform input injection trait) ────

#[async_trait::async_trait]
impl InputBackend for YdotoolBackend {
    async fn key_combo(&self, keys: &str) -> Result<()> {
        YdotoolBackend::key_combo(self, keys).await
    }

    async fn type_string(&self, text: &str) -> Result<()> {
        YdotoolBackend::type_string(self, text).await
    }

    async fn click_at(&self, x: i32, y: i32) -> Result<()> {
        YdotoolBackend::click_at(self, x, y).await
    }

    async fn double_click_at(&self, x: i32, y: i32) -> Result<()> {
        YdotoolBackend::double_click_at(self, x, y).await
    }

    async fn middle_click_at(&self, x: i32, y: i32) -> Result<()> {
        YdotoolBackend::middle_click_at(self, x, y).await
    }

    async fn right_click_at(&self, x: i32, y: i32) -> Result<()> {
        YdotoolBackend::right_click_at(self, x, y).await
    }

    async fn mouse_move_to(&self, x: i32, y: i32) -> Result<()> {
        YdotoolBackend::mouse_move_to(self, x, y).await
    }

    async fn scroll(&self, direction: ScrollDirection, amount: u32) -> Result<()> {
        YdotoolBackend::scroll(self, direction, amount).await
    }

    async fn drag(&self, from_x: i32, from_y: i32, to_x: i32, to_y: i32) -> Result<()> {
        YdotoolBackend::drag(self, from_x, from_y, to_x, to_y).await
    }

    async fn multi_click_at(&self, positions: &[(i32, i32)]) -> Result<()> {
        YdotoolBackend::multi_click_at(self, positions).await
    }
}

// ── UiBackend impl ─────────────────────────────────────────────

#[async_trait::async_trait]
impl UiBackend for YdotoolBackend {
    fn name(&self) -> &str {
        "ydotool"
    }

    async fn inspect(&self, _window: &WindowRef, _max_depth: u32) -> Result<Vec<UiTreeNode>> {
        Err(AivyxError::Other(
            "ydotool cannot inspect UI trees — enable the accessibility feature for AT-SPI2 support"
                .into(),
        ))
    }

    async fn find_element(
        &self,
        _window: &WindowRef,
        _query: &ElementQuery,
    ) -> Result<Vec<UiElement>> {
        Err(AivyxError::Other(
            "ydotool cannot find UI elements — enable the accessibility feature for AT-SPI2 support"
                .into(),
        ))
    }

    async fn click(&self, element: &UiElement) -> Result<()> {
        let bounds = element.bounds.ok_or_else(|| {
            AivyxError::Other("ydotool click requires element bounds (x, y, width, height)".into())
        })?;
        // Click center of the element.
        let cx = bounds[0] + bounds[2] / 2;
        let cy = bounds[1] + bounds[3] / 2;
        self.click_at(cx, cy).await
    }

    async fn type_text(&self, _element: Option<&UiElement>, text: &str) -> Result<()> {
        // ydotool types into whatever is currently focused.
        self.type_string(text).await
    }

    async fn read_text(&self, _element: &UiElement) -> Result<String> {
        Err(AivyxError::Other(
            "ydotool cannot read text from UI elements — enable the accessibility feature".into(),
        ))
    }

    async fn double_click(&self, element: &UiElement) -> Result<()> {
        let bounds = element.bounds.ok_or_else(|| {
            AivyxError::Other(
                "ydotool double-click requires element bounds (x, y, width, height)".into(),
            )
        })?;
        let cx = bounds[0] + bounds[2] / 2;
        let cy = bounds[1] + bounds[3] / 2;
        self.double_click_at(cx, cy).await
    }

    async fn middle_click(&self, element: &UiElement) -> Result<()> {
        let bounds = element.bounds.ok_or_else(|| {
            AivyxError::Other(
                "ydotool middle-click requires element bounds (x, y, width, height)".into(),
            )
        })?;
        let cx = bounds[0] + bounds[2] / 2;
        let cy = bounds[1] + bounds[3] / 2;
        self.middle_click_at(cx, cy).await
    }

    async fn scroll(
        &self,
        _window: &WindowRef,
        direction: ScrollDirection,
        amount: u32,
    ) -> Result<()> {
        YdotoolBackend::scroll(self, direction, amount).await
    }

    async fn right_click(&self, element: &UiElement) -> Result<()> {
        let bounds = element.bounds.ok_or_else(|| {
            AivyxError::Other(
                "ydotool right-click requires element bounds (x, y, width, height)".into(),
            )
        })?;
        let cx = bounds[0] + bounds[2] / 2;
        let cy = bounds[1] + bounds[3] / 2;
        self.right_click_at(cx, cy).await
    }

    async fn hover(&self, element: &UiElement) -> Result<()> {
        let bounds = element.bounds.ok_or_else(|| {
            AivyxError::Other("ydotool hover requires element bounds (x, y, width, height)".into())
        })?;
        let cx = bounds[0] + bounds[2] / 2;
        let cy = bounds[1] + bounds[3] / 2;
        self.mouse_move_to(cx, cy).await
    }

    async fn drag(&self, from: &UiElement, to: &UiElement) -> Result<()> {
        let from_bounds = from.bounds.ok_or_else(|| {
            AivyxError::Other("ydotool drag requires source element bounds".into())
        })?;
        let to_bounds = to.bounds.ok_or_else(|| {
            AivyxError::Other("ydotool drag requires destination element bounds".into())
        })?;
        let from_cx = from_bounds[0] + from_bounds[2] / 2;
        let from_cy = from_bounds[1] + from_bounds[3] / 2;
        let to_cx = to_bounds[0] + to_bounds[2] / 2;
        let to_cy = to_bounds[1] + to_bounds[3] / 2;
        YdotoolBackend::drag(self, from_cx, from_cy, to_cx, to_cy).await
    }

    async fn mouse_move(&self, x: i32, y: i32) -> Result<()> {
        self.mouse_move_to(x, y).await
    }
}

// ── Key combo parsing ────────────────────────────────────────────

/// Parse a human-readable key combo (e.g., "ctrl+s") into ydotool key spec.
///
/// ydotool uses Linux input event codes. Key combos are expressed as
/// sequences of down/up events: `29:1 31:1 31:0 29:0` for Ctrl+S.
fn parse_key_combo(combo: &str) -> Result<String> {
    let parts: Vec<&str> = combo.split('+').map(|s| s.trim()).collect();
    if parts.is_empty() {
        return Err(AivyxError::Other("empty key combo".into()));
    }

    let mut codes = Vec::new();
    for part in &parts {
        let code = key_name_to_code(part)
            .ok_or_else(|| AivyxError::Other(format!("unknown key: '{part}'")))?;
        codes.push(code);
    }

    // Generate down events for all keys, then up in reverse order.
    let mut spec = String::new();
    for &code in &codes {
        if !spec.is_empty() {
            spec.push(' ');
        }
        spec.push_str(&format!("{code}:1")); // key down
    }
    for &code in codes.iter().rev() {
        spec.push(' ');
        spec.push_str(&format!("{code}:0")); // key up
    }
    Ok(spec)
}

/// Map a key name to a Linux input event code.
fn key_name_to_code(name: &str) -> Option<u16> {
    let lower = name.to_lowercase();
    match lower.as_str() {
        // Modifiers
        "ctrl" | "control" | "lctrl" => Some(29),
        "rctrl" => Some(97),
        "shift" | "lshift" => Some(42),
        "rshift" => Some(54),
        "alt" | "lalt" => Some(56),
        "ralt" | "altgr" => Some(100),
        "super" | "meta" | "win" | "lmeta" => Some(125),
        "rmeta" | "rsuper" => Some(126),

        // Letters
        "a" => Some(30),
        "b" => Some(48),
        "c" => Some(46),
        "d" => Some(32),
        "e" => Some(18),
        "f" => Some(33),
        "g" => Some(34),
        "h" => Some(35),
        "i" => Some(23),
        "j" => Some(36),
        "k" => Some(37),
        "l" => Some(38),
        "m" => Some(50),
        "n" => Some(49),
        "o" => Some(24),
        "p" => Some(25),
        "q" => Some(16),
        "r" => Some(19),
        "s" => Some(31),
        "t" => Some(20),
        "u" => Some(22),
        "v" => Some(47),
        "w" => Some(17),
        "x" => Some(45),
        "y" => Some(21),
        "z" => Some(44),

        // Numbers
        "0" => Some(11),
        "1" => Some(2),
        "2" => Some(3),
        "3" => Some(4),
        "4" => Some(5),
        "5" => Some(6),
        "6" => Some(7),
        "7" => Some(8),
        "8" => Some(9),
        "9" => Some(10),

        // Function keys
        "f1" => Some(59),
        "f2" => Some(60),
        "f3" => Some(61),
        "f4" => Some(62),
        "f5" => Some(63),
        "f6" => Some(64),
        "f7" => Some(65),
        "f8" => Some(66),
        "f9" => Some(67),
        "f10" => Some(68),
        "f11" => Some(87),
        "f12" => Some(88),

        // Special keys
        "esc" | "escape" => Some(1),
        "tab" => Some(15),
        "enter" | "return" => Some(28),
        "space" | " " => Some(57),
        "backspace" | "bs" => Some(14),
        "delete" | "del" => Some(111),
        "insert" | "ins" => Some(110),
        "home" => Some(102),
        "end" => Some(107),
        "pageup" | "pgup" => Some(104),
        "pagedown" | "pgdn" => Some(109),
        "up" => Some(103),
        "down" => Some(108),
        "left" => Some(105),
        "right" => Some(106),
        "capslock" => Some(58),
        "printscreen" | "prtsc" => Some(99),
        "scrolllock" => Some(70),
        "pause" | "break" => Some(119),

        // Punctuation
        "minus" | "-" => Some(12),
        "equal" | "=" => Some(13),
        "leftbrace" | "[" => Some(26),
        "rightbrace" | "]" => Some(27),
        "semicolon" | ";" => Some(39),
        "apostrophe" | "'" => Some(40),
        "grave" | "`" => Some(41),
        "backslash" | "\\" => Some(43),
        "comma" | "," => Some(51),
        "dot" | "period" | "." => Some(52),
        "slash" | "/" => Some(53),

        _ => None,
    }
}

// ── Subprocess runner ────────────────────────────────────────────

/// Run ydotool with the given arguments. Returns error if ydotoold isn't running.
async fn run_ydotool(args: &[&str]) -> Result<String> {
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::process::Command::new("ydotool").args(args).output(),
    )
    .await;

    match result {
        Ok(Ok(output)) => {
            if output.status.success() {
                Ok(String::from_utf8_lossy(&output.stdout).into_owned())
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                if stderr.contains("Failed to connect")
                    || stderr.contains("No such file or directory")
                {
                    Err(AivyxError::Other(
                        "ydotoold daemon is not running. Start it with: ydotoold &".into(),
                    ))
                } else {
                    Err(AivyxError::Other(format!("ydotool failed: {stderr}")))
                }
            }
        }
        Ok(Err(e)) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                Err(AivyxError::Other(
                    "ydotool is not installed. Install it with your package manager \
                     (e.g., sudo apt install ydotool)"
                        .into(),
                ))
            } else {
                Err(AivyxError::Other(format!("failed to run ydotool: {e}")))
            }
        }
        Err(_) => Err(AivyxError::Other("ydotool timed out after 5s".into())),
    }
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ctrl_s() {
        let spec = parse_key_combo("ctrl+s").unwrap();
        // Ctrl=29, S=31: down-down, up-up (reverse)
        assert_eq!(spec, "29:1 31:1 31:0 29:0");
    }

    #[test]
    fn parse_alt_f4() {
        let spec = parse_key_combo("alt+F4").unwrap();
        // Alt=56, F4=62
        assert_eq!(spec, "56:1 62:1 62:0 56:0");
    }

    #[test]
    fn parse_single_key() {
        let spec = parse_key_combo("escape").unwrap();
        assert_eq!(spec, "1:1 1:0");
    }

    #[test]
    fn parse_ctrl_shift_t() {
        let spec = parse_key_combo("ctrl+shift+t").unwrap();
        // Ctrl=29, Shift=42, T=20
        assert_eq!(spec, "29:1 42:1 20:1 20:0 42:0 29:0");
    }

    #[test]
    fn parse_super_key() {
        let spec = parse_key_combo("super").unwrap();
        assert_eq!(spec, "125:1 125:0");
    }

    #[test]
    fn parse_unknown_key_fails() {
        assert!(parse_key_combo("ctrl+unicorn").is_err());
    }

    #[test]
    fn parse_empty_fails() {
        assert!(parse_key_combo("").is_err());
    }

    #[test]
    fn scroll_directions() {
        // Verify ScrollDirection variants exist and parse.
        use super::super::ScrollDirection;
        assert_eq!(ScrollDirection::parse("up").unwrap(), ScrollDirection::Up);
        assert_eq!(
            ScrollDirection::parse("down").unwrap(),
            ScrollDirection::Down
        );
        assert_eq!(
            ScrollDirection::parse("left").unwrap(),
            ScrollDirection::Left
        );
        assert_eq!(
            ScrollDirection::parse("right").unwrap(),
            ScrollDirection::Right
        );
    }

    #[test]
    fn click_button_codes() {
        assert_eq!(0xC0u32, 192); // left click (down+up)
        assert_eq!(0xC1u32, 193); // right click (down+up)
        assert_eq!(0xC2u32, 194); // middle click (down+up)
    }

    #[test]
    fn drag_button_codes() {
        // 0x40 = 64 = left button down only.
        assert_eq!(0x40u32, 64);
        // 0x80 = 128 = left button up only.
        assert_eq!(0x80u32, 128);
    }

    #[test]
    fn key_code_coverage() {
        // Verify all letters map
        for c in 'a'..='z' {
            assert!(
                key_name_to_code(&c.to_string()).is_some(),
                "missing key code for '{c}'"
            );
        }
        // Verify all digits map
        for d in '0'..='9' {
            assert!(
                key_name_to_code(&d.to_string()).is_some(),
                "missing key code for '{d}'"
            );
        }
        // Verify F1-F12
        for i in 1..=12 {
            assert!(
                key_name_to_code(&format!("f{i}")).is_some(),
                "missing key code for 'f{i}'"
            );
        }
    }
}
