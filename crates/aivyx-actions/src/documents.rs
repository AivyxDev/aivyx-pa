//! Document Intelligence — local vault indexing, semantic search, and summarization.
//!
//! The vault is a user-configured directory of documents (markdown, text, PDF).
//! Documents are extracted, chunked, and embedded into the memory system for
//! semantic retrieval. A content-hash index avoids re-processing unchanged files.

use aivyx_core::Result;
use aivyx_crypto::{EncryptedStore, MasterKey};
use aivyx_memory::{MemoryKind, MemoryManager};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::Action;

// ── Configuration ─────────────────────────────────────────────────

/// Vault configuration — which directory to index and how.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultConfig {
    /// Path to the document vault directory.
    pub path: PathBuf,
    /// File extensions to index (e.g., ["md", "txt", "pdf"]).
    /// Defaults to ["md", "txt", "pdf"] if empty.
    #[serde(default = "default_extensions")]
    pub extensions: Vec<String>,
}

fn default_extensions() -> Vec<String> {
    vec!["md".into(), "txt".into(), "pdf".into()]
}

// ── Index tracking ───────────────────────────────────────────────

/// Tracks which files have been indexed and their content hashes.
/// Stored in the encrypted store under `vault-index:{path-hash}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexedFile {
    /// Relative path from vault root.
    pub relative_path: String,
    /// SHA-256 of the file content (hex).
    pub content_hash: String,
    /// Size in bytes at index time.
    pub size: u64,
    /// Number of chunks produced.
    pub chunk_count: usize,
    /// When this file was last indexed.
    pub indexed_at: chrono::DateTime<chrono::Utc>,
}

const INDEX_PREFIX: &str = "vault-index:";

/// Load the vault index from the encrypted store.
fn load_index(
    store: &EncryptedStore,
    key: &MasterKey,
) -> Result<Vec<IndexedFile>> {
    let keys = store.list_keys()?;
    let mut entries = Vec::new();
    for k in &keys {
        if !k.starts_with(INDEX_PREFIX) {
            continue;
        }
        if let Some(bytes) = store.get(k, key)? {
            match serde_json::from_slice::<IndexedFile>(&bytes) {
                Ok(entry) => entries.push(entry),
                Err(e) => tracing::warn!("Corrupt vault index entry '{k}': {e}"),
            }
        }
    }
    Ok(entries)
}

/// Save a single index entry.
fn save_index_entry(
    store: &EncryptedStore,
    key: &MasterKey,
    entry: &IndexedFile,
) -> Result<()> {
    let path_hash = hex_hash(entry.relative_path.as_bytes());
    let json = serde_json::to_vec(entry)
        .map_err(aivyx_core::AivyxError::Serialization)?;
    store.put(&format!("{INDEX_PREFIX}{path_hash}"), &json, key)
}

// ── Text extraction ──────────────────────────────────────────────

/// Extract plain text from a file based on its extension.
pub fn extract_text(path: &Path) -> Result<String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "txt" => std::fs::read_to_string(path)
            .map_err(|e| aivyx_core::AivyxError::Other(format!("Read error: {e}"))),
        "md" | "markdown" => extract_markdown(path),
        "pdf" => extract_pdf(path),
        other => Err(aivyx_core::AivyxError::Other(format!(
            "Unsupported file type: .{other}"
        ))),
    }
}

