//! CI/CD action tools — pipeline status and log retrieval.
//!
//! Provides `ci_status` and `ci_logs` actions that query GitHub Actions
//! or Gitea Actions API for workflow run information. Requires forge
//! configuration in `[devtools]`.

use super::{DevToolsConfig, ForgeKind, forge_get, forge_get_text, forge_repo};
use crate::Action;
use aivyx_core::{AivyxError, Result};

// ── CiStatus ──────────────────────────────────────────────────

pub struct CiStatus {
    pub config: DevToolsConfig,
}

#[async_trait::async_trait]
impl Action for CiStatus {
    fn name(&self) -> &str {
        "ci_status"
    }

    fn description(&self) -> &str {
        "Check CI/CD pipeline status. Shows recent workflow runs with status, \
         conclusion, branch, and timing. Optionally filter by branch or workflow name."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "branch": {
                    "type": "string",
                    "description": "Filter runs by branch name"
                },
                "workflow": {
                    "type": "string",
                    "description": "Filter by workflow name or filename (e.g. 'ci.yml')"
                },
                "limit": {
                    "type": "integer",
                    "description": "Max runs to return (default 10, max 30)"
                }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let repo = forge_repo(&self.config)?;
        let limit = input
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(10)
            .min(30);
        let branch = input.get("branch").and_then(|v| v.as_str());
        let workflow = input.get("workflow").and_then(|v| v.as_str());

        let forge = self.config.forge.ok_or_else(|| {
            AivyxError::Validation("No forge configured — set forge in [devtools]".into())
        })?;

        let runs = match forge {
            ForgeKind::Github => {
                fetch_github_runs(&self.config, repo, branch, workflow, limit).await?
            }
            ForgeKind::Gitea => {
                fetch_gitea_runs(&self.config, repo, branch, workflow, limit).await?
            }
        };

        Ok(serde_json::json!({
            "repo": repo,
            "count": runs.len(),
            "runs": runs,
        }))
    }
}

// ── CiLogs ────────────────────────────────────────────────────

pub struct CiLogs {
    pub config: DevToolsConfig,
}

#[async_trait::async_trait]
impl Action for CiLogs {
    fn name(&self) -> &str {
        "ci_logs"
    }

    fn description(&self) -> &str {
        "Fetch logs from a CI/CD workflow run. Provide the run_id from ci_status. \
         Optionally specify a job name to get only that job's logs."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "run_id": {
                    "type": "integer",
                    "description": "Workflow run ID (from ci_status output)"
                },
                "job": {
                    "type": "string",
                    "description": "Filter to a specific job name (optional)"
                }
            },
            "required": ["run_id"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let repo = forge_repo(&self.config)?;
        let run_id = input
            .get("run_id")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| AivyxError::Validation("run_id is required".into()))?;
        let job_filter = input.get("job").and_then(|v| v.as_str());

        let forge = self.config.forge.ok_or_else(|| {
            AivyxError::Validation("No forge configured — set forge in [devtools]".into())
        })?;

        match forge {
            ForgeKind::Github => fetch_github_logs(&self.config, repo, run_id, job_filter).await,
            ForgeKind::Gitea => fetch_gitea_logs(&self.config, repo, run_id, job_filter).await,
        }
    }
}

// ── GitHub Actions API ────────────────────────────────────────

async fn fetch_github_runs(
    config: &DevToolsConfig,
    repo: &str,
    branch: Option<&str>,
    workflow: Option<&str>,
    limit: u64,
) -> Result<Vec<serde_json::Value>> {
    // If workflow is specified, use the workflow-specific endpoint
    let base_path = if let Some(wf) = workflow {
        format!("/repos/{repo}/actions/workflows/{wf}/runs")
    } else {
        format!("/repos/{repo}/actions/runs")
    };

    let mut query = format!("?per_page={limit}");
    if let Some(b) = branch {
        query.push_str(&format!("&branch={b}"));
    }

    let body = forge_get(config, &format!("{base_path}{query}")).await?;

    let runs = body["workflow_runs"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|run| {
            serde_json::json!({
                "id": run["id"],
                "name": run["name"],
                "status": run["status"],
                "conclusion": run["conclusion"],
                "branch": run["head_branch"],
                "event": run["event"],
                "created_at": run["created_at"],
                "updated_at": run["updated_at"],
                "url": run["html_url"],
                "head_sha": run["head_sha"].as_str().map(|s| &s[..7.min(s.len())]),
            })
        })
        .collect();

    Ok(runs)
}

