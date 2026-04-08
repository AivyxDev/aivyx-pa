//! Local git action tools.
//!
//! Provides `git_log`, `git_diff`, `git_status`, and `git_branches` actions
//! that shell out to the local `git` binary. Each tool accepts an optional
//! `repo_path` override; otherwise uses the configured default.

use super::{resolve_repo, run_git, DevToolsConfig};
use crate::Action;
use aivyx_core::Result;

// ── GitLog ────────────────────────────────────────────────────

pub struct GitLog {
    pub config: DevToolsConfig,
}

#[async_trait::async_trait]
impl Action for GitLog {
    fn name(&self) -> &str {
        "git_log"
    }

    fn description(&self) -> &str {
        "Show recent git commits. Supports limit (default 20), branch, author, \
         since/until dates, and path filters."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "repo_path": {
                    "type": "string",
                    "description": "Absolute path to git repo (optional, uses default)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Max commits to show (default 20, max 100)"
                },
                "branch": {
                    "type": "string",
                    "description": "Branch or ref to show (default: current)"
                },
                "author": {
                    "type": "string",
                    "description": "Filter by author name/email"
                },
                "since": {
                    "type": "string",
                    "description": "Show commits after date (e.g. '2026-01-01', '1 week ago')"
                },
                "until": {
                    "type": "string",
                    "description": "Show commits before date"
                },
                "path": {
                    "type": "string",
                    "description": "Show commits touching this file/directory"
                }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let repo = resolve_repo(&self.config, &input)?;

        let limit = input.get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(20)
            .min(100);

        let mut args = vec![
            "log",
            "--format=%H%x00%an%x00%ae%x00%aI%x00%s",
        ];

        let limit_str = format!("-{limit}");
        args.push(&limit_str);

        // Optional filters — collected into owned strings
        let branch_val;
        if let Some(b) = input.get("branch").and_then(|v| v.as_str()) {
            branch_val = b.to_string();
            args.push(&branch_val);
        }

        let author_val;
        if let Some(a) = input.get("author").and_then(|v| v.as_str()) {
            author_val = format!("--author={a}");
            args.push(&author_val);
        }

        let since_val;
        if let Some(s) = input.get("since").and_then(|v| v.as_str()) {
            since_val = format!("--since={s}");
            args.push(&since_val);
        }

        let until_val;
        if let Some(u) = input.get("until").and_then(|v| v.as_str()) {
            until_val = format!("--until={u}");
            args.push(&until_val);
        }

        // Path filter goes after `--`
        let path_val;
        let has_path = input.get("path").and_then(|v| v.as_str()).is_some();
        if has_path {
            args.push("--");
            path_val = input["path"].as_str().unwrap().to_string();
            args.push(&path_val);
        }

        let output = run_git(&repo, &args).await?;

        let commits: Vec<serde_json::Value> = output
            .lines()
            .filter(|l| !l.is_empty())
            .map(|line| {
                let parts: Vec<&str> = line.splitn(5, '\0').collect();
                if parts.len() == 5 {
                    serde_json::json!({
                        "hash": parts[0],
                        "author": parts[1],
                        "email": parts[2],
                        "date": parts[3],
                        "subject": parts[4],
                    })
                } else {
                    serde_json::json!({ "raw": line })
                }
            })
            .collect();

        Ok(serde_json::json!({
            "repo": repo.display().to_string(),
            "count": commits.len(),
            "commits": commits,
        }))
    }
}

// ── GitDiff ───────────────────────────────────────────────────

pub struct GitDiff {
    pub config: DevToolsConfig,
}

#[async_trait::async_trait]
impl Action for GitDiff {
    fn name(&self) -> &str {
        "git_diff"
    }

