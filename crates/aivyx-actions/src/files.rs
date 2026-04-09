//! File actions — read, write, and list local files with path validation.
//!
//! All file operations validate paths against a denylist of sensitive
//! system locations. ReadFile caps file size to prevent OOM.

use crate::Action;
use aivyx_core::Result;

/// Maximum file size for ReadFile (10 MB).
const MAX_READ_BYTES: u64 = 10 * 1024 * 1024;

/// Maximum directory entries returned by ListDirectory.
const MAX_DIR_ENTRIES: usize = 500;

/// Paths that must never be read or written by the agent.
const DENIED_PATHS: &[&str] = &["/etc/shadow", "/etc/gshadow", "/etc/master.passwd"];

/// Path prefixes that must never be read or written.
const DENIED_PREFIXES: &[&str] = &["/etc/ssl/private", "/proc/", "/sys/"];

/// Home-relative paths that must never be accessed.
const DENIED_HOME_PATHS: &[&str] = &[".ssh/", ".gnupg/", ".aws/credentials", ".config/gcloud/"];

/// Resolve `.` and `..` components without touching the filesystem.
///
/// This is used as a fallback when `canonicalize()` fails (file doesn't exist yet).
/// Without this, a path like `/home/user/safe/../../.ssh/id_rsa` would keep the
/// `..` components and bypass the home-relative denylist check.
fn normalize_path(path: &std::path::Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                components.pop();
            }
            Component::CurDir => {}
            other => components.push(other),
        }
    }
    components.iter().collect()
}

/// Validate that a path is safe to access.
///
/// Returns `Err` if the path matches any denied pattern.
pub fn validate_path(path: &str) -> Result<()> {
    if path.trim().is_empty() {
        return Err(aivyx_core::AivyxError::Validation(
            "path must not be empty".into(),
        ));
    }

    let normalized = std::path::Path::new(path);

    // Resolve the path to catch .. traversal.
    // canonicalize() works for existing files but fails for non-existent paths
    // (the normal case for WriteFile). For those, manually resolve components
    // to prevent paths like `/home/user/Documents/../../.ssh/authorized_keys`
    // from bypassing the home-relative denylist check.
    let resolved = normalized
        .canonicalize()
        .unwrap_or_else(|_| normalize_path(normalized));
    let resolved_str = resolved.to_string_lossy();

    // Check absolute denied paths
    for denied in DENIED_PATHS {
        if resolved_str == *denied {
            return Err(aivyx_core::AivyxError::CapabilityDenied(format!(
                "Access denied: {denied}"
            )));
        }
    }

    // Check denied prefixes
    for prefix in DENIED_PREFIXES {
        if resolved_str.starts_with(prefix) {
            return Err(aivyx_core::AivyxError::CapabilityDenied(format!(
                "Access denied: path under {prefix}"
            )));
        }
    }

    // Check home-relative denied paths
    if let Some(home) = dirs::home_dir() {
        let home_str = home.to_string_lossy();
        if resolved_str.starts_with(home_str.as_ref()) {
            let relative = &resolved_str[home_str.len()..].trim_start_matches('/');
            for denied in DENIED_HOME_PATHS {
                if relative.starts_with(denied) {
                    return Err(aivyx_core::AivyxError::CapabilityDenied(format!(
                        "Access denied: ~/{denied}"
                    )));
                }
            }
        }
    }

    Ok(())
}

pub struct ReadFile;

#[async_trait::async_trait]
impl Action for ReadFile {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read the contents of a local file (max 10 MB, sensitive paths blocked)"
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Absolute path to the file" }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let path = input["path"]
            .as_str()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("path is required".into()))?;

        validate_path(path)?;

        // Check file size before reading
        let metadata = tokio::fs::metadata(path)
            .await
            .map_err(aivyx_core::AivyxError::Io)?;

        if metadata.len() > MAX_READ_BYTES {
            return Err(aivyx_core::AivyxError::Validation(format!(
                "File too large ({} bytes, max {} bytes)",
                metadata.len(),
                MAX_READ_BYTES,
            )));
        }

        let contents = tokio::fs::read_to_string(path)
            .await
            .map_err(aivyx_core::AivyxError::Io)?;

        Ok(serde_json::json!({ "path": path, "contents": contents }))
    }
}

pub struct WriteFile;

#[async_trait::async_trait]
impl Action for WriteFile {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Write content to a local file (creates or overwrites). Sensitive paths are blocked."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Absolute path to write" },
                "content": { "type": "string", "description": "Content to write" }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let path = input["path"]
            .as_str()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("path is required".into()))?;
        let content = input["content"]
            .as_str()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("content is required".into()))?;

        validate_path(path)?;

        tokio::fs::write(path, content)
            .await
            .map_err(aivyx_core::AivyxError::Io)?;

        Ok(serde_json::json!({ "status": "written", "path": path }))
    }
}

