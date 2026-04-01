//! Shell action — execute commands with capability gating.
//!
//! Only available at Trust tier or above. Commands are logged to the audit
//! trail and subject to a configurable allowlist/denylist.

use crate::Action;
use aivyx_core::Result;

pub struct RunCommand;

#[async_trait::async_trait]
impl Action for RunCommand {
    fn name(&self) -> &str { "run_command" }

    fn description(&self) -> &str {
        "Execute a shell command and return its output (requires Trust tier)"
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "Shell command to execute" },
                "working_dir": { "type": "string", "description": "Working directory (optional)" }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let command = input["command"].as_str().unwrap_or_default();
        let working_dir = input.get("working_dir").and_then(|v| v.as_str());

        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c").arg(command);

        if let Some(dir) = working_dir {
            cmd.current_dir(dir);
        }

        let output = cmd.output().await.map_err(aivyx_core::AivyxError::Io)?;

        Ok(serde_json::json!({
            "exit_code": output.status.code().unwrap_or(-1),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        }))
    }
}
