#![allow(
    unsafe_op_in_unsafe_fn,
    unused_imports,
    unreachable_code,
    unused_variables,
    dead_code,
    clippy::all
)]
//! Win32 SendInput backend — universal input injection on Windows.
//!
//! Uses the Win32 `SendInput` API for coordinate-based clicks, keyboard input,
//! scroll, and drag operations. This is the Windows equivalent of `ydotool`
//! on Linux.
//!
//! No external daemon required — SendInput writes directly to the system input
//! queue. Requires the calling process to have appropriate foreground privileges.

use aivyx_core::{AivyxError, Result};

use super::{
    ElementQuery, InputBackend, ScrollDirection, UiBackend, UiElement, UiTreeNode, WindowRef,
};

#[cfg(target_os = "windows")]
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_TYPE, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
    MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN,
    MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEINPUT, SendInput,
    VIRTUAL_KEY,
};
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

/// Win32 SendInput-based input backend.
pub struct WinInputBackend;

impl WinInputBackend {
    pub fn new() -> Self {
        Self
    }

    /// Send a key combination (e.g., "ctrl+s", "alt+F4").
    ///
    /// Parses modifier+key combos and translates to virtual key codes,
    /// then synthesizes key down/up events via SendInput.
    pub async fn key_combo(&self, keys: &str) -> Result<()> {
        let vk_codes = parse_key_combo(keys)?;
        send_key_combo(&vk_codes)
    }

    /// Click at absolute screen coordinates (left button).
    pub async fn click_at(&self, x: i32, y: i32) -> Result<()> {
        send_mouse_click(x, y, MouseButton::Left)
    }

    /// Type a string via keyboard simulation.
    ///
    /// Uses `KEYEVENTF_UNICODE` to send each character as a Unicode event,
    /// which works with any keyboard layout and input method.
    pub async fn type_string(&self, text: &str) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        send_unicode_string(text)
    }

    /// Double-click at absolute screen coordinates.
    pub async fn double_click_at(&self, x: i32, y: i32) -> Result<()> {
        send_mouse_click(x, y, MouseButton::Left)?;
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        send_mouse_click(x, y, MouseButton::Left)
    }

    /// Middle-click at absolute screen coordinates.
    pub async fn middle_click_at(&self, x: i32, y: i32) -> Result<()> {
        send_mouse_click(x, y, MouseButton::Middle)
    }

    /// Right-click at absolute screen coordinates.
    pub async fn right_click_at(&self, x: i32, y: i32) -> Result<()> {
        send_mouse_click(x, y, MouseButton::Right)
    }

    /// Scroll the mouse wheel.
    ///
    /// SendInput uses MOUSEEVENTF_WHEEL (vertical) and MOUSEEVENTF_HWHEEL
    /// (horizontal). One "click" of the wheel is WHEEL_DELTA (120 units).
    pub async fn scroll(&self, direction: ScrollDirection, amount: u32) -> Result<()> {
        send_scroll(direction, amount)
    }

    /// Move the mouse to absolute screen coordinates.
    pub async fn mouse_move_to(&self, x: i32, y: i32) -> Result<()> {
        send_mouse_move(x, y)
    }

    /// Drag from one position to another (left-button hold + move + release).
    pub async fn drag(&self, from_x: i32, from_y: i32, to_x: i32, to_y: i32) -> Result<()> {
        // Move to start position.
        send_mouse_move(from_x, from_y)?;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        // Press left button.
        send_mouse_button_event(from_x, from_y, &MouseButton::Left, true)?;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        // Move to destination.
        send_mouse_move(to_x, to_y)?;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        // Release left button.
        send_mouse_button_event(to_x, to_y, &MouseButton::Left, false)
    }

    /// Ctrl+click at multiple coordinates (for multi-select).
    pub async fn multi_click_at(&self, positions: &[(i32, i32)]) -> Result<()> {
        if positions.is_empty() {
            return Err(AivyxError::Validation(
                "At least one position is required for multi-click".into(),
            ));
        }

        // Press Ctrl.
        send_key_event(VK_CONTROL_CODE, false)?;
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;

        for (x, y) in positions {
            self.click_at(*x, *y).await?;
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        // Release Ctrl.
        send_key_event(VK_CONTROL_CODE, true)
    }
}

// ── InputBackend impl ──────────────────────────────────────────