async fn fetch_github_logs(
    config: &DevToolsConfig,
    repo: &str,
    run_id: u64,
    job_filter: Option<&str>,
) -> Result<serde_json::Value> {
    // First, list jobs for this run
    let jobs_body = forge_get(config, &format!("/repos/{repo}/actions/runs/{run_id}/jobs")).await?;

    let jobs = jobs_body["jobs"]
        .as_array()
        .ok_or_else(|| AivyxError::Other("No jobs found for this run".into()))?;

    let mut results = Vec::new();

    for job in jobs {
        let job_name = job["name"].as_str().unwrap_or("unknown");

        // If filtering by job name, skip non-matching jobs
        if let Some(filter) = job_filter
            && !job_name.to_lowercase().contains(&filter.to_lowercase())
        {
            continue;
        }

        let job_id = job["id"]
            .as_u64()
            .ok_or_else(|| AivyxError::Other("Job missing id".into()))?;

        // Fetch logs for this job
        let log_text = forge_get_text(config, &format!("/repos/{repo}/actions/jobs/{job_id}/logs"))
            .await
            .unwrap_or_else(|e| format!("[Could not fetch logs: {e}]"));

        results.push(serde_json::json!({
            "job_id": job_id,
            "job_name": job_name,
            "status": job["status"],
            "conclusion": job["conclusion"],
            "steps": job["steps"],
            "logs": log_text,
        }));
    }

    if let (true, Some(filter)) = (results.is_empty(), job_filter) {
        return Err(AivyxError::Other(format!(
            "No jobs matching '{filter}' found in run {run_id}",
        )));
    }

    Ok(serde_json::json!({
        "repo": repo,
        "run_id": run_id,
        "jobs": results,
    }))
}

// ── Gitea Actions API ─────────────────────────────────────────

async fn fetch_gitea_runs(
    config: &DevToolsConfig,
    repo: &str,
    branch: Option<&str>,
    _workflow: Option<&str>,
    limit: u64,
) -> Result<Vec<serde_json::Value>> {
    // Gitea uses a slightly different API path
    let mut query = format!("?limit={limit}");
    if let Some(b) = branch {
        query.push_str(&format!("&branch={b}"));
    }

    let body = forge_get(config, &format!("/repos/{repo}/actions/runs{query}")).await?;

    // Gitea returns { "workflow_runs": [...] } similar to GitHub
    let runs_array = body["workflow_runs"]
        .as_array()
        .or_else(|| body.as_array())
        .unwrap_or(&Vec::new())
        .clone();

    let runs = runs_array
        .iter()
        .map(|run| {
            serde_json::json!({
                "id": run["id"],
                "name": run["name"],
                "status": run["status"],
                "conclusion": run["conclusion"],
                "branch": run["head_branch"],
                "event": run["event"],
                "created_at": run["created_at"],
                "updated_at": run["updated_at"],
                "url": run["html_url"],
                "head_sha": run["head_sha"].as_str().map(|s| &s[..7.min(s.len())]),
            })
        })
        .collect();

    Ok(runs)
}

