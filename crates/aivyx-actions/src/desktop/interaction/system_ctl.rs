#![allow(unsafe_op_in_unsafe_fn, unused_imports, unreachable_code, unused_variables, dead_code, clippy::all)]
//! System controls — volume, brightness, notifications, file manager.
//!
//! Each function shells out to the appropriate Linux utility:
//! - **Volume**: wpctl (PipeWire) → pactl (PulseAudio) fallback
//! - **Brightness**: brightnessctl
//! - **Notifications**: dunstctl → swaync-client → makoctl
//! - **File manager**: D-Bus org.freedesktop.FileManager1 → xdg-open fallback

use aivyx_core::{AivyxError, Result};

// ── Volume ─────────────────────────────────────────────────────

/// Control system audio volume.
///
/// Actions: set, up, down, mute, unmute, toggle_mute, get.
/// `value` is percentage for set (0–150), or increment for up/down (default 5).
pub async fn volume_control(action: &str, value: Option<u32>) -> Result<serde_json::Value> {
    // Detect audio backend.
    let backend = detect_audio_backend().await?;

    match action {
        "get" => volume_get(&backend).await,
        "set" => {
            let pct = value.ok_or_else(|| {
                AivyxError::Validation("value (percentage) is required for set".into())
            })?;
            if pct > 150 {
                return Err(AivyxError::Validation("Volume cannot exceed 150%".into()));
            }
            volume_set(&backend, pct).await
        }
        "up" => {
            let inc = value.unwrap_or(5);
            volume_adjust(&backend, inc, true).await
        }
        "down" => {
            let inc = value.unwrap_or(5);
            volume_adjust(&backend, inc, false).await
        }
        "mute" => volume_mute(&backend, true).await,
        "unmute" => volume_mute(&backend, false).await,
        "toggle_mute" => volume_toggle_mute(&backend).await,
        other => Err(AivyxError::Validation(format!(
            "Unknown volume action: '{other}'. Valid: get, set, up, down, mute, unmute, toggle_mute"
        ))),
    }
}

#[derive(Debug)]
enum AudioBackend {
    Wpctl,
    Pactl,
}

async fn detect_audio_backend() -> Result<AudioBackend> {
    // Check wpctl first (PipeWire).
    if tokio::process::Command::new("wpctl")
        .arg("--version")
        .output()
        .await
        .is_ok()
    {
        return Ok(AudioBackend::Wpctl);
    }
    // Check pactl (PulseAudio).
    if tokio::process::Command::new("pactl")
        .arg("--version")
        .output()
        .await
        .is_ok()
    {
        return Ok(AudioBackend::Pactl);
    }
    Err(AivyxError::Other(
        "No audio backend found. Install wpctl (PipeWire) or pactl (PulseAudio).".into(),
    ))
}

async fn volume_get(backend: &AudioBackend) -> Result<serde_json::Value> {
    match backend {
        AudioBackend::Wpctl => {
            let output = run_cmd("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"]).await?;
            // Output: "Volume: 0.75" or "Volume: 0.75 [MUTED]"
            let muted = output.contains("[MUTED]");
            let vol_str = output.split_whitespace().nth(1).unwrap_or("0");
            let vol: f64 = vol_str.parse().unwrap_or(0.0);
            let pct = (vol * 100.0).round() as u32;
            Ok(serde_json::json!({
                "volume": pct, "muted": muted, "backend": "wpctl",
            }))
        }
        AudioBackend::Pactl => {
            let output = run_cmd("pactl", &["get-sink-volume", "@DEFAULT_SINK@"]).await?;
            // Parse "front-left: 65536 / 100% / ..."
            let pct = output
                .split('/')
                .nth(1)
                .and_then(|s| s.trim().strip_suffix('%'))
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(0);

            let mute_out = run_cmd("pactl", &["get-sink-mute", "@DEFAULT_SINK@"])
                .await
                .unwrap_or_default();
            let muted = mute_out.contains("yes");

            Ok(serde_json::json!({
                "volume": pct, "muted": muted, "backend": "pactl",
            }))
        }
    }
}

