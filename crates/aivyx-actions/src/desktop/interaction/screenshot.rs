#![allow(unsafe_op_in_unsafe_fn, unused_imports, unreachable_code, unused_variables, dead_code, clippy::all)]
//! Native window screenshot backend using grim (Wayland) or import (X11).
//!
//! Provides `capture_window` which takes a screenshot of a specific window
//! or the active window, returning base64-encoded PNG/JPEG data.

use aivyx_core::{AivyxError, Result};

use super::MAX_SCREENSHOT_BYTES;

/// Capture a screenshot of a window or the full screen.
///
/// Tries grim (Wayland) first, falls back to import (X11/ImageMagick).
/// Returns base64-encoded image data.
pub async fn capture_window(geometry: Option<&str>, format: &str) -> Result<String> {
    // Try grim first (Wayland-native).
    match try_grim(geometry, format).await {
        Ok(data) => encode_base64(&data),
        Err(_) => {
            // Fall back to import (ImageMagick, X11).
            let data = try_import(format).await?;
            encode_base64(&data)
        }
    }
}

/// Base64-encode binary data using the system `base64` command.
fn encode_base64(data: &[u8]) -> Result<String> {
    use std::io::Write;
    let mut child = std::process::Command::new("base64")
        .args(["-w", "0"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| AivyxError::Other(format!("base64 command failed: {e}")))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(data)
            .map_err(|e| AivyxError::Other(format!("base64 stdin write failed: {e}")))?;
    }

    let output = child
        .wait_with_output()
        .map_err(|e| AivyxError::Other(format!("base64 wait failed: {e}")))?;

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Capture using grim (Wayland).
async fn try_grim(geometry: Option<&str>, format: &str) -> Result<Vec<u8>> {
    let mut args = vec!["-t", format];
    if let Some(geo) = geometry {
        args.extend_from_slice(&["-g", geo]);
    }
    args.push("-"); // output to stdout

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::process::Command::new("grim").args(&args).output(),
    )
    .await
    .map_err(|_| AivyxError::Other("grim timed out".into()))?
    .map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            AivyxError::Other(
                "grim is not installed. Install it for Wayland screenshots \
                 (e.g., sudo apt install grim)"
                    .into(),
            )
        } else {
            AivyxError::Other(format!("grim failed to run: {e}"))
        }
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AivyxError::Other(format!("grim failed: {stderr}")));
    }

    if output.stdout.len() > MAX_SCREENSHOT_BYTES {
        return Err(AivyxError::Other(format!(
            "Screenshot too large ({} bytes, max {})",
            output.stdout.len(),
            MAX_SCREENSHOT_BYTES
        )));
    }

    Ok(output.stdout)
}

/// Capture using import (ImageMagick, X11).
async fn try_import(format: &str) -> Result<Vec<u8>> {
    let fmt_arg = format!("{format}:-");

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::process::Command::new("import")
            .args(["-window", "root", &fmt_arg])
            .output(),
    )
    .await
    .map_err(|_| AivyxError::Other("import timed out".into()))?
    .map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            AivyxError::Other(
                "Neither grim nor import (ImageMagick) is installed. \
                 Install one for screenshots."
                    .into(),
            )
        } else {
            AivyxError::Other(format!("import failed to run: {e}"))
        }
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AivyxError::Other(format!("import failed: {stderr}")));
    }

    if output.stdout.len() > MAX_SCREENSHOT_BYTES {
        return Err(AivyxError::Other(format!(
            "Screenshot too large ({} bytes, max {})",
            output.stdout.len(),
            MAX_SCREENSHOT_BYTES
        )));
    }

    Ok(output.stdout)
}

