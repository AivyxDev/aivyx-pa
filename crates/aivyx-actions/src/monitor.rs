//! System monitoring tools — proactive environment awareness.
//!
//! Gives the agent eyes on the host system so it can alert the user
//! before problems become crises.
//!
//! Tools:
//! - `check_disk_space` — disk usage per mount point with configurable alert threshold
//! - `check_process` — verify a named process is running (optionally restart)
//! - `tail_log` — read the tail of a log file, optionally filtering by pattern
//! - `check_url_health` — HTTP health check for a URL with latency measurement
//! - `system_stats` — CPU, memory, load average snapshot

use crate::Action;
use aivyx_core::Result;

// ── CheckDiskSpace ──────────────────────────────────────────────

pub struct CheckDiskSpace;

#[async_trait::async_trait]
impl Action for CheckDiskSpace {
    fn name(&self) -> &str {
        "check_disk_space"
    }

    fn description(&self) -> &str {
        "Check disk space usage for all mounted filesystems. \
         Returns used/available space and a health status. \
         Alert threshold: configurable percentage (default: warn at 85%, critical at 95%)."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Specific path to check (optional). If omitted, checks all mounted filesystems."
                },
                "warn_threshold_pct": {
                    "type": "integer",
                    "description": "Warn when usage exceeds this percentage. Default: 85.",
                    "default": 85
                },
                "critical_threshold_pct": {
                    "type": "integer",
                    "description": "Critical when usage exceeds this percentage. Default: 95.",
                    "default": 95
                }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let warn_pct = input["warn_threshold_pct"].as_u64().unwrap_or(85) as u8;
        let crit_pct = input["critical_threshold_pct"].as_u64().unwrap_or(95) as u8;
        let target_path = input["path"].as_str().unwrap_or("/");

        // Use `df -B1` for byte-accurate output (portable across Linux distros)
        let output = tokio::process::Command::new("df")
            .args([
                "-B1",
                "--output=source,fstype,size,used,avail,pcent,target",
                target_path,
            ])
            .output()
            .await
            .map_err(|e| aivyx_core::AivyxError::Other(format!("df failed: {e}")))?;