    fn description(&self) -> &str {
        "Show git diff. Defaults to unstaged changes. Use staged=true for staged, \
         or provide from/to refs for comparing branches/commits."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "repo_path": {
                    "type": "string",
                    "description": "Absolute path to git repo (optional)"
                },
                "staged": {
                    "type": "boolean",
                    "description": "Show staged (cached) changes instead of unstaged"
                },
                "from": {
                    "type": "string",
                    "description": "Base ref for comparison (e.g. 'main', commit hash)"
                },
                "to": {
                    "type": "string",
                    "description": "Target ref for comparison (default: HEAD)"
                },
                "stat_only": {
                    "type": "boolean",
                    "description": "Show only file-level summary (--stat), not full diff"
                },
                "path": {
                    "type": "string",
                    "description": "Restrict diff to this file/directory"
                }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let repo = resolve_repo(&self.config, &input)?;

        let mut args = vec!["diff"];

        let staged = input.get("staged").and_then(|v| v.as_bool()).unwrap_or(false);
        let stat_only = input.get("stat_only").and_then(|v| v.as_bool()).unwrap_or(false);
        let from = input.get("from").and_then(|v| v.as_str());
        let to = input.get("to").and_then(|v| v.as_str());

        if staged && from.is_none() {
            args.push("--cached");
        }

        if stat_only {
            args.push("--stat");
        }

        // Ref comparison: from..to
        let range_val;
        if let Some(f) = from {
            let t = to.unwrap_or("HEAD");
            range_val = format!("{f}..{t}");
            args.push(&range_val);
        }

        // Path filter
        let path_val;
        if let Some(p) = input.get("path").and_then(|v| v.as_str()) {
            args.push("--");
            path_val = p.to_string();
            args.push(&path_val);
        }

        let output = run_git(&repo, &args).await?;

        if output.trim().is_empty() {
            return Ok(serde_json::json!({
                "repo": repo.display().to_string(),
                "diff": "",
                "summary": "No changes",
            }));
        }

        Ok(serde_json::json!({
            "repo": repo.display().to_string(),
            "diff": output,
        }))
    }
}

// ── GitStatus ─────────────────────────────────────────────────

pub struct GitStatus {
    pub config: DevToolsConfig,
}

#[async_trait::async_trait]
impl Action for GitStatus {
    fn name(&self) -> &str {
        "git_status"
    }

    fn description(&self) -> &str {
        "Show working tree status: branch, staged/unstaged/untracked files, \
         ahead/behind remote tracking branch."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "repo_path": {
                    "type": "string",
                    "description": "Absolute path to git repo (optional)"
                }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let repo = resolve_repo(&self.config, &input)?;

        // Porcelain v2 for machine-readable output
        let output = run_git(&repo, &["status", "--porcelain=v2", "--branch"]).await?;

        let mut branch = String::new();
        let mut upstream = String::new();
        let mut ahead: i64 = 0;
        let mut behind: i64 = 0;
        let mut staged = Vec::new();
        let mut modified = Vec::new();
        let mut untracked = Vec::new();

        for line in output.lines() {
            if let Some(rest) = line.strip_prefix("# branch.head ") {
                branch = rest.to_string();
            } else if let Some(rest) = line.strip_prefix("# branch.upstream ") {
                upstream = rest.to_string();
            } else if let Some(rest) = line.strip_prefix("# branch.ab ") {
                // Format: "+N -M"
                for part in rest.split_whitespace() {
                    if let Some(n) = part.strip_prefix('+') {
                        ahead = n.parse().unwrap_or(0);
                    } else if let Some(n) = part.strip_prefix('-') {
                        behind = n.parse().unwrap_or(0);
                    }
                }
            } else if line.starts_with("1 ") || line.starts_with("2 ") {
                // Changed entry: "1 XY sub mH mI mW hH hI path"
                let parts: Vec<&str> = line.splitn(9, ' ').collect();
                if parts.len() >= 9 {
                    let xy = parts[1];
                    let path = parts[8];
                    let x = xy.chars().next().unwrap_or('.');
                    let y = xy.chars().nth(1).unwrap_or('.');

                    if x != '.' {
                        staged.push(format!("{x} {path}"));
                    }
                    if y != '.' {
                        modified.push(format!("{y} {path}"));
                    }
                }
            } else if let Some(rest) = line.strip_prefix("? ") {
                untracked.push(rest.to_string());
            }
        }

        Ok(serde_json::json!({
            "repo": repo.display().to_string(),
            "branch": branch,
            "upstream": if upstream.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(upstream) },
            "ahead": ahead,
            "behind": behind,
            "staged": staged,
            "modified": modified,
            "untracked": untracked,
            "clean": staged.is_empty() && modified.is_empty() && untracked.is_empty(),
        }))
    }
}

// ── GitBranches ───────────────────────────────────────────────

pub struct GitBranches {
    pub config: DevToolsConfig,
}

#[async_trait::async_trait]
impl Action for GitBranches {
    fn name(&self) -> &str {
        "git_branches"
    }