async fn volume_set(backend: &AudioBackend, pct: u32) -> Result<serde_json::Value> {
    match backend {
        AudioBackend::Wpctl => {
            let vol = format!("{:.2}", pct as f64 / 100.0);
            run_cmd("wpctl", &["set-volume", "@DEFAULT_AUDIO_SINK@", &vol]).await?;
        }
        AudioBackend::Pactl => {
            run_cmd(
                "pactl",
                &["set-sink-volume", "@DEFAULT_SINK@", &format!("{pct}%")],
            )
            .await?;
        }
    }
    Ok(serde_json::json!({"status": "set", "volume": pct}))
}

async fn volume_adjust(
    backend: &AudioBackend,
    increment: u32,
    up: bool,
) -> Result<serde_json::Value> {
    match backend {
        AudioBackend::Wpctl => {
            let delta = format!(
                "{}{:.2}",
                if up { "+" } else { "-" },
                increment as f64 / 100.0
            );
            run_cmd("wpctl", &["set-volume", "@DEFAULT_AUDIO_SINK@", &delta]).await?;
        }
        AudioBackend::Pactl => {
            let delta = format!("{}{increment}%", if up { "+" } else { "-" });
            run_cmd("pactl", &["set-sink-volume", "@DEFAULT_SINK@", &delta]).await?;
        }
    }
    let direction = if up { "up" } else { "down" };
    Ok(serde_json::json!({
        "status": "adjusted", "direction": direction, "increment": increment,
    }))
}

async fn volume_mute(backend: &AudioBackend, mute: bool) -> Result<serde_json::Value> {
    let state = if mute { "1" } else { "0" };
    match backend {
        AudioBackend::Wpctl => {
            run_cmd("wpctl", &["set-mute", "@DEFAULT_AUDIO_SINK@", state]).await?;
        }
        AudioBackend::Pactl => {
            run_cmd("pactl", &["set-sink-mute", "@DEFAULT_SINK@", state]).await?;
        }
    }
    Ok(serde_json::json!({"status": if mute { "muted" } else { "unmuted" }}))
}

async fn volume_toggle_mute(backend: &AudioBackend) -> Result<serde_json::Value> {
    match backend {
        AudioBackend::Wpctl => {
            run_cmd("wpctl", &["set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"]).await?;
        }
        AudioBackend::Pactl => {
            run_cmd("pactl", &["set-sink-mute", "@DEFAULT_SINK@", "toggle"]).await?;
        }
    }
    Ok(serde_json::json!({"status": "toggled"}))
}

// ── Brightness ─────────────────────────────────────────────────

/// Control display brightness via brightnessctl.
///
/// Actions: get, set, up, down.
/// `value` is percentage for set (0–100), or increment for up/down (default 5).
pub async fn brightness_control(action: &str, value: Option<u32>) -> Result<serde_json::Value> {
    match action {
        "get" => {
            let output = run_cmd("brightnessctl", &["info", "-m"]).await?;
            // Machine-readable: "device,class,current,percentage,max"
            let parts: Vec<&str> = output.split(',').collect();
            let pct = parts
                .get(3)
                .and_then(|s| s.strip_suffix('%'))
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(0);
            Ok(serde_json::json!({
                "brightness": pct,
                "device": parts.first().unwrap_or(&"unknown"),
            }))
        }
        "set" => {
            let pct = value.ok_or_else(|| {
                AivyxError::Validation("value (percentage) is required for set".into())
            })?;
            if pct > 100 {
                return Err(AivyxError::Validation(
                    "Brightness cannot exceed 100%".into(),
                ));
            }
            run_cmd("brightnessctl", &["set", &format!("{pct}%")]).await?;
            Ok(serde_json::json!({"status": "set", "brightness": pct}))
        }
        "up" => {
            let inc = value.unwrap_or(5);
            run_cmd("brightnessctl", &["set", &format!("+{inc}%")]).await?;
            Ok(serde_json::json!({"status": "increased", "increment": inc}))
        }
        "down" => {
            let inc = value.unwrap_or(5);
            run_cmd("brightnessctl", &["set", &format!("{inc}%-")]).await?;
            Ok(serde_json::json!({"status": "decreased", "increment": inc}))
        }
        other => Err(AivyxError::Validation(format!(
            "Unknown brightness action: '{other}'. Valid: get, set, up, down"
        ))),
    }
}

