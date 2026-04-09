//! Window management actions — list, focus, and query windows via wmctrl/xdotool.
//!
//! X11 only. On Wayland, these tools return a clear error explaining the limitation.

use crate::Action;
use aivyx_core::Result;

use super::{DEFAULT_TIMEOUT_SECS, DisplayServer, detect_display_server, run_desktop_command};

fn require_x11() -> Result<()> {
    match detect_display_server() {
        DisplayServer::X11 => Ok(()),
        DisplayServer::Wayland => Err(aivyx_core::AivyxError::Other(
            "Window management requires X11. Wayland compositors do not expose a \
             universal window list API. Use compositor-specific tools (e.g., swaymsg) instead."
                .into(),
        )),
        DisplayServer::Unknown => Err(aivyx_core::AivyxError::Other(
            "No display server detected (neither WAYLAND_DISPLAY nor DISPLAY set)".into(),
        )),
    }
}

// ── ListWindows ───────────────────────────────────────────────────

pub struct ListWindows;

#[async_trait::async_trait]
impl Action for ListWindows {
    fn name(&self) -> &str {
        "list_windows"
    }

    fn description(&self) -> &str {
        "List all open windows on the desktop, showing window ID, desktop number, PID, \
         and title. Useful for understanding what the user has open. X11 only."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
        })
    }

    async fn execute(&self, _input: serde_json::Value) -> Result<serde_json::Value> {
        require_x11()?;

        let output = run_desktop_command("wmctrl", &["-l", "-p"], DEFAULT_TIMEOUT_SECS).await?;

        if output.exit_code != 0 {
            return Err(aivyx_core::AivyxError::Other(format!(
                "wmctrl failed (exit {}). Is wmctrl installed? stderr: {}",
                output.exit_code,
                output.stderr.trim()
            )));
        }

        let windows: Vec<serde_json::Value> = output
            .stdout
            .lines()
            .filter_map(parse_wmctrl_line)
            .collect();

        Ok(serde_json::json!({
            "windows": windows,
            "count": windows.len(),
        }))
    }
}

/// Parse a line of `wmctrl -l -p` output into a JSON object.
///
/// Format: `0x04a00003  0 12345 hostname Window Title Here`
fn parse_wmctrl_line(line: &str) -> Option<serde_json::Value> {
    let parts: Vec<&str> = line.splitn(5, char::is_whitespace).collect();
    if parts.len() < 5 {
        return None;
    }
    // Skip empty segments from multiple spaces
    let mut iter = line.split_whitespace();
    let id = iter.next()?;
    let desktop = iter.next()?;
    let pid = iter.next()?;
    let _hostname = iter.next()?;
    let title: String = iter.collect::<Vec<&str>>().join(" ");

    if title.is_empty() {
        return None;
    }

    Some(serde_json::json!({
        "id": id,
        "desktop": desktop,
        "pid": pid,
        "title": title,
    }))
}

// ── GetActiveWindow ───────────────────────────────────────────────

pub struct GetActiveWindow;

#[async_trait::async_trait]
impl Action for GetActiveWindow {
    fn name(&self) -> &str {
        "get_active_window"
    }

    fn description(&self) -> &str {
        "Get the title and application class of the currently active (focused) window. \
         Useful for context-aware actions. X11 only."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
        })
    }

    async fn execute(&self, _input: serde_json::Value) -> Result<serde_json::Value> {
        require_x11()?;

        // Get window ID
        let id_output =
            run_desktop_command("xdotool", &["getactivewindow"], DEFAULT_TIMEOUT_SECS).await?;

        if id_output.exit_code != 0 {
            return Err(aivyx_core::AivyxError::Other(format!(
                "xdotool failed (exit {}). Is xdotool installed? stderr: {}",
                id_output.exit_code,
                id_output.stderr.trim()
            )));
        }

        let window_id = id_output.stdout.trim();

        // Get title
        let title_output = run_desktop_command(
            "xdotool",
            &["getactivewindow", "getwindowname"],
            DEFAULT_TIMEOUT_SECS,
        )
        .await?;

        // Get class
        let class_output = run_desktop_command(
            "xdotool",
            &["getactivewindow", "getwindowclassname"],
            DEFAULT_TIMEOUT_SECS,
        )
        .await?;

        Ok(serde_json::json!({
            "window_id": window_id,
            "title": title_output.stdout.trim(),
            "class": class_output.stdout.trim(),
        }))
    }
}

// ── FocusWindow ───────────────────────────────────────────────────

pub struct FocusWindow;

#[async_trait::async_trait]
impl Action for FocusWindow {
    fn name(&self) -> &str {
        "focus_window"
    }

    fn description(&self) -> &str {
        "Bring a window to the front by its title (substring match) or window ID. \
         Use list_windows first to find the target window. X11 only."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "Substring of the window title to match"
                },
                "window_id": {
                    "type": "string",
                    "description": "Window ID from list_windows (e.g., '0x04a00003')"
                }
            },
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let title = input.get("title").and_then(|v| v.as_str());
        let window_id = input.get("window_id").and_then(|v| v.as_str());

        let (args, target_desc) = match (title, window_id) {
            (_, Some(id)) => {
                // Prefer window ID if both provided
                (vec!["-i", "-a", id], format!("id {id}"))
            }
            (Some(t), None) => (vec!["-a", t], format!("title \"{t}\"")),
            (None, None) => {
                return Err(aivyx_core::AivyxError::Validation(
                    "Either title or window_id is required".into(),
                ));
            }
        };

        require_x11()?;

        let output = run_desktop_command("wmctrl", &args, DEFAULT_TIMEOUT_SECS).await?;

        if output.exit_code != 0 {
            return Err(aivyx_core::AivyxError::Other(format!(
                "Failed to focus window ({}). wmctrl stderr: {}",
                target_desc,
                output.stderr.trim()
            )));
        }

        Ok(serde_json::json!({
            "status": "focused",
            "target": target_desc,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_wmctrl_output() {
        let line = "0x04a00003  0 12345 myhost Firefox - Google";
        let result = parse_wmctrl_line(line);
        assert!(result.is_some());
        let val = result.unwrap();
        assert_eq!(val["id"], "0x04a00003");
        assert_eq!(val["desktop"], "0");
        assert_eq!(val["pid"], "12345");
        assert_eq!(val["title"], "Firefox - Google");
    }

    #[test]
    fn parse_wmctrl_multiword_title() {
        let line = "0x06000003  1 98765 myhost Code - main.rs - Visual Studio Code";
        let val = parse_wmctrl_line(line).unwrap();
        assert_eq!(val["title"], "Code - main.rs - Visual Studio Code");
    }

    #[test]
    fn parse_wmctrl_empty_title_skipped() {
        let line = "0x06000003  1 98765 myhost";
        assert!(parse_wmctrl_line(line).is_none());
    }

    #[tokio::test]
    async fn focus_window_rejects_no_args() {
        let action = FocusWindow;
        let result = action.execute(serde_json::json!({})).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("title or window_id"), "error: {err}");
    }
}