#[async_trait::async_trait]
impl InputBackend for WinInputBackend {
    async fn key_combo(&self, keys: &str) -> Result<()> {
        WinInputBackend::key_combo(self, keys).await
    }

    async fn type_string(&self, text: &str) -> Result<()> {
        WinInputBackend::type_string(self, text).await
    }

    async fn click_at(&self, x: i32, y: i32) -> Result<()> {
        WinInputBackend::click_at(self, x, y).await
    }

    async fn double_click_at(&self, x: i32, y: i32) -> Result<()> {
        WinInputBackend::double_click_at(self, x, y).await
    }

    async fn middle_click_at(&self, x: i32, y: i32) -> Result<()> {
        WinInputBackend::middle_click_at(self, x, y).await
    }

    async fn right_click_at(&self, x: i32, y: i32) -> Result<()> {
        WinInputBackend::right_click_at(self, x, y).await
    }

    async fn mouse_move_to(&self, x: i32, y: i32) -> Result<()> {
        WinInputBackend::mouse_move_to(self, x, y).await
    }

    async fn scroll(&self, direction: ScrollDirection, amount: u32) -> Result<()> {
        WinInputBackend::scroll(self, direction, amount).await
    }

    async fn drag(&self, from_x: i32, from_y: i32, to_x: i32, to_y: i32) -> Result<()> {
        WinInputBackend::drag(self, from_x, from_y, to_x, to_y).await
    }

    async fn multi_click_at(&self, positions: &[(i32, i32)]) -> Result<()> {
        WinInputBackend::multi_click_at(self, positions).await
    }
}

// ── UiBackend impl ─────────────────────────────────────────────

#[async_trait::async_trait]
impl UiBackend for WinInputBackend {
    fn name(&self) -> &str {
        "win_input"
    }

    async fn inspect(&self, _window: &WindowRef, _max_depth: u32) -> Result<Vec<UiTreeNode>> {
        Err(AivyxError::Other(
            "SendInput cannot inspect UI trees — enable the windows-automation feature for UIA support"
                .into(),
        ))
    }

    async fn find_element(
        &self,
        _window: &WindowRef,
        _query: &ElementQuery,
    ) -> Result<Vec<UiElement>> {
        Err(AivyxError::Other(
            "SendInput cannot find UI elements — enable the windows-automation feature for UIA support"
                .into(),
        ))
    }

    async fn click(&self, element: &UiElement) -> Result<()> {
        let bounds = element.bounds.ok_or_else(|| {
            AivyxError::Other(
                "SendInput click requires element bounds (x, y, width, height)".into(),
            )
        })?;
        let cx = bounds[0] + bounds[2] / 2;
        let cy = bounds[1] + bounds[3] / 2;
        self.click_at(cx, cy).await
    }

    async fn type_text(&self, _element: Option<&UiElement>, text: &str) -> Result<()> {
        self.type_string(text).await
    }

    async fn read_text(&self, _element: &UiElement) -> Result<String> {
        Err(AivyxError::Other(
            "SendInput cannot read text — enable the windows-automation feature for UIA support"
                .into(),
        ))
    }

    async fn double_click(&self, element: &UiElement) -> Result<()> {
        let bounds = element.bounds.ok_or_else(|| {
            AivyxError::Other(
                "SendInput double-click requires element bounds (x, y, width, height)".into(),
            )
        })?;
        let cx = bounds[0] + bounds[2] / 2;
        let cy = bounds[1] + bounds[3] / 2;
        self.double_click_at(cx, cy).await
    }

