//! Clipboard actions — read/write system clipboard text.

use crate::Action;
use aivyx_core::Result;

use super::{DEFAULT_TIMEOUT_SECS, DisplayServer, detect_display_server, run_desktop_command};

/// Maximum clipboard write size (1 MB).
const MAX_WRITE_BYTES: usize = 1_024 * 1_024;

pub struct ClipboardRead;

#[async_trait::async_trait]
impl Action for ClipboardRead {
    fn name(&self) -> &str {
        "clipboard_read"
    }

    fn description(&self) -> &str {
        "Read the current text content from the system clipboard. Returns the clipboard text, \
         or an empty string if the clipboard is empty or contains non-text data."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
        })
    }

    async fn execute(&self, _input: serde_json::Value) -> Result<serde_json::Value> {
        let (program, args): (&str, Vec<&str>) = match detect_display_server() {
            DisplayServer::Wayland => ("wl-paste", vec!["--no-newline"]),
            DisplayServer::X11 => ("xclip", vec!["-selection", "clipboard", "-o"]),
            DisplayServer::Unknown => {
                return Err(aivyx_core::AivyxError::Other(
                    "No display server detected (neither WAYLAND_DISPLAY nor DISPLAY set)".into(),
                ));
            }
        };

        let output = run_desktop_command(program, &args, DEFAULT_TIMEOUT_SECS).await?;

        if output.exit_code != 0 && output.stdout.is_empty() {
            // Non-zero exit with no output typically means empty clipboard
            return Ok(serde_json::json!({
                "text": "",
                "note": "clipboard is empty or contains non-text data",
            }));
        }

        Ok(serde_json::json!({
            "text": output.stdout,
        }))
    }
}

pub struct ClipboardWrite;

#[async_trait::async_trait]
impl Action for ClipboardWrite {
    fn name(&self) -> &str {
        "clipboard_write"
    }

    fn description(&self) -> &str {
        "Write text to the system clipboard, making it available for pasting into other \
         applications. Useful for transferring data to GUI apps."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "Text content to write to the clipboard"
                }
            },
            "required": ["text"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let text = input["text"]
            .as_str()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("text is required".into()))?;

        if text.len() > MAX_WRITE_BYTES {
            return Err(aivyx_core::AivyxError::Validation(format!(
                "Clipboard write too large ({} bytes, max {})",
                text.len(),
                MAX_WRITE_BYTES
            )));
        }

        let ds = detect_display_server();

        let (program, args): (&str, &[&str]) = match ds {
            DisplayServer::Wayland => ("wl-copy", &[]),
            DisplayServer::X11 => ("xclip", &["-selection", "clipboard"]),
            DisplayServer::Unknown => {
                return Err(aivyx_core::AivyxError::Other(
                    "No display server detected (neither WAYLAND_DISPLAY nor DISPLAY set)".into(),
                ));
            }
        };

        // Pipe text via stdin
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(DEFAULT_TIMEOUT_SECS),
            async {
                let mut child = tokio::process::Command::new(program)
                    .args(args)
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
                    .map_err(|e| {
                        aivyx_core::AivyxError::Other(format!("Failed to run {program}: {e}"))
                    })?;

                if let Some(mut stdin) = child.stdin.take() {
                    use tokio::io::AsyncWriteExt;
                    stdin.write_all(text.as_bytes()).await.map_err(|e| {
                        aivyx_core::AivyxError::Other(format!(
                            "Failed to write to {program} stdin: {e}"
                        ))
                    })?;
                    // Drop stdin to close the pipe and let the process finish
                }

                child
                    .wait()
                    .await
                    .map_err(|e| aivyx_core::AivyxError::Other(format!("{program} failed: {e}")))
            },
        )
        .await;

        match result {
            Ok(Ok(status)) if status.success() => Ok(serde_json::json!({
                "status": "written",
                "bytes": text.len(),
            })),
            Ok(Ok(status)) => Err(aivyx_core::AivyxError::Other(format!(
                "{program} exited with status {}",
                status.code().unwrap_or(-1)
            ))),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(aivyx_core::AivyxError::Other(format!(
                "{program} timed out after {DEFAULT_TIMEOUT_SECS}s"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_oversized_write() {
        let action = ClipboardWrite;
        let huge = "x".repeat(MAX_WRITE_BYTES + 1);
        let result = action.execute(serde_json::json!({ "text": huge })).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("too large"), "error: {err}");
    }

    #[tokio::test]
    async fn rejects_missing_text() {
        let action = ClipboardWrite;
        let result = action.execute(serde_json::json!({})).await;
        assert!(result.is_err());
    }
}
