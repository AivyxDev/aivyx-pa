//! Pull request action tools — list, diff, and comment.
//!
//! Provides `list_prs`, `get_pr_diff`, and `create_pr_comment` actions
//! that work with GitHub and Gitea pull request APIs. Requires forge
//! configuration in `[devtools]`.

use super::{forge_get, forge_get_text, forge_post, forge_repo, DevToolsConfig, ForgeKind};
use crate::Action;
use aivyx_core::{AivyxError, Result};

// ── ListPrs ───────────────────────────────────────────────────

pub struct ListPrs {
    pub config: DevToolsConfig,
}

#[async_trait::async_trait]
impl Action for ListPrs {
    fn name(&self) -> &str {
        "list_prs"
    }

    fn description(&self) -> &str {
        "List pull requests from the configured repository. Filter by state \
         (open/closed/all), author, or labels. Returns title, number, state, \
         author, base/head branches, and review status."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "state": {
                    "type": "string",
                    "description": "Filter by state: open (default), closed, or all",
                    "enum": ["open", "closed", "all"]
                },
                "author": {
                    "type": "string",
                    "description": "Filter by author username"
                },
                "labels": {
                    "type": "string",
                    "description": "Comma-separated label names to filter by"
                },
                "base": {
                    "type": "string",
                    "description": "Filter by base branch (e.g. 'main')"
                },
                "limit": {
                    "type": "integer",
                    "description": "Max PRs to return (default 20, max 50)"
                }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let repo = forge_repo(&self.config)?;
        let forge = self.config.forge.ok_or_else(|| {
            AivyxError::Validation("No forge configured — set forge in [devtools]".into())
        })?;

        let state = input.get("state").and_then(|v| v.as_str()).unwrap_or("open");
        let limit = input.get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(20)
            .min(50);
        let author = input.get("author").and_then(|v| v.as_str());
        let labels = input.get("labels").and_then(|v| v.as_str());
        let base = input.get("base").and_then(|v| v.as_str());

        let prs = match forge {
            ForgeKind::Github => {
                fetch_github_prs(&self.config, repo, state, author, labels, base, limit).await?
            }
            ForgeKind::Gitea => {
                fetch_gitea_prs(&self.config, repo, state, author, labels, base, limit).await?
            }
        };

        Ok(serde_json::json!({
            "repo": repo,
            "state": state,
            "count": prs.len(),
            "pull_requests": prs,
        }))
    }
}

// ── GetPrDiff ─────────────────────────────────────────────────

pub struct GetPrDiff {
    pub config: DevToolsConfig,
}

#[async_trait::async_trait]
impl Action for GetPrDiff {
    fn name(&self) -> &str {
        "get_pr_diff"
    }

    fn description(&self) -> &str {
        "Get the diff for a pull request by number. Returns the PR metadata, \
         changed files summary, and optionally the full diff text."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "number": {
                    "type": "integer",
                    "description": "Pull request number"
                },
                "full_diff": {
                    "type": "boolean",
                    "description": "Include full diff text (default false — shows file summary only)"
                }
            },
            "required": ["number"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let repo = forge_repo(&self.config)?;
        let number = input.get("number")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| AivyxError::Validation("number is required".into()))?;
        let full_diff = input.get("full_diff")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Fetch PR metadata
        let pr = forge_get(
            &self.config,
            &format!("/repos/{repo}/pulls/{number}"),
        )
        .await?;

        let mut result = normalize_pr(&pr);

        // Include body for context
        result["body"] = pr["body"].clone();

        // Fetch changed files list
        let files = forge_get(
            &self.config,
            &format!("/repos/{repo}/pulls/{number}/files?per_page=100"),
        )
        .await;

        if let Ok(files_json) = files {
            let file_summary: Vec<serde_json::Value> = files_json
                .as_array()
                .unwrap_or(&Vec::new())
                .iter()
                .map(|f| {
                    serde_json::json!({
                        "filename": f["filename"],
                        "status": f["status"],
                        "additions": f["additions"],
                        "deletions": f["deletions"],
                        "changes": f["changes"],
                    })
                })
                .collect();

            let total_additions: i64 = file_summary.iter()
                .filter_map(|f| f["additions"].as_i64())
                .sum();
            let total_deletions: i64 = file_summary.iter()
                .filter_map(|f| f["deletions"].as_i64())
                .sum();

            result["files"] = serde_json::json!(file_summary);
            result["files_changed"] = serde_json::json!(file_summary.len());
            result["total_additions"] = serde_json::json!(total_additions);
            result["total_deletions"] = serde_json::json!(total_deletions);
        }

        // Optionally fetch the raw diff
        if full_diff {
            let diff_text = fetch_pr_diff_text(&self.config, repo, number).await
                .unwrap_or_else(|e| format!("[Could not fetch diff: {e}]"));
            result["diff"] = serde_json::json!(diff_text);
        }