pub struct ListDirectory;

#[async_trait::async_trait]
impl Action for ListDirectory {
    fn name(&self) -> &str {
        "list_directory"
    }

    fn description(&self) -> &str {
        "List files and directories at a given path (max 500 entries)"
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Directory path to list" }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let path = input["path"]
            .as_str()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("path is required".into()))?;

        validate_path(path)?;

        let mut entries = Vec::new();
        let mut dir = tokio::fs::read_dir(path)
            .await
            .map_err(aivyx_core::AivyxError::Io)?;

        while let Ok(Some(entry)) = dir.next_entry().await {
            if entries.len() >= MAX_DIR_ENTRIES {
                break;
            }
            let meta = entry.metadata().await.ok();
            entries.push(serde_json::json!({
                "name": entry.file_name().to_string_lossy(),
                "is_dir": meta.as_ref().map(|m| m.is_dir()).unwrap_or(false),
                "size": meta.as_ref().map(|m| m.len()).unwrap_or(0),
            }));
        }

        let truncated = entries.len() >= MAX_DIR_ENTRIES;
        Ok(serde_json::json!({
            "path": path,
            "entries": entries,
            "truncated": truncated,
        }))
    }
}

pub struct ReplaceFileContent;

#[async_trait::async_trait]
impl Action for ReplaceFileContent {
    fn name(&self) -> &str {
        "replace_file_content"
    }

    fn description(&self) -> &str {
        "Edit an existing file by replacing a specific target string with a new string. \
         The target_content must match exactly. Useful for partial code updates without hallucination."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Absolute path to the file" },
                "target_content": { "type": "string", "description": "Exact text chunk to be replaced" },
                "replacement_content": { "type": "string", "description": "New text to insert" }
            },
            "required": ["path", "target_content", "replacement_content"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let path = input["path"]
            .as_str()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("path required".into()))?;
        let target = input["target_content"]
            .as_str()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("target_content required".into()))?;
        let replacement = input["replacement_content"].as_str().ok_or_else(|| {
            aivyx_core::AivyxError::Validation("replacement_content required".into())
        })?;

        validate_path(path)?;

        let contents = tokio::fs::read_to_string(path)
            .await
            .map_err(aivyx_core::AivyxError::Io)?;

        if !contents.contains(target) {
            return Err(aivyx_core::AivyxError::Validation(
                "target_content not found in file".into(),
            ));
        }

        // Only replace first occurrence or error if multiple? Let's just replace all.
        // Usually, the LLM provides enough context so it matches uniquely.
        let new_contents = contents.replacen(target, replacement, 1);

        tokio::fs::write(path, new_contents)
            .await
            .map_err(aivyx_core::AivyxError::Io)?;

        Ok(serde_json::json!({ "status": "replaced", "path": path }))
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn denies_etc_shadow() {
        assert!(validate_path("/etc/shadow").is_err());
    }

    #[test]
    fn denies_ssh_directory() {
        let home = dirs::home_dir().unwrap();
        let ssh_path = format!("{}/.ssh/id_rsa", home.display());
        assert!(validate_path(&ssh_path).is_err());
    }

    #[test]
    fn denies_aws_credentials() {
        let home = dirs::home_dir().unwrap();
        let aws_path = format!("{}/.aws/credentials", home.display());
        assert!(validate_path(&aws_path).is_err());
    }

    #[test]
    fn denies_proc_filesystem() {
        assert!(validate_path("/proc/1/environ").is_err());
    }

    #[test]
    fn allows_normal_paths() {
        // These paths don't need to exist — validate_path checks patterns, not existence
        assert!(validate_path("/tmp/test.txt").is_ok());
        let home = dirs::home_dir().unwrap();
        let doc_path = format!("{}/Documents/notes.txt", home.display());
        assert!(validate_path(&doc_path).is_ok());
    }

    #[test]
    fn denies_empty_path() {
        assert!(validate_path("").is_err());
        assert!(validate_path("   ").is_err());
    }

    #[tokio::test]
    async fn read_file_rejects_sensitive_path() {
        let action = ReadFile;
        let input = serde_json::json!({ "path": "/etc/shadow" });
        assert!(action.execute(input).await.is_err());
    }

    #[tokio::test]
    async fn write_file_rejects_sensitive_path() {
        let action = WriteFile;
        let input = serde_json::json!({
            "path": "/etc/shadow",
            "content": "pwned"
        });
        assert!(action.execute(input).await.is_err());
    }

    #[tokio::test]
    async fn read_file_rejects_missing_path() {
        let action = ReadFile;
        let input = serde_json::json!({});
        assert!(action.execute(input).await.is_err());
    }
}
