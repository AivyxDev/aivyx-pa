//! Shell action — execute commands with capability gating.
//!
//! Only available at Trust tier or above. Commands are validated against
//! a denylist of dangerous patterns, executed with a timeout, and have
//! output size capped to prevent OOM.

use crate::Action;
use aivyx_core::Result;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

/// Maximum command execution time (seconds).
const COMMAND_TIMEOUT_SECS: u64 = 60;

/// Maximum output size in bytes (1 MB).
const MAX_OUTPUT_BYTES: usize = 1_024 * 1_024;

/// Patterns that are always denied regardless of capability tier.
/// These prevent catastrophic damage even if the LLM is confused or injected.
const DENIED_PATTERNS: &[&str] = &[
    "rm -rf /", "rm -rf /*", "mkfs", "dd if=", "> /dev/sd",
    "cat /etc/shadow", "cat /etc/passwd",
    "chmod 777 /", "chown root",
    "nc -e", "ncat -e", "bash -i >& /dev/tcp", "/dev/tcp/", "curl.*|.*sh", "wget.*|.*sh",
    "shutdown", "reboot", "init 0", "init 6", "poweroff", "halt",
];

const DENIED_PATH_FRAGMENTS: &[&str] = &[
    "/etc/shadow", "/.ssh/", "/.gnupg/", "/.aws/credentials", "/.config/gcloud",
];

fn is_command_denied(command: &str) -> Option<&'static str> {
    let lower = command.to_lowercase();
    for pattern in DENIED_PATTERNS {
        if lower.contains(&pattern.to_lowercase()) {
            return Some(pattern);
        }
    }
    DENIED_PATH_FRAGMENTS.iter().find(|&path| lower.contains(&path.to_lowercase())).map(|v| v as _)
}

fn cap_output(bytes: &[u8]) -> String {
    if bytes.len() <= MAX_OUTPUT_BYTES {
        String::from_utf8_lossy(bytes).into_owned()
    } else {
        let truncated = String::from_utf8_lossy(&bytes[..MAX_OUTPUT_BYTES]);
        format!("{truncated}\n...[output truncated at {MAX_OUTPUT_BYTES} bytes]")
    }
}

// ── Background Tasks Map ─────────────────────────────────────────────────────────

static BACKGROUND_TASKS: OnceLock<Mutex<HashMap<String, BackgroundTask>>> = OnceLock::new();

fn get_tasks_map() -> &'static Mutex<HashMap<String, BackgroundTask>> {
    BACKGROUND_TASKS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub struct BackgroundTask {
    pub command: String,
    pub pid: Option<u32>,
    pub stdin: Option<tokio::process::ChildStdin>,
    pub stdout_buf: Arc<Mutex<Vec<u8>>>,
    pub stderr_buf: Arc<Mutex<Vec<u8>>>,
    pub exited: Arc<Mutex<Option<i32>>>,
}

struct WriteAdapter(Arc<Mutex<Vec<u8>>>);

impl tokio::io::AsyncWrite for WriteAdapter {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::result::Result<usize, std::io::Error>> {
        let mut try_lock = match self.0.try_lock() {
            Ok(lock) => lock,
            Err(_) => return std::task::Poll::Ready(Ok(buf.len())),
        };
        try_lock.extend_from_slice(buf);
        if try_lock.len() > MAX_OUTPUT_BYTES {
            let overflow = try_lock.len() - MAX_OUTPUT_BYTES;
            try_lock.drain(0..overflow);
        }
        std::task::Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: std::pin::Pin<&mut Self>, _cx: &mut std::task::Context<'_>) -> std::task::Poll<std::result::Result<(), std::io::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: std::pin::Pin<&mut Self>, _cx: &mut std::task::Context<'_>) -> std::task::Poll<std::result::Result<(), std::io::Error>> {
        std::task::Poll::Ready(Ok(()))
    }
}

// ── RunCommand ─────────────────────────────────────────────────────────

pub struct RunCommand;

#[async_trait::async_trait]
impl Action for RunCommand {
    fn name(&self) -> &str { "run_command" }