        if !output.status.success() {
            return Err(aivyx_core::AivyxError::Other(format!(
                "df error: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut filesystems = Vec::new();
        let mut alerts = Vec::new();

        for line in stdout.lines().skip(1) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 7 {
                continue;
            }

            let source = parts[0];
            let fstype = parts[1];
            let size_bytes: u64 = parts[2].parse().unwrap_or(0);
            let used_bytes: u64 = parts[3].parse().unwrap_or(0);
            let avail_bytes: u64 = parts[4].parse().unwrap_or(0);
            let pct_str = parts[5].trim_end_matches('%');
            let used_pct: u8 = pct_str.parse().unwrap_or(0);
            let mount = parts[6];

            // Skip pseudo-filesystems
            if matches!(
                fstype,
                "tmpfs"
                    | "devtmpfs"
                    | "devpts"
                    | "proc"
                    | "sysfs"
                    | "cgroup2"
                    | "cgroup"
                    | "overlay"
            ) {
                continue;
            }

            let status = if used_pct >= crit_pct {
                "critical"
            } else if used_pct >= warn_pct {
                "warning"
            } else {
                "ok"
            };

            if status != "ok" {
                alerts.push(format!(
                    "{mount} at {used_pct}% ({} available)",
                    format_bytes(avail_bytes)
                ));
            }

            filesystems.push(serde_json::json!({
                "source": source,
                "mount": mount,
                "size": format_bytes(size_bytes),
                "used": format_bytes(used_bytes),
                "available": format_bytes(avail_bytes),
                "used_pct": used_pct,
                "status": status,
            }));
        }

        let overall = if alerts.is_empty() {
            "healthy"
        } else {
            "alerts"
        };

        Ok(serde_json::json!({
            "overall_status": overall,
            "alerts": alerts,
            "filesystems": filesystems,
            "thresholds": {
                "warn_pct": warn_pct,
                "critical_pct": crit_pct,
            }
        }))
    }
}

fn format_bytes(bytes: u64) -> String {
    const GB: u64 = 1_073_741_824;
    const MB: u64 = 1_048_576;
    const KB: u64 = 1_024;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

// ── CheckProcess ────────────────────────────────────────────────

pub struct CheckProcess;

#[async_trait::async_trait]
impl Action for CheckProcess {
    fn name(&self) -> &str {
        "check_process"
    }

    fn description(&self) -> &str {
        "Check if a named process is currently running. \
         Returns the PID list and process details if found. \
         Optionally specify a command to run if the process is NOT found \
         (e.g., to restart a crashed service)."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "process_name": {
                    "type": "string",
                    "description": "Name of the process to look for (matched against command name)"
                },
                "restart_command": {
                    "type": "string",
                    "description": "Shell command to run if the process is not found (optional restart action)"
                }
            },
            "required": ["process_name"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let process_name = input["process_name"].as_str().ok_or_else(|| {
            aivyx_core::AivyxError::Validation("check_process: 'process_name' required".into())
        })?;

        // Validate process name is safe (no shell metacharacters)
        if process_name
            .chars()
            .any(|c| matches!(c, ';' | '|' | '&' | '`' | '$' | '>' | '<' | '*' | '?'))
        {
            return Err(aivyx_core::AivyxError::Validation(
                "process_name contains invalid characters".into(),
            ));
        }

        // Use `pgrep -l` to find matching processes
        let output = tokio::process::Command::new("pgrep")
            .args(["-l", "-f", process_name])
            .output()
            .await
            .map_err(|e| aivyx_core::AivyxError::Other(format!("pgrep failed: {e}")))?;

        let running = output.status.success();
        let stdout = String::from_utf8_lossy(&output.stdout);

        let mut processes = Vec::new();
        if running {
            for line in stdout.lines() {
                let parts: Vec<&str> = line.splitn(2, ' ').collect();
                if let (Some(pid), Some(cmd)) = (parts.first(), parts.get(1)) {
                    processes.push(serde_json::json!({
                        "pid": pid,
                        "command": cmd,
                    }));
                }
            }
        }

        // Optionally restart if not running
        let restart_result = if !running {
            if let Some(restart_cmd) = input["restart_command"].as_str() {
                // Safety: only allow restart if process isn't running
                let result = tokio::process::Command::new("sh")
                    .arg("-c")
                    .arg(restart_cmd)
                    .output()
                    .await;
                match result {
                    Ok(out) => Some(serde_json::json!({
                        "attempted": true,
                        "success": out.status.success(),
                        "stdout": String::from_utf8_lossy(&out.stdout).to_string(),
                        "stderr": String::from_utf8_lossy(&out.stderr).to_string(),
                    })),
                    Err(e) => Some(serde_json::json!({
                        "attempted": true,
                        "success": false,
                        "error": e.to_string(),
                    })),
                }
            } else {
                None
            }
        } else {
            None
        };

        Ok(serde_json::json!({
            "process_name": process_name,
            "running": running,
            "process_count": processes.len(),
            "processes": processes,
            "restart": restart_result,
        }))
    }
}

// ── TailLog ─────────────────────────────────────────────────────

pub struct TailLog;

#[async_trait::async_trait]
impl Action for TailLog {
    fn name(&self) -> &str {
        "tail_log"
    }

    fn description(&self) -> &str {
        "Read the tail of a log file. Optionally filter lines by a grep pattern. \
         Useful for monitoring application logs, system logs, or any text file \
         for errors, warnings, or specific events."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute path to the log file"
                },
                "lines": {
                    "type": "integer",
                    "description": "Number of lines to read from the end. Default: 50. Max: 500.",
                    "default": 50
                },
                "pattern": {
                    "type": "string",
                    "description": "Grep pattern to filter lines (optional). Case-insensitive."
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let path = input["path"].as_str().ok_or_else(|| {
            aivyx_core::AivyxError::Validation("tail_log: 'path' is required".into())
        })?;

        // Security: must be absolute path, block sensitive system files
        if !std::path::Path::new(path).is_absolute() {
            return Err(aivyx_core::AivyxError::Validation(
                "path must be absolute".into(),
            ));
        }
        let denied_prefixes = ["/etc/shadow", "/etc/passwd", "/etc/sudoers", "/root/"];
        for prefix in denied_prefixes {
            if path.starts_with(prefix) {
                return Err(aivyx_core::AivyxError::CapabilityDenied(format!(
                    "Access to '{path}' is not permitted"
                )));
            }
        }

        let lines = input["lines"].as_u64().unwrap_or(50).min(500);
        let pattern = input["pattern"].as_str();

        let mut cmd = tokio::process::Command::new("tail");
        cmd.args(["-n", &lines.to_string(), path]);

        let tail_output = cmd
            .output()
            .await
            .map_err(|e| aivyx_core::AivyxError::Other(format!("tail failed: {e}")))?;

        if !tail_output.status.success() {
            let err = String::from_utf8_lossy(&tail_output.stderr);
            return Err(aivyx_core::AivyxError::Other(format!("tail error: {err}")));
        }

        let content = String::from_utf8_lossy(&tail_output.stdout).to_string();

        // Apply grep filter if requested
        let filtered = if let Some(pat) = pattern {
            let lower_pat = pat.to_lowercase();
            content
                .lines()
                .filter(|l| l.to_lowercase().contains(&lower_pat))
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            content
        };

        let line_count = filtered.lines().count();
        let error_indicators = ["error", "fatal", "panic", "exception", "critical", "fail"];
        let error_lines: usize = filtered
            .lines()
            .filter(|l| {
                let ll = l.to_lowercase();
                error_indicators.iter().any(|e| ll.contains(e))
            })
            .count();

        Ok(serde_json::json!({
            "path": path,
            "lines_read": line_count,
            "error_lines_detected": error_lines,
            "pattern_applied": pattern,
            "content": filtered,
        }))
    }
}

// ── CheckUrlHealth ──────────────────────────────────────────────

pub struct CheckUrlHealth;

#[async_trait::async_trait]
impl Action for CheckUrlHealth {
    fn name(&self) -> &str {
        "check_url_health"
    }

    fn description(&self) -> &str {
        "Perform an HTTP health check for a URL. Returns HTTP status code, \
         response time in milliseconds, and whether the service is considered healthy. \
         Use this to monitor internal services, APIs, or websites."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "URL to check (must be http:// or https://)"
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Request timeout in seconds. Default: 10. Max: 30.",
                    "default": 10
                },
                "expected_status": {
                    "type": "integer",
                    "description": "Expected HTTP status code for a healthy response. Default: 200.",
                    "default": 200
                },
                "expect_body_contains": {
                    "type": "string",
                    "description": "If set, the response body must contain this string to count as healthy."
                }
            },
            "required": ["url"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let url = input["url"].as_str().ok_or_else(|| {
            aivyx_core::AivyxError::Validation("check_url_health: 'url' is required".into())
        })?;

        // Only allow http/https (SSRF protection)
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(aivyx_core::AivyxError::Validation(
                "URL must start with http:// or https://".into(),
            ));
        }

        let timeout_secs = input["timeout_secs"].as_u64().unwrap_or(10).min(30);
        let expected_status = input["expected_status"].as_u64().unwrap_or(200) as u16;
        let expect_body = input["expect_body_contains"].as_str();

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .user_agent("aivyx-health-check/1.0")
            .build()
            .map_err(|e| aivyx_core::AivyxError::Other(format!("HTTP client error: {e}")))?;

        let start = std::time::Instant::now();
        let resp = client.get(url).send().await;
        let latency_ms = start.elapsed().as_millis() as u64;

        match resp {
            Ok(response) => {
                let status = response.status().as_u16();
                let status_ok = status == expected_status;

                let body_ok = if let Some(expected) = expect_body {
                    let body = response.text().await.unwrap_or_default();
                    body.contains(expected)
                } else {
                    true
                };

                let healthy = status_ok && body_ok;

                Ok(serde_json::json!({
                    "url": url,
                    "healthy": healthy,
                    "status_code": status,
                    "expected_status": expected_status,
                    "status_ok": status_ok,
                    "body_contains_ok": body_ok,
                    "latency_ms": latency_ms,
                    "timeout_secs": timeout_secs,
                }))
            }
            Err(e) => Ok(serde_json::json!({
                "url": url,
                "healthy": false,
                "error": e.to_string(),
                "latency_ms": latency_ms,
                "timed_out": e.is_timeout(),
                "connection_error": e.is_connect(),
            })),
        }
    }
}

// ── SystemStats ─────────────────────────────────────────────────

pub struct SystemStats;

#[async_trait::async_trait]
impl Action for SystemStats {
    fn name(&self) -> &str {
        "system_stats"
    }