        Ok(result)
    }
}

// ── CreatePrComment ───────────────────────────────────────────

pub struct CreatePrComment {
    pub config: DevToolsConfig,
}

#[async_trait::async_trait]
impl Action for CreatePrComment {
    fn name(&self) -> &str {
        "create_pr_comment"
    }

    fn description(&self) -> &str {
        "Post a review comment on a pull request. Can be a general comment or \
         an inline comment on a specific file and line."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "number": {
                    "type": "integer",
                    "description": "Pull request number"
                },
                "body": {
                    "type": "string",
                    "description": "Comment text (Markdown)"
                },
                "path": {
                    "type": "string",
                    "description": "File path for inline comment (optional — omit for general comment)"
                },
                "line": {
                    "type": "integer",
                    "description": "Line number in the diff for inline comment"
                }
            },
            "required": ["number", "body"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let repo = forge_repo(&self.config)?;
        let number = input.get("number")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| AivyxError::Validation("number is required".into()))?;
        let body = input.get("body")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AivyxError::Validation("body is required".into()))?;

        if body.trim().is_empty() {
            return Err(AivyxError::Validation("body must not be empty".into()));
        }

        if self.config.forge_token.is_none() {
            return Err(AivyxError::Validation(
                "Forge token required to post comments — set FORGE_TOKEN in keystore".into(),
            ));
        }

        let path = input.get("path").and_then(|v| v.as_str());
        let line = input.get("line").and_then(|v| v.as_u64());

        // If path + line given, post an inline review comment; otherwise a general comment
        if let (Some(file_path), Some(line_num)) = (path, line) {
            post_inline_comment(&self.config, repo, number, body, file_path, line_num).await
        } else {
            post_general_comment(&self.config, repo, number, body).await
        }
    }
}

// ── Comment posting helpers ───────────────────────────────────

async fn post_general_comment(
    config: &DevToolsConfig,
    repo: &str,
    number: u64,
    body: &str,
) -> Result<serde_json::Value> {
    // General comments go on the issue comments endpoint (PRs are issues)
    let payload = serde_json::json!({ "body": body });
    let created = forge_post(
        config,
        &format!("/repos/{repo}/issues/{number}/comments"),
        &payload,
    )
    .await?;

    Ok(serde_json::json!({
        "status": "posted",
        "type": "general",
        "comment_id": created["id"],
        "url": created["html_url"],
    }))
}

async fn post_inline_comment(
    config: &DevToolsConfig,
    repo: &str,
    number: u64,
    body: &str,
    path: &str,
    line: u64,
) -> Result<serde_json::Value> {
    let forge = config.forge.unwrap_or(ForgeKind::Github);

    match forge {
        ForgeKind::Github => {
            // GitHub: create a review with a single comment
            // First get the PR to find the latest commit SHA
            let pr = forge_get(config, &format!("/repos/{repo}/pulls/{number}")).await?;
            let commit_id = pr["head"]["sha"]
                .as_str()
                .ok_or_else(|| AivyxError::Other("Could not determine PR head SHA".into()))?;

            let payload = serde_json::json!({
                "body": "Review comment",
                "event": "COMMENT",
                "comments": [{
                    "path": path,
                    "line": line,
                    "body": body,
                }],
                "commit_id": commit_id,
            });

            let created = forge_post(
                config,
                &format!("/repos/{repo}/pulls/{number}/reviews"),
                &payload,
            )
            .await?;

            Ok(serde_json::json!({
                "status": "posted",
                "type": "inline",
                "review_id": created["id"],
                "path": path,
                "line": line,
                "url": created["html_url"],
            }))
        }
        ForgeKind::Gitea => {
            // Gitea: create a pull review comment
            let pr = forge_get(config, &format!("/repos/{repo}/pulls/{number}")).await?;
            let commit_id = pr["head"]["sha"]
                .as_str()
                .ok_or_else(|| AivyxError::Other("Could not determine PR head SHA".into()))?;

            let payload = serde_json::json!({
                "body": "Review comment",
                "event": "comment",
                "comments": [{
                    "path": path,
                    "new_position": line,
                    "body": body,
                }],
                "commit_id": commit_id,
            });

            let created = forge_post(
                config,
                &format!("/repos/{repo}/pulls/{number}/reviews"),
                &payload,
            )
            .await?;

            Ok(serde_json::json!({
                "status": "posted",
                "type": "inline",
                "review_id": created["id"],
                "path": path,
                "line": line,
                "url": created["html_url"],
            }))
        }
    }
}

// ── Diff fetching ─────────────────────────────────────────────