    fn description(&self) -> &str {
        "Execute a shell command and return its output (requires Trust tier). \
         Dangerous commands are blocked."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": { "type": "string" },
                "working_dir": { "type": "string" }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let command = input["command"].as_str().unwrap_or_default();
        if command.trim().is_empty() {
            return Err(aivyx_core::AivyxError::Validation("command empty".into()));
        }
        if let Some(pattern) = is_command_denied(command) {
            return Err(aivyx_core::AivyxError::CapabilityDenied(format!("Blocked: {pattern}")));
        }

        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c").arg(command);
        if let Some(dir) = input.get("working_dir").and_then(|v| v.as_str()) {
            cmd.current_dir(dir);
        }

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(COMMAND_TIMEOUT_SECS),
            cmd.output(),
        ).await;

        match result {
            Ok(Ok(output)) => {
                Ok(serde_json::json!({
                    "exit_code": output.status.code().unwrap_or(-1),
                    "stdout": cap_output(&output.stdout),
                    "stderr": cap_output(&output.stderr),
                }))
            }
            Ok(Err(e)) => Err(aivyx_core::AivyxError::Io(e)),
            Err(_) => Err(aivyx_core::AivyxError::Other("Timeout".into())),
        }
    }
}

// ── SpawnBackgroundCommand ───────────────────────────────────────────────

pub struct SpawnBackgroundCommand;

#[async_trait::async_trait]
impl Action for SpawnBackgroundCommand {
    fn name(&self) -> &str { "spawn_background_command" }

    fn description(&self) -> &str {
        "Spawn a background shell command interactively."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": { "type": "string" },
                "working_dir": { "type": "string" }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let command = input["command"].as_str().unwrap_or_default();
        if command.trim().is_empty() {
            return Err(aivyx_core::AivyxError::Validation("command empty".into()));
        }
        if let Some(pattern) = is_command_denied(command) {
            return Err(aivyx_core::AivyxError::CapabilityDenied(format!("Blocked: {pattern}")));
        }

        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c").arg(command);
        if let Some(dir) = input.get("working_dir").and_then(|v| v.as_str()) {
            cmd.current_dir(dir);
        }

        use std::process::Stdio;
        cmd.stdin(Stdio::piped())
           .stdout(Stdio::piped())
           .stderr(Stdio::piped());

        let mut child = cmd.spawn().map_err(aivyx_core::AivyxError::Io)?;

        let stdin = child.stdin.take();
        let stdout_buf = Arc::new(Mutex::new(Vec::new()));
        let stderr_buf = Arc::new(Mutex::new(Vec::new()));

        if let Some(mut stdout) = child.stdout.take() {
            let buf_clone = stdout_buf.clone();
            tokio::spawn(async move {
                let _ = tokio::io::copy(&mut stdout, &mut WriteAdapter(buf_clone)).await;
            });
        }

        if let Some(mut stderr) = child.stderr.take() {
            let buf_clone = stderr_buf.clone();
            tokio::spawn(async move {
                let _ = tokio::io::copy(&mut stderr, &mut WriteAdapter(buf_clone)).await;
            });
        }

        let exited = Arc::new(Mutex::new(None));
        let exited_clone = exited.clone();
        
        let pid = child.id();

        tokio::spawn(async move {
            if let Ok(status) = child.wait().await {
                *exited_clone.lock().await = Some(status.code().unwrap_or(-1));
            } else {
                *exited_clone.lock().await = Some(-1);
            }
        });

        let task_id = uuid::Uuid::new_v4().to_string();
        let task = BackgroundTask {
            command: command.to_string(),
            pid,
            stdin,
            stdout_buf,
            stderr_buf,
            exited,
        };

        get_tasks_map().lock().await.insert(task_id.clone(), task);

        Ok(serde_json::json!({
            "task_id": task_id,
            "status": "Running",
            "pid": pid,
        }))
    }
}

// ── GetCommandStatus ───────────────────────────────────────────────

pub struct GetCommandStatus;

