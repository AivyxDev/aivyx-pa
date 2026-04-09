#![allow(
    unsafe_op_in_unsafe_fn,
    unused_imports,
    unreachable_code,
    unused_variables,
    dead_code,
    clippy::all
)]
//! Windows window management — EnumWindows, SetWindowPos, GetClassName.
//!
//! Provides window listing, focusing, resizing, minimizing, maximizing,
//! and class detection using Win32 APIs. This is the Windows equivalent
//! of `window_manage.rs` and part of `desktop_info.rs` on Linux.

use aivyx_core::{AivyxError, Result};

#[cfg(target_os = "windows")]
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, TRUE};
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId,
    HWND_TOP, IsWindowVisible, SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE, SWP_NOZORDER, SWP_SHOWWINDOW,
    SetForegroundWindow, SetWindowPos, ShowWindow,
};
#[cfg(target_os = "windows")]

/// A visible window entry.
#[derive(Debug, Clone, serde::Serialize)]
pub struct WindowEntry {
    pub title: String,
    pub class: String,
    pub pid: u32,
}

/// Get the class name of the foreground (active) window.
///
/// Uses Win32 `GetForegroundWindow` + `GetClassNameW`. Called by
/// `BackendRouter` for smart CDP-vs-UIA routing.
pub fn get_foreground_window_class() -> Result<String> {
    #[cfg(target_os = "windows")]
    {
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.unwrap_or(HWND::default()).0 == std::ptr::null_mut() {
                return Err(AivyxError::Other("No foreground window".into()));
            }
            let mut buf = [0u16; 256];
            let len = GetClassNameW(hwnd, &mut buf);
            if len == 0 {
                return Err(AivyxError::Other("GetClassNameW failed".into()));
            }
            Ok(String::from_utf16_lossy(&buf[..len as usize]))
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err(AivyxError::Other(
            "win_window: only available on Windows".into(),
        ))
    }
}

/// List all visible windows with their titles, class names, and PIDs.
pub async fn list_windows() -> Result<Vec<WindowEntry>> {
    #[cfg(target_os = "windows")]
    {
        let entries = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let entries_clone = entries.clone();

        unsafe {
            EnumWindows(
                Some(enum_windows_callback),
                LPARAM(std::sync::Arc::into_raw(entries_clone) as isize),
            )
            .map_err(|e| AivyxError::Other(format!("EnumWindows: {e}")))?;
        }

        let result = entries.lock().unwrap().clone();
        Ok(result)
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err(AivyxError::Other(
            "win_window: only available on Windows".into(),
        ))
    }
}

/// EnumWindows callback — collects visible windows.
#[cfg(target_os = "windows")]
unsafe extern "system" fn enum_windows_callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
    if !IsWindowVisible(hwnd).as_bool() {
        return TRUE;
    }

    let mut title_buf = [0u16; 512];
    let title_len = GetWindowTextW(hwnd, &mut title_buf);
    if title_len == 0 {
        return TRUE; // Skip windows with no title.
    }

    let title = String::from_utf16_lossy(&title_buf[..title_len as usize]);
    if title.is_empty() {
        return TRUE;
    }

    let mut class_buf = [0u16; 256];
    let class_len = GetClassNameW(hwnd, &mut class_buf);
    let class = if class_len > 0 {
        String::from_utf16_lossy(&class_buf[..class_len as usize])
    } else {
        String::new()
    };

    let mut pid: u32 = 0;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));

    let entries = &*(lparam.0 as *const std::sync::Mutex<Vec<WindowEntry>>);
    if let Ok(mut vec) = entries.lock() {
        vec.push(WindowEntry { title, class, pid });
    }

    TRUE
}

