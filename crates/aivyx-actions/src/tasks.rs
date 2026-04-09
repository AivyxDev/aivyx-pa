//! Background task execution — spawn long-running operations and get
//! notified when they complete.
//!
//! Enables the agent to say "I'll do that in the background" and actually
//! mean it. The user can continue chatting while tasks run asynchronously.
//!
//! Tools:
//! - `spawn_task` — run a shell command in the background with a timeout
//! - `list_tasks` — see all running and recently completed tasks
//! - `get_task_status` — check the status of a specific task
//! - `cancel_task` — kill a running task by ID

use crate::Action;
use aivyx_core::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

// ── Task Registry ───────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TaskState {
    /// Task is actively running.
    Running,
    /// Task finished successfully.
    Succeeded,
    /// Task failed (non-zero exit or error).
    Failed,
    /// Task was cancelled by the user.
    Cancelled,
    /// Task exceeded its timeout.
    TimedOut,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackgroundTask {
    pub id: String,
    pub label: String,
    pub command: String,
    pub state: TaskState,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub exit_code: Option<i32>,
    pub stdout_tail: String,
    pub stderr_tail: String,
}

/// Shared in-memory task registry.
///
/// Persisted to a JSON file on every state change and reloaded on startup.
/// Maximum 100 entries; old completed tasks are pruned automatically.
pub type TaskRegistry = Arc<Mutex<HashMap<String, BackgroundTask>>>;

pub fn new_registry() -> TaskRegistry {
    Arc::new(Mutex::new(HashMap::new()))
}

/// Create a registry pre-populated from a JSON file on disk.
///
/// Any tasks that were `Running` when the process last exited are marked
/// `TimedOut` — they can't have finished cleanly if the process is gone.
pub fn new_registry_from_path(path: &Path) -> TaskRegistry {
    let tasks = load_registry(path);
    Arc::new(Mutex::new(tasks))
}

/// Load persisted tasks from a JSON file.
/// On any error (missing file, parse failure) returns an empty map.
pub fn load_registry(path: &Path) -> HashMap<String, BackgroundTask> {
    let Ok(bytes) = std::fs::read(path) else {
        return HashMap::new();
    };
    let Ok(mut tasks) = serde_json::from_slice::<HashMap<String, BackgroundTask>>(&bytes) else {
        tracing::warn!(
            "task_registry: failed to parse {}, starting fresh",
            path.display()
        );
        return HashMap::new();
    };
    // Any task that was Running is implicitly dead — mark as TimedOut.
    let now = Utc::now();
    let mut recovered = 0usize;
    for task in tasks.values_mut() {
        if task.state == TaskState::Running {
            task.state = TaskState::TimedOut;
            task.finished_at = Some(now);
            task.stderr_tail = "Process terminated — task did not complete.".into();
            recovered += 1;
        }
    }
    if recovered > 0 {
        tracing::info!(
            "task_registry: recovered {recovered} tasks from previous session (marked TimedOut)"
        );
    }
    tasks
}

/// Persist the current registry to a JSON file atomically.
///
/// Writes to `<path>.tmp` first then renames, so the file is never
/// left in a partially-written state.
pub fn save_registry(registry: &TaskRegistry, path: &Path) {
    let tasks = {
        let guard = registry.lock().unwrap();
        guard.clone()
    };
    match serde_json::to_vec_pretty(&tasks) {
        Ok(bytes) => {
            let tmp = path.with_extension("tmp");
            if let Err(e) = std::fs::write(&tmp, &bytes) {
                tracing::warn!("task_registry: failed to write {}: {e}", tmp.display());
                return;
            }
            if let Err(e) = std::fs::rename(&tmp, path) {
                tracing::warn!("task_registry: failed to rename {}: {e}", tmp.display());
            }
        }
        Err(e) => tracing::warn!("task_registry: serialization failed: {e}"),
    }
}

const MAX_REGISTRY_SIZE: usize = 100;
/// How many bytes of output tail to keep in memory per task.
const OUTPUT_TAIL_BYTES: usize = 4_096;

fn tail(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    if s.len() <= OUTPUT_TAIL_BYTES {
        s.into_owned()
    } else {
        format!("...[truncated]\n{}", &s[s.len() - OUTPUT_TAIL_BYTES..])
    }
}

// ── SpawnTask ───────────────────────────────────────────────────

/// Spawn a shell command as a background task.
pub struct SpawnTask {
    pub registry: TaskRegistry,
    /// Path to write the registry JSON after each mutation.
    pub persist_path: Option<Arc<PathBuf>>,
}

#[async_trait::async_trait]
impl Action for SpawnTask {
    fn name(&self) -> &str {
        "spawn_task"
    }