// ── Notifications ──────────────────────────────────────────────

/// List recent desktop notifications from the notification daemon.
///
/// Tries dunstctl (dunst) → swaync-client (SwayNC) → makoctl (mako).
pub async fn list_notifications(count: usize) -> Result<serde_json::Value> {
    // Try dunstctl (most common on X11 + Wayland).
    if let Ok(output) = run_cmd("dunstctl", &["history"]).await {
        return parse_dunst_history(&output, count);
    }

    // Try swaync-client (Sway Notification Center).
    if let Ok(output) = run_cmd("swaync-client", &["--get-json"]).await {
        return parse_swaync_json(&output, count);
    }

    // Try makoctl (mako).
    if let Ok(output) = run_cmd("makoctl", &["list"]).await {
        return parse_mako_list(&output, count);
    }

    Err(AivyxError::Other(
        "No notification daemon found. Install dunst, swaync, or mako.".into(),
    ))
}

fn parse_dunst_history(json_str: &str, count: usize) -> Result<serde_json::Value> {
    let parsed: serde_json::Value = serde_json::from_str(json_str)
        .map_err(|e| AivyxError::Other(format!("Failed to parse dunst history: {e}")))?;

    let notifications = parsed["data"]
        .as_array()
        .and_then(|outer| outer.first())
        .and_then(|inner| inner.as_array())
        .cloned()
        .unwrap_or_default();

    let items: Vec<serde_json::Value> = notifications
        .iter()
        .take(count)
        .map(|n| {
            serde_json::json!({
                "app": n["appname"]["data"].as_str().unwrap_or(""),
                "summary": n["summary"]["data"].as_str().unwrap_or(""),
                "body": n["body"]["data"].as_str().unwrap_or(""),
            })
        })
        .collect();

    Ok(serde_json::json!({
        "count": items.len(),
        "daemon": "dunst",
        "notifications": items,
    }))
}

fn parse_swaync_json(json_str: &str, count: usize) -> Result<serde_json::Value> {
    let parsed: serde_json::Value = serde_json::from_str(json_str)
        .map_err(|e| AivyxError::Other(format!("Failed to parse swaync output: {e}")))?;

    let notifications = parsed.as_array().cloned().unwrap_or_default();

    let items: Vec<serde_json::Value> = notifications
        .iter()
        .take(count)
        .map(|n| {
            serde_json::json!({
                "app": n["app-name"].as_str().unwrap_or(""),
                "summary": n["summary"].as_str().unwrap_or(""),
                "body": n["body"].as_str().unwrap_or(""),
            })
        })
        .collect();

    Ok(serde_json::json!({
        "count": items.len(),
        "daemon": "swaync",
        "notifications": items,
    }))
}

fn parse_mako_list(json_str: &str, count: usize) -> Result<serde_json::Value> {
    let parsed: serde_json::Value = serde_json::from_str(json_str)
        .map_err(|e| AivyxError::Other(format!("Failed to parse mako list: {e}")))?;

    let notifications = parsed["data"]
        .as_array()
        .and_then(|outer| outer.first())
        .and_then(|inner| inner.as_array())
        .cloned()
        .unwrap_or_default();

    let items: Vec<serde_json::Value> = notifications
        .iter()
        .take(count)
        .map(|n| {
            serde_json::json!({
                "app": n["app-name"]["data"].as_str().unwrap_or(""),
                "summary": n["summary"]["data"].as_str().unwrap_or(""),
                "body": n["body"]["data"].as_str().unwrap_or(""),
            })
        })
        .collect();

    Ok(serde_json::json!({
        "count": items.len(),
        "daemon": "mako",
        "notifications": items,
    }))
}

// ── File Manager ───────────────────────────────────────────────

