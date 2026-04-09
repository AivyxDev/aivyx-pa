//! Desktop information — running applications, workspaces, and window listing.
//!
//! Provides system-level awareness of the desktop environment: what apps are
//! running, which workspace is active, and how to switch between them.
//! Supports both X11 (wmctrl/xdotool) and Wayland (hyprctl for Hyprland).

use aivyx_core::{AivyxError, Result};

// ── Running Applications ───────────────────────────────────────

/// List all running GUI applications with their window titles, classes, and PIDs.
pub async fn list_running_apps() -> Result<serde_json::Value> {
    // Try hyprctl first (Hyprland/Wayland).
    if std::env::var("HYPRLAND_INSTANCE_SIGNATURE").is_ok() {
        return list_apps_hyprctl().await;
    }

    // Try wmctrl (X11).
    if let Ok(result) = list_apps_wmctrl().await {
        return Ok(result);
    }

    // Fallback: xlsclients (basic X11).
    list_apps_xlsclients().await
}

async fn list_apps_hyprctl() -> Result<serde_json::Value> {
    let output = tokio::process::Command::new("hyprctl")
        .args(["clients", "-j"])
        .output()
        .await
        .map_err(|e| AivyxError::Other(format!("hyprctl: {e}")))?;

    if !output.status.success() {
        return Err(AivyxError::Other("hyprctl clients failed".into()));
    }

    let clients: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| AivyxError::Other(format!("hyprctl parse: {e}")))?;

    let apps: Vec<serde_json::Value> = clients
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|c| {
            serde_json::json!({
                "title": c["title"].as_str().unwrap_or(""),
                "class": c["class"].as_str().unwrap_or(""),
                "pid": c["pid"].as_u64().unwrap_or(0),
                "workspace": c["workspace"]["name"].as_str().unwrap_or(""),
                "focused": c["focusHistoryID"].as_i64() == Some(0),
            })
        })
        .collect();

    Ok(serde_json::json!({
        "count": apps.len(),
        "backend": "hyprctl",
        "apps": apps,
    }))
}

async fn list_apps_wmctrl() -> Result<serde_json::Value> {
    let output = tokio::process::Command::new("wmctrl")
        .args(["-l", "-p"])
        .output()
        .await
        .map_err(|e| AivyxError::Other(format!("wmctrl: {e}")))?;

    if !output.status.success() {
        return Err(AivyxError::Other("wmctrl -l failed".into()));
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let apps: Vec<serde_json::Value> = text
        .lines()
        .filter_map(|line| {
            // wmctrl -l -p format: "0x04000003  0 12345  hostname Window Title"
            let parts: Vec<&str> = line.splitn(5, char::is_whitespace).collect();
            if parts.len() < 5 {
                return None;
            }
            let wid = parts[0].trim();
            let workspace = parts[1].trim();
            let pid = parts[2].trim();
            // parts[3] is hostname, parts[4..] is title
            let title = parts[4..].join(" ").trim().to_string();

            Some(serde_json::json!({
                "title": title,
                "window_id": wid,
                "pid": pid.parse::<u64>().unwrap_or(0),
                "workspace": workspace.parse::<i32>().unwrap_or(-1),
                "class": get_class_sync(wid),
            }))
        })
        .collect();

    Ok(serde_json::json!({
        "count": apps.len(),
        "backend": "wmctrl",
        "apps": apps,
    }))
}

/// Synchronous window class lookup for wmctrl results (best-effort).
fn get_class_sync(window_id: &str) -> String {
    std::process::Command::new("xprop")
        .args(["-id", window_id, "WM_CLASS"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                let s = String::from_utf8_lossy(&o.stdout);
                // WM_CLASS(STRING) = "instance", "Class"
                s.split('"')
                    .nth(3) // The class name (second quoted string)
                    .map(|c| c.to_string())
            } else {
                None
            }
        })
        .unwrap_or_default()
}

