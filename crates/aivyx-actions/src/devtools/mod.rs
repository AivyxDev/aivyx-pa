//! Dev tools — Git, CI/CD, issue tracking, and code review integrations.
//!
//! Provides tools for development workflow awareness: local git operations,
//! forge API access (GitHub/Gitea), and CI/CD pipeline status.

pub mod ci;
pub mod git;
pub mod issues;
pub mod pr;

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;

// ── Shared Config ─────────────────────────────────────────────

/// Runtime configuration for dev tools.
///
/// `repo_path` is always required (local git operations).
/// Forge fields are optional — when present, enables remote API tools
/// (issues, PRs, CI/CD).
#[derive(Clone)]
pub struct DevToolsConfig {
    /// Default repository path for git operations.
    pub repo_path: PathBuf,
    /// Forge type (github, gitea).
    pub forge: Option<ForgeKind>,
    /// Forge API base URL (e.g., `https://api.github.com`).
    pub forge_api_url: Option<String>,
    /// Repository owner/name (e.g., `AivyxDev/aivyx`).
    pub forge_repo: Option<String>,
    /// API token from encrypted keystore — never in config.toml.
    pub forge_token: Option<String>,
}

impl fmt::Debug for DevToolsConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DevToolsConfig")
            .field("repo_path", &self.repo_path)
            .field("forge", &self.forge)
            .field("forge_api_url", &self.forge_api_url)
            .field("forge_repo", &self.forge_repo)
            .field(
                "forge_token",
                &self.forge_token.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

/// Supported forge (code hosting) platforms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ForgeKind {
    Github,
    Gitea,
}

// ── Git helpers ───────────────────────────────────────────────

/// Maximum output size from git commands (512 KB).
const MAX_GIT_OUTPUT: usize = 512 * 1024;

/// Run a git command in the given repo directory and return stdout.
async fn run_git(repo: &std::path::Path, args: &[&str]) -> aivyx_core::Result<String> {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        tokio::process::Command::new("git")
            .args(args)
            .current_dir(repo)
            .output(),
    )
    .await
    .map_err(|_| aivyx_core::AivyxError::Other("git command timed out after 30s".into()))?
    .map_err(aivyx_core::AivyxError::Io)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(aivyx_core::AivyxError::Other(format!(
            "git {} failed: {}",
            args.first().unwrap_or(&""),
            stderr.trim()
        )));
    }

    let stdout = &output.stdout;
    if stdout.len() > MAX_GIT_OUTPUT {
        Ok(format!(
            "{}\n...[output truncated at {} bytes]",
            String::from_utf8_lossy(&stdout[..MAX_GIT_OUTPUT]),
            MAX_GIT_OUTPUT
        ))
    } else {
        Ok(String::from_utf8_lossy(stdout).into_owned())
    }
}

/// Resolve repo path — use input override or fall back to config default.
fn resolve_repo(config: &DevToolsConfig, input: &serde_json::Value) -> aivyx_core::Result<PathBuf> {
    let path = if let Some(p) = input.get("repo_path").and_then(|v| v.as_str()) {
        PathBuf::from(p)
    } else {
        config.repo_path.clone()
    };

    if !path.is_absolute() {
        return Err(aivyx_core::AivyxError::Validation(
            "repo_path must be an absolute path".into(),
        ));
    }

    Ok(path)
}

// ── Forge API helpers ─────────────────────────────────────────

/// Resolve the forge API base URL, with defaults per platform.
fn forge_base_url(config: &DevToolsConfig) -> aivyx_core::Result<String> {
    if let Some(ref url) = config.forge_api_url {
        Ok(url.trim_end_matches('/').to_string())
    } else {
        match config.forge {
            Some(ForgeKind::Github) => Ok("https://api.github.com".into()),
            Some(ForgeKind::Gitea) => Err(aivyx_core::AivyxError::Validation(
                "Gitea requires forge_api_url in config".into(),
            )),
            None => Err(aivyx_core::AivyxError::Validation(
                "No forge configured — set forge and forge_api_url in [devtools]".into(),
            )),
        }
    }
}

/// Resolve the owner/repo string, returning an error if not configured.
fn forge_repo(config: &DevToolsConfig) -> aivyx_core::Result<&str> {
    config.forge_repo.as_deref().ok_or_else(|| {
        aivyx_core::AivyxError::Validation(
            "No repo configured — set repo in [devtools] (e.g. 'owner/repo')".into(),
        )
    })
}

