//! Window management via wmctrl, xdotool, or hyprctl subprocess.
//!
//! Supports minimize, maximize, restore, close, fullscreen, resize, and move
//! operations on the active window or a window matched by title. Tries
//! Hyprland-native commands first, then falls back to X11 tools.

use aivyx_core::{AivyxError, Result};

/// Execute a window management action.
///
/// `action` is one of: minimize, maximize, restore, close, fullscreen, resize, move.
/// `window` optionally matches by title (None = active window).
/// `input` carries `width`, `height`, `x`, `y` for resize/move.
pub async fn manage_window(
    action: &str,
    window: Option<&str>,
    input: &serde_json::Value,
) -> Result<String> {
    match action {
        "minimize" => minimize(window).await,
        "maximize" => maximize(window).await,
        "restore" => restore(window).await,
        "close" => close(window).await,
        "fullscreen" => fullscreen(window).await,
        "resize" => {
            let w = input["width"]
                .as_i64()
                .ok_or_else(|| AivyxError::Validation("width is required for resize".into()))?
                as i32;
            let h = input["height"]
                .as_i64()
                .ok_or_else(|| AivyxError::Validation("height is required for resize".into()))?
                as i32;
            if w < 1 || h < 1 {
                return Err(AivyxError::Validation(
                    "width and height must be positive".into(),
                ));
            }
            resize(window, w, h).await
        }
        "move" => {
            let x = input["x"]
                .as_i64()
                .ok_or_else(|| AivyxError::Validation("x is required for move".into()))?
                as i32;
            let y = input["y"]
                .as_i64()
                .ok_or_else(|| AivyxError::Validation("y is required for move".into()))?
                as i32;
            move_window(window, x, y).await
        }
        other => Err(AivyxError::Validation(format!(
            "Unknown window action: '{other}'. Valid: minimize, maximize, restore, \
             close, fullscreen, resize, move"
        ))),
    }
}

// ── Hyprland (Wayland) ─────────────────────────────────────────