async fn list_apps_xlsclients() -> Result<serde_json::Value> {
    let output = tokio::process::Command::new("xlsclients")
        .args(["-l"])
        .output()
        .await
        .map_err(|e| {
            AivyxError::Other(format!(
                "No window lister available. Install wmctrl or use Hyprland. Error: {e}"
            ))
        })?;

    if !output.status.success() {
        return Err(AivyxError::Other("xlsclients failed".into()));
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = text.lines().collect();
    let mut apps = Vec::new();
    let mut current_name = String::new();

    for line in &lines {
        let trimmed = line.trim();
        if trimmed.starts_with("Window") {
            current_name.clear();
        } else if trimmed.starts_with("Name:") {
            current_name = trimmed
                .strip_prefix("Name:")
                .unwrap_or("")
                .trim()
                .to_string();
            if !current_name.is_empty() {
                apps.push(serde_json::json!({
                    "title": current_name,
                    "class": "",
                    "pid": 0,
                }));
            }
        }
    }

    Ok(serde_json::json!({
        "count": apps.len(),
        "backend": "xlsclients",
        "apps": apps,
    }))
}

// ── Workspace Management ───────────────────────────────────────

/// Control and query workspaces.
///
/// Actions:
/// - `list` — list all workspaces
/// - `current` — get the active workspace
/// - `switch` — switch to a workspace by name or number
/// - `move_window` — move the active (or named) window to a workspace
pub async fn workspace_control(
    action: &str,
    target: Option<&str>,
    window: Option<&str>,
) -> Result<serde_json::Value> {
    match action {
        "list" => workspace_list().await,
        "current" => workspace_current().await,
        "switch" => {
            let target = target.ok_or_else(|| {
                AivyxError::Validation("target workspace is required for switch".into())
            })?;
            workspace_switch(target).await
        }
        "move_window" => {
            let target = target.ok_or_else(|| {
                AivyxError::Validation("target workspace is required for move_window".into())
            })?;
            workspace_move_window(target, window).await
        }
        other => Err(AivyxError::Validation(format!(
            "Unknown workspace action: '{other}'. Valid: list, current, switch, move_window"
        ))),
    }
}

async fn workspace_list() -> Result<serde_json::Value> {
    if std::env::var("HYPRLAND_INSTANCE_SIGNATURE").is_ok() {
        let output = run_cmd("hyprctl", &["workspaces", "-j"]).await?;
        let workspaces: serde_json::Value = serde_json::from_str(&output)
            .map_err(|e| AivyxError::Other(format!("hyprctl parse: {e}")))?;

        let items: Vec<serde_json::Value> = workspaces
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|ws| {
                serde_json::json!({
                    "id": ws["id"],
                    "name": ws["name"].as_str().unwrap_or(""),
                    "windows": ws["windows"].as_u64().unwrap_or(0),
                    "monitor": ws["monitor"].as_str().unwrap_or(""),
                })
            })
            .collect();

        return Ok(serde_json::json!({
            "count": items.len(),
            "backend": "hyprctl",
            "workspaces": items,
        }));
    }

    // X11: wmctrl -d
    let output = run_cmd("wmctrl", &["-d"]).await?;
    let workspaces: Vec<serde_json::Value> = output
        .lines()
        .filter_map(|line| {
            // "0  * DG: 1920x1080  VP: 0,0  WA: 0,0 1920x1080  Workspace 1"
            let parts: Vec<&str> = line.splitn(2, char::is_whitespace).collect();
            let id = parts.first()?.trim();
            let active = line.contains(" * ");
            let name = line.rsplit("  ").next().unwrap_or("").trim();

            Some(serde_json::json!({
                "id": id.parse::<i32>().unwrap_or(-1),
                "name": name,
                "active": active,
            }))
        })
        .collect();

    Ok(serde_json::json!({
        "count": workspaces.len(),
        "backend": "wmctrl",
        "workspaces": workspaces,
    }))
}

async fn workspace_current() -> Result<serde_json::Value> {
    if std::env::var("HYPRLAND_INSTANCE_SIGNATURE").is_ok() {
        let output = run_cmd("hyprctl", &["activeworkspace", "-j"]).await?;
        let ws: serde_json::Value = serde_json::from_str(&output)
            .map_err(|e| AivyxError::Other(format!("hyprctl parse: {e}")))?;

        return Ok(serde_json::json!({
            "id": ws["id"],
            "name": ws["name"].as_str().unwrap_or(""),
            "windows": ws["windows"].as_u64().unwrap_or(0),
            "backend": "hyprctl",
        }));
    }

    // X11: find the active workspace from wmctrl -d.
    let output = run_cmd("wmctrl", &["-d"]).await?;
    for line in output.lines() {
        if line.contains(" * ") {
            let id = line.split_whitespace().next().unwrap_or("0");
            let name = line.rsplit("  ").next().unwrap_or("").trim();
            return Ok(serde_json::json!({
                "id": id.parse::<i32>().unwrap_or(0),
                "name": name,
                "active": true,
                "backend": "wmctrl",
            }));
        }
    }

    Err(AivyxError::Other(
        "Could not determine active workspace".into(),
    ))
}

async fn workspace_switch(target: &str) -> Result<serde_json::Value> {
    if std::env::var("HYPRLAND_INSTANCE_SIGNATURE").is_ok() {
        run_cmd("hyprctl", &["dispatch", "workspace", target]).await?;
        return Ok(serde_json::json!({
            "status": "switched",
            "workspace": target,
            "backend": "hyprctl",
        }));
    }

    // X11: wmctrl -s <workspace_number>
    run_cmd("wmctrl", &["-s", target]).await?;
    Ok(serde_json::json!({
        "status": "switched",
        "workspace": target,
        "backend": "wmctrl",
    }))
}

async fn workspace_move_window(target: &str, window: Option<&str>) -> Result<serde_json::Value> {
    if std::env::var("HYPRLAND_INSTANCE_SIGNATURE").is_ok() {
        if let Some(title) = window {
            run_cmd(
                "hyprctl",
                &["dispatch", "focuswindow", &format!("title:{title}")],
            )
            .await?;
        }
        run_cmd("hyprctl", &["dispatch", "movetoworkspace", target]).await?;
        return Ok(serde_json::json!({
            "status": "moved",
            "workspace": target,
            "backend": "hyprctl",
        }));
    }

    // X11: wmctrl -r <window> -t <workspace>
    let win = window.unwrap_or(":ACTIVE:");
    run_cmd("wmctrl", &["-r", win, "-t", target]).await?;
    Ok(serde_json::json!({
        "status": "moved",
        "workspace": target,
        "window": win,
        "backend": "wmctrl",
    }))
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
    async fn workspace_rejects_unknown_action() {
        let result = workspace_control("teleport", None, None).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unknown workspace action"), "error: {err}");
    }

    #[tokio::test]
    async fn workspace_switch_requires_target() {
        let result = workspace_control("switch", None, None).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("target workspace is required"), "error: {err}");
    }

    #[tokio::test]
    async fn workspace_move_requires_target() {
        let result = workspace_control("move_window", None, None).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("target workspace is required"), "error: {err}");
    }
}
