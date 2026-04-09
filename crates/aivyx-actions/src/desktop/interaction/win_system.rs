#![allow(
    unsafe_op_in_unsafe_fn,
    unused_imports,
    unreachable_code,
    unused_variables,
    dead_code,
    clippy::all
)]
//! Windows system controls — WASAPI volume, WMI brightness, Toast notifications.
//!
//! Provides system-level controls for volume, brightness, and desktop
//! notifications on Windows. This is the Windows equivalent of `system_ctl.rs`
//! (wpctl/brightnessctl/notify-send) on Linux.
//!
//! Each function mirrors the interface of its Linux counterpart so the
//! tool layer (tools.rs) can remain platform-agnostic.

use aivyx_core::{AivyxError, Result};

// ── Volume Control (WASAPI IAudioEndpointVolume) ────────────────

/// Get or set the system master volume.
///
/// `action`: "get", "set", "mute", "unmute", "toggle_mute"
/// `level`: volume level (0-100) for "set" action
pub async fn volume_control(action: &str, level: Option<u32>) -> Result<serde_json::Value> {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
        use windows::Win32::Media::Audio::{
            IMMDeviceEnumerator, MMDeviceEnumerator, eConsole, eRender,
        };
        use windows::Win32::System::Com::{
            CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
        };

        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                    .map_err(|e| AivyxError::Other(format!("MMDeviceEnumerator: {e}")))?;

            let device = enumerator
                .GetDefaultAudioEndpoint(eRender, eConsole)
                .map_err(|e| AivyxError::Other(format!("GetDefaultAudioEndpoint: {e}")))?;

            let volume: IAudioEndpointVolume = device
                .Activate(CLSCTX_ALL, None)
                .map_err(|e| AivyxError::Other(format!("Activate IAudioEndpointVolume: {e}")))?;

            match action {
                "get" => {
                    let scalar = volume.GetMasterVolumeLevelScalar().map_err(|e| {
                        AivyxError::Other(format!("GetMasterVolumeLevelScalar: {e}"))
                    })?;
                    let muted = volume
                        .GetMute()
                        .map_err(|e| AivyxError::Other(format!("GetMute: {e}")))?;
                    Ok(serde_json::json!({
                        "volume": (scalar * 100.0).round() as u32,
                        "muted": muted.as_bool(),
                    }))
                }
                "set" => {
                    let lvl = level.ok_or_else(|| {
                        AivyxError::Validation("level is required for set".into())
                    })?;
                    let scalar = (lvl.min(100) as f32) / 100.0;
                    volume
                        .SetMasterVolumeLevelScalar(scalar, std::ptr::null())
                        .map_err(|e| {
                            AivyxError::Other(format!("SetMasterVolumeLevelScalar: {e}"))
                        })?;
                    Ok(serde_json::json!({ "volume": lvl.min(100) }))
                }
                "mute" => {
                    volume
                        .SetMute(true, std::ptr::null())
                        .map_err(|e| AivyxError::Other(format!("SetMute: {e}")))?;
                    Ok(serde_json::json!({ "muted": true }))
                }
                "unmute" => {
                    volume
                        .SetMute(false, std::ptr::null())
                        .map_err(|e| AivyxError::Other(format!("SetMute: {e}")))?;
                    Ok(serde_json::json!({ "muted": false }))
                }
                "toggle_mute" => {
                    let muted = volume
                        .GetMute()
                        .map_err(|e| AivyxError::Other(format!("GetMute: {e}")))?;
                    let new_state = !muted.as_bool();
                    volume
                        .SetMute(new_state, std::ptr::null())
                        .map_err(|e| AivyxError::Other(format!("SetMute: {e}")))?;
                    Ok(serde_json::json!({ "muted": new_state }))
                }
                _ => Err(AivyxError::Validation(format!(
                    "Unknown volume action: '{action}'. Valid: get, set, mute, unmute, toggle_mute"
                ))),
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (action, level);
        Err(AivyxError::Other(
            "win_system: only available on Windows".into(),
        ))
    }
}

// ── Brightness Control (WMI WmiMonitorBrightness) ──────────────