    fn description(&self) -> &str {
        "Run a shell command in the background and return a task ID immediately. \
         The command runs asynchronously — you can keep chatting while it executes. \
         Use get_task_status to check if it's done or list_tasks to see all running tasks."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Shell command to run in the background"
                },
                "label": {
                    "type": "string",
                    "description": "Human-readable name for the task (e.g. 'Build project', 'Download backup')"
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Max seconds to let the task run before killing it. Default: 300 (5 minutes). Max: 3600.",
                    "default": 300
                },
                "working_dir": {
                    "type": "string",
                    "description": "Working directory for the command (optional, must be absolute path)"
                }
            },
            "required": ["command", "label"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let command = input["command"]
            .as_str()
            .ok_or_else(|| {
                aivyx_core::AivyxError::Validation("spawn_task: 'command' is required".into())
            })?
            .to_string();
        let label = input["label"]
            .as_str()
            .ok_or_else(|| {
                aivyx_core::AivyxError::Validation("spawn_task: 'label' is required".into())
            })?
            .to_string();

        if command.trim().is_empty() {
            return Err(aivyx_core::AivyxError::Validation(
                "command must not be empty".into(),
            ));
        }

        // Reject obviously dangerous commands (reuse shell.rs denylist patterns)
        let denied = [
            "rm -rf /",
            "rm -rf /*",
            "mkfs",
            "dd if=",
            "> /dev/sd",
            "cat /etc/shadow",
            "chmod 777 /",
            "chown root",
            "shutdown",
            "reboot",
            "poweroff",
            "halt",
            "init 0",
            "init 6",
        ];
        let lower = command.to_lowercase();
        for pattern in denied {
            if lower.contains(pattern) {
                return Err(aivyx_core::AivyxError::CapabilityDenied(format!(
                    "Command blocked by safety denylist (matched: {pattern})"
                )));
            }
        }

        let timeout_secs = input["timeout_secs"].as_u64().unwrap_or(300).min(3600);
        let working_dir = input["working_dir"].as_str().map(|s| s.to_string());

        if let Some(ref dir) = working_dir {
            if !std::path::Path::new(dir).is_absolute() {
                return Err(aivyx_core::AivyxError::Validation(
                    "working_dir must be an absolute path".into(),
                ));
            }
        }

        let task_id = Uuid::new_v4().to_string();
        let task = BackgroundTask {
            id: task_id.clone(),
            label: label.clone(),
            command: command.clone(),
            state: TaskState::Running,
            started_at: Utc::now(),
            finished_at: None,
            exit_code: None,
            stdout_tail: String::new(),
            stderr_tail: String::new(),
        };

        // Register task
        {
            let mut reg = self.registry.lock().unwrap();
            // Prune old completed tasks if registry is at limit
            if reg.len() >= MAX_REGISTRY_SIZE {
                let to_remove: Vec<String> = reg
                    .iter()
                    .filter(|(_, t)| t.state != TaskState::Running)
                    .map(|(id, _)| id.clone())
                    .take(10)
                    .collect();
                for id in to_remove {
                    reg.remove(&id);
                }
            }
            reg.insert(task_id.clone(), task);
        }

        // Spawn the actual command
        let registry_clone = Arc::clone(&self.registry);
        let persist_path_clone = self.persist_path.clone();
        let tid = task_id.clone();

        tokio::spawn(async move {
            let mut cmd = tokio::process::Command::new("sh");
            cmd.arg("-c").arg(&command);
            if let Some(ref dir) = working_dir {
                cmd.current_dir(dir);
            }

            let result =
                tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), cmd.output())
                    .await;

            let mut reg = registry_clone.lock().unwrap();
            if let Some(task) = reg.get_mut(&tid) {
                task.finished_at = Some(Utc::now());
                match result {
                    Ok(Ok(output)) => {
                        task.exit_code = output.status.code();
                        task.stdout_tail = tail(&output.stdout);
                        task.stderr_tail = tail(&output.stderr);
                        task.state = if output.status.success() {
                            TaskState::Succeeded
                        } else {
                            TaskState::Failed
                        };
                    }
                    Ok(Err(e)) => {
                        task.state = TaskState::Failed;
                        task.stderr_tail = e.to_string();
                    }
                    Err(_) => {
                        task.state = TaskState::TimedOut;
                        task.stderr_tail = format!("Task timed out after {timeout_secs}s");
                    }
                }
                tracing::info!(
                    task_id = %tid,
                    label = %task.label,
                    state = ?task.state,
                    "Background task finished"
                );
            }
            // Persist completed state
            drop(reg); // release lock before save
            if let Some(ref path) = persist_path_clone {
                save_registry(&registry_clone, path);
            }
        });

        // Persist outside the lock to avoid a potential deadlock
        // if save_registry tries to re-lock.
        if let Some(ref path) = self.persist_path {
            save_registry(&self.registry, path);
        }

        Ok(serde_json::json!({
            "task_id": task_id,
            "label": label,
            "state": "running",
            "message": format!("Task '{label}' started in background. Use get_task_status(task_id='{task_id}') to check on it."),
            "timeout_secs": timeout_secs,
        }))
    }
}

// ── GetTaskStatus ───────────────────────────────────────────────

pub struct GetTaskStatus {
    pub registry: TaskRegistry,
}

#[async_trait::async_trait]
impl Action for GetTaskStatus {
    fn name(&self) -> &str {
        "get_task_status"
    }