#[async_trait::async_trait]
impl Action for GetCommandStatus {
    fn name(&self) -> &str { "get_command_status" }

    fn description(&self) -> &str {
        "Check status and get output of a background command."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task_id": { "type": "string" },
                "clear_output": { "type": "boolean" }
            },
            "required": ["task_id"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let task_id = input["task_id"].as_str().unwrap_or("");
        let clear = input["clear_output"].as_bool().unwrap_or(false);

        let map = get_tasks_map().lock().await;
        if let Some(task) = map.get(task_id) {
            let mut out = task.stdout_buf.lock().await;
            let mut err = task.stderr_buf.lock().await;
            
            let stdout_str = cap_output(&out);
            let stderr_str = cap_output(&err);
            
            if clear {
                out.clear();
                err.clear();
            }

            let exited = task.exited.lock().await;
            Ok(serde_json::json!({
                "task_id": task_id,
                "running": exited.is_none(),
                "exit_code": *exited,
                "stdout": stdout_str,
                "stderr": stderr_str,
            }))
        } else {
            Err(aivyx_core::AivyxError::Validation("Task not found".into()))
        }
    }
}

// ── SendCommandInput ───────────────────────────────────────────────

pub struct SendCommandInput;

#[async_trait::async_trait]
impl Action for SendCommandInput {
    fn name(&self) -> &str { "send_command_input" }

    fn description(&self) -> &str {
        "Write to stdin of a background command."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task_id": { "type": "string" },
                "input": { "type": "string" }
            },
            "required": ["task_id", "input"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let task_id = input["task_id"].as_str().unwrap_or("");
        let in_str = input["input"].as_str().unwrap_or("");
        
        let mut map = get_tasks_map().lock().await;
        if let Some(task) = map.get_mut(task_id) {
            if let Some(ref mut stdin) = task.stdin {
                stdin.write_all(in_str.as_bytes()).await.map_err(aivyx_core::AivyxError::Io)?;
                stdin.flush().await.map_err(aivyx_core::AivyxError::Io)?;
                Ok(serde_json::json!({ "status": "sent" }))
            } else {
                Err(aivyx_core::AivyxError::Validation("Stdin closed".into()))
            }
        } else {
            Err(aivyx_core::AivyxError::Validation("Task not found".into()))
        }
    }
}

// ── ListBackgroundCommands ────────────────────────────────────────

pub struct ListBackgroundCommands;

#[async_trait::async_trait]
impl Action for ListBackgroundCommands {
    fn name(&self) -> &str { "list_background_commands" }

    fn description(&self) -> &str {
        "List all tracked background commands."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }

    async fn execute(&self, _input: serde_json::Value) -> Result<serde_json::Value> {
        let map = get_tasks_map().lock().await;
        let mut tasks = Vec::new();
        
        for (id, task) in map.iter() {
            let exited = task.exited.lock().await;
            tasks.push(serde_json::json!({
                "task_id": id,
                "command": task.command,
                "pid": task.pid,
                "running": exited.is_none(),
                "exit_code": *exited
            }));
        }
        Ok(serde_json::json!({ "tasks": tasks }))
    }
}

// ── CancelBackgroundCommand ───────────────────────────────────────

pub struct CancelBackgroundCommand;

#[async_trait::async_trait]
impl Action for CancelBackgroundCommand {
    fn name(&self) -> &str { "cancel_background_command" }

    fn description(&self) -> &str {
        "Kill a background command by task_id."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": { "task_id": { "type": "string" } },
            "required": ["task_id"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let task_id = input["task_id"].as_str().unwrap_or("");
        
        let mut map = get_tasks_map().lock().await;
        if let Some(task) = map.remove(task_id) {
            if let Some(pid) = task.pid {
                let _ = tokio::process::Command::new("kill").arg("-9").arg(pid.to_string()).output().await;
            }
            Ok(serde_json::json!({ "status": "killed", "task_id": task_id }))
        } else {
            Err(aivyx_core::AivyxError::Validation("Invalid task_id".into()))
        }
    }
}
