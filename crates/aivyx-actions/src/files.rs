//! File actions — read, write, list, search, and organize local files.

use crate::Action;
use aivyx_core::Result;

pub struct ReadFile;

#[async_trait::async_trait]
impl Action for ReadFile {
    fn name(&self) -> &str { "read_file" }

    fn description(&self) -> &str {
        "Read the contents of a local file"
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
        let path = input["path"].as_str().unwrap_or_default();
        let contents = tokio::fs::read_to_string(path).await.map_err(aivyx_core::AivyxError::Io)?;
        Ok(serde_json::json!({ "path": path, "contents": contents }))
    }
}

pub struct WriteFile;

#[async_trait::async_trait]
impl Action for WriteFile {
    fn name(&self) -> &str { "write_file" }

    fn description(&self) -> &str {
        "Write content to a local file (creates or overwrites)"
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
        let path = input["path"].as_str().unwrap_or_default();
        let content = input["content"].as_str().unwrap_or_default();
        tokio::fs::write(path, content).await.map_err(aivyx_core::AivyxError::Io)?;
        Ok(serde_json::json!({ "status": "written", "path": path }))
    }
}

pub struct ListDirectory;

#[async_trait::async_trait]
impl Action for ListDirectory {
    fn name(&self) -> &str { "list_directory" }

    fn description(&self) -> &str {
        "List files and directories at a given path"
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
        let path = input["path"].as_str().unwrap_or_default();
        let mut entries = Vec::new();

        let mut dir = tokio::fs::read_dir(path).await.map_err(aivyx_core::AivyxError::Io)?;

        while let Ok(Some(entry)) = dir.next_entry().await {
            let meta = entry.metadata().await.ok();
            entries.push(serde_json::json!({
                "name": entry.file_name().to_string_lossy(),
                "is_dir": meta.as_ref().map(|m| m.is_dir()).unwrap_or(false),
                "size": meta.as_ref().map(|m| m.len()).unwrap_or(0),
            }));
        }

        Ok(serde_json::json!({ "path": path, "entries": entries }))
    }
}