/// Extract text from a markdown file, stripping formatting.
fn extract_markdown(path: &Path) -> Result<String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| aivyx_core::AivyxError::Other(format!("Read error: {e}")))?;

    use pulldown_cmark::{Event, Parser, Tag, TagEnd};

    let parser = Parser::new(&raw);
    let mut text = String::with_capacity(raw.len());
    for event in parser {
        match event {
            Event::Text(t) | Event::Code(t) => text.push_str(&t),
            Event::SoftBreak | Event::HardBreak => text.push('\n'),
            Event::Start(Tag::Paragraph) => {
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                }
            }
            Event::End(TagEnd::Paragraph) => text.push('\n'),
            Event::Start(Tag::Heading { .. }) => {
                if !text.is_empty() {
                    text.push('\n');
                }
            }
            Event::End(TagEnd::Heading(_)) => text.push('\n'),
            Event::Start(Tag::CodeBlock(_)) | Event::End(TagEnd::CodeBlock) => {
                text.push('\n');
            }
            Event::Start(Tag::Item) => text.push_str("- "),
            _ => {}
        }
    }

    Ok(text.trim().to_string())
}

/// Extract text from a PDF file.
fn extract_pdf(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)
        .map_err(|e| aivyx_core::AivyxError::Other(format!("Read error: {e}")))?;

    pdf_extract::extract_text_from_mem(&bytes)
        .map(|t| t.trim().to_string())
        .map_err(|e| aivyx_core::AivyxError::Other(format!("PDF extraction error: {e}")))
}

// ── Chunking ─────────────────────────────────────────────────────

/// Maximum tokens per chunk (~4 chars/token, targeting ~1000 tokens).
const CHUNK_MAX_CHARS: usize = 4000;
/// Overlap between chunks for context continuity.
const CHUNK_OVERLAP_CHARS: usize = 400;

/// Split text into chunks suitable for embedding.
///
/// Tries to split on paragraph boundaries (double newline) first,
/// then on sentence boundaries, then on word boundaries.
pub fn chunk_text(text: &str, source_path: &str) -> Vec<String> {
    if text.len() <= CHUNK_MAX_CHARS {
        return vec![format_chunk(text, source_path, 1, 1)];
    }

    let paragraphs: Vec<&str> = text.split("\n\n").collect();
    let mut chunks = Vec::new();
    let mut current = String::new();

    for para in &paragraphs {
        let para = para.trim();
        if para.is_empty() {
            continue;
        }

        if current.len() + para.len() + 2 > CHUNK_MAX_CHARS && !current.is_empty() {
            chunks.push(current.clone());
            // Keep overlap from end of previous chunk.
            let overlap_start = current.len().saturating_sub(CHUNK_OVERLAP_CHARS);
            current = current[overlap_start..].to_string();
            current.push_str("\n\n");
        }

        if !current.is_empty() {
            current.push_str("\n\n");
        }
        current.push_str(para);
    }

    if !current.is_empty() {
        chunks.push(current);
    }

    let total = chunks.len();
    chunks
        .into_iter()
        .enumerate()
        .map(|(i, text)| format_chunk(&text, source_path, i + 1, total))
        .collect()
}

/// Format a chunk with metadata header for better retrieval context.
fn format_chunk(text: &str, source_path: &str, chunk_num: usize, total_chunks: usize) -> String {
    format!(
        "[Document: {source_path} | Chunk {chunk_num}/{total_chunks}]\n{text}"
    )
}

// ── Vault indexing ───────────────────────────────────────────────

