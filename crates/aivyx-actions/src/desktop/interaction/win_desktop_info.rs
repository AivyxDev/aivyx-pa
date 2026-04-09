#![allow(unsafe_op_in_unsafe_fn, unused_imports, unreachable_code, unused_variables, dead_code, clippy::all)]
//! Windows desktop info — running applications, screen properties, environment.
//!
//! Provides desktop environment information, running app listing, and
//! screen properties on Windows. This is the Windows equivalent of
//! `desktop_info.rs` on Linux.

use aivyx_core::{AivyxError, Result};

/// Desktop environment info.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DesktopInfo {
    pub screen_width: u32,
    pub screen_height: u32,
    pub dpi: u32,
    pub theme: String,
    pub os_version: String,
}

/// Get basic desktop environment information.
pub async fn desktop_info() -> Result<DesktopInfo> {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::Graphics::Gdi::{GetDC, GetDeviceCaps, LOGPIXELSX, ReleaseDC};
        use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

        let screen_width = unsafe { GetSystemMetrics(SM_CXSCREEN) } as u32;
        let screen_height = unsafe { GetSystemMetrics(SM_CYSCREEN) } as u32;

        let dpi = unsafe {
            let hdc = GetDC(HWND(std::ptr::null_mut()));
            let dpi = GetDeviceCaps(hdc, LOGPIXELSX) as u32;
            ReleaseDC(HWND(std::ptr::null_mut()), hdc);
            dpi
        };

        // Detect light/dark theme from registry.
        let theme = detect_theme().unwrap_or_else(|_| "unknown".into());

        // Get Windows version.
        let os_version = get_os_version().unwrap_or_else(|_| "Windows".into());

        Ok(DesktopInfo {
            screen_width,
            screen_height,
            dpi,
            theme,
            os_version,
        })
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err(AivyxError::Other(
            "win_desktop_info: only available on Windows".into(),
        ))
    }
}

/// Detect Windows light/dark theme from the registry.
#[cfg(target_os = "windows")]
fn detect_theme() -> Result<String> {
    use windows::Win32::System::Registry::{
        HKEY_CURRENT_USER, KEY_READ, REG_DWORD, RegOpenKeyExW, RegQueryValueExW,
    };
    use windows::core::PCWSTR;

    let subkey: Vec<u16> = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let value_name: Vec<u16> = "AppsUseLightTheme"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        let mut hkey = Default::default();
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            0,
            KEY_READ,
            &mut hkey,
        )
        .ok()
        .map_err(|e| AivyxError::Other(format!("RegOpenKeyExW: {e}")))?;

        let mut data = [0u8; 4];
        let mut data_size = 4u32;
        let mut data_type = REG_DWORD;

        RegQueryValueExW(
            hkey,
            PCWSTR(value_name.as_ptr()),
            None,
            Some(&mut data_type),
            Some(data.as_mut_ptr()),
            Some(&mut data_size),
        )
        .ok()
        .map_err(|e| AivyxError::Other(format!("RegQueryValueExW: {e}")))?;

        let value = u32::from_le_bytes(data);
        Ok(if value == 0 { "dark" } else { "light" }.into())
    }
}

#[cfg(not(target_os = "windows"))]
fn detect_theme() -> Result<String> {
    Err(AivyxError::Other("not on Windows".into()))
}

/// Get the Windows version string.
#[cfg(target_os = "windows")]
fn get_os_version() -> Result<String> {
    // Use powershell for a simple, reliable version string.
    let output = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "[System.Environment]::OSVersion.VersionString",
        ])
        .output()
        .map_err(|e| AivyxError::Other(format!("powershell: {e}")))?;

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(not(target_os = "windows"))]
fn get_os_version() -> Result<String> {
    Err(AivyxError::Other("not on Windows".into()))
}

/// List all running GUI applications with their window titles, classes, and PIDs.
///
/// Delegates to `win_window::list_windows()` and formats as JSON.
pub async fn list_running_apps() -> Result<serde_json::Value> {
    #[cfg(target_os = "windows")]
    {
        let windows = super::win_window::list_windows().await?;
        let apps: Vec<serde_json::Value> = windows
            .iter()
            .map(|w| {
                serde_json::json!({
                    "title": w.title,
                    "class": w.class,
                    "pid": w.pid,
                })
            })
            .collect();
        Ok(serde_json::json!({ "applications": apps }))
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err(AivyxError::Other(
            "win_desktop_info: only available on Windows".into(),
        ))
    }
}

/// Open an application or file via ShellExecuteW.
///
/// Equivalent of `xdg-open` on Linux. Opens files with their default
/// handler, URLs in the default browser, etc.
pub async fn open_app(path: &str) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::core::PCWSTR;

        let operation: Vec<u16> = "open".encode_utf16().chain(std::iter::once(0)).collect();
        let file: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();

        let result = unsafe {
            ShellExecuteW(
                HWND(std::ptr::null_mut()),
                PCWSTR(operation.as_ptr()),
                PCWSTR(file.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                windows::Win32::UI::WindowsAndMessaging::SW_SHOW,
            )
        };

        // ShellExecuteW returns HINSTANCE > 32 on success.
        if (result.0 as usize) > 32 {
            Ok(())
        } else {
            Err(AivyxError::Other(format!(
                "ShellExecuteW failed for '{path}' (code: {:?})",
                result.0
            )))
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = path;
        Err(AivyxError::Other(
            "win_desktop_info: only available on Windows".into(),
        ))
    }
}

// ── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_info_serialize() {
        let info = DesktopInfo {
            screen_width: 1920,
            screen_height: 1080,
            dpi: 96,
            theme: "dark".into(),
            os_version: "Windows 11".into(),
        };
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["screen_width"], 1920);
        assert_eq!(json["theme"], "dark");
    }

    #[test]
    fn non_windows_stubs() {
        #[cfg(not(target_os = "windows"))]
        {
            let rt = tokio::runtime::Runtime::new().unwrap();
            assert!(rt.block_on(desktop_info()).is_err());
            assert!(rt.block_on(list_running_apps()).is_err());
            assert!(rt.block_on(open_app("notepad")).is_err());
        }
    }
}