/// Make an authenticated GET request to the forge API.
///
/// Returns the parsed JSON body. Handles auth headers for GitHub and Gitea.
async fn forge_get(config: &DevToolsConfig, path: &str) -> aivyx_core::Result<serde_json::Value> {
    let base = forge_base_url(config)?;
    let url = format!("{base}{path}");

    let client = crate::http_client();
    let mut req = client.get(&url);

    // Set auth header if token available
    if let Some(ref token) = config.forge_token {
        req = match config.forge {
            Some(ForgeKind::Github) => req.bearer_auth(token),
            Some(ForgeKind::Gitea) => req.header("Authorization", format!("token {token}")),
            None => req.bearer_auth(token),
        };
    }

    // GitHub requires a User-Agent header
    req = req.header("User-Agent", "aivyx-pa/0.1");
    req = req.header("Accept", "application/json");

    let resp = tokio::time::timeout(std::time::Duration::from_secs(30), req.send())
        .await
        .map_err(|_| aivyx_core::AivyxError::Other("Forge API request timed out after 30s".into()))?
        .map_err(|e| aivyx_core::AivyxError::Channel(format!("Forge API request failed: {e}")))?;

    let status = resp.status();
    let body: serde_json::Value = resp.json().await.map_err(|e| {
        aivyx_core::AivyxError::Channel(format!("Forge API response parse error: {e}"))
    })?;

    if !status.is_success() {
        let msg = body["message"].as_str().unwrap_or("unknown error");
        return Err(aivyx_core::AivyxError::Channel(format!(
            "Forge API error ({}): {msg}",
            status.as_u16()
        )));
    }

    Ok(body)
}

/// Make an authenticated GET request that returns raw text (for log downloads).
async fn forge_get_text(config: &DevToolsConfig, path: &str) -> aivyx_core::Result<String> {
    let base = forge_base_url(config)?;
    let url = format!("{base}{path}");

    let client = crate::http_client();
    let mut req = client.get(&url);

    if let Some(ref token) = config.forge_token {
        req = match config.forge {
            Some(ForgeKind::Github) => req.bearer_auth(token),
            Some(ForgeKind::Gitea) => req.header("Authorization", format!("token {token}")),
            None => req.bearer_auth(token),
        };
    }

    req = req.header("User-Agent", "aivyx-pa/0.1");

    let resp = tokio::time::timeout(std::time::Duration::from_secs(30), req.send())
        .await
        .map_err(|_| aivyx_core::AivyxError::Other("Forge API request timed out after 30s".into()))?
        .map_err(|e| aivyx_core::AivyxError::Channel(format!("Forge API request failed: {e}")))?;

    let status = resp.status();
    if !status.is_success() {
        return Err(aivyx_core::AivyxError::Channel(format!(
            "Forge API error ({})",
            status.as_u16()
        )));
    }

    let text = resp.text().await.map_err(|e| {
        aivyx_core::AivyxError::Channel(format!("Forge API response read error: {e}"))
    })?;

    // Cap log output
    if text.len() > MAX_GIT_OUTPUT {
        Ok(format!(
            "{}\n...[output truncated at {} bytes]",
            &text[..MAX_GIT_OUTPUT],
            MAX_GIT_OUTPUT
        ))
    } else {
        Ok(text)
    }
}