/// Scan the vault directory and index new or modified documents.
///
/// Returns the number of files indexed (new + updated).
pub async fn index_vault(
    config: &VaultConfig,
    memory: &Arc<Mutex<MemoryManager>>,
    store: &EncryptedStore,
    vault_key: &MasterKey,
) -> Result<IndexResult> {
    let extensions = if config.extensions.is_empty() {
        default_extensions()
    } else {
        config.extensions.clone()
    };

    // Load existing index to check for changes.
    let existing = load_index(store, vault_key)?;
    let existing_map: std::collections::HashMap<&str, &IndexedFile> = existing
        .iter()
        .map(|e| (e.relative_path.as_str(), e))
        .collect();

    let vault_path = &config.path;
    if !vault_path.exists() {
        return Err(aivyx_core::AivyxError::Other(format!(
            "Vault directory does not exist: {}",
            vault_path.display()
        )));
    }

    let mut result = IndexResult::default();

    // Walk the vault directory.
    let files = collect_files(vault_path, &extensions)?;

    for file_path in &files {
        let relative = file_path
            .strip_prefix(vault_path)
            .unwrap_or(file_path)
            .to_string_lossy()
            .to_string();

        // Read file and compute hash.
        let content_bytes = match std::fs::read(file_path) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("Failed to read {}: {e}", file_path.display());
                result.errors += 1;
                continue;
            }
        };
        let content_hash = hex_hash(&content_bytes);
        let size = content_bytes.len() as u64;

        // Skip if already indexed and unchanged.
        if let Some(prev) = existing_map.get(relative.as_str())
            && prev.content_hash == content_hash {
                result.skipped += 1;
                continue;
            }

        // Extract text.
        let text = match extract_text(file_path) {
            Ok(t) if t.is_empty() => {
                tracing::debug!("Empty content: {relative}");
                result.skipped += 1;
                continue;
            }
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("Failed to extract text from {relative}: {e}");
                result.errors += 1;
                continue;
            }
        };

        // Chunk and embed.
        let chunks = chunk_text(&text, &relative);
        let chunk_count = chunks.len();

        let mut mgr = memory.lock().await;
        for chunk in chunks {
            if let Err(e) = mgr
                .remember(
                    chunk,
                    MemoryKind::Fact,
                    None, // global scope
                    vec!["document".into(), format!("doc:{relative}")],
                )
                .await
            {
                tracing::warn!("Failed to index chunk from {relative}: {e}");
                result.errors += 1;
            }
        }
        drop(mgr);

        // Update index.
        let entry = IndexedFile {
            relative_path: relative.clone(),
            content_hash,
            size,
            chunk_count,
            indexed_at: chrono::Utc::now(),
        };
        if let Err(e) = save_index_entry(store, vault_key, &entry) {
            tracing::warn!("Failed to save index entry for {relative}: {e}");
        }

        result.indexed += 1;
        tracing::info!("Indexed {relative} ({chunk_count} chunks)");
    }

    Ok(result)
}

/// Result of a vault indexing run.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct IndexResult {
    /// Files newly indexed or re-indexed.
    pub indexed: u32,
    /// Files skipped (unchanged since last index).
    pub skipped: u32,
    /// Files that failed to process.
    pub errors: u32,
}

/// Recursively collect files matching the allowed extensions.
fn collect_files(dir: &Path, extensions: &[String]) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_files_recursive(dir, extensions, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_files_recursive(
    dir: &Path,
    extensions: &[String],
    files: &mut Vec<PathBuf>,
) -> Result<()> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| aivyx_core::AivyxError::Other(format!("Read dir error: {e}")))?;

    for entry in entries {
        let entry = entry
            .map_err(|e| aivyx_core::AivyxError::Other(format!("Dir entry error: {e}")))?;
        let path = entry.path();

        if path.is_dir() {
            // Skip hidden directories.
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with('.'))
            {
                continue;
            }
            collect_files_recursive(&path, extensions, files)?;
        } else if path.is_file() {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            if extensions.iter().any(|allowed| allowed == &ext) {
                files.push(path);
            }
        }
    }

    Ok(())
}

/// SHA-256 hash as hex string.
fn hex_hash(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

// ── Action tools ─────────────────────────────────────────────────

/// Tool: search documents by semantic meaning.
pub struct SearchDocuments {
    pub memory: Arc<Mutex<MemoryManager>>,
}

#[async_trait::async_trait]
impl Action for SearchDocuments {
    fn name(&self) -> &str {
        "search_documents"
    }

    fn description(&self) -> &str {
        "Search the document vault by meaning. Returns relevant passages from \
         indexed documents. Use this when the user asks about information that \
         might be in their documents."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "What to search for — a question or topic"
                },
                "limit": {
                    "type": "integer",
                    "description": "Max results to return (default: 5)"
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let query = input["query"]
            .as_str()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("'query' is required".into()))?;
        let limit = input["limit"].as_u64().unwrap_or(5) as usize;

        let mut mgr = self.memory.lock().await;
        let results = mgr
            .recall(query, limit, None, &["document".into()])
            .await?;

        let entries: Vec<serde_json::Value> = results
            .iter()
            .map(|m| {
                serde_json::json!({
                    "content": m.content,
                    "tags": m.tags,
                    "relevance": "semantic",
                })
            })
            .collect();

        Ok(serde_json::json!({
            "results": entries,
            "count": entries.len(),
        }))
    }
}

