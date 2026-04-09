//! Document creation and manipulation — text, spreadsheet, PDF, conversions.
//!
//! Subprocess-based, no heavy crate dependencies:
//! - **Text/Markdown**: Direct file I/O
//! - **Spreadsheet**: CSV natively, XLSX via `ssconvert` (gnumeric)
//! - **PDF creation**: `pandoc` or `weasyprint`
//! - **PDF reading**: `pdftotext` (poppler-utils)
//! - **Format conversion**: `pandoc` + `libreoffice --convert-to`
//!
//! All output paths are validated against the standard denylist to prevent
//! writes to sensitive locations.

use aivyx_core::{AivyxError, Result};
use std::path::Path;

/// Maximum content size for document creation (1 MB).
pub const MAX_DOC_CONTENT_BYTES: usize = 1_024 * 1024;

/// Maximum PDF file size for reading (50 MB).
const MAX_PDF_READ_BYTES: usize = 50 * 1024 * 1024;

// ── Text / Markdown ────────────────────────────────────────────

/// Create a text or markdown file with the given content.
///
/// The format is determined by the file extension:
/// - `.md`, `.markdown` → Markdown
/// - `.txt`, `.text`, `.log` → Plain text
/// - `.html`, `.htm` → HTML
/// - `.json`, `.yaml`, `.yml`, `.toml`, `.xml`, `.csv` → Structured text
/// - Any other → Plain text
pub async fn create_text_file(path: &str, content: &str) -> Result<String> {
    validate_output_path(path)?;
    validate_content_size(content)?;

    // Ensure parent directory exists.
    ensure_parent_dir(path).await?;

    tokio::fs::write(path, content)
        .await
        .map_err(|e| AivyxError::Io(e))?;

    let bytes = content.len();
    Ok(format!("Created {path} ({bytes} bytes)"))
}

// ── Spreadsheet ────────────────────────────────────────────────

/// Create a spreadsheet from structured data.
///
/// Always writes CSV natively. If the path ends in `.xlsx`, `.xls`, or `.ods`,
/// writes CSV first then converts via `ssconvert` (gnumeric) or
/// `libreoffice --convert-to`.
pub async fn create_spreadsheet(
    path: &str,
    headers: &[String],
    rows: &[Vec<String>],
) -> Result<String> {
    validate_output_path(path)?;
    ensure_parent_dir(path).await?;

    // Build CSV content.
    let mut csv_content = String::new();

    // Headers.
    csv_content.push_str(&csv_encode_row(headers));
    csv_content.push('\n');

    // Rows.
    for row in rows {
        csv_content.push_str(&csv_encode_row(row));
        csv_content.push('\n');
    }

    validate_content_size(&csv_content)?;

    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    if ext == "csv" {
        tokio::fs::write(path, &csv_content)
            .await
            .map_err(|e| AivyxError::Io(e))?;
        return Ok(format!(
            "Created {path} ({} rows, {} columns)",
            rows.len(),
            headers.len()
        ));
    }

    // Write CSV to temp file, then convert.
    let temp_csv = format!("{}.tmp.csv", path);
    tokio::fs::write(&temp_csv, &csv_content)
        .await
        .map_err(|e| AivyxError::Io(e))?;

    let convert_result = convert_csv_to(&temp_csv, path, &ext).await;

    // Clean up temp file.
    let _ = tokio::fs::remove_file(&temp_csv).await;

    convert_result?;

    Ok(format!(
        "Created {path} ({} rows, {} columns, converted from CSV)",
        rows.len(),
        headers.len()
    ))
}