    fn description(&self) -> &str {
        "List git branches with current branch indicator, last commit date, \
         and optional remote tracking info."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "repo_path": {
                    "type": "string",
                    "description": "Absolute path to git repo (optional)"
                },
                "all": {
                    "type": "boolean",
                    "description": "Include remote-tracking branches (default: false)"
                }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let repo = resolve_repo(&self.config, &input)?;
        let all = input.get("all").and_then(|v| v.as_bool()).unwrap_or(false);

        let mut args = vec![
            "branch",
            "--format=%(HEAD)%00%(refname:short)%00%(upstream:short)%00%(committerdate:iso8601)%00%(subject)",
        ];
        if all {
            args.push("--all");
        }

        let output = run_git(&repo, &args).await?;

        let branches: Vec<serde_json::Value> = output
            .lines()
            .filter(|l| !l.is_empty())
            .map(|line| {
                let parts: Vec<&str> = line.splitn(5, '\0').collect();
                if parts.len() == 5 {
                    serde_json::json!({
                        "current": parts[0].trim() == "*",
                        "name": parts[1],
                        "upstream": if parts[2].is_empty() { serde_json::Value::Null } else { serde_json::Value::String(parts[2].to_string()) },
                        "last_commit_date": parts[3],
                        "last_commit_subject": parts[4],
                    })
                } else {
                    serde_json::json!({ "name": line.trim_start_matches("* ").trim() })
                }
            })
            .collect();

        Ok(serde_json::json!({
            "repo": repo.display().to_string(),
            "count": branches.len(),
            "branches": branches,
        }))
    }
}


// ── GitCommit ─────────────────────────────────────────────────

pub struct GitCommit {
    pub config: DevToolsConfig,
}

#[async_trait::async_trait]
impl Action for GitCommit {
    fn name(&self) -> &str { "git_commit" }

    fn description(&self) -> &str {
        "Commit changes to the local git repository. Use add_all=true to stage all modified files first."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "repo_path": { "type": "string", "description": "Absolute path to git repo (optional)" },
                "message": { "type": "string", "description": "Commit message" },
                "add_all": { "type": "boolean", "description": "Run 'git add .' before committing" }
            },
            "required": ["message"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let repo = resolve_repo(&self.config, &input)?;
        let message = input["message"].as_str().unwrap_or("Update");
        let add_all = input["add_all"].as_bool().unwrap_or(false);

        if add_all {
            let _ = run_git(&repo, &["add", "."]).await?;
        }

        let output = run_git(&repo, &["commit", "-m", message]).await?;
        
        Ok(serde_json::json!({
            "repo": repo.display().to_string(),
            "output": output,
        }))
    }
}

// ── GitPush ───────────────────────────────────────────────────

pub struct GitPush {
    pub config: DevToolsConfig,
}

#[async_trait::async_trait]
impl Action for GitPush {
    fn name(&self) -> &str { "git_push" }

    fn description(&self) -> &str {
        "Push local commits to the remote repository. Note: If your SSH key requires a passphrase, this tool will timeout. You must use spawn_background_command to push interactively."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "repo_path": { "type": "string", "description": "Absolute path to git repo (optional)" },
                "remote": { "type": "string", "description": "Remote name (default: origin)" },
                "branch": { "type": "string", "description": "Branch name (optional)" }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let repo = resolve_repo(&self.config, &input)?;
        let remote = input["remote"].as_str().unwrap_or("origin");
        
        let mut args = vec!["push", remote];
        if let Some(branch) = input.get("branch").and_then(|v| v.as_str()) {
            args.push(branch);
        }

        let output = match run_git(&repo, &args).await {
            Ok(o) => o,
            Err(e) => return Err(aivyx_core::AivyxError::Other(format!("Push failed or timed out: {e}"))),
        };
        
        Ok(serde_json::json!({
            "repo": repo.display().to_string(),
            "output": output,
        }))
    }
}

// ── GitPull ───────────────────────────────────────────────────

pub struct GitPull {
    pub config: DevToolsConfig,
}

#[async_trait::async_trait]
impl Action for GitPull {
    fn name(&self) -> &str { "git_pull" }