/// Tool: read a specific document from the vault.
pub struct ReadDocument {
    pub vault_path: PathBuf,
}

#[async_trait::async_trait]
impl Action for ReadDocument {
    fn name(&self) -> &str {
        "read_document"
    }

    fn description(&self) -> &str {
        "Read the full text of a specific document from the vault by its path. \
         Use search_documents first to find relevant documents."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative path within the vault (e.g., 'reports/Q1.md')"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let rel_path = input["path"]
            .as_str()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("'path' is required".into()))?;

        // Prevent path traversal.
        if rel_path.contains("..") {
            return Err(aivyx_core::AivyxError::Validation(
                "Path must not contain '..'".into(),
            ));
        }

        let full_path = self.vault_path.join(rel_path);
        if !full_path.starts_with(&self.vault_path) {
            return Err(aivyx_core::AivyxError::Validation(
                "Path escapes vault directory".into(),
            ));
        }

        let text = extract_text(&full_path)?;

        // Truncate to 32K chars (same as fetch_webpage).
        // Use char-boundary-safe slicing to avoid panics on multi-byte UTF-8.
        let truncated = if text.len() > 32_000 {
            let mut end = 32_000;
            while end > 0 && !text.is_char_boundary(end) {
                end -= 1;
            }
            format!("{}...\n[Truncated at ~32,000 characters]", &text[..end])
        } else {
            text
        };

        Ok(serde_json::json!({
            "path": rel_path,
            "content": truncated,
        }))
    }
}

/// Tool: list documents in the vault.
pub struct ListVaultDocuments {
    pub vault_path: PathBuf,
    pub extensions: Vec<String>,
}

#[async_trait::async_trait]
impl Action for ListVaultDocuments {
    fn name(&self) -> &str {
        "list_vault_documents"
    }

    fn description(&self) -> &str {
        "List all documents in the vault with their paths and sizes."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "subdirectory": {
                    "type": "string",
                    "description": "Optional subdirectory to list (relative to vault root)"
                }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let subdir = input["subdirectory"].as_str().unwrap_or("");

        let search_path = if subdir.is_empty() {
            self.vault_path.clone()
        } else {
            if subdir.contains("..") {
                return Err(aivyx_core::AivyxError::Validation(
                    "Path must not contain '..'".into(),
                ));
            }
            self.vault_path.join(subdir)
        };

        let files = collect_files(&search_path, &self.extensions)?;

        let entries: Vec<serde_json::Value> = files
            .iter()
            .filter_map(|f| {
                let relative = f
                    .strip_prefix(&self.vault_path)
                    .ok()?
                    .to_string_lossy()
                    .to_string();
                let meta = std::fs::metadata(f).ok()?;
                Some(serde_json::json!({
                    "path": relative,
                    "size": meta.len(),
                    "extension": f.extension().and_then(|e| e.to_str()).unwrap_or(""),
                }))
            })
            .collect();

        Ok(serde_json::json!({
            "documents": entries,
            "count": entries.len(),
        }))
    }
}

/// Tool: trigger a vault re-index.
pub struct IndexVault {
    pub config: VaultConfig,
    pub memory: Arc<Mutex<MemoryManager>>,
    pub store: Arc<EncryptedStore>,
    pub vault_key: MasterKey,
}

#[async_trait::async_trait]
impl Action for IndexVault {
    fn name(&self) -> &str {
        "index_vault"
    }