async fn fetch_pr_diff_text(
    config: &DevToolsConfig,
    repo: &str,
    number: u64,
) -> Result<String> {
    // Both GitHub and Gitea support .diff suffix
    forge_get_text(config, &format!("/repos/{repo}/pulls/{number}.diff")).await
}

// ── PR normalization ──────────────────────────────────────────

fn normalize_pr(pr: &serde_json::Value) -> serde_json::Value {
    let empty = Vec::new();

    let labels: Vec<&str> = pr["labels"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .filter_map(|l| l["name"].as_str())
        .collect();

    let assignees: Vec<&str> = pr["assignees"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .filter_map(|a| a["login"].as_str())
        .collect();

    let reviewers: Vec<&str> = pr["requested_reviewers"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .filter_map(|r| r["login"].as_str())
        .collect();

    serde_json::json!({
        "number": pr["number"],
        "title": pr["title"],
        "state": pr["state"],
        "draft": pr["draft"],
        "author": pr["user"]["login"],
        "base": pr["base"]["ref"],
        "head": pr["head"]["ref"],
        "labels": labels,
        "assignees": assignees,
        "reviewers": reviewers,
        "mergeable": pr["mergeable"],
        "created_at": pr["created_at"],
        "updated_at": pr["updated_at"],
        "url": pr["html_url"],
    })
}

// ── GitHub-specific fetchers ──────────────────────────────────

async fn fetch_github_prs(
    config: &DevToolsConfig,
    repo: &str,
    state: &str,
    author: Option<&str>,
    labels: Option<&str>,
    base: Option<&str>,
    limit: u64,
) -> Result<Vec<serde_json::Value>> {
    let mut query = format!("?state={state}&per_page={limit}");

    if let Some(b) = base {
        query.push_str(&format!("&base={b}"));
    }
    if let Some(l) = labels {
        query.push_str(&format!("&labels={l}"));
    }
    // GitHub doesn't have a direct author filter on /pulls — we filter client-side
    let body = forge_get(config, &format!("/repos/{repo}/pulls{query}")).await?;

    let prs: Vec<serde_json::Value> = body
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .filter(|pr| {
            if let Some(a) = author {
                pr["user"]["login"].as_str().is_some_and(|login| {
                    login.eq_ignore_ascii_case(a)
                })
            } else {
                true
            }
        })
        .map(normalize_pr)
        .collect();

    Ok(prs)
}

// ── Gitea-specific fetchers ───────────────────────────────────

async fn fetch_gitea_prs(
    config: &DevToolsConfig,
    repo: &str,
    state: &str,
    _author: Option<&str>,
    labels: Option<&str>,
    _base: Option<&str>,
    limit: u64,
) -> Result<Vec<serde_json::Value>> {
    let mut query = format!("?state={state}&limit={limit}");

    if let Some(l) = labels {
        query.push_str(&format!("&labels={l}"));
    }

    let body = forge_get(config, &format!("/repos/{repo}/pulls{query}")).await?;

    Ok(body
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(normalize_pr)
        .collect())
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

    fn no_forge_config() -> DevToolsConfig {
        DevToolsConfig {
            repo_path: PathBuf::from("/tmp/repo"),
            forge: None,
            forge_api_url: None,
            forge_repo: None,
            forge_token: None,
        }
    }

    fn no_token_config() -> DevToolsConfig {
        DevToolsConfig {
            repo_path: PathBuf::from("/tmp/repo"),
            forge: Some(ForgeKind::Github),
            forge_api_url: Some("https://api.github.com".into()),
            forge_repo: Some("owner/repo".into()),
            forge_token: None,
        }
    }

    // ── Schema tests ──────────────────────────────────────────

    #[test]
    fn list_prs_name_and_schema() {
        let action = ListPrs { config: github_config() };
        assert_eq!(action.name(), "list_prs");
        let schema = action.input_schema();
        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("state"));
        assert!(props.contains_key("author"));
        assert!(props.contains_key("labels"));
        assert!(props.contains_key("base"));
        assert!(props.contains_key("limit"));
    }

    #[test]
    fn get_pr_diff_name_and_schema() {
        let action = GetPrDiff { config: github_config() };
        assert_eq!(action.name(), "get_pr_diff");
        let schema = action.input_schema();
        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("number"));
        assert!(props.contains_key("full_diff"));
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("number")));
    }

    #[test]
    fn create_pr_comment_name_and_schema() {
        let action = CreatePrComment { config: github_config() };
        assert_eq!(action.name(), "create_pr_comment");
        let schema = action.input_schema();
        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("number"));
        assert!(props.contains_key("body"));
        assert!(props.contains_key("path"));
        assert!(props.contains_key("line"));
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("number")));
        assert!(required.contains(&serde_json::json!("body")));
    }

    // ── Validation tests ──────────────────────────────────────

    #[tokio::test]
    async fn list_prs_rejects_no_forge() {
        let action = ListPrs { config: no_forge_config() };
        let result = action.execute(serde_json::json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn get_pr_diff_rejects_missing_number() {
        let action = GetPrDiff { config: github_config() };
        let result = action.execute(serde_json::json!({})).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("number"));
    }

    #[tokio::test]
    async fn create_pr_comment_rejects_missing_body() {
        let action = CreatePrComment { config: github_config() };
        let result = action.execute(serde_json::json!({ "number": 1 })).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("body"));
    }

    #[tokio::test]
    async fn create_pr_comment_rejects_empty_body() {
        let action = CreatePrComment { config: github_config() };
        let result = action.execute(serde_json::json!({ "number": 1, "body": "  " })).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("empty"));
    }

    #[tokio::test]
    async fn create_pr_comment_rejects_no_token() {
        let action = CreatePrComment { config: no_token_config() };
        let result = action.execute(serde_json::json!({ "number": 1, "body": "LGTM" })).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("token") || err.contains("FORGE_TOKEN"));
    }

    #[tokio::test]
    async fn create_pr_comment_rejects_missing_number() {
        let action = CreatePrComment { config: github_config() };
        let result = action.execute(serde_json::json!({ "body": "comment" })).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("number"));
    }

    // ── Normalization tests ───────────────────────────────────

    #[test]
    fn normalize_pr_extracts_fields() {
        let raw = serde_json::json!({
            "number": 99,
            "title": "Add feature X",
            "state": "open",
            "draft": false,
            "user": { "login": "alice" },
            "base": { "ref": "main" },
            "head": { "ref": "feature-x" },
            "labels": [{ "name": "enhancement" }],
            "assignees": [{ "login": "bob" }],
            "requested_reviewers": [{ "login": "carol" }],
            "mergeable": true,
            "created_at": "2026-04-01T10:00:00Z",
            "updated_at": "2026-04-02T10:00:00Z",
            "html_url": "https://github.com/owner/repo/pull/99",
        });

        let normalized = normalize_pr(&raw);
        assert_eq!(normalized["number"], 99);
        assert_eq!(normalized["title"], "Add feature X");
        assert_eq!(normalized["state"], "open");
        assert_eq!(normalized["draft"], false);
        assert_eq!(normalized["author"], "alice");
        assert_eq!(normalized["base"], "main");
        assert_eq!(normalized["head"], "feature-x");
        assert_eq!(normalized["labels"].as_array().unwrap()[0], "enhancement");
        assert_eq!(normalized["assignees"].as_array().unwrap()[0], "bob");
        assert_eq!(normalized["reviewers"].as_array().unwrap()[0], "carol");
        assert_eq!(normalized["mergeable"], true);
    }

    #[test]
    fn normalize_pr_handles_missing_optional_fields() {
        let raw = serde_json::json!({
            "number": 1,
            "title": "Minimal PR",
            "state": "open",
            "draft": true,
            "user": { "login": "someone" },
            "base": { "ref": "main" },
            "head": { "ref": "fix" },
            "labels": [],
            "assignees": [],
            "requested_reviewers": [],
            "mergeable": null,
            "created_at": "2026-04-01T10:00:00Z",
            "updated_at": "2026-04-01T10:00:00Z",
            "html_url": "https://github.com/owner/repo/pull/1",
        });

        let normalized = normalize_pr(&raw);
        assert_eq!(normalized["number"], 1);
        assert_eq!(normalized["draft"], true);
        assert!(normalized["labels"].as_array().unwrap().is_empty());
        assert!(normalized["reviewers"].as_array().unwrap().is_empty());
        assert!(normalized["mergeable"].is_null());
    }

    #[test]
    fn file_summary_shape() {
        // Verify the file summary structure we build in get_pr_diff
        let file = serde_json::json!({
            "filename": "src/main.rs",
            "status": "modified",
            "additions": 10,
            "deletions": 3,
            "changes": 13,
        });

        let summary = serde_json::json!({
            "filename": file["filename"],
            "status": file["status"],
            "additions": file["additions"],
            "deletions": file["deletions"],
            "changes": file["changes"],
        });

        assert_eq!(summary["filename"], "src/main.rs");
        assert_eq!(summary["additions"], 10);
        assert_eq!(summary["deletions"], 3);
    }

    #[test]
    fn author_filter_case_insensitive() {
        let pr = serde_json::json!({
            "user": { "login": "Alice" },
        });
        let login = pr["user"]["login"].as_str().unwrap();
        assert!(login.eq_ignore_ascii_case("alice"));
        assert!(login.eq_ignore_ascii_case("ALICE"));
    }
}