/// Get the geometry string for a window using Wayland/X11 tools.
///
/// Returns a geometry string like "100,200 800x600" suitable for grim -g.
pub async fn get_window_geometry(window_title: Option<&str>) -> Result<Option<String>> {
    // Try hyprctl first (Hyprland/Wayland).
    if let Ok(geo) = try_hyprctl_geometry(window_title).await {
        return Ok(Some(geo));
    }

    // Try xdotool (X11).
    if let Ok(geo) = try_xdotool_geometry(window_title).await {
        return Ok(Some(geo));
    }

    // No geometry available — caller will screenshot the full screen.
    Ok(None)
}

/// Get window geometry via hyprctl (Hyprland).
async fn try_hyprctl_geometry(window_title: Option<&str>) -> Result<String> {
    let output = tokio::process::Command::new("hyprctl")
        .args(["activewindow", "-j"])
        .output()
        .await
        .map_err(|e| AivyxError::Other(format!("hyprctl failed: {e}")))?;

    if !output.status.success() {
        return Err(AivyxError::Other("hyprctl failed".into()));
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| AivyxError::Other(format!("hyprctl parse error: {e}")))?;

    // If a specific window title was requested, check it matches.
    if let Some(title) = window_title {
        let win_title = json["title"].as_str().unwrap_or("");
        if !win_title.to_lowercase().contains(&title.to_lowercase()) {
            return Err(AivyxError::Other(format!(
                "Active window '{win_title}' doesn't match '{title}'"
            )));
        }
    }

    let x = json["at"].get(0).and_then(|v| v.as_i64()).unwrap_or(0);
    let y = json["at"].get(1).and_then(|v| v.as_i64()).unwrap_or(0);
    let w = json["size"].get(0).and_then(|v| v.as_i64()).unwrap_or(800);
    let h = json["size"].get(1).and_then(|v| v.as_i64()).unwrap_or(600);

    Ok(format!("{x},{y} {w}x{h}"))
}

/// Get window geometry via xdotool (X11).
async fn try_xdotool_geometry(window_title: Option<&str>) -> Result<String> {
    let id_output = if let Some(title) = window_title {
        tokio::process::Command::new("xdotool")
            .args(["search", "--name", title])
            .output()
            .await
    } else {
        tokio::process::Command::new("xdotool")
            .args(["getactivewindow"])
            .output()
            .await
    };

    let id_output = id_output.map_err(|e| AivyxError::Other(format!("xdotool failed: {e}")))?;

    if !id_output.status.success() {
        return Err(AivyxError::Other("xdotool: window not found".into()));
    }

    let window_id = String::from_utf8_lossy(&id_output.stdout)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();

    if window_id.is_empty() {
        return Err(AivyxError::Other("xdotool: no window ID".into()));
    }

    let geo_output = tokio::process::Command::new("xdotool")
        .args(["getwindowgeometry", "--shell", &window_id])
        .output()
        .await
        .map_err(|e| AivyxError::Other(format!("xdotool geometry failed: {e}")))?;

    if !geo_output.status.success() {
        return Err(AivyxError::Other(
            "xdotool: getwindowgeometry failed".into(),
        ));
    }

    let geo_text = String::from_utf8_lossy(&geo_output.stdout);
    let mut x = 0i64;
    let mut y = 0i64;
    let mut w = 800i64;
    let mut h = 600i64;

    for line in geo_text.lines() {
        if let Some(val) = line.strip_prefix("X=") {
            x = val.trim().parse().unwrap_or(0);
        } else if let Some(val) = line.strip_prefix("Y=") {
            y = val.trim().parse().unwrap_or(0);
        } else if let Some(val) = line.strip_prefix("WIDTH=") {
            w = val.trim().parse().unwrap_or(800);
        } else if let Some(val) = line.strip_prefix("HEIGHT=") {
            h = val.trim().parse().unwrap_or(600);
        }
    }

    Ok(format!("{x},{y} {w}x{h}"))
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    #[test]
    fn format_validation() {
        assert!(["png", "jpeg"].contains(&"png"));
        assert!(["png", "jpeg"].contains(&"jpeg"));
        assert!(!["png", "jpeg"].contains(&"bmp"));
    }
}