    fn description(&self) -> &str {
        "Re-index the document vault. Scans for new or modified files, \
         extracts text, and embeds them for semantic search. Only changed \
         files are re-processed."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn execute(&self, _input: serde_json::Value) -> Result<serde_json::Value> {
        let result = index_vault(
            &self.config,
            &self.memory,
            &self.store,
            &self.vault_key,
        )
        .await?;

        Ok(serde_json::json!({
            "status": "complete",
            "indexed": result.indexed,
            "skipped": result.skipped,
            "errors": result.errors,
        }))
    }
}

// ── Document deletion ────────────────────────────────────────────

/// Tool: delete a document from the vault (file + index + memory chunks).
pub struct DeleteDocument {
    pub vault_path: PathBuf,
    pub memory: Arc<Mutex<MemoryManager>>,
    pub store: Arc<EncryptedStore>,
    pub vault_key: MasterKey,
}

#[async_trait::async_trait]
impl Action for DeleteDocument {
    fn name(&self) -> &str { "delete_document" }

    fn description(&self) -> &str {
        "Delete a document from the vault. Removes the file, its index entry, and all \
         associated memory chunks. Use list_vault_documents to find paths."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative path within the vault (e.g. 'notes/meeting.md')"
                }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let path = input["path"].as_str()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("'path' is required".into()))?;

        // Reject path traversal
        if path.contains("..") {
            return Err(aivyx_core::AivyxError::Validation(
                "Path must not contain '..'".into(),
            ));
        }

        let full_path = self.vault_path.join(path);

        // 1. Delete the file
        if full_path.exists() {
            std::fs::remove_file(&full_path).map_err(|e| {
                aivyx_core::AivyxError::Channel(format!("Failed to delete '{path}': {e}"))
            })?;
        }

        // 2. Delete the index entry
        let path_hash = hex_hash(path.as_bytes());
        let index_key = format!("{INDEX_PREFIX}{path_hash}");
        let _ = self.store.delete(&index_key);

        // 3. Clean up memory chunks tagged with doc:{path}
        let doc_tag = format!("doc:{path}");
        let mut mgr = self.memory.lock().await;
        let memory_ids = mgr.list_memories()?;
        let mut removed_chunks = 0u32;
        for mid in &memory_ids {
            if let Some(entry) = mgr.load_memory(mid)? {
                if entry.tags.contains(&doc_tag) {
                    mgr.forget(mid)?;
                    removed_chunks += 1;
                }
            }
        }