/// Make an authenticated POST request with a JSON body to the forge API.
///
/// Returns the parsed JSON response body.
async fn forge_post(
    config: &DevToolsConfig,
    path: &str,
    body: &serde_json::Value,
) -> aivyx_core::Result<serde_json::Value> {
    let base = forge_base_url(config)?;
    let url = format!("{base}{path}");

    let client = crate::http_client();
    let mut req = client.post(&url);

    if let Some(ref token) = config.forge_token {
        req = match config.forge {
            Some(ForgeKind::Github) => req.bearer_auth(token),
            Some(ForgeKind::Gitea) => req.header("Authorization", format!("token {token}")),
            None => req.bearer_auth(token),
        };
    }

    req = req.header("User-Agent", "aivyx-pa/0.1");
    req = req.header("Accept", "application/json");

    let resp = tokio::time::timeout(std::time::Duration::from_secs(30), req.json(body).send())
        .await
        .map_err(|_| aivyx_core::AivyxError::Other("Forge API request timed out after 30s".into()))?
        .map_err(|e| aivyx_core::AivyxError::Channel(format!("Forge API request failed: {e}")))?;

    let status = resp.status();
    let resp_body: serde_json::Value = resp.json().await.map_err(|e| {
        aivyx_core::AivyxError::Channel(format!("Forge API response parse error: {e}"))
    })?;

    if !status.is_success() {
        let msg = resp_body["message"].as_str().unwrap_or("unknown error");
        return Err(aivyx_core::AivyxError::Channel(format!(
            "Forge API error ({}): {msg}",
            status.as_u16()
        )));
    }

    Ok(resp_body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts_token() {
        let config = DevToolsConfig {
            repo_path: PathBuf::from("/home/user/project"),
            forge: Some(ForgeKind::Github),
            forge_api_url: Some("https://api.github.com".into()),
            forge_repo: Some("owner/repo".into()),
            forge_token: Some("ghp_secret_token_12345".into()),
        };
        let debug = format!("{config:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("ghp_secret"));
        assert!(debug.contains("/home/user/project"));
    }

    #[test]
    fn debug_without_forge() {
        let config = DevToolsConfig {
            repo_path: PathBuf::from("/repo"),
            forge: None,
            forge_api_url: None,
            forge_repo: None,
            forge_token: None,
        };
        let debug = format!("{config:?}");
        assert!(debug.contains("/repo"));
        assert!(debug.contains("None"));
    }

    #[test]
    fn forge_kind_deserializes() {
        let github: ForgeKind = serde_json::from_str("\"github\"").unwrap();
        assert_eq!(github, ForgeKind::Github);
        let gitea: ForgeKind = serde_json::from_str("\"gitea\"").unwrap();
        assert_eq!(gitea, ForgeKind::Gitea);
    }

    #[test]
    fn resolve_repo_uses_input_override() {
        let config = DevToolsConfig {
            repo_path: PathBuf::from("/default"),
            forge: None,
            forge_api_url: None,
            forge_repo: None,
            forge_token: None,
        };
        let input = serde_json::json!({ "repo_path": "/override" });
        let path = resolve_repo(&config, &input).unwrap();
        assert_eq!(path, PathBuf::from("/override"));
    }

    #[test]
    fn resolve_repo_falls_back_to_config() {
        let config = DevToolsConfig {
            repo_path: PathBuf::from("/default"),
            forge: None,
            forge_api_url: None,
            forge_repo: None,
            forge_token: None,
        };
        let input = serde_json::json!({});
        let path = resolve_repo(&config, &input).unwrap();
        assert_eq!(path, PathBuf::from("/default"));
    }

    #[test]
    fn resolve_repo_rejects_relative() {
        let config = DevToolsConfig {
            repo_path: PathBuf::from("/default"),
            forge: None,
            forge_api_url: None,
            forge_repo: None,
            forge_token: None,
        };
        let input = serde_json::json!({ "repo_path": "relative/path" });
        assert!(resolve_repo(&config, &input).is_err());
    }

    #[test]
    fn forge_base_url_defaults_github() {
        let config = DevToolsConfig {
            repo_path: PathBuf::from("/repo"),
            forge: Some(ForgeKind::Github),
            forge_api_url: None,
            forge_repo: None,
            forge_token: None,
        };
        assert_eq!(forge_base_url(&config).unwrap(), "https://api.github.com");
    }

    #[test]
    fn forge_base_url_uses_override() {
        let config = DevToolsConfig {
            repo_path: PathBuf::from("/repo"),
            forge: Some(ForgeKind::Gitea),
            forge_api_url: Some("https://gitea.example.com/api/v1".into()),
            forge_repo: None,
            forge_token: None,
        };
        assert_eq!(
            forge_base_url(&config).unwrap(),
            "https://gitea.example.com/api/v1"
        );
    }

    #[test]
    fn forge_base_url_strips_trailing_slash() {
        let config = DevToolsConfig {
            repo_path: PathBuf::from("/repo"),
            forge: Some(ForgeKind::Github),
            forge_api_url: Some("https://api.github.com/".into()),
            forge_repo: None,
            forge_token: None,
        };
        assert_eq!(forge_base_url(&config).unwrap(), "https://api.github.com");
    }

    #[test]
    fn forge_base_url_gitea_requires_url() {
        let config = DevToolsConfig {
            repo_path: PathBuf::from("/repo"),
            forge: Some(ForgeKind::Gitea),
            forge_api_url: None,
            forge_repo: None,
            forge_token: None,
        };
        assert!(forge_base_url(&config).is_err());
    }

    #[test]
    fn forge_base_url_no_forge_errors() {
        let config = DevToolsConfig {
            repo_path: PathBuf::from("/repo"),
            forge: None,
            forge_api_url: None,
            forge_repo: None,
            forge_token: None,
        };
        assert!(forge_base_url(&config).is_err());
    }

    #[test]
    fn forge_repo_returns_configured() {
        let config = DevToolsConfig {
            repo_path: PathBuf::from("/repo"),
            forge: Some(ForgeKind::Github),
            forge_api_url: None,
            forge_repo: Some("owner/repo".into()),
            forge_token: None,
        };
        assert_eq!(forge_repo(&config).unwrap(), "owner/repo");
    }

    #[test]
    fn forge_repo_missing_errors() {
        let config = DevToolsConfig {
            repo_path: PathBuf::from("/repo"),
            forge: Some(ForgeKind::Github),
            forge_api_url: None,
            forge_repo: None,
            forge_token: None,
        };
        assert!(forge_repo(&config).is_err());
    }
}