    fn description(&self) -> &str {
        "Get a snapshot of system resource usage: CPU load average, memory usage, \
         uptime, and top memory-consuming processes. Useful for diagnosing \
         performance issues or checking if the system is under load."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn execute(&self, _input: serde_json::Value) -> Result<serde_json::Value> {
        // Read /proc/loadavg for load averages
        let loadavg = tokio::fs::read_to_string("/proc/loadavg")
            .await
            .unwrap_or_default();
        let load_parts: Vec<&str> = loadavg.split_whitespace().collect();
        let load_1 = load_parts.first().copied().unwrap_or("?");
        let load_5 = load_parts.get(1).copied().unwrap_or("?");
        let load_15 = load_parts.get(2).copied().unwrap_or("?");

        // Read /proc/meminfo
        let meminfo = tokio::fs::read_to_string("/proc/meminfo")
            .await
            .unwrap_or_default();
        let mut mem_total_kb: u64 = 0;
        let mut mem_available_kb: u64 = 0;
        for line in meminfo.lines() {
            if line.starts_with("MemTotal:") {
                mem_total_kb = parse_kb_field(line);
            } else if line.starts_with("MemAvailable:") {
                mem_available_kb = parse_kb_field(line);
            }
        }
        let mem_used_kb = mem_total_kb.saturating_sub(mem_available_kb);
        let mem_used_pct = if mem_total_kb > 0 {
            (mem_used_kb as f64 / mem_total_kb as f64 * 100.0).round() as u8
        } else {
            0
        };

        // Read /proc/uptime
        let uptime_str = tokio::fs::read_to_string("/proc/uptime")
            .await
            .unwrap_or_default();
        let uptime_secs: u64 = uptime_str
            .split_whitespace()
            .next()
            .and_then(|s| s.parse::<f64>().ok())
            .map(|f| f as u64)
            .unwrap_or(0);
        let uptime_fmt = format_uptime(uptime_secs);

        // Get top 5 memory-consuming processes via `ps`
        let ps_out = tokio::process::Command::new("ps")
            .args(["aux", "--sort=-%mem"])
            .output()
            .await
            .ok();
        let empty_stdout = Vec::new();
        let ps_stdout = String::from_utf8_lossy(
            ps_out
                .as_ref()
                .map(|o| o.stdout.as_slice())
                .unwrap_or(&empty_stdout),
        );
        let top_procs: Vec<serde_json::Value> = ps_stdout
            .lines()
            .skip(1)
            .take(5)
            .map(|line| {
                let cols: Vec<&str> = line.splitn(11, ' ').filter(|s| !s.is_empty()).collect();
                serde_json::json!({
                    "pid": cols.get(1).copied().unwrap_or("?"),
                    "cpu_pct": cols.get(2).copied().unwrap_or("?"),
                    "mem_pct": cols.get(3).copied().unwrap_or("?"),
                    "command": cols.get(10).copied().unwrap_or("?"),
                })
            })
            .collect();

        Ok(serde_json::json!({
            "load_average": {
                "1min": load_1,
                "5min": load_5,
                "15min": load_15,
            },
            "memory": {
                "total": format_bytes(mem_total_kb * 1024),
                "used": format_bytes(mem_used_kb * 1024),
                "available": format_bytes(mem_available_kb * 1024),
                "used_pct": mem_used_pct,
            },
            "uptime": uptime_fmt,
            "top_processes_by_memory": top_procs,
        }))
    }
}

fn parse_kb_field(line: &str) -> u64 {
    line.split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

fn format_uptime(secs: u64) -> String {
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    if days > 0 {
        format!("{days}d {hours}h {mins}m")
    } else if hours > 0 {
        format!("{hours}h {mins}m")
    } else {
        format!("{mins}m")
    }
}