/// Try hyprctl dispatch. Returns Ok if hyprctl is available and succeeds.
async fn hyprctl_dispatch(dispatcher: &str, args: &str) -> Result<String> {
    let output = tokio::process::Command::new("hyprctl")
        .args(["dispatch", dispatcher, args])
        .output()
        .await
        .map_err(|e| AivyxError::Other(format!("hyprctl: {e}")))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(AivyxError::Other(format!(
            "hyprctl dispatch {dispatcher} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

/// Check if Hyprland is running.
async fn has_hyprland() -> bool {
    std::env::var("HYPRLAND_INSTANCE_SIGNATURE").is_ok()
}

// ── X11 helpers ────────────────────────────────────────────────

/// Get window ID via xdotool (active or by title).
async fn xdotool_window_id(window: Option<&str>) -> Result<String> {
    let output = if let Some(title) = window {
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

    let output = output.map_err(|e| AivyxError::Other(format!("xdotool: {e}")))?;
    if !output.status.success() {
        return Err(AivyxError::Other("xdotool: window not found".into()));
    }

    let id = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();

    if id.is_empty() {
        return Err(AivyxError::Other("xdotool: empty window ID".into()));
    }
    Ok(id)
}

/// Run wmctrl with the given arguments.
async fn wmctrl(args: &[&str]) -> Result<String> {
    let output = tokio::process::Command::new("wmctrl")
        .args(args)
        .output()
        .await
        .map_err(|e| AivyxError::Other(format!("wmctrl: {e}")))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(AivyxError::Other(format!(
            "wmctrl failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

/// Run xdotool with the given arguments.
async fn xdotool(args: &[&str]) -> Result<String> {
    let output = tokio::process::Command::new("xdotool")
        .args(args)
        .output()
        .await
        .map_err(|e| AivyxError::Other(format!("xdotool: {e}")))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(AivyxError::Other(format!(
            "xdotool failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

// ── Actions ────────────────────────────────────────────────────

async fn minimize(window: Option<&str>) -> Result<String> {
    if has_hyprland().await {
        // Hyprland: focus window by title, then minimize.
        if let Some(title) = window {
            hyprctl_dispatch("focuswindow", &format!("title:{title}")).await?;
        }
        hyprctl_dispatch("movetoworkspacesilent", "special").await?;
        return Ok("Minimized (Hyprland)".into());
    }

    let id = xdotool_window_id(window).await?;
    xdotool(&["windowminimize", &id]).await?;
    Ok(format!("Minimized window {id}"))
}

async fn maximize(window: Option<&str>) -> Result<String> {
    if has_hyprland().await {
        if let Some(title) = window {
            hyprctl_dispatch("focuswindow", &format!("title:{title}")).await?;
        }
        hyprctl_dispatch("fullscreen", "1").await?;
        return Ok("Maximized (Hyprland)".into());
    }

    // wmctrl: add maximized_vert + maximized_horz.
    let target = if let Some(title) = window {
        vec!["-r", title, "-b", "add,maximized_vert,maximized_horz"]
    } else {
        vec!["-r", ":ACTIVE:", "-b", "add,maximized_vert,maximized_horz"]
    };
    wmctrl(&target.iter().copied().collect::<Vec<_>>()).await?;
    Ok("Maximized".into())
}

async fn restore(window: Option<&str>) -> Result<String> {
    if has_hyprland().await {
        if let Some(title) = window {
            hyprctl_dispatch("focuswindow", &format!("title:{title}")).await?;
        }
        // Toggle fullscreen off if on.
        hyprctl_dispatch("fullscreen", "0").await.ok();
        return Ok("Restored (Hyprland)".into());
    }

    let target = if let Some(title) = window {
        vec!["-r", title, "-b", "remove,maximized_vert,maximized_horz"]
    } else {
        vec![
            "-r",
            ":ACTIVE:",
            "-b",
            "remove,maximized_vert,maximized_horz",
        ]
    };
    wmctrl(&target.iter().copied().collect::<Vec<_>>()).await?;
    Ok("Restored".into())
}

async fn close(window: Option<&str>) -> Result<String> {
    if has_hyprland().await {
        if let Some(title) = window {
            hyprctl_dispatch("focuswindow", &format!("title:{title}")).await?;
        }
        hyprctl_dispatch("killactive", "").await?;
        return Ok("Closed (Hyprland)".into());
    }

    let target = if let Some(title) = window {
        vec!["-c", title]
    } else {
        vec!["-c", ":ACTIVE:"]
    };
    wmctrl(&target.iter().copied().collect::<Vec<_>>()).await?;
    Ok("Closed".into())
}

async fn fullscreen(window: Option<&str>) -> Result<String> {
    if has_hyprland().await {
        if let Some(title) = window {
            hyprctl_dispatch("focuswindow", &format!("title:{title}")).await?;
        }
        hyprctl_dispatch("fullscreen", "0").await?;
        return Ok("Toggled fullscreen (Hyprland)".into());
    }

    let target = if let Some(title) = window {
        vec!["-r", title, "-b", "toggle,fullscreen"]
    } else {
        vec!["-r", ":ACTIVE:", "-b", "toggle,fullscreen"]
    };
    wmctrl(&target.iter().copied().collect::<Vec<_>>()).await?;
    Ok("Toggled fullscreen".into())
}

async fn resize(window: Option<&str>, width: i32, height: i32) -> Result<String> {
    if has_hyprland().await {
        if let Some(title) = window {
            hyprctl_dispatch("focuswindow", &format!("title:{title}")).await?;
        }
        hyprctl_dispatch("resizeactive", &format!("exact {width} {height}")).await?;
        return Ok(format!("Resized to {width}x{height} (Hyprland)"));
    }

    let id = xdotool_window_id(window).await?;
    xdotool(&["windowsize", &id, &width.to_string(), &height.to_string()]).await?;
    Ok(format!("Resized window {id} to {width}x{height}"))
}

async fn move_window(window: Option<&str>, x: i32, y: i32) -> Result<String> {
    if has_hyprland().await {
        if let Some(title) = window {
            hyprctl_dispatch("focuswindow", &format!("title:{title}")).await?;
        }
        hyprctl_dispatch("moveactive", &format!("exact {x} {y}")).await?;
        return Ok(format!("Moved to ({x}, {y}) (Hyprland)"));
    }

    let id = xdotool_window_id(window).await?;
    xdotool(&["windowmove", &id, &x.to_string(), &y.to_string()]).await?;
    Ok(format!("Moved window {id} to ({x}, {y})"))
}

// ── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn manage_rejects_unknown_action() {
        let input = serde_json::json!({});
        let result = manage_window("explode", None, &input).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unknown window action"), "error: {err}");
    }

    #[tokio::test]
    async fn resize_requires_dimensions() {
        let input = serde_json::json!({"width": 800});
        let result = manage_window("resize", None, &input).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("height is required"), "error: {err}");
    }

    #[tokio::test]
    async fn resize_rejects_negative() {
        let input = serde_json::json!({"width": -100, "height": 600});
        let result = manage_window("resize", None, &input).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("positive"), "error: {err}");
    }

    #[tokio::test]
    async fn move_requires_coordinates() {
        let input = serde_json::json!({"x": 100});
        let result = manage_window("move", None, &input).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("y is required"), "error: {err}");
    }
}