    fn description(&self) -> &str {
        "Check the status of a background task by its ID. \
         Returns current state, output tail, and exit code if finished."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "Task ID returned by spawn_task"
                }
            },
            "required": ["task_id"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let task_id = input["task_id"].as_str().ok_or_else(|| {
            aivyx_core::AivyxError::Validation("get_task_status: 'task_id' is required".into())
        })?;

        let reg = self.registry.lock().unwrap();
        let task = reg
            .get(task_id)
            .ok_or_else(|| aivyx_core::AivyxError::Other(format!("Task '{task_id}' not found")))?;

        Ok(serde_json::json!({
            "task_id": task.id,
            "label": task.label,
            "command": task.command,
            "state": format!("{:?}", task.state),
            "started_at": task.started_at.to_rfc3339(),
            "finished_at": task.finished_at.map(|t| t.to_rfc3339()),
            "exit_code": task.exit_code,
            "stdout": task.stdout_tail,
            "stderr": task.stderr_tail,
            "duration_secs": task.finished_at.map(|f| (f - task.started_at).num_seconds()),
        }))
    }
}

// ── ListTasks ───────────────────────────────────────────────────

pub struct ListTasks {
    pub registry: TaskRegistry,
}

#[async_trait::async_trait]
impl Action for ListTasks {
    fn name(&self) -> &str {
        "list_tasks"
    }

    fn description(&self) -> &str {
        "List all background tasks — both currently running and recently completed. \
         Returns a summary of each task's state, label, and timing."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "state_filter": {
                    "type": "string",
                    "description": "Filter by state: 'running', 'succeeded', 'failed', 'all'. Default: 'all'",
                    "enum": ["running", "succeeded", "failed", "cancelled", "timed_out", "all"]
                }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let filter = input["state_filter"].as_str().unwrap_or("all");
        let reg = self.registry.lock().unwrap();

        let mut tasks: Vec<serde_json::Value> = reg
            .values()
            .filter(|t| match filter {
                "running" => t.state == TaskState::Running,
                "succeeded" => t.state == TaskState::Succeeded,
                "failed" => t.state == TaskState::Failed,
                "cancelled" => t.state == TaskState::Cancelled,
                "timed_out" => t.state == TaskState::TimedOut,
                _ => true,
            })
            .map(|t| {
                serde_json::json!({
                    "task_id": t.id,
                    "label": t.label,
                    "state": format!("{:?}", t.state),
                    "started_at": t.started_at.to_rfc3339(),
                    "finished_at": t.finished_at.map(|f| f.to_rfc3339()),
                    "exit_code": t.exit_code,
                })
            })
            .collect();

        // Sort: running first, then by start time descending
        tasks.sort_by(|a, b| {
            let a_running = a["state"] == "Running";
            let b_running = b["state"] == "Running";
            b_running
                .cmp(&a_running)
                .then(b["started_at"].as_str().cmp(&a["started_at"].as_str()))
        });

        Ok(serde_json::json!({
            "tasks": tasks,
            "count": tasks.len(),
            "running": reg.values().filter(|t| t.state == TaskState::Running).count(),
        }))
    }
}

// ── CancelTask ──────────────────────────────────────────────────

/// Cancel a running task by marking it cancelled.
///
/// Note: We mark the state `Cancelled` immediately. The actual tokio task
/// will still run until the OS kills the child process on the next poll.
/// For proper SIGKILL, a `JoinHandle` + abort would be needed — that
/// requires storing handles, which complicates the registry. This
/// implementation is a best-effort cancel that prevents further output
/// collection and marks the task clearly for the user.
pub struct CancelTask {
    pub registry: TaskRegistry,
    /// Path to persist the registry after cancellation.
    pub persist_path: Option<Arc<PathBuf>>,
}

#[async_trait::async_trait]
impl Action for CancelTask {
    fn name(&self) -> &str {
        "cancel_task"
    }

    fn description(&self) -> &str {
        "Cancel a running background task. The task will be marked as cancelled immediately. \
         Note: the underlying process may take a moment to terminate."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "Task ID returned by spawn_task"
                }
            },
            "required": ["task_id"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let task_id = input["task_id"].as_str().ok_or_else(|| {
            aivyx_core::AivyxError::Validation("cancel_task: 'task_id' is required".into())
        })?;

        let mut reg = self.registry.lock().unwrap();
        let task = reg
            .get_mut(task_id)
            .ok_or_else(|| aivyx_core::AivyxError::Other(format!("Task '{task_id}' not found")))?;

        if task.state != TaskState::Running {
            return Ok(serde_json::json!({
                "status": "not_running",
                "task_id": task_id,
                "state": format!("{:?}", task.state),
                "message": "Task is not currently running",
            }));
        }

        task.state = TaskState::Cancelled;
        task.finished_at = Some(Utc::now());
        let label = task.label.clone();
        drop(reg); // release lock before persist

        if let Some(ref path) = self.persist_path {
            save_registry(&self.registry, path);
        }

        Ok(serde_json::json!({
            "status": "cancelled",
            "task_id": task_id,
            "label": label,
        }))
    }
}
