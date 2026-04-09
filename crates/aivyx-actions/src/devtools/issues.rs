//! Issue tracking action tools — list, read, and create issues.
//!
//! Provides `list_issues`, `get_issue`, and `create_issue` actions that
//! work with GitHub and Gitea issue APIs. Requires forge configuration
//! in `[devtools]`.

use super::{DevToolsConfig, ForgeKind, forge_get, forge_post, forge_repo};
use crate::Action;
use aivyx_core::{AivyxError, Result};

// ── ListIssues ────────────────────────────────────────────────

pub struct ListIssues {
    pub config: DevToolsConfig,
}

#[async_trait::async_trait]
impl Action for ListIssues {
    fn name(&self) -> &str {
        "list_issues"
    }

    fn description(&self) -> &str {
        "List issues from the configured repository. Filter by state (open/closed/all), \
         labels, assignee, or milestone. Returns title, number, state, author, and labels."
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
                "labels": {
                    "type": "string",
                    "description": "Comma-separated label names to filter by"
                },
                "assignee": {
                    "type": "string",
                    "description": "Filter by assignee username"
                },
                "milestone": {
                    "type": "string",
                    "description": "Filter by milestone title or number"
                },
                "limit": {
                    "type": "integer",
                    "description": "Max issues to return (default 20, max 50)"
                }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let repo = forge_repo(&self.config)?;
        let forge = self.config.forge.ok_or_else(|| {
            AivyxError::Validation("No forge configured — set forge in [devtools]".into())
        })?;

        let state = input
            .get("state")
            .and_then(|v| v.as_str())
            .unwrap_or("open");
        let limit = input
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(20)
            .min(50);
        let labels = input.get("labels").and_then(|v| v.as_str());
        let assignee = input.get("assignee").and_then(|v| v.as_str());
        let milestone = input.get("milestone").and_then(|v| v.as_str());

        let issues = match forge {
            ForgeKind::Github => {
                fetch_github_issues(
                    &self.config,
                    repo,
                    state,
                    labels,
                    assignee,
                    milestone,
                    limit,
                )
                .await?
            }
            ForgeKind::Gitea => {
                fetch_gitea_issues(
                    &self.config,
                    repo,
                    state,
                    labels,
                    assignee,
                    milestone,
                    limit,
                )
                .await?
            }
        };

        Ok(serde_json::json!({
            "repo": repo,
            "state": state,
            "count": issues.len(),
            "issues": issues,
        }))
    }
}

// ── GetIssue ──────────────────────────────────────────────────

pub struct GetIssue {
    pub config: DevToolsConfig,
}

#[async_trait::async_trait]
impl Action for GetIssue {
    fn name(&self) -> &str {
        "get_issue"
    }

    fn description(&self) -> &str {
        "Get a specific issue by number, including body text and comments."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "number": {
                    "type": "integer",
                    "description": "Issue number"
                },
                "include_comments": {
                    "type": "boolean",
                    "description": "Include comments (default true)"
                }
            },
            "required": ["number"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let repo = forge_repo(&self.config)?;
        let number = input
            .get("number")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| AivyxError::Validation("number is required".into()))?;
        let include_comments = input
            .get("include_comments")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        // Fetch the issue itself
        let issue = forge_get(&self.config, &format!("/repos/{repo}/issues/{number}")).await?;

        let mut result = normalize_issue(&issue);

        // Include the full body
        result["body"] = issue["body"].clone();

        // Optionally fetch comments
        if include_comments {
            let comments_body = forge_get(
                &self.config,
                &format!("/repos/{repo}/issues/{number}/comments?per_page=50"),
            )
            .await;

            if let Ok(comments_json) = comments_body {
                let comments: Vec<serde_json::Value> = comments_json
                    .as_array()
                    .unwrap_or(&Vec::new())
                    .iter()
                    .map(normalize_comment)
                    .collect();
                result["comments"] = serde_json::json!(comments);
                result["comment_count"] = serde_json::json!(comments.len());
            }
        }

        Ok(result)
    }
}

// ── CreateIssue ───────────────────────────────────────────────

pub struct CreateIssue {
    pub config: DevToolsConfig,
}

#[async_trait::async_trait]
impl Action for CreateIssue {
    fn name(&self) -> &str {
        "create_issue"
    }