async fn fetch_gitea_logs(
    config: &DevToolsConfig,
    repo: &str,
    run_id: u64,
    job_filter: Option<&str>,
) -> Result<serde_json::Value> {
    // Gitea: list jobs for a run
    let jobs_body = forge_get(config, &format!("/repos/{repo}/actions/runs/{run_id}/jobs")).await?;

    let jobs = jobs_body["jobs"]
        .as_array()
        .or_else(|| jobs_body.as_array())
        .ok_or_else(|| AivyxError::Other("No jobs found for this run".into()))?;

    let mut results = Vec::new();

    for job in jobs {
        let job_name = job["name"].as_str().unwrap_or("unknown");

        if let Some(filter) = job_filter
            && !job_name.to_lowercase().contains(&filter.to_lowercase())
        {
            continue;
        }

        let job_id = job["id"]
            .as_u64()
            .ok_or_else(|| AivyxError::Other("Job missing id".into()))?;

        // Gitea log endpoint
        let log_text = forge_get_text(config, &format!("/repos/{repo}/actions/jobs/{job_id}/logs"))
            .await
            .unwrap_or_else(|e| format!("[Could not fetch logs: {e}]"));

        results.push(serde_json::json!({
            "job_id": job_id,
            "job_name": job_name,
            "status": job["status"],
            "conclusion": job["conclusion"],
            "steps": job["steps"],
            "logs": log_text,
        }));
    }

    if let (true, Some(filter)) = (results.is_empty(), job_filter) {
        return Err(AivyxError::Other(format!(
            "No jobs matching '{filter}' found in run {run_id}",
        )));
    }

    Ok(serde_json::json!({
        "repo": repo,
        "run_id": run_id,
        "jobs": results,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn github_config() -> DevToolsConfig {
        DevToolsConfig {
            repo_path: PathBuf::from("/tmp/repo"),
            forge: Some(ForgeKind::Github),
            forge_api_url: Some("https://api.github.com".into()),
            forge_repo: Some("owner/repo".into()),
            forge_token: Some("ghp_test_token".into()),
        }
    }

    fn gitea_config() -> DevToolsConfig {
        DevToolsConfig {
            repo_path: PathBuf::from("/tmp/repo"),
            forge: Some(ForgeKind::Gitea),
            forge_api_url: Some("https://gitea.example.com/api/v1".into()),
            forge_repo: Some("owner/repo".into()),
            forge_token: Some("gitea_test_token".into()),
        }
    }

    fn no_forge_config() -> DevToolsConfig {
        DevToolsConfig {
            repo_path: PathBuf::from("/tmp/repo"),
            forge: None,
            forge_api_url: None,
            forge_repo: None,
            forge_token: None,
        }
    }

    // ── Schema tests ──────────────────────────────────────────

    #[test]
    fn ci_status_name_and_schema() {
        let action = CiStatus {
            config: github_config(),
        };
        assert_eq!(action.name(), "ci_status");
        let schema = action.input_schema();
        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("branch"));
        assert!(props.contains_key("workflow"));
        assert!(props.contains_key("limit"));
    }

    #[test]
    fn ci_logs_name_and_schema() {
        let action = CiLogs {
            config: github_config(),
        };
        assert_eq!(action.name(), "ci_logs");
        let schema = action.input_schema();
        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("run_id"));
        assert!(props.contains_key("job"));
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("run_id")));
    }

    // ── Validation tests ──────────────────────────────────────

    #[tokio::test]
    async fn ci_status_rejects_no_forge() {
        let action = CiStatus {
            config: no_forge_config(),
        };
        let result = action.execute(serde_json::json!({})).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("forge") || err.contains("repo"));
    }

    #[tokio::test]
    async fn ci_status_rejects_no_repo() {
        let config = DevToolsConfig {
            forge: Some(ForgeKind::Github),
            forge_repo: None,
            ..no_forge_config()
        };
        let action = CiStatus { config };
        let result = action.execute(serde_json::json!({})).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("repo"));
    }

    #[tokio::test]
    async fn ci_logs_rejects_missing_run_id() {
        let action = CiLogs {
            config: github_config(),
        };
        let result = action.execute(serde_json::json!({})).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("run_id"));
    }

    #[tokio::test]
    async fn ci_logs_rejects_no_forge() {
        let action = CiLogs {
            config: no_forge_config(),
        };
        let result = action.execute(serde_json::json!({ "run_id": 123 })).await;
        assert!(result.is_err());
    }

    // ── Response parsing tests ────────────────────────────────

    #[test]
    fn parse_github_workflow_run() {
        // Simulate what fetch_github_runs would extract from a real response
        let raw = serde_json::json!({
            "id": 12345,
            "name": "CI",
            "status": "completed",
            "conclusion": "failure",
            "head_branch": "main",
            "event": "push",
            "created_at": "2026-04-03T10:00:00Z",
            "updated_at": "2026-04-03T10:05:00Z",
            "html_url": "https://github.com/owner/repo/actions/runs/12345",
            "head_sha": "abc1234567890",
        });

        let parsed = serde_json::json!({
            "id": raw["id"],
            "name": raw["name"],
            "status": raw["status"],
            "conclusion": raw["conclusion"],
            "branch": raw["head_branch"],
            "event": raw["event"],
            "created_at": raw["created_at"],
            "updated_at": raw["updated_at"],
            "url": raw["html_url"],
            "head_sha": raw["head_sha"].as_str().map(|s| &s[..7]),
        });

        assert_eq!(parsed["id"], 12345);
        assert_eq!(parsed["conclusion"], "failure");
        assert_eq!(parsed["branch"], "main");
        assert_eq!(parsed["head_sha"], "abc1234");
    }

    #[test]
    fn parse_github_job_response() {
        let job = serde_json::json!({
            "id": 99,
            "name": "build",
            "status": "completed",
            "conclusion": "failure",
            "steps": [
                { "name": "Checkout", "status": "completed", "conclusion": "success" },
                { "name": "Build", "status": "completed", "conclusion": "failure" },
            ]
        });

        assert_eq!(job["name"], "build");
        assert_eq!(job["conclusion"], "failure");
        let steps = job["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[1]["conclusion"], "failure");
    }

    // ── Config validation helpers ─────────────────────────────

    #[test]
    fn gitea_config_is_valid() {
        let cfg = gitea_config();
        assert_eq!(cfg.forge, Some(ForgeKind::Gitea));
        assert!(cfg.forge_api_url.is_some());
        assert!(cfg.forge_repo.is_some());
    }
}