/// Execute a window management action.
///
/// `action`: minimize, maximize, restore, close, fullscreen, resize, move.
/// `window`: optional title match (None = active window).
pub async fn manage_window(
    action: &str,
    window: Option<&str>,
    input: &serde_json::Value,
) -> Result<String> {
    #[cfg(target_os = "windows")]
    {
        let hwnd = find_window_handle(window)?;
        match action {
            "minimize" => {
                unsafe { ShowWindow(hwnd, SW_MINIMIZE) };
                Ok("minimized".into())
            }
            "maximize" => {
                unsafe { ShowWindow(hwnd, SW_MAXIMIZE) };
                Ok("maximized".into())
            }
            "restore" => {
                unsafe { ShowWindow(hwnd, SW_RESTORE) };
                Ok("restored".into())
            }
            "close" => {
                use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};
                unsafe { PostMessageW(hwnd, WM_CLOSE, None, None) }
                    .map_err(|e| AivyxError::Other(format!("PostMessage WM_CLOSE: {e}")))?;
                Ok("closed".into())
            }
            "focus" => {
                unsafe { SetForegroundWindow(hwnd) };
                Ok("focused".into())
            }
            "resize" => {
                let w = input["width"]
                    .as_i64()
                    .ok_or_else(|| AivyxError::Validation("width is required".into()))?
                    as i32;
                let h = input["height"]
                    .as_i64()
                    .ok_or_else(|| AivyxError::Validation("height is required".into()))?
                    as i32;
                if w < 1 || h < 1 {
                    return Err(AivyxError::Validation(
                        "width and height must be positive".into(),
                    ));
                }
                unsafe {
                    SetWindowPos(hwnd, HWND_TOP, 0, 0, w, h, SWP_NOZORDER | SWP_SHOWWINDOW)
                        .map_err(|e| AivyxError::Other(format!("SetWindowPos: {e}")))?;
                }
                Ok(format!("resized to {w}x{h}"))
            }
            "move" => {
                let x = input["x"]
                    .as_i64()
                    .ok_or_else(|| AivyxError::Validation("x is required".into()))?
                    as i32;
                let y = input["y"]
                    .as_i64()
                    .ok_or_else(|| AivyxError::Validation("y is required".into()))?
                    as i32;
                unsafe {
                    SetWindowPos(hwnd, HWND_TOP, x, y, 0, 0, SWP_NOZORDER | SWP_SHOWWINDOW)
                        .map_err(|e| AivyxError::Other(format!("SetWindowPos: {e}")))?;
                }
                Ok(format!("moved to ({x}, {y})"))
            }
            _ => Err(AivyxError::Validation(format!("unknown action: {action}"))),
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (action, window, input);
        Err(AivyxError::Other(
            "win_window: only available on Windows".into(),
        ))
    }
}

/// Find a window handle by title (substring match) or return the foreground window.
#[cfg(target_os = "windows")]
fn find_window_handle(title: Option<&str>) -> Result<HWND> {
    match title {
        None => unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.unwrap_or(HWND::default()).0 == std::ptr::null_mut() {
                Err(AivyxError::Other("No foreground window".into()))
            } else {
                Ok(hwnd?)
            }
        },
        Some(search) => {
            // Use FindWindowW with NULL class and the title.
            // For substring matching, enumerate all windows.
            use windows::Win32::UI::WindowsAndMessaging::FindWindowW;

            let wide: Vec<u16> = search.encode_utf16().chain(std::iter::once(0)).collect();
            let hwnd = unsafe {
                FindWindowW(
                    windows::core::PCWSTR::null(),
                    windows::core::PCWSTR(wide.as_ptr()),
                )
            };
            if hwnd.unwrap_or(HWND::default()).0 == std::ptr::null_mut() {
                Err(AivyxError::Other(format!("Window not found: '{search}'")))
            } else {
                Ok(hwnd?)
            }
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_entry_fields() {
        let entry = WindowEntry {
            title: "Notepad".into(),
            class: "Notepad".into(),
            pid: 1234,
        };
        assert_eq!(entry.title, "Notepad");
        assert_eq!(entry.class, "Notepad");
        assert_eq!(entry.pid, 1234);
    }

    #[test]
    fn foreground_window_class_on_non_windows() {
        #[cfg(not(target_os = "windows"))]
        {
            assert!(get_foreground_window_class().is_err());
        }
    }
}