    fn description(&self) -> &str {
        "Create a new issue. Requires a title; body, labels, assignees, and \
         milestone are optional. Returns the created issue number and URL."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "Issue title"
                },
                "body": {
                    "type": "string",
                    "description": "Issue body (Markdown)"
                },
                "labels": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Label names to apply"
                },
                "assignees": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Usernames to assign"
                },
                "milestone": {
                    "type": "integer",
                    "description": "Milestone number to associate"
                }
            },
            "required": ["title"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let repo = forge_repo(&self.config)?;

        let title = input
            .get("title")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AivyxError::Validation("title is required".into()))?;

        if title.trim().is_empty() {
            return Err(AivyxError::Validation("title must not be empty".into()));
        }

        // Require a forge token for write operations
        if self.config.forge_token.is_none() {
            return Err(AivyxError::Validation(
                "Forge token required to create issues — set FORGE_TOKEN in keystore".into(),
            ));
        }

        let mut payload = serde_json::json!({ "title": title });

        if let Some(body) = input.get("body").and_then(|v| v.as_str()) {
            payload["body"] = serde_json::json!(body);
        }
        if let Some(labels) = input.get("labels").and_then(|v| v.as_array()) {
            payload["labels"] = serde_json::json!(labels);
        }
        if let Some(assignees) = input.get("assignees").and_then(|v| v.as_array()) {
            payload["assignees"] = serde_json::json!(assignees);
        }
        if let Some(milestone) = input.get("milestone").and_then(|v| v.as_u64()) {
            payload["milestone"] = serde_json::json!(milestone);
        }

        let created = forge_post(&self.config, &format!("/repos/{repo}/issues"), &payload).await?;

        Ok(serde_json::json!({
            "status": "created",
            "number": created["number"],
            "url": created["html_url"],
            "title": created["title"],
        }))
    }
}

// ── Shared helpers ────────────────────────────────────────────

/// Normalize an issue JSON response into a consistent shape.
fn normalize_issue(issue: &serde_json::Value) -> serde_json::Value {
    let empty = Vec::new();

    let labels: Vec<&str> = issue["labels"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .filter_map(|l| l["name"].as_str())
        .collect();

    let assignees: Vec<&str> = issue["assignees"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .filter_map(|a| a["login"].as_str())
        .collect();

    serde_json::json!({
        "number": issue["number"],
        "title": issue["title"],
        "state": issue["state"],
        "author": issue["user"]["login"],
        "labels": labels,
        "assignees": assignees,
        "milestone": issue["milestone"]["title"],
        "created_at": issue["created_at"],
        "updated_at": issue["updated_at"],
        "url": issue["html_url"],
    })
}

/// Normalize a comment JSON response.
fn normalize_comment(comment: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "id": comment["id"],
        "author": comment["user"]["login"],
        "body": comment["body"],
        "created_at": comment["created_at"],
        "updated_at": comment["updated_at"],
    })
}

// ── GitHub-specific fetchers ──────────────────────────────────

async fn fetch_github_issues(
    config: &DevToolsConfig,
    repo: &str,
    state: &str,
    labels: Option<&str>,
    assignee: Option<&str>,
    milestone: Option<&str>,
    limit: u64,
) -> Result<Vec<serde_json::Value>> {
    let mut query = format!("?state={state}&per_page={limit}");

    if let Some(l) = labels {
        query.push_str(&format!("&labels={l}"));
    }
    if let Some(a) = assignee {
        query.push_str(&format!("&assignee={a}"));
    }
    if let Some(m) = milestone {
        query.push_str(&format!("&milestone={m}"));
    }

    let body = forge_get(config, &format!("/repos/{repo}/issues{query}")).await?;

    Ok(body
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        // GitHub returns PRs in the issues endpoint — filter them out
        .filter(|i| i.get("pull_request").is_none())
        .map(normalize_issue)
        .collect())
}

// ── Gitea-specific fetchers ───────────────────────────────────