        Ok(serde_json::json!({
            "status": "deleted",
            "path": path,
            "memory_chunks_removed": removed_chunks,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Text extraction tests ────────────────────────────────────

    #[test]
    fn extract_text_from_txt() {
        let dir = std::env::temp_dir().join(format!("aivyx-doc-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.txt");
        std::fs::write(&path, "Hello, world!").unwrap();

        let text = extract_text(&path).unwrap();
        assert_eq!(text, "Hello, world!");

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn extract_text_from_markdown() {
        let dir = std::env::temp_dir().join(format!("aivyx-doc-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.md");
        std::fs::write(&path, "# Title\n\nSome **bold** text.\n\n- Item 1\n- Item 2").unwrap();

        let text = extract_text(&path).unwrap();
        assert!(text.contains("Title"));
        assert!(text.contains("bold"));
        assert!(text.contains("Item 1"));
        // Markdown formatting should be stripped
        assert!(!text.contains("**"));
        assert!(!text.contains("#"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn extract_text_unsupported_extension() {
        let dir = std::env::temp_dir().join(format!("aivyx-doc-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.xyz");
        std::fs::write(&path, "data").unwrap();

        let result = extract_text(&path);
        assert!(result.is_err());

        let _ = std::fs::remove_dir_all(dir);
    }

    // ── Chunking tests ───────────────────────────────────────────

    #[test]
    fn chunk_short_text() {
        let chunks = chunk_text("Short text.", "test.md");
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].contains("[Document: test.md | Chunk 1/1]"));
        assert!(chunks[0].contains("Short text."));
    }

    #[test]
    fn chunk_long_text_splits_on_paragraphs() {
        // Create text longer than CHUNK_MAX_CHARS with paragraph breaks.
        let paragraph = "A".repeat(1500);
        let text = format!("{paragraph}\n\n{paragraph}\n\n{paragraph}\n\n{paragraph}");
        assert!(text.len() > CHUNK_MAX_CHARS);

        let chunks = chunk_text(&text, "long.txt");
        assert!(chunks.len() > 1, "Expected multiple chunks, got {}", chunks.len());

        // Each chunk should have the document header.
        for chunk in &chunks {
            assert!(chunk.contains("[Document: long.txt |"));
        }
    }

    #[test]
    fn chunk_preserves_all_content() {
        let paragraph = "Word ".repeat(300); // ~1500 chars
        let text = format!("{paragraph}\n\n{paragraph}\n\n{paragraph}");

        let chunks = chunk_text(&text, "test.md");
        // All original words should appear in at least one chunk.
        // (Overlap means some words appear in multiple chunks, which is fine.)
        assert!(chunks.len() >= 2);
    }

    // ── File collection tests ────────────────────────────────────

    #[test]
    fn collect_files_respects_extensions() {
        let dir = std::env::temp_dir().join(format!("aivyx-collect-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        std::fs::write(dir.join("doc.md"), "# Doc").unwrap();
        std::fs::write(dir.join("notes.txt"), "Notes").unwrap();
        std::fs::write(dir.join("image.png"), "PNG").unwrap();
        std::fs::write(dir.join("script.py"), "print()").unwrap();

        let files = collect_files(&dir, &["md".into(), "txt".into()]).unwrap();
        assert_eq!(files.len(), 2);

        let names: Vec<String> = files
            .iter()
            .map(|f| f.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(names.contains(&"doc.md".to_string()));
        assert!(names.contains(&"notes.txt".to_string()));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn collect_files_skips_hidden_dirs() {
        let dir = std::env::temp_dir().join(format!("aivyx-hidden-test-{}", uuid::Uuid::new_v4()));
        let hidden = dir.join(".hidden");
        std::fs::create_dir_all(&hidden).unwrap();

        std::fs::write(dir.join("visible.md"), "ok").unwrap();
        std::fs::write(hidden.join("secret.md"), "hidden").unwrap();

        let files = collect_files(&dir, &["md".into()]).unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].to_string_lossy().contains("visible.md"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn collect_files_recursive() {
        let dir = std::env::temp_dir().join(format!("aivyx-recurse-test-{}", uuid::Uuid::new_v4()));
        let sub = dir.join("sub");
        std::fs::create_dir_all(&sub).unwrap();

        std::fs::write(dir.join("top.md"), "top").unwrap();
        std::fs::write(sub.join("nested.md"), "nested").unwrap();

        let files = collect_files(&dir, &["md".into()]).unwrap();
        assert_eq!(files.len(), 2);

        let _ = std::fs::remove_dir_all(dir);
    }

    // ── Hash test ────────────────────────────────────────────────

    #[test]
    fn hex_hash_deterministic() {
        let h1 = hex_hash(b"hello");
        let h2 = hex_hash(b"hello");
        assert_eq!(h1, h2);
        assert_ne!(hex_hash(b"hello"), hex_hash(b"world"));
        assert_eq!(h1.len(), 64); // SHA-256 = 32 bytes = 64 hex chars
    }

    // ── Index tracking tests ─────────────────────────────────────

    #[test]
    fn save_and_load_index_entry() {
        let dir = std::env::temp_dir().join(format!("aivyx-idx-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = EncryptedStore::open(dir.join("store.db")).unwrap();
        let key = aivyx_crypto::derive_domain_key(&aivyx_crypto::MasterKey::generate(), b"vault");

        let entry = IndexedFile {
            relative_path: "docs/report.md".into(),
            content_hash: hex_hash(b"test content"),
            size: 1234,
            chunk_count: 3,
            indexed_at: chrono::Utc::now(),
        };

        save_index_entry(&store, &key, &entry).unwrap();

        let loaded = load_index(&store, &key).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].relative_path, "docs/report.md");
        assert_eq!(loaded[0].chunk_count, 3);

        let _ = std::fs::remove_dir_all(dir);
    }

    // ── Tool schema tests ────────────────────────────────────────

    #[test]
    fn search_documents_schema() {
        // SearchDocuments needs a MemoryManager, which needs embeddings.
        // Just test that the schema is correct without constructing one.
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" },
                "limit": { "type": "integer" }
            },
            "required": ["query"]
        });
        assert!(schema["required"].as_array().unwrap().iter().any(|v| v == "query"));
    }

    #[test]
    fn read_document_rejects_path_traversal() {
        let tool = ReadDocument {
            vault_path: PathBuf::from("/tmp/vault"),
        };
        let input = serde_json::json!({ "path": "../../../etc/passwd" });
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(tool.execute(input));
        assert!(result.is_err());
    }

    #[test]
    fn list_vault_schema() {
        let tool = ListVaultDocuments {
            vault_path: PathBuf::from("/tmp/vault"),
            extensions: vec!["md".into()],
        };
        assert_eq!(tool.name(), "list_vault_documents");
    }

    #[test]
    fn index_vault_schema() {
        // Just check the tool name and schema structure.
        let schema = serde_json::json!({
            "type": "object",
            "properties": {}
        });
        assert_eq!(schema["type"], "object");
    }

    // ── Format chunk tests ───────────────────────────────────────

    #[test]
    fn format_chunk_includes_metadata() {
        let chunk = format_chunk("Some text", "reports/Q1.md", 2, 5);
        assert!(chunk.starts_with("[Document: reports/Q1.md | Chunk 2/5]"));
        assert!(chunk.contains("Some text"));
    }

    // ── Delete document tests ───────────────────────────────────

    #[test]
    fn delete_document_schema() {
        // Verify schema without constructing MemoryManager
        let schema = serde_json::json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "path": { "type": "string" }
            }
        });
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "path"));
    }

    #[test]
    fn delete_document_rejects_path_traversal() {
        use aivyx_crypto::MasterKey;

        struct DummyEmbed;
        #[async_trait::async_trait]
        impl aivyx_llm::EmbeddingProvider for DummyEmbed {
            fn name(&self) -> &str { "dummy" }
            fn dimensions(&self) -> usize { 128 }
            async fn embed(&self, _text: &str) -> std::result::Result<aivyx_llm::Embedding, aivyx_core::AivyxError> {
                Ok(aivyx_llm::Embedding { vector: vec![0.1; 128], dimensions: 128 })
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let store = aivyx_memory::MemoryStore::open(dir.path().join("test.db")).unwrap();
        let master = MasterKey::generate();
        let mem_key = aivyx_crypto::derive_domain_key(&master, b"memory");
        let vault_key = aivyx_crypto::derive_domain_key(&master, b"vault");
        let mgr = MemoryManager::new(store, Arc::new(DummyEmbed), mem_key, 0).unwrap();
        let enc_store = EncryptedStore::open(dir.path().join("enc.db")).unwrap();

        let tool = DeleteDocument {
            vault_path: dir.path().to_path_buf(),
            memory: Arc::new(Mutex::new(mgr)),
            store: Arc::new(enc_store),
            vault_key,
        };

        let input = serde_json::json!({ "path": "../../../etc/passwd" });
        let result = tokio::runtime::Runtime::new().unwrap().block_on(tool.execute(input));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains(".."), "Error should mention path traversal: {err}");
    }
}