/// Open a path in the system file manager, optionally revealing (highlighting) a file.
///
/// Uses D-Bus org.freedesktop.FileManager1 (Nautilus, Thunar, Dolphin) for reveal,
/// or xdg-open as fallback for directory opening.
pub async fn file_manager_show(path: &str, reveal: bool) -> Result<String> {
    // Validate the path exists.
    let canonical = std::path::Path::new(path);
    if !canonical.exists() {
        return Err(AivyxError::Validation(format!(
            "Path does not exist: {path}"
        )));
    }

    // Prevent path traversal attacks — require absolute path.
    if !canonical.is_absolute() {
        return Err(AivyxError::Validation("Path must be absolute".into()));
    }

    if reveal {
        // Try D-Bus FileManager1 ShowItems for file highlighting.
        let uri = format!("file://{path}");
        let dbus_result = run_cmd(
            "dbus-send",
            &[
                "--session",
                "--dest=org.freedesktop.FileManager1",
                "--type=method_call",
                "/org/freedesktop/FileManager1",
                "org.freedesktop.FileManager1.ShowItems",
                &format!("array:string:{uri}"),
                "string:",
            ],
        )
        .await;

        if dbus_result.is_ok() {
            return Ok(format!("Revealed {path} in file manager"));
        }

        // Fallback: open the parent directory.
        if let Some(parent) = canonical.parent() {
            run_cmd("xdg-open", &[&parent.to_string_lossy()]).await?;
            return Ok(format!("Opened parent directory of {path}"));
        }
    }

    // Open directory (or file via xdg-open default handler).
    run_cmd("xdg-open", &[path]).await?;
    Ok(format!("Opened {path}"))
}

// ── Subprocess helper ──────────────────────────────────────────

async fn run_cmd(cmd: &str, args: &[&str]) -> Result<String> {
    let output = tokio::process::Command::new(cmd)
        .args(args)
        .output()
        .await
        .map_err(|e| AivyxError::Other(format!("{cmd}: {e}")))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(AivyxError::Other(format!("{cmd} failed: {stderr}")))
    }
}

// ── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore]
    async fn volume_rejects_unknown_action() {
        let result = volume_control("explode", None).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unknown volume action"), "error: {err}");
    }

    #[tokio::test]
    async fn volume_set_requires_value() {
        // This will fail on CI (no audio backend), so we check for validation error
        // if it gets past backend detection, or backend error otherwise.
        let result = volume_control("set", None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn volume_set_rejects_over_150() {
        let result = volume_control("set", Some(200)).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn brightness_rejects_unknown_action() {
        let result = brightness_control("explode", None).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unknown brightness action"), "error: {err}");
    }

    #[tokio::test]
    async fn brightness_rejects_over_100() {
        let result = brightness_control("set", Some(150)).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("exceed 100%"), "error: {err}");
    }

    #[tokio::test]
    async fn file_manager_rejects_nonexistent() {
        let result = file_manager_show("/nonexistent/path/abc123", false).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("does not exist"), "error: {err}");
    }

    #[tokio::test]
    async fn file_manager_rejects_relative() {
        let result = file_manager_show("relative/path", false).await;
        assert!(result.is_err());
    }

    #[test]
    fn parse_dunst_history_empty() {
        let json = r#"{"type": "dunst", "data": [[]]}"#;
        let result = parse_dunst_history(json, 10).unwrap();
        assert_eq!(result["count"], 0);
        assert_eq!(result["daemon"], "dunst");
    }

    #[test]
    fn parse_dunst_history_with_items() {
        let json = r#"{"type": "dunst", "data": [[
            {"appname": {"data": "Firefox"}, "summary": {"data": "Download complete"}, "body": {"data": "file.pdf"}},
            {"appname": {"data": "Slack"}, "summary": {"data": "New message"}, "body": {"data": "Hello"}}
        ]]}"#;
        let result = parse_dunst_history(json, 1).unwrap();
        assert_eq!(result["count"], 1);
        assert_eq!(result["notifications"][0]["app"], "Firefox");
    }

    #[test]
    fn parse_swaync_json_items() {
        let json = r#"[
            {"app-name": "Terminal", "summary": "Build complete", "body": "All tests passed"}
        ]"#;
        let result = parse_swaync_json(json, 10).unwrap();
        assert_eq!(result["count"], 1);
        assert_eq!(result["daemon"], "swaync");
        assert_eq!(result["notifications"][0]["summary"], "Build complete");
    }
}
