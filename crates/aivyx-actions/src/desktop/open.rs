//! Open application action — launch apps, files, and URLs.

use crate::Action;
use aivyx_core::Result;

use super::{DesktopConfig, is_app_denied, is_path_denied};

pub struct OpenApplication {
    pub config: DesktopConfig,
}

#[async_trait::async_trait]
impl Action for OpenApplication {
    fn name(&self) -> &str {
        "open_application"
    }

    fn description(&self) -> &str {
        "Open a file, URL, or application on the desktop. Uses the system's default handler \
         (xdg-open) for files and URLs. Can also launch named applications directly \
         (e.g., 'firefox', 'gimp photo.png', 'gnome-terminal'). Package managers and \
         system admin tools are blocked for safety."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "target": {
                    "type": "string",
                    "description": "URL, file path, or application name to open"
                },
                "args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Additional arguments (e.g., a file to open with the app)"
                }
            },
            "required": ["target"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let target = input["target"]
            .as_str()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("target is required".into()))?;

        if target.trim().is_empty() {
            return Err(aivyx_core::AivyxError::Validation(
                "target must not be empty".into(),
            ));
        }

        let args: Vec<String> = input
            .get("args")
            .and_then(|a| a.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let is_url = target.starts_with("http://")
            || target.starts_with("https://")
            || target.starts_with("file://");

        let is_file = !is_url && std::path::Path::new(target).exists();

        // URLs and existing files → xdg-open (respects default handlers)
        if is_url || is_file {
            if is_file && let Some(frag) = is_path_denied(target) {
                return Err(aivyx_core::AivyxError::CapabilityDenied(format!(
                    "Cannot open sensitive path (matched: {frag})"
                )));
            }

            let mut cmd = tokio::process::Command::new("xdg-open");
            cmd.arg(target);
            // Detach: don't wait for the GUI app to exit
            cmd.stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());

            match cmd.spawn() {
                Ok(_) => Ok(serde_json::json!({
                    "status": "launched",
                    "target": target,
                    "method": "xdg-open",
                })),
                Err(e) => Err(aivyx_core::AivyxError::Other(format!(
                    "Failed to launch xdg-open: {e}"
                ))),
            }
        } else {
            // Named application — direct spawn (no shell)
            if let Some(reason) = is_app_denied(target, &self.config) {
                return Err(aivyx_core::AivyxError::CapabilityDenied(format!(
                    "Application blocked: {reason}"
                )));
            }

            // Check file args for sensitive paths
            for arg in &args {
                if let Some(frag) = is_path_denied(arg) {
                    return Err(aivyx_core::AivyxError::CapabilityDenied(format!(
                        "Cannot open sensitive path in args (matched: {frag})"
                    )));
                }
            }

            let mut cmd = tokio::process::Command::new(target);
            cmd.args(&args);
            cmd.stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());

            match cmd.spawn() {
                Ok(_) => Ok(serde_json::json!({
                    "status": "launched",
                    "target": target,
                    "method": "direct",
                    "args": args,
                })),
                Err(e) => Err(aivyx_core::AivyxError::Other(format!(
                    "Failed to launch {target}: {e}"
                ))),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> DesktopConfig {
        DesktopConfig::default()
    }

    #[tokio::test]
    async fn rejects_empty_target() {
        let action = OpenApplication {
            config: default_config(),
        };
        let result = action.execute(serde_json::json!({ "target": "" })).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn rejects_denied_app() {
        let action = OpenApplication {
            config: default_config(),
        };
        let result = action
            .execute(serde_json::json!({ "target": "bash" }))
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("blocked"), "error: {err}");
    }

    #[tokio::test]
    async fn rejects_sensitive_path() {
        let action = OpenApplication {
            config: default_config(),
        };
        let result = action
            .execute(serde_json::json!({ "target": "vim", "args": ["/home/user/.ssh/id_rsa"] }))
            .await;
        assert!(result.is_err());
    }

    #[test]
    fn detects_url_targets() {
        assert!("https://example.com".starts_with("https://"));
        assert!("http://localhost".starts_with("http://"));
        assert!(!"firefox".starts_with("http"));
    }
}