async fn fetch_gitea_issues(
    config: &DevToolsConfig,
    repo: &str,
    state: &str,
    labels: Option<&str>,
    assignee: Option<&str>,
    milestone: Option<&str>,
    limit: u64,
) -> Result<Vec<serde_json::Value>> {
    let mut query = format!("?state={state}&limit={limit}&type=issues");

    if let Some(l) = labels {
        query.push_str(&format!("&labels={l}"));
    }
    if let Some(a) = assignee {
        query.push_str(&format!("&assignee={a}"));
    }
    if let Some(m) = milestone {
        query.push_str(&format!("&milestone={m}"));
    }

    let body = forge_get(config, &format!("/repos/{repo}/issues{query}")).await?;

    Ok(body
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(normalize_issue)
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
    fn list_issues_name_and_schema() {
        let action = ListIssues {
            config: github_config(),
        };
        assert_eq!(action.name(), "list_issues");
        let schema = action.input_schema();
        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("state"));
        assert!(props.contains_key("labels"));
        assert!(props.contains_key("assignee"));
        assert!(props.contains_key("milestone"));
        assert!(props.contains_key("limit"));
    }

    #[test]
    fn get_issue_name_and_schema() {
        let action = GetIssue {
            config: github_config(),
        };
        assert_eq!(action.name(), "get_issue");
        let schema = action.input_schema();
        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("number"));
        assert!(props.contains_key("include_comments"));
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("number")));
    }

    #[test]
    fn create_issue_name_and_schema() {
        let action = CreateIssue {
            config: github_config(),
        };
        assert_eq!(action.name(), "create_issue");
        let schema = action.input_schema();
        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("title"));
        assert!(props.contains_key("body"));
        assert!(props.contains_key("labels"));
        assert!(props.contains_key("assignees"));
        assert!(props.contains_key("milestone"));
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("title")));
    }

    // ── Validation tests ──────────────────────────────────────

    #[tokio::test]
    async fn list_issues_rejects_no_forge() {
        let action = ListIssues {
            config: no_forge_config(),
        };
        let result = action.execute(serde_json::json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn get_issue_rejects_missing_number() {
        let action = GetIssue {
            config: github_config(),
        };
        let result = action.execute(serde_json::json!({})).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("number"));
    }

    #[tokio::test]
    async fn create_issue_rejects_empty_title() {
        let action = CreateIssue {
            config: github_config(),
        };
        let result = action.execute(serde_json::json!({ "title": "  " })).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("empty"));
    }

    #[tokio::test]
    async fn create_issue_rejects_missing_title() {
        let action = CreateIssue {
            config: github_config(),
        };
        let result = action.execute(serde_json::json!({})).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("title"));
    }

    #[tokio::test]
    async fn create_issue_rejects_no_token() {
        let action = CreateIssue {
            config: no_token_config(),
        };
        let result = action.execute(serde_json::json!({ "title": "Bug" })).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("token") || err.contains("FORGE_TOKEN"));
    }

    // ── Normalization tests ───────────────────────────────────

    #[test]
    fn normalize_issue_extracts_fields() {
        let raw = serde_json::json!({
            "number": 42,
            "title": "Fix the widget",
            "state": "open",
            "user": { "login": "alice" },
            "labels": [
                { "name": "bug" },
                { "name": "urgent" },
            ],
            "assignees": [
                { "login": "bob" },
            ],
            "milestone": { "title": "v1.0" },
            "created_at": "2026-04-01T10:00:00Z",
            "updated_at": "2026-04-02T10:00:00Z",
            "html_url": "https://github.com/owner/repo/issues/42",
        });

        let normalized = normalize_issue(&raw);
        assert_eq!(normalized["number"], 42);
        assert_eq!(normalized["title"], "Fix the widget");
        assert_eq!(normalized["state"], "open");
        assert_eq!(normalized["author"], "alice");
        let labels = normalized["labels"].as_array().unwrap();
        assert_eq!(labels.len(), 2);
        assert_eq!(labels[0], "bug");
        assert_eq!(labels[1], "urgent");
        let assignees = normalized["assignees"].as_array().unwrap();
        assert_eq!(assignees[0], "bob");
        assert_eq!(normalized["milestone"], "v1.0");
        assert_eq!(normalized["url"], "https://github.com/owner/repo/issues/42");
    }

    #[test]
    fn normalize_issue_handles_missing_optional_fields() {
        let raw = serde_json::json!({
            "number": 1,
            "title": "Minimal",
            "state": "open",
            "user": { "login": "someone" },
            "labels": [],
            "assignees": [],
            "milestone": null,
            "created_at": "2026-04-01T10:00:00Z",
            "updated_at": "2026-04-01T10:00:00Z",
            "html_url": "https://github.com/owner/repo/issues/1",
        });

        let normalized = normalize_issue(&raw);
        assert_eq!(normalized["number"], 1);
        assert!(normalized["labels"].as_array().unwrap().is_empty());
        assert!(normalized["assignees"].as_array().unwrap().is_empty());
        assert!(normalized["milestone"].is_null());
    }

    #[test]
    fn normalize_comment_extracts_fields() {
        let raw = serde_json::json!({
            "id": 999,
            "user": { "login": "commenter" },
            "body": "This is a comment.",
            "created_at": "2026-04-02T12:00:00Z",
            "updated_at": "2026-04-02T12:00:00Z",
        });

        let normalized = normalize_comment(&raw);
        assert_eq!(normalized["id"], 999);
        assert_eq!(normalized["author"], "commenter");
        assert_eq!(normalized["body"], "This is a comment.");
    }

    #[test]
    fn github_pr_filter() {
        // GitHub returns PRs in the issues endpoint — verify our filter logic
        let issues = vec![
            serde_json::json!({
                "number": 1, "title": "Real issue", "state": "open",
                "user": { "login": "a" }, "labels": [], "assignees": [],
                "milestone": null, "created_at": "", "updated_at": "", "html_url": "",
            }),
            serde_json::json!({
                "number": 2, "title": "This is a PR", "state": "open",
                "pull_request": { "url": "..." },
                "user": { "login": "b" }, "labels": [], "assignees": [],
                "milestone": null, "created_at": "", "updated_at": "", "html_url": "",
            }),
        ];

        let filtered: Vec<_> = issues
            .iter()
            .filter(|i| i.get("pull_request").is_none())
            .map(|i| normalize_issue(i))
            .collect();

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0]["number"], 1);
    }
}