/// Encode a row as CSV (with proper quoting).
fn csv_encode_row(fields: &[String]) -> String {
    fields
        .iter()
        .map(|f| {
            if f.contains(',') || f.contains('"') || f.contains('\n') {
                format!("\"{}\"", f.replace('"', "\"\""))
            } else {
                f.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Convert a CSV file to another spreadsheet format.
async fn convert_csv_to(csv_path: &str, output_path: &str, ext: &str) -> Result<()> {
    // Try ssconvert first (gnumeric — fast, reliable).
    let ssconvert_result = tokio::process::Command::new("ssconvert")
        .args([csv_path, output_path])
        .output()
        .await;

    if let Ok(output) = ssconvert_result {
        if output.status.success() {
            return Ok(());
        }
    }

    // Fallback: libreoffice --convert-to.
    let target_format = match ext {
        "xlsx" => "xlsx",
        "xls" => "xls",
        "ods" => "ods",
        _ => "xlsx",
    };

    let output_dir = Path::new(output_path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| ".".into());

    let lo_result = tokio::process::Command::new("libreoffice")
        .args([
            "--headless",
            "--convert-to",
            target_format,
            "--outdir",
            &output_dir,
            csv_path,
        ])
        .output()
        .await
        .map_err(|e| {
            AivyxError::Other(format!(
                "Neither ssconvert nor libreoffice available for spreadsheet conversion: {e}. \
             Install gnumeric or libreoffice."
            ))
        })?;

    if !lo_result.status.success() {
        return Err(AivyxError::Other(format!(
            "libreoffice conversion failed: {}",
            String::from_utf8_lossy(&lo_result.stderr).trim()
        )));
    }

    // libreoffice puts the output file with the same stem in output_dir.
    // Rename if needed (it uses the CSV's stem, not our target name).
    let lo_output = Path::new(&output_dir)
        .join(Path::new(csv_path).file_stem().unwrap_or_default())
        .with_extension(target_format);

    if lo_output != Path::new(output_path) {
        tokio::fs::rename(&lo_output, output_path)
            .await
            .map_err(|e| AivyxError::Io(e))?;
    }

    Ok(())
}

// ── PDF Creation ───────────────────────────────────────────────

/// Create a PDF from markdown or HTML content.
///
/// Tries pandoc first (best for markdown), falls back to weasyprint (best for HTML).
pub async fn create_pdf(path: &str, content: &str, source_format: &str) -> Result<String> {
    validate_output_path(path)?;
    validate_content_size(content)?;
    ensure_parent_dir(path).await?;

    let format = match source_format {
        "markdown" | "md" => "markdown",
        "html" => "html",
        "latex" | "tex" => "latex",
        "rst" => "rst",
        other => {
            return Err(AivyxError::Validation(format!(
                "Unsupported source format: '{other}'. Valid: markdown, html, latex, rst"
            )));
        }
    };

    // Write content to a temp file.
    let temp_ext = match format {
        "markdown" => "md",
        "html" => "html",
        "latex" => "tex",
        "rst" => "rst",
        _ => "txt",
    };
    let temp_input = format!("{path}.tmp.{temp_ext}");
    tokio::fs::write(&temp_input, content)
        .await
        .map_err(|e| AivyxError::Io(e))?;

    let result = try_pandoc_pdf(&temp_input, path, format)
        .await
        .or_else(|_| {
            // Block on weasyprint fallback only for HTML.
            if format == "html" {
                // We can't nest await in or_else, so return the error
                // and handle weasyprint separately.
                Err(AivyxError::Other("pandoc failed, trying weasyprint".into()))
            } else {
                Err(AivyxError::Other(
                    "pandoc is required for non-HTML PDF creation. \
                     Install pandoc (sudo apt install pandoc)."
                        .into(),
                ))
            }
        });

    // If pandoc failed and source is HTML, try weasyprint.
    if result.is_err() && format == "html" {
        let wp_result = try_weasyprint_pdf(&temp_input, path).await;
        let _ = tokio::fs::remove_file(&temp_input).await;
        wp_result?;
        let size = tokio::fs::metadata(path)
            .await
            .map(|m| m.len())
            .unwrap_or(0);
        return Ok(format!("Created PDF {path} ({size} bytes, via weasyprint)"));
    }

    let _ = tokio::fs::remove_file(&temp_input).await;
    result?;

    let size = tokio::fs::metadata(path)
        .await
        .map(|m| m.len())
        .unwrap_or(0);
    Ok(format!("Created PDF {path} ({size} bytes, via pandoc)"))
}

async fn try_pandoc_pdf(input: &str, output: &str, format: &str) -> Result<()> {
    let output_result = tokio::process::Command::new("pandoc")
        .args(["-f", format, "-o", output, input])
        .output()
        .await
        .map_err(|e| AivyxError::Other(format!("pandoc: {e}")))?;

    if output_result.status.success() {
        Ok(())
    } else {
        Err(AivyxError::Other(format!(
            "pandoc failed: {}",
            String::from_utf8_lossy(&output_result.stderr).trim()
        )))
    }
}

async fn try_weasyprint_pdf(input: &str, output: &str) -> Result<()> {
    let output_result = tokio::process::Command::new("weasyprint")
        .args([input, output])
        .output()
        .await
        .map_err(|e| {
            AivyxError::Other(format!(
                "Neither pandoc nor weasyprint available for PDF creation: {e}. \
             Install pandoc (sudo apt install pandoc) or weasyprint (pip install weasyprint)."
            ))
        })?;

    if output_result.status.success() {
        Ok(())
    } else {
        Err(AivyxError::Other(format!(
            "weasyprint failed: {}",
            String::from_utf8_lossy(&output_result.stderr).trim()
        )))
    }
}

// ── Text Editing ───────────────────────────────────────────────

/// Edit a text file with structured operations.
///
/// Operations:
/// - `find_replace` — find and replace text (first occurrence or all)
/// - `insert_at` — insert text at a line number
/// - `append` — append text to the end
/// - `prepend` — prepend text to the beginning
/// - `delete_lines` — delete a range of lines
pub async fn edit_text_file(
    path: &str,
    operation: &str,
    input: &serde_json::Value,
) -> Result<String> {
    crate::files::validate_path(path)?;

    // Read existing file.
    let content = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| AivyxError::Io(e))?;

    let new_content = match operation {
        "find_replace" => {
            let find = input["find"]
                .as_str()
                .ok_or_else(|| AivyxError::Validation("find is required".into()))?;
            let replace = input["replace"]
                .as_str()
                .ok_or_else(|| AivyxError::Validation("replace is required".into()))?;
            let all = input["all"].as_bool().unwrap_or(false);

            if find.is_empty() {
                return Err(AivyxError::Validation("find must not be empty".into()));
            }

            if all {
                let count = content.matches(find).count();
                let result = content.replace(find, replace);
                tokio::fs::write(path, &result)
                    .await
                    .map_err(|e| AivyxError::Io(e))?;
                return Ok(format!("Replaced {count} occurrences in {path}"));
            } else {
                let result = content.replacen(find, replace, 1);
                if result == content {
                    return Ok(format!("No match found for '{find}' in {path}"));
                }
                result
            }
        }
        "insert_at" => {
            let line = input["line"]
                .as_u64()
                .ok_or_else(|| AivyxError::Validation("line number is required".into()))?
                as usize;
            let text = input["text"]
                .as_str()
                .ok_or_else(|| AivyxError::Validation("text is required".into()))?;

            let mut lines: Vec<&str> = content.lines().collect();
            let insert_idx = line.saturating_sub(1).min(lines.len());
            lines.insert(insert_idx, text);
            lines.join("\n") + "\n"
        }
        "append" => {
            let text = input["text"]
                .as_str()
                .ok_or_else(|| AivyxError::Validation("text is required".into()))?;

            let mut result = content;
            if !result.ends_with('\n') && !result.is_empty() {
                result.push('\n');
            }
            result.push_str(text);
            if !result.ends_with('\n') {
                result.push('\n');
            }
            result
        }
        "prepend" => {
            let text = input["text"]
                .as_str()
                .ok_or_else(|| AivyxError::Validation("text is required".into()))?;

            let mut result = String::with_capacity(text.len() + content.len() + 1);
            result.push_str(text);
            if !text.ends_with('\n') {
                result.push('\n');
            }
            result.push_str(&content);
            result
        }
        "delete_lines" => {
            let from = input["from"]
                .as_u64()
                .ok_or_else(|| AivyxError::Validation("from line number is required".into()))?
                as usize;
            let to = input["to"]
                .as_u64()
                .ok_or_else(|| AivyxError::Validation("to line number is required".into()))?
                as usize;

            if from == 0 || to == 0 {
                return Err(AivyxError::Validation("Line numbers are 1-based".into()));
            }
            if from > to {
                return Err(AivyxError::Validation("from must be <= to".into()));
            }

            let lines: Vec<&str> = content.lines().collect();
            let mut result: Vec<&str> = Vec::new();
            for (i, line) in lines.iter().enumerate() {
                let line_num = i + 1;
                if line_num < from || line_num > to {
                    result.push(line);
                }
            }
            let deleted = to.min(lines.len()).saturating_sub(from.saturating_sub(1));
            let new = result.join("\n") + "\n";
            tokio::fs::write(path, &new)
                .await
                .map_err(|e| AivyxError::Io(e))?;
            return Ok(format!("Deleted {deleted} lines ({from}-{to}) from {path}"));
        }
        other => {
            return Err(AivyxError::Validation(format!(
                "Unknown edit operation: '{other}'. Valid: find_replace, insert_at, \
                 append, prepend, delete_lines"
            )));
        }
    };

    validate_content_size(&new_content)?;
    tokio::fs::write(path, &new_content)
        .await
        .map_err(|e| AivyxError::Io(e))?;

    Ok(format!("Edited {path} ({operation})"))
}

// ── Format Conversion ──────────────────────────────────────────

/// Supported conversion paths.
const PANDOC_INPUT_FORMATS: &[&str] = &[
    "markdown",
    "md",
    "html",
    "latex",
    "tex",
    "rst",
    "docx",
    "odt",
    "epub",
    "textile",
    "org",
    "mediawiki",
];

const PANDOC_OUTPUT_FORMATS: &[&str] = &[
    "pdf", "html", "docx", "odt", "epub", "latex", "rst", "plain", "markdown", "md",
];

/// Convert a document between formats.
///
/// Uses pandoc for most conversions. Falls back to LibreOffice for
/// formats pandoc can't handle (e.g., xlsx, pptx, xls).
pub async fn convert_document(
    input_path: &str,
    output_path: &str,
    from_format: Option<&str>,
    to_format: Option<&str>,
) -> Result<String> {
    crate::files::validate_path(input_path)?;
    validate_output_path(output_path)?;
    ensure_parent_dir(output_path).await?;

    // Verify input exists.
    if !Path::new(input_path).exists() {
        return Err(AivyxError::Validation(format!(
            "Input file does not exist: {input_path}"
        )));
    }

    // Detect formats from extensions if not specified.
    let from = from_format
        .map(|s| s.to_string())
        .or_else(|| {
            Path::new(input_path)
                .extension()
                .and_then(|e| e.to_str())
                .map(|s| s.to_lowercase())
        })
        .ok_or_else(|| {
            AivyxError::Validation(
                "Cannot detect input format — specify from_format or use a file extension".into(),
            )
        })?;

    let to = to_format
        .map(|s| s.to_string())
        .or_else(|| {
            Path::new(output_path)
                .extension()
                .and_then(|e| e.to_str())
                .map(|s| s.to_lowercase())
        })
        .ok_or_else(|| {
            AivyxError::Validation(
                "Cannot detect output format — specify to_format or use a file extension".into(),
            )
        })?;

    // Try pandoc first.
    let from_ref = from.as_str();
    let to_ref = to.as_str();

    if is_pandoc_format(from_ref) && is_pandoc_format(to_ref) {
        return try_pandoc_convert(input_path, output_path, from_ref, to_ref).await;
    }

    // Fallback: LibreOffice for office formats.
    try_libreoffice_convert(input_path, output_path, &to).await
}

fn is_pandoc_format(fmt: &str) -> bool {
    PANDOC_INPUT_FORMATS.contains(&fmt) || PANDOC_OUTPUT_FORMATS.contains(&fmt)
}

async fn try_pandoc_convert(input: &str, output: &str, from: &str, to: &str) -> Result<String> {
    // Normalize format aliases.
    let from_norm = match from {
        "md" => "markdown",
        "tex" => "latex",
        f => f,
    };
    let to_norm = match to {
        "md" => "markdown",
        "tex" => "latex",
        t => t,
    };

    let result = tokio::process::Command::new("pandoc")
        .args(["-f", from_norm, "-t", to_norm, "-o", output, input])
        .output()
        .await
        .map_err(|e| AivyxError::Other(format!("pandoc: {e}")))?;

    if result.status.success() {
        let size = tokio::fs::metadata(output)
            .await
            .map(|m| m.len())
            .unwrap_or(0);
        Ok(format!(
            "Converted {input} ({from}) → {output} ({to}), {size} bytes"
        ))
    } else {
        let stderr = String::from_utf8_lossy(&result.stderr);
        // Try libreoffice as fallback.
        try_libreoffice_convert(input, output, to)
            .await
            .map_err(|_| {
                AivyxError::Other(format!(
                    "pandoc conversion failed: {stderr}. \
                     libreoffice fallback also unavailable."
                ))
            })
    }
}

async fn try_libreoffice_convert(input: &str, output: &str, to_format: &str) -> Result<String> {
    let output_dir = Path::new(output)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| ".".into());

    let result = tokio::process::Command::new("libreoffice")
        .args([
            "--headless",
            "--convert-to",
            to_format,
            "--outdir",
            &output_dir,
            input,
        ])
        .output()
        .await
        .map_err(|e| {
            AivyxError::Other(format!(
                "Neither pandoc nor libreoffice available for conversion: {e}"
            ))
        })?;

    if !result.status.success() {
        return Err(AivyxError::Other(format!(
            "libreoffice conversion failed: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        )));
    }

    // LibreOffice names output using the input stem + new extension.
    let lo_output = Path::new(&output_dir)
        .join(Path::new(input).file_stem().unwrap_or_default())
        .with_extension(to_format);

    if lo_output != Path::new(output) {
        tokio::fs::rename(&lo_output, output)
            .await
            .map_err(|e| AivyxError::Io(e))?;
    }

    let size = tokio::fs::metadata(output)
        .await
        .map(|m| m.len())
        .unwrap_or(0);
    Ok(format!(
        "Converted → {output} ({size} bytes, via libreoffice)"
    ))
}

// ── PDF Reading ────────────────────────────────────────────────

/// Extract text from a PDF file using `pdftotext` (poppler-utils).
///
/// Optionally reads only specific pages.
pub async fn read_pdf(
    path: &str,
    first_page: Option<u32>,
    last_page: Option<u32>,
) -> Result<String> {
    crate::files::validate_path(path)?;

    if !Path::new(path).exists() {
        return Err(AivyxError::Validation(format!(
            "PDF file does not exist: {path}"
        )));
    }

    // Check file size.
    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|e| AivyxError::Io(e))?;
    if metadata.len() as usize > MAX_PDF_READ_BYTES {
        return Err(AivyxError::Validation(format!(
            "PDF too large ({} bytes, max {MAX_PDF_READ_BYTES}). \
             Specify page range with first_page/last_page.",
            metadata.len()
        )));
    }

    let mut args = vec!["-layout".to_string()];

    if let Some(first) = first_page {
        args.push("-f".to_string());
        args.push(first.to_string());
    }
    if let Some(last) = last_page {
        args.push("-l".to_string());
        args.push(last.to_string());
    }

    args.push(path.to_string());
    args.push("-".to_string()); // Output to stdout.

    let output = tokio::process::Command::new("pdftotext")
        .args(&args)
        .output()
        .await
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AivyxError::Other(
                    "pdftotext is not installed. Install poppler-utils \
                     (sudo apt install poppler-utils)"
                        .into(),
                )
            } else {
                AivyxError::Other(format!("pdftotext: {e}"))
            }
        })?;

    if !output.status.success() {
        return Err(AivyxError::Other(format!(
            "pdftotext failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    let text = String::from_utf8_lossy(&output.stdout).to_string();

    // Cap output at 256KB to avoid overwhelming LLM context.
    if text.len() > 256 * 1024 {
        Ok(format!(
            "{}... [truncated at 256KB, use page range for more]",
            &text[..256 * 1024]
        ))
    } else {
        Ok(text)
    }
}

// ── Helpers ────────────────────────────────────────────────────

/// Validate an output path using the standard denylist.
fn validate_output_path(path: &str) -> Result<()> {
    crate::files::validate_path(path)?;

    // Must be absolute.
    if !Path::new(path).is_absolute() {
        return Err(AivyxError::Validation(
            "Output path must be absolute".into(),
        ));
    }

    Ok(())
}

/// Validate content size.
fn validate_content_size(content: &str) -> Result<()> {
    if content.len() > MAX_DOC_CONTENT_BYTES {
        return Err(AivyxError::Validation(format!(
            "Content too large ({} bytes, max {MAX_DOC_CONTENT_BYTES})",
            content.len()
        )));
    }
    Ok(())
}

/// Ensure the parent directory of a path exists.
async fn ensure_parent_dir(path: &str) -> Result<()> {
    if let Some(parent) = Path::new(path).parent() {
        if !parent.exists() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| AivyxError::Io(e))?;
        }
    }
    Ok(())
}

// ── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_encode_simple() {
        let row = vec!["a".into(), "b".into(), "c".into()];
        assert_eq!(csv_encode_row(&row), "a,b,c");
    }

    #[test]
    fn csv_encode_with_commas() {
        let row = vec!["hello, world".into(), "normal".into()];
        assert_eq!(csv_encode_row(&row), "\"hello, world\",normal");
    }

    #[test]
    fn csv_encode_with_quotes() {
        let row = vec!["say \"hi\"".into()];
        assert_eq!(csv_encode_row(&row), "\"say \"\"hi\"\"\"");
    }

    #[test]
    fn validate_content_size_ok() {
        let small = "x".repeat(100);
        assert!(validate_content_size(&small).is_ok());
    }

    #[test]
    fn validate_content_size_too_large() {
        let big = "x".repeat(MAX_DOC_CONTENT_BYTES + 1);
        assert!(validate_content_size(&big).is_err());
    }

    #[tokio::test]
    async fn edit_rejects_unknown_operation() {
        // Create a temp file to edit.
        let temp = std::env::temp_dir().join("aivyx_test_edit.txt");
        let path = temp.to_string_lossy().to_string();
        tokio::fs::write(&path, "hello").await.unwrap();

        let result = edit_text_file(&path, "explode", &serde_json::json!({})).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unknown edit operation"), "error: {err}");

        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn edit_find_replace_requires_find() {
        let temp = std::env::temp_dir().join("aivyx_test_edit2.txt");
        let path = temp.to_string_lossy().to_string();
        tokio::fs::write(&path, "hello world").await.unwrap();

        let result =
            edit_text_file(&path, "find_replace", &serde_json::json!({"replace": "x"})).await;
        assert!(result.is_err());

        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn edit_append_works() {
        let temp = std::env::temp_dir().join("aivyx_test_append.txt");
        let path = temp.to_string_lossy().to_string();
        tokio::fs::write(&path, "line1\n").await.unwrap();

        let result = edit_text_file(&path, "append", &serde_json::json!({"text": "line2"})).await;
        assert!(result.is_ok());

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(content.contains("line1"));
        assert!(content.contains("line2"));

        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn edit_delete_lines_validates() {
        let temp = std::env::temp_dir().join("aivyx_test_delete.txt");
        let path = temp.to_string_lossy().to_string();
        tokio::fs::write(&path, "a\nb\nc\n").await.unwrap();

        // from > to should fail.
        let result = edit_text_file(
            &path,
            "delete_lines",
            &serde_json::json!({"from": 3, "to": 1}),
        )
        .await;
        assert!(result.is_err());

        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn read_pdf_nonexistent() {
        let result = read_pdf("/tmp/nonexistent_aivyx_test.pdf", None, None).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("does not exist"), "error: {err}");
    }

    #[tokio::test]
    async fn create_text_rejects_relative() {
        let result = create_text_file("relative/path.txt", "hello").await;
        assert!(result.is_err());
    }

    #[test]
    fn pandoc_format_detection() {
        assert!(is_pandoc_format("markdown"));
        assert!(is_pandoc_format("md"));
        assert!(is_pandoc_format("html"));
        assert!(is_pandoc_format("pdf"));
        assert!(is_pandoc_format("docx"));
        assert!(!is_pandoc_format("xlsx"));
        assert!(!is_pandoc_format("pptx"));
    }
}