    async fn middle_click(&self, element: &UiElement) -> Result<()> {
        let bounds = element.bounds.ok_or_else(|| {
            AivyxError::Other(
                "SendInput middle-click requires element bounds (x, y, width, height)".into(),
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
        WinInputBackend::scroll(self, direction, amount).await
    }

    async fn right_click(&self, element: &UiElement) -> Result<()> {
        let bounds = element.bounds.ok_or_else(|| {
            AivyxError::Other(
                "SendInput right-click requires element bounds (x, y, width, height)".into(),
            )
        })?;
        let cx = bounds[0] + bounds[2] / 2;
        let cy = bounds[1] + bounds[3] / 2;
        self.right_click_at(cx, cy).await
    }

    async fn hover(&self, element: &UiElement) -> Result<()> {
        let bounds = element.bounds.ok_or_else(|| {
            AivyxError::Other(
                "SendInput hover requires element bounds (x, y, width, height)".into(),
            )
        })?;
        let cx = bounds[0] + bounds[2] / 2;
        let cy = bounds[1] + bounds[3] / 2;
        self.mouse_move_to(cx, cy).await
    }

    async fn drag(&self, from: &UiElement, to: &UiElement) -> Result<()> {
        let from_bounds = from.bounds.ok_or_else(|| {
            AivyxError::Other("SendInput drag requires source element bounds".into())
        })?;
        let to_bounds = to.bounds.ok_or_else(|| {
            AivyxError::Other("SendInput drag requires destination element bounds".into())
        })?;
        let from_cx = from_bounds[0] + from_bounds[2] / 2;
        let from_cy = from_bounds[1] + from_bounds[3] / 2;
        let to_cx = to_bounds[0] + to_bounds[2] / 2;
        let to_cy = to_bounds[1] + to_bounds[3] / 2;
        WinInputBackend::drag(self, from_cx, from_cy, to_cx, to_cy).await
    }

    async fn mouse_move(&self, x: i32, y: i32) -> Result<()> {
        self.mouse_move_to(x, y).await
    }
}

// ── Mouse helpers ──────────────────────────────────────────────

/// Mouse button type for click operations.
enum MouseButton {
    Left,
    Right,
    Middle,
}

/// VK_CONTROL virtual key code.
const VK_CONTROL_CODE: u16 = 0x11;

/// One wheel "click" in Windows input units.
const WHEEL_DELTA: i32 = 120;

/// Normalize screen coordinates to the 0-65535 range that SendInput expects
/// for `MOUSEEVENTF_ABSOLUTE`.
#[cfg(target_os = "windows")]
fn normalize_coords(x: i32, y: i32) -> (i32, i32) {
    unsafe {
        let screen_w = GetSystemMetrics(SM_CXSCREEN);
        let screen_h = GetSystemMetrics(SM_CYSCREEN);
        if screen_w == 0 || screen_h == 0 {
            return (x, y);
        }
        let nx = (x * 65535 + screen_w / 2) / screen_w;
        let ny = (y * 65535 + screen_h / 2) / screen_h;
        (nx, ny)
    }
}

/// Stub for non-Windows compilation.
#[cfg(not(target_os = "windows"))]
fn normalize_coords(x: i32, y: i32) -> (i32, i32) {
    (x, y)
}

/// Send a mouse move to absolute screen coordinates.
#[cfg(target_os = "windows")]
fn send_mouse_move(x: i32, y: i32) -> Result<()> {
    let (nx, ny) = normalize_coords(x, y);
    let input = INPUT {
        r#type: INPUT_TYPE(0), // INPUT_MOUSE
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: nx,
                dy: ny,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    send_input_safe(&[input])
}

#[cfg(not(target_os = "windows"))]
fn send_mouse_move(_x: i32, _y: i32) -> Result<()> {
    Err(AivyxError::Other(
        "SendInput is only available on Windows".into(),
    ))
}

/// Send a mouse click (down + up) at absolute screen coordinates.
#[cfg(target_os = "windows")]
fn send_mouse_click(x: i32, y: i32, button: MouseButton) -> Result<()> {
    send_mouse_button_event(x, y, &button, true)?;
    send_mouse_button_event(x, y, &button, false)
}

#[cfg(not(target_os = "windows"))]
fn send_mouse_click(_x: i32, _y: i32, _button: MouseButton) -> Result<()> {
    Err(AivyxError::Other(
        "SendInput is only available on Windows".into(),
    ))
}

/// Send a single mouse button event (down or up) at screen coordinates.
#[cfg(target_os = "windows")]
fn send_mouse_button_event(x: i32, y: i32, button: &MouseButton, down: bool) -> Result<()> {
    let (nx, ny) = normalize_coords(x, y);
    let flags = match (button, down) {
        (MouseButton::Left, true) => MOUSEEVENTF_LEFTDOWN,
        (MouseButton::Left, false) => MOUSEEVENTF_LEFTUP,
        (MouseButton::Right, true) => MOUSEEVENTF_RIGHTDOWN,
        (MouseButton::Right, false) => MOUSEEVENTF_RIGHTUP,
        (MouseButton::Middle, true) => MOUSEEVENTF_MIDDLEDOWN,
        (MouseButton::Middle, false) => MOUSEEVENTF_MIDDLEUP,
    };
    let input = INPUT {
        r#type: INPUT_TYPE(0), // INPUT_MOUSE
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: nx,
                dy: ny,
                mouseData: 0,
                dwFlags: flags | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    send_input_safe(&[input])
}

#[cfg(not(target_os = "windows"))]
fn send_mouse_button_event(_x: i32, _y: i32, _button: MouseButton, _down: bool) -> Result<()> {
    Err(AivyxError::Other(
        "SendInput is only available on Windows".into(),
    ))
}

/// Send a scroll event.
#[cfg(target_os = "windows")]
fn send_scroll(direction: ScrollDirection, amount: u32) -> Result<()> {
    let (flags, data) = match direction {
        ScrollDirection::Up => (MOUSEEVENTF_WHEEL, (amount as i32) * WHEEL_DELTA),
        ScrollDirection::Down => (MOUSEEVENTF_WHEEL, -(amount as i32) * WHEEL_DELTA),
        ScrollDirection::Right => (MOUSEEVENTF_HWHEEL, (amount as i32) * WHEEL_DELTA),
        ScrollDirection::Left => (MOUSEEVENTF_HWHEEL, -(amount as i32) * WHEEL_DELTA),
    };
    let input = INPUT {
        r#type: INPUT_TYPE(0), // INPUT_MOUSE
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: data as u32,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    send_input_safe(&[input])
}

#[cfg(not(target_os = "windows"))]
fn send_scroll(_direction: ScrollDirection, _amount: u32) -> Result<()> {
    Err(AivyxError::Other(
        "SendInput is only available on Windows".into(),
    ))
}

// ── Keyboard helpers ───────────────────────────────────────────

/// Send a single key event (down or up) by virtual key code.
#[cfg(target_os = "windows")]
fn send_key_event(vk: u16, key_up: bool) -> Result<()> {
    let flags = if key_up {
        KEYEVENTF_KEYUP
    } else {
        KEYBD_EVENT_FLAGS(0)
    };
    let input = INPUT {
        r#type: INPUT_TYPE(1), // INPUT_KEYBOARD
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    send_input_safe(&[input])
}

#[cfg(not(target_os = "windows"))]
fn send_key_event(_vk: u16, _key_up: bool) -> Result<()> {
    Err(AivyxError::Other(
        "SendInput is only available on Windows".into(),
    ))
}

/// Send a key combo: press all keys down in order, release in reverse.
#[cfg(target_os = "windows")]
fn send_key_combo(vk_codes: &[u16]) -> Result<()> {
    let mut inputs = Vec::with_capacity(vk_codes.len() * 2);

    // Key-down events.
    for &vk in vk_codes {
        inputs.push(INPUT {
            r#type: INPUT_TYPE(1),
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(vk),
                    wScan: 0,
                    dwFlags: KEYBD_EVENT_FLAGS(0),
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        });
    }

    // Key-up events (reverse order).
    for &vk in vk_codes.iter().rev() {
        inputs.push(INPUT {
            r#type: INPUT_TYPE(1),
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(vk),
                    wScan: 0,
                    dwFlags: KEYEVENTF_KEYUP,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        });
    }

    send_input_safe(&inputs)
}

#[cfg(not(target_os = "windows"))]
fn send_key_combo(_vk_codes: &[u16]) -> Result<()> {
    Err(AivyxError::Other(
        "SendInput is only available on Windows".into(),
    ))
}

/// Type a Unicode string character by character via KEYEVENTF_UNICODE.
#[cfg(target_os = "windows")]
fn send_unicode_string(text: &str) -> Result<()> {
    let mut inputs = Vec::with_capacity(text.len() * 2);

    for ch in text.encode_utf16() {
        // Key down.
        inputs.push(INPUT {
            r#type: INPUT_TYPE(1),
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(0),
                    wScan: ch,
                    dwFlags: KEYEVENTF_UNICODE,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        });
        // Key up.
        inputs.push(INPUT {
            r#type: INPUT_TYPE(1),
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(0),
                    wScan: ch,
                    dwFlags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        });
    }

    send_input_safe(&inputs)
}

#[cfg(not(target_os = "windows"))]
fn send_unicode_string(_text: &str) -> Result<()> {
    Err(AivyxError::Other(
        "SendInput is only available on Windows".into(),
    ))
}

/// Safe wrapper around SendInput that checks the return value.
#[cfg(target_os = "windows")]
fn send_input_safe(inputs: &[INPUT]) -> Result<()> {
    let sent = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize != inputs.len() {
        Err(AivyxError::Other(format!(
            "SendInput: only {sent}/{} events were injected (UIPI may be blocking)",
            inputs.len()
        )))
    } else {
        Ok(())
    }
}

// ── Key combo parsing ──────────────────────────────────────────

/// Parse a human-readable key combo (e.g., "ctrl+s") into Windows virtual key codes.
fn parse_key_combo(combo: &str) -> Result<Vec<u16>> {
    let parts: Vec<&str> = combo.split('+').map(|s| s.trim()).collect();
    if parts.is_empty() || (parts.len() == 1 && parts[0].is_empty()) {
        return Err(AivyxError::Other("empty key combo".into()));
    }

    let mut codes = Vec::new();
    for part in &parts {
        let code = key_name_to_vk(part)
            .ok_or_else(|| AivyxError::Other(format!("unknown key: '{part}'")))?;
        codes.push(code);
    }
    Ok(codes)
}

/// Map a key name to a Windows virtual key code.
fn key_name_to_vk(name: &str) -> Option<u16> {
    let lower = name.to_lowercase();
    match lower.as_str() {
        // Modifiers
        "ctrl" | "control" | "lctrl" => Some(0x11), // VK_CONTROL
        "rctrl" => Some(0xA3),                      // VK_RCONTROL
        "shift" | "lshift" => Some(0x10),           // VK_SHIFT
        "rshift" => Some(0xA1),                     // VK_RSHIFT
        "alt" | "lalt" => Some(0x12),               // VK_MENU
        "ralt" | "altgr" => Some(0xA5),             // VK_RMENU
        "super" | "meta" | "win" | "lmeta" => Some(0x5B), // VK_LWIN
        "rmeta" | "rsuper" => Some(0x5C),           // VK_RWIN

        // Letters (VK_A through VK_Z = 0x41-0x5A)
        "a" => Some(0x41),
        "b" => Some(0x42),
        "c" => Some(0x43),
        "d" => Some(0x44),
        "e" => Some(0x45),
        "f" => Some(0x46),
        "g" => Some(0x47),
        "h" => Some(0x48),
        "i" => Some(0x49),
        "j" => Some(0x4A),
        "k" => Some(0x4B),
        "l" => Some(0x4C),
        "m" => Some(0x4D),
        "n" => Some(0x4E),
        "o" => Some(0x4F),
        "p" => Some(0x50),
        "q" => Some(0x51),
        "r" => Some(0x52),
        "s" => Some(0x53),
        "t" => Some(0x54),
        "u" => Some(0x55),
        "v" => Some(0x56),
        "w" => Some(0x57),
        "x" => Some(0x58),
        "y" => Some(0x59),
        "z" => Some(0x5A),

        // Numbers (VK_0 through VK_9 = 0x30-0x39)
        "0" => Some(0x30),
        "1" => Some(0x31),
        "2" => Some(0x32),
        "3" => Some(0x33),
        "4" => Some(0x34),
        "5" => Some(0x35),
        "6" => Some(0x36),
        "7" => Some(0x37),
        "8" => Some(0x38),
        "9" => Some(0x39),

        // Function keys (VK_F1-VK_F12 = 0x70-0x7B)
        "f1" => Some(0x70),
        "f2" => Some(0x71),
        "f3" => Some(0x72),
        "f4" => Some(0x73),
        "f5" => Some(0x74),
        "f6" => Some(0x75),
        "f7" => Some(0x76),
        "f8" => Some(0x77),
        "f9" => Some(0x78),
        "f10" => Some(0x79),
        "f11" => Some(0x7A),
        "f12" => Some(0x7B),

        // Special keys
        "esc" | "escape" => Some(0x1B),        // VK_ESCAPE
        "tab" => Some(0x09),                   // VK_TAB
        "enter" | "return" => Some(0x0D),      // VK_RETURN
        "space" | " " => Some(0x20),           // VK_SPACE
        "backspace" | "bs" => Some(0x08),      // VK_BACK
        "delete" | "del" => Some(0x2E),        // VK_DELETE
        "insert" | "ins" => Some(0x2D),        // VK_INSERT
        "home" => Some(0x24),                  // VK_HOME
        "end" => Some(0x23),                   // VK_END
        "pageup" | "pgup" => Some(0x21),       // VK_PRIOR
        "pagedown" | "pgdn" => Some(0x22),     // VK_NEXT
        "up" => Some(0x26),                    // VK_UP
        "down" => Some(0x28),                  // VK_DOWN
        "left" => Some(0x25),                  // VK_LEFT
        "right" => Some(0x27),                 // VK_RIGHT
        "capslock" => Some(0x14),              // VK_CAPITAL
        "printscreen" | "prtsc" => Some(0x2C), // VK_SNAPSHOT
        "scrolllock" => Some(0x91),            // VK_SCROLL
        "pause" | "break" => Some(0x13),       // VK_PAUSE

        // Punctuation (using VK_OEM codes)
        "minus" | "-" => Some(0xBD),          // VK_OEM_MINUS
        "equal" | "=" => Some(0xBB),          // VK_OEM_PLUS (unshifted = '=')
        "leftbrace" | "[" => Some(0xDB),      // VK_OEM_4
        "rightbrace" | "]" => Some(0xDD),     // VK_OEM_6
        "semicolon" | ";" => Some(0xBA),      // VK_OEM_1
        "apostrophe" | "'" => Some(0xDE),     // VK_OEM_7
        "grave" | "`" => Some(0xC0),          // VK_OEM_3
        "backslash" | "\\" => Some(0xDC),     // VK_OEM_5
        "comma" | "," => Some(0xBC),          // VK_OEM_COMMA
        "dot" | "period" | "." => Some(0xBE), // VK_OEM_PERIOD
        "slash" | "/" => Some(0xBF),          // VK_OEM_2

        _ => None,
    }
}

// ── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ctrl_s() {
        let codes = parse_key_combo("ctrl+s").unwrap();
        assert_eq!(codes, vec![0x11, 0x53]); // VK_CONTROL, VK_S
    }

    #[test]
    fn parse_alt_f4() {
        let codes = parse_key_combo("alt+F4").unwrap();
        assert_eq!(codes, vec![0x12, 0x73]); // VK_MENU, VK_F4
    }

    #[test]
    fn parse_single_key() {
        let codes = parse_key_combo("escape").unwrap();
        assert_eq!(codes, vec![0x1B]); // VK_ESCAPE
    }

    #[test]
    fn parse_ctrl_shift_t() {
        let codes = parse_key_combo("ctrl+shift+t").unwrap();
        assert_eq!(codes, vec![0x11, 0x10, 0x54]); // VK_CONTROL, VK_SHIFT, VK_T
    }

    #[test]
    fn parse_super_key() {
        let codes = parse_key_combo("super").unwrap();
        assert_eq!(codes, vec![0x5B]); // VK_LWIN
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
    fn key_code_coverage() {
        // All letters
        for c in 'a'..='z' {
            assert!(
                key_name_to_vk(&c.to_string()).is_some(),
                "missing VK code for '{c}'"
            );
        }
        // All digits
        for d in '0'..='9' {
            assert!(
                key_name_to_vk(&d.to_string()).is_some(),
                "missing VK code for '{d}'"
            );
        }
        // F1-F12
        for i in 1..=12 {
            assert!(
                key_name_to_vk(&format!("f{i}")).is_some(),
                "missing VK code for 'f{i}'"
            );
        }
    }

    #[test]
    fn vk_letter_range() {
        // VK_A (0x41) through VK_Z (0x5A) — verify contiguous mapping.
        for (i, c) in ('a'..='z').enumerate() {
            let vk = key_name_to_vk(&c.to_string()).unwrap();
            assert_eq!(vk, 0x41 + i as u16, "VK code mismatch for '{c}'");
        }
    }

    #[test]
    fn vk_digit_range() {
        // VK_0 (0x30) through VK_9 (0x39) — verify contiguous mapping.
        for (i, d) in ('0'..='9').enumerate() {
            let vk = key_name_to_vk(&d.to_string()).unwrap();
            assert_eq!(vk, 0x30 + i as u16, "VK code mismatch for '{d}'");
        }
    }

    #[test]
    fn wheel_delta_value() {
        assert_eq!(WHEEL_DELTA, 120);
    }

    #[test]
    fn normalize_coords_stub() {
        // On non-Windows, the stub just returns the same coords.
        #[cfg(not(target_os = "windows"))]
        {
            let (nx, ny) = normalize_coords(100, 200);
            assert_eq!((nx, ny), (100, 200));
        }
    }
}