/// Get or set the screen brightness.
///
/// `action`: "get", "set"
/// `level`: brightness level (0-100) for "set" action
///
/// Uses PowerShell WMI commands as a portable approach that works across
/// laptop vendors. Direct WMI COM is complex; PowerShell is the pragmatic path.
pub async fn brightness_control(action: &str, level: Option<u32>) -> Result<serde_json::Value> {
    #[cfg(target_os = "windows")]
    {
        match action {
            "get" => {
                let output = tokio::process::Command::new("powershell")
                    .args(["-NoProfile", "-Command",
                        "(Get-WmiObject -Namespace root/WMI -Class WmiMonitorBrightness).CurrentBrightness"])
                    .output()
                    .await
                    .map_err(|e| AivyxError::Other(format!("powershell: {e}")))?;

                if !output.status.success() {
                    return Err(AivyxError::Other(
                        "Brightness query failed (may not be supported on desktop monitors)".into(),
                    ));
                }

                let brightness: u32 = String::from_utf8_lossy(&output.stdout)
                    .trim()
                    .parse()
                    .unwrap_or(0);

                Ok(serde_json::json!({ "brightness": brightness }))
            }
            "set" => {
                let lvl = level
                    .ok_or_else(|| AivyxError::Validation("level is required for set".into()))?
                    .min(100);

                let cmd = format!(
                    "(Get-WmiObject -Namespace root/WMI -Class WmiMonitorBrightnessMethods).WmiSetBrightness(1,{lvl})"
                );
                let output = tokio::process::Command::new("powershell")
                    .args(["-NoProfile", "-Command", &cmd])
                    .output()
                    .await
                    .map_err(|e| AivyxError::Other(format!("powershell: {e}")))?;

                if !output.status.success() {
                    return Err(AivyxError::Other("Brightness set failed".into()));
                }

                Ok(serde_json::json!({ "brightness": lvl }))
            }
            _ => Err(AivyxError::Validation(format!(
                "Unknown brightness action: '{action}'. Valid: get, set"
            ))),
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (action, level);
        Err(AivyxError::Other(
            "win_system: only available on Windows".into(),
        ))
    }
}

// ── Notifications (Windows Toast via PowerShell) ────────────────

/// Show a Windows Toast notification.
///
/// Uses PowerShell [Windows.UI.Notifications] API, which is available
/// on Windows 10+ without requiring an app identity for basic toasts.
pub async fn notify(summary: &str, body: &str) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        // Escape single quotes for PowerShell string.
        let summary_escaped = summary.replace('\'', "''");
        let body_escaped = body.replace('\'', "''");

        let script = format!(
            r#"
            [Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null
            [Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime] | Out-Null
            $xml = New-Object Windows.Data.Xml.Dom.XmlDocument
            $template = '<toast><visual><binding template="ToastGeneric"><text>{summary_escaped}</text><text>{body_escaped}</text></binding></visual></toast>'
            $xml.LoadXml($template)
            $toast = [Windows.UI.Notifications.ToastNotification]::new($xml)
            $notifier = [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('Aivyx PA')
            $notifier.Show($toast)
            "#
        );

        let output = tokio::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .output()
            .await
            .map_err(|e| AivyxError::Other(format!("powershell toast: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(AivyxError::Other(format!(
                "Toast notification failed: {stderr}"
            )));
        }

        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (summary, body);
        Err(AivyxError::Other(
            "win_system: only available on Windows".into(),
        ))
    }
}

// ── File Manager (Explorer) ─────────────────────────────────────

/// Open Explorer at a path, optionally revealing/selecting a specific file.
///
/// Equivalent of `xdg-open` / `nautilus --select` on Linux.
pub async fn file_manager_show(path: &str, reveal: bool) -> Result<String> {
    #[cfg(target_os = "windows")]
    {
        let args = if reveal {
            vec!["/select,", path]
        } else {
            vec![path]
        };

        let output = tokio::process::Command::new("explorer.exe")
            .args(&args)
            .output()
            .await
            .map_err(|e| AivyxError::Other(format!("explorer.exe: {e}")))?;

        // explorer.exe often returns non-zero even on success.
        Ok(format!("Opened Explorer at: {path}"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (path, reveal);
        Err(AivyxError::Other(
            "win_system: only available on Windows".into(),
        ))
    }
}

// ── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_on_non_windows() {
        #[cfg(not(target_os = "windows"))]
        {
            let rt = tokio::runtime::Runtime::new().unwrap();
            assert!(rt.block_on(volume_control("get", None)).is_err());
        }
    }

    #[test]
    fn brightness_on_non_windows() {
        #[cfg(not(target_os = "windows"))]
        {
            let rt = tokio::runtime::Runtime::new().unwrap();
            assert!(rt.block_on(brightness_control("get", None)).is_err());
        }
    }

    #[test]
    fn notify_on_non_windows() {
        #[cfg(not(target_os = "windows"))]
        {
            let rt = tokio::runtime::Runtime::new().unwrap();
            assert!(rt.block_on(notify("test", "body")).is_err());
        }
    }
}