    fn description(&self) -> &str {
        "Pull (fetch and merge) changes from the remote repository."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "repo_path": { "type": "string", "description": "Absolute path to git repo (optional)" },
                "remote": { "type": "string", "description": "Remote name (default: origin)" },
                "branch": { "type": "string", "description": "Branch name (optional)" }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let repo = resolve_repo(&self.config, &input)?;
        let remote = input["remote"].as_str().unwrap_or("origin");
        
        let mut args = vec!["pull", remote];
        if let Some(branch) = input.get("branch").and_then(|v| v.as_str()) {
            args.push(branch);
        }

        let output = match run_git(&repo, &args).await {
            Ok(o) => o,
            Err(e) => return Err(aivyx_core::AivyxError::Other(format!("Pull failed or timed out: {e}"))),
        };
        
        Ok(serde_json::json!({
            "repo": repo.display().to_string(),
            "output": output,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_config() -> DevToolsConfig {
        DevToolsConfig {
            repo_path: PathBuf::from("/tmp/nonexistent"),
            forge: None,
            forge_api_url: None,
            forge_repo: None,
            forge_token: None,
        }
    }

    // ── Schema tests ──────────────────────────────────────────

    #[test]
    fn git_log_schema_has_required_fields() {
        let action = GitLog { config: test_config() };
        let schema = action.input_schema();
        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("limit"));
        assert!(props.contains_key("branch"));
        assert!(props.contains_key("since"));
        assert!(props.contains_key("author"));
        assert!(props.contains_key("path"));
    }

    #[test]
    fn git_diff_schema_has_required_fields() {
        let action = GitDiff { config: test_config() };
        let schema = action.input_schema();
        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("staged"));
        assert!(props.contains_key("from"));
        assert!(props.contains_key("to"));
        assert!(props.contains_key("stat_only"));
    }

    #[test]
    fn git_status_name() {
        let action = GitStatus { config: test_config() };
        assert_eq!(action.name(), "git_status");
    }

    #[test]
    fn git_branches_name() {
        let action = GitBranches { config: test_config() };
        assert_eq!(action.name(), "git_branches");
    }

    // ── Integration tests (require a real git repo) ───────────

    /// Helper: create a temporary git repo with one commit.
    async fn setup_temp_repo() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();

        // Init repo
        run_git(&path, &["init"]).await.unwrap();
        run_git(&path, &["config", "user.email", "test@test.com"]).await.unwrap();
        run_git(&path, &["config", "user.name", "Test"]).await.unwrap();

        // Create initial commit
        let readme = path.join("README.md");
        std::fs::write(&readme, "# Test\n").unwrap();
        run_git(&path, &["add", "."]).await.unwrap();
        run_git(&path, &["commit", "-m", "Initial commit"]).await.unwrap();

        (dir, path)
    }

    #[tokio::test]
    async fn git_log_on_real_repo() {
        let (_dir, path) = setup_temp_repo().await;
        let config = DevToolsConfig {
            repo_path: path,
            forge: None,
            forge_api_url: None,
            forge_repo: None,
            forge_token: None,
        };
        let action = GitLog { config };
        let result = action.execute(serde_json::json!({})).await.unwrap();
        assert_eq!(result["count"], 1);
        let commits = result["commits"].as_array().unwrap();
        assert_eq!(commits[0]["subject"], "Initial commit");
        assert_eq!(commits[0]["author"], "Test");
    }

    #[tokio::test]
    async fn git_log_with_limit() {
        let (_dir, path) = setup_temp_repo().await;

        // Add a second commit
        std::fs::write(path.join("file2.txt"), "content").unwrap();
        run_git(&path, &["add", "."]).await.unwrap();
        run_git(&path, &["commit", "-m", "Second commit"]).await.unwrap();

        let config = DevToolsConfig {
            repo_path: path,
            forge: None,
            forge_api_url: None,
            forge_repo: None,
            forge_token: None,
        };
        let action = GitLog { config };
        let result = action.execute(serde_json::json!({ "limit": 1 })).await.unwrap();
        assert_eq!(result["count"], 1);
        let commits = result["commits"].as_array().unwrap();
        assert_eq!(commits[0]["subject"], "Second commit");
    }

    #[tokio::test]
    async fn git_status_clean_repo() {
        let (_dir, path) = setup_temp_repo().await;
        let config = DevToolsConfig {
            repo_path: path,
            forge: None,
            forge_api_url: None,
            forge_repo: None,
            forge_token: None,
        };
        let action = GitStatus { config };
        let result = action.execute(serde_json::json!({})).await.unwrap();
        assert_eq!(result["clean"], true);
        assert!(result["staged"].as_array().unwrap().is_empty());
        assert!(result["modified"].as_array().unwrap().is_empty());
        assert!(result["untracked"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn git_status_with_changes() {
        let (_dir, path) = setup_temp_repo().await;

        // Create untracked file
        std::fs::write(path.join("new.txt"), "new file").unwrap();
        // Modify tracked file
        std::fs::write(path.join("README.md"), "# Modified\n").unwrap();

        let config = DevToolsConfig {
            repo_path: path,
            forge: None,
            forge_api_url: None,
            forge_repo: None,
            forge_token: None,
        };
        let action = GitStatus { config };
        let result = action.execute(serde_json::json!({})).await.unwrap();
        assert_eq!(result["clean"], false);
        assert!(!result["untracked"].as_array().unwrap().is_empty());
        assert!(!result["modified"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn git_status_staged_files() {
        let (_dir, path) = setup_temp_repo().await;

        // Stage a new file
        std::fs::write(path.join("staged.txt"), "staged content").unwrap();
        run_git(&path, &["add", "staged.txt"]).await.unwrap();

        let config = DevToolsConfig {
            repo_path: path,
            forge: None,
            forge_api_url: None,
            forge_repo: None,
            forge_token: None,
        };
        let action = GitStatus { config };
        let result = action.execute(serde_json::json!({})).await.unwrap();
        assert!(!result["staged"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn git_diff_no_changes() {
        let (_dir, path) = setup_temp_repo().await;
        let config = DevToolsConfig {
            repo_path: path,
            forge: None,
            forge_api_url: None,
            forge_repo: None,
            forge_token: None,
        };
        let action = GitDiff { config };
        let result = action.execute(serde_json::json!({})).await.unwrap();
        assert_eq!(result["summary"], "No changes");
    }

    #[tokio::test]
    async fn git_diff_with_unstaged_changes() {
        let (_dir, path) = setup_temp_repo().await;
        std::fs::write(path.join("README.md"), "# Changed\n").unwrap();

        let config = DevToolsConfig {
            repo_path: path,
            forge: None,
            forge_api_url: None,
            forge_repo: None,
            forge_token: None,
        };
        let action = GitDiff { config };
        let result = action.execute(serde_json::json!({})).await.unwrap();
        let diff = result["diff"].as_str().unwrap();
        assert!(diff.contains("README.md"));
        assert!(diff.contains("Changed"));
    }

    #[tokio::test]
    async fn git_diff_stat_only() {
        let (_dir, path) = setup_temp_repo().await;
        std::fs::write(path.join("README.md"), "# Changed\n").unwrap();

        let config = DevToolsConfig {
            repo_path: path,
            forge: None,
            forge_api_url: None,
            forge_repo: None,
            forge_token: None,
        };
        let action = GitDiff { config };
        let result = action.execute(serde_json::json!({ "stat_only": true })).await.unwrap();
        let diff = result["diff"].as_str().unwrap();
        assert!(diff.contains("README.md"));
        // --stat output has the +/- summary line
        assert!(diff.contains("1 file changed") || diff.contains("insertion") || diff.contains("deletion"));
    }

    #[tokio::test]
    async fn git_branches_lists_current() {
        let (_dir, path) = setup_temp_repo().await;
        let config = DevToolsConfig {
            repo_path: path,
            forge: None,
            forge_api_url: None,
            forge_repo: None,
            forge_token: None,
        };
        let action = GitBranches { config };
        let result = action.execute(serde_json::json!({})).await.unwrap();
        assert!(result["count"].as_u64().unwrap() >= 1);
        let branches = result["branches"].as_array().unwrap();
        let current = branches.iter().find(|b| b["current"] == true);
        assert!(current.is_some());
    }

    #[tokio::test]
    async fn git_branches_with_multiple() {
        let (_dir, path) = setup_temp_repo().await;
        // Create a second branch
        run_git(&path, &["branch", "feature-x"]).await.unwrap();

        let config = DevToolsConfig {
            repo_path: path,
            forge: None,
            forge_api_url: None,
            forge_repo: None,
            forge_token: None,
        };
        let action = GitBranches { config };
        let result = action.execute(serde_json::json!({})).await.unwrap();
        assert!(result["count"].as_u64().unwrap() >= 2);
        let branches = result["branches"].as_array().unwrap();
        let names: Vec<&str> = branches.iter()
            .filter_map(|b| b["name"].as_str())
            .collect();
        assert!(names.contains(&"feature-x"));
    }

    #[tokio::test]
    async fn git_log_rejects_relative_path() {
        let config = DevToolsConfig {
            repo_path: PathBuf::from("/tmp"),
            forge: None,
            forge_api_url: None,
            forge_repo: None,
            forge_token: None,
        };
        let action = GitLog { config };
        let result = action.execute(serde_json::json!({ "repo_path": "relative" })).await;
        assert!(result.is_err());
    }
}
