//! Settings snapshot and config editing.
//!
//! Provides a flat view of all configured features (`SettingsSnapshot`) and
//! functions for editing `config.toml` programmatically — toggling booleans,
//! updating string lists, and configuring integrations.

use std::path::Path;

use aivyx_config::AivyxConfig;

use crate::config::PaConfig;

/// Write content to a file atomically by writing to a temporary sibling file
/// first, then renaming. This prevents corruption if the process crashes or
/// is killed mid-write.
fn atomic_write(path: &Path, content: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)
}

/// Atomic write with TOML validation. Parses the content before committing —
/// if the result is not valid TOML, returns an error and leaves the original
/// file untouched. This prevents config corruption from buggy edits.
fn validated_atomic_write(path: &Path, content: &str) -> std::io::Result<()> {
    if let Err(e) = content.parse::<toml::Value>() {
        tracing::error!("Config validation failed — aborting write: {e}");
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("TOML parse error: {e}"),
        ));
    }
    atomic_write(path, content)
}

// ── Integration Kind ──────────────────────────────────────────

/// Which integration to configure via the setup wizard or API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IntegrationKind {
    Email,
    Telegram,
    Matrix,
    Signal,
    Sms,
    Calendar,
    Contacts,
    Vault,
    Finance,
    Desktop,
    DevTools,
}

// ── Persona Dimensions ────────────────────────────────────────

/// Persona dimension values for settings display.
#[derive(Debug, serde::Serialize)]
pub struct PaPersonaDimensions {
    pub formality: f32,
    pub verbosity: f32,
    pub warmth: f32,
    pub humor: f32,
    pub confidence: f32,
    pub curiosity: f32,
    pub tone: Option<String>,
    pub uses_emoji: bool,
    pub uses_analogies: bool,
    pub asks_followups: bool,
    pub admits_uncertainty: bool,
}

// ── Settings Snapshot ─────────────────────────────────────────

/// A flat snapshot of all configured features.
///
/// Reconstructed from `config.toml` on demand (not cached). Gives the
/// frontend a complete picture of the agent's configuration without
/// needing to understand nested TOML sections.
#[derive(Debug, Default, serde::Serialize)]
pub struct SettingsSnapshot {
    // Agent
    pub agent_name: String,
    pub agent_persona: String,
    pub agent_max_tokens: u32,
    pub agent_skills: Vec<String>,
    pub has_custom_soul: bool,
    // Provider
    pub provider_label: String,
    pub model_name: String,
    pub provider_base_url: Option<String>,
    // Embedding
    pub embedding_model: Option<String>,
    pub embedding_dimensions: Option<usize>,
    // Memory
    pub max_memories: usize,
    pub profile_extraction_threshold: u64,
    pub session_max_age_hours: u64,
    pub use_graph_recall: bool,
    // Autonomy
    pub autonomy_tier: String,
    pub max_tool_calls_per_min: u32,
    pub max_cost_usd: f64,
    pub max_retries: u32,
    pub require_approval_destructive: bool,
    pub escalation_threshold: f32,
    pub scope_overrides: Vec<(String, String)>,
    // Persona dimensions
    pub persona_dimensions: Option<PaPersonaDimensions>,
    // Tool discovery
    pub tool_discovery_mode: Option<String>,
    pub tool_discovery_top_k: Option<usize>,
    pub tool_discovery_threshold: Option<f32>,
    pub tool_discovery_always_include: Vec<String>,
    // Heartbeat
    pub heartbeat_enabled: bool,
    pub heartbeat_interval: u32,
    pub heartbeat_can_reflect: bool,
    pub heartbeat_can_consolidate: bool,
    pub heartbeat_can_analyze_failures: bool,
    pub heartbeat_can_extract_knowledge: bool,
    pub heartbeat_can_plan_review: bool,
    pub heartbeat_can_strategy_review: bool,
    pub heartbeat_can_track_mood: bool,
    pub heartbeat_can_encourage: bool,
    pub heartbeat_can_track_milestones: bool,
    pub heartbeat_notification_pacing: bool,
    pub heartbeat_max_notifications_per_hour: u8,
    // Loop
    pub loop_check_interval: u32,
    pub morning_briefing: bool,
    pub briefing_hour: u8,
    // Schedules
    pub schedules: Vec<(String, String, bool)>, // (name, cron, enabled)
    // Integrations
    pub email_configured: bool,
    pub email_address: Option<String>,
    pub telegram_configured: bool,
    pub matrix_configured: bool,
    pub matrix_homeserver: Option<String>,
    pub signal_configured: bool,
    pub sms_configured: bool,
    pub calendar_configured: bool,
    pub contacts_configured: bool,
    // Vault & Finance
    pub vault_configured: bool,
    pub finance_configured: bool,
    // Desktop & Dev Tools
    pub desktop_configured: bool,
    pub desktop_app_access: Vec<(String, String, String)>, // (binary, display_name, access_level)
    pub devtools_configured: bool,
    // Voice
    pub voice_enabled: bool,
    pub stt_model_path: Option<String>,
    pub tts_model_path: Option<String>,
    // Webhooks
    pub webhook_configured: bool,
    // Triage
    pub triage_configured: bool,
    // MCP servers
    pub mcp_servers: Vec<String>,
    // Style
    pub style_tone: Option<String>,
    pub style_detail: Option<String>,
    pub style_prefs: Vec<String>,
    // Resilience
    pub resilience_circuit_breaker: bool,
    pub resilience_cache_enabled: bool,
    pub resilience_fallback_count: usize,
    // Consolidation
    pub consolidation_merge_threshold: f32,
    pub consolidation_stale_days: u64,
    pub consolidation_mine_patterns: bool,
    // Missions
    pub mission_default_mode: String,
    pub mission_recipe_dir: String,
    pub mission_experiment_tracking: bool,
    // Abuse Detection
    pub abuse_detection_enabled: bool,
    // Routing
    pub routing_enabled: bool,
}

// ── Config Writers ────────────────────────────────────────────

/// Ensure a TOML section exists in the content, appending it if it doesn't.
fn ensure_section(content: &mut String, section: &str) {
    if !content
        .lines()
        .any(|l| l.trim() == section || l.trim().starts_with(&format!("{section}]")))
    {
        if !content.ends_with('\n') && !content.is_empty() {
            content.push('\n');
        }
        content.push('\n');
        content.push_str(section);
        content.push('\n');
    }
}

/// Toggle a boolean value in config.toml.
///
/// Finds the key within the given `[section]` header (e.g. `[heartbeat]`)
/// and replacing `true` with `false` or vice versa.
///
/// This preserves all comments and formatting — only the value changes.
pub fn toggle_config_bool(
    config_path: &Path,
    section: &str,
    key: &str,
    new_val: bool,
) -> Result<(), std::io::Error> {
    let mut content = std::fs::read_to_string(config_path)?;
    ensure_section(&mut content, section);
    let mut lines: Vec<String> = content.lines().map(String::from).collect();
    let val_str = if new_val { "true" } else { "false" };

    let mut in_section = false;
    let mut found = false;
    let mut section_end = lines.len();
    for (i, line) in lines.iter_mut().enumerate() {
        let trimmed = line.trim();

        if trimmed.starts_with('[') {
            if in_section && !found {
                section_end = i;
            }
            in_section = trimmed == section || trimmed.starts_with(&format!("{section}]"));
        }

        if in_section && !found {
            let stripped = trimmed.split('#').next().unwrap_or("").trim();
            if let Some((k, _v)) = stripped.split_once('=')
                && k.trim().trim_matches('"') == key.trim_matches('"')
            {
                let leading: String = line.chars().take_while(|c| c.is_whitespace()).collect();
                let comment = if let Some(hash_pos) = line.find('#') {
                    &line[hash_pos..]
                } else {
                    ""
                };
                if comment.is_empty() {
                    *line = format!("{leading}{key} = {val_str}");
                } else {
                    *line = format!("{leading}{key} = {val_str}  {comment}");
                }
                found = true;
            }
        }
    }
    if !found {
        lines.insert(section_end, format!("{key} = {val_str}"));
    }

    let mut output = lines.join("\n");
    if !output.ends_with('\n') {
        output.push('\n');
    }
    validated_atomic_write(config_path, &output)?;
    Ok(())
}

/// Write a quoted string value in config.toml.
///
/// Finds `key = "..."` within the given `[section]` and replaces the value.
/// If the key doesn't exist in the section, appends it before the next section.
pub fn write_toml_string(
    config_path: &Path,
    section: &str,
    key: &str,
    value: &str,
) -> Result<(), std::io::Error> {
    let mut content = std::fs::read_to_string(config_path)?;
    ensure_section(&mut content, section);
    let mut lines: Vec<String> = content.lines().map(String::from).collect();
    let safe: String = value
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| match c {
            '"' => "\\\"".into(),
            '\\' => "\\\\".into(),
            o => o.to_string(),
        })
        .collect();

    let mut in_section = false;
    let mut found = false;
    let mut section_end = lines.len();
    for (i, line) in lines.iter_mut().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            if in_section && !found {
                section_end = i;
            }
            in_section = trimmed == section || trimmed.starts_with(&format!("{section}]"));
        }
        if in_section && !found {
            let stripped = trimmed.split('#').next().unwrap_or("").trim();
            if let Some((k, _)) = stripped.split_once('=')
                && k.trim().trim_matches('"') == key.trim_matches('"')
            {
                let leading: String = line.chars().take_while(|c| c.is_whitespace()).collect();
                let comment = line
                    .find('#')
                    .map(|p| &line[p..])
                    .filter(|_| stripped.contains('='))
                    .unwrap_or("");
                *line = if comment.is_empty() {
                    format!("{leading}{key} = \"{safe}\"")
                } else {
                    format!("{leading}{key} = \"{safe}\"  {comment}")
                };
                found = true;
            }
        }
    }
    if !found {
        lines.insert(section_end, format!("{key} = \"{safe}\""));
    }

    let mut output = lines.join("\n");
    if !output.ends_with('\n') {
        output.push('\n');
    }
    validated_atomic_write(config_path, &output)?;
    Ok(())
}

/// Like [`write_toml_string`], but creates the `[section]` header if it doesn't
/// exist yet.  Useful for optional sections like `[tool_discovery]` that may be
/// absent in the config file.
pub fn write_toml_string_create(
    config_path: &Path,
    section: &str,
    key: &str,
    value: &str,
) -> Result<(), std::io::Error> {
    write_toml_string(config_path, section, key, value)
}

/// Write an unquoted numeric value in config.toml.
///
/// Finds `key = ...` within the given `[section]` and replaces the value.
/// If the key doesn't exist in the section, appends it before the next section.
/// The caller formats the number as a string (e.g. `"30"`, `"0.5"`).
pub fn write_toml_number(
    config_path: &Path,
    section: &str,
    key: &str,
    value: &str,
) -> Result<(), std::io::Error> {
    let mut content = std::fs::read_to_string(config_path)?;
    ensure_section(&mut content, section);
    let mut lines: Vec<String> = content.lines().map(String::from).collect();

    let mut in_section = false;
    let mut found = false;
    let mut section_end = lines.len();
    for (i, line) in lines.iter_mut().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            if in_section && !found {
                section_end = i;
            }
            in_section = trimmed == section || trimmed.starts_with(&format!("{section}]"));
        }
        if in_section && !found {
            let stripped = trimmed.split('#').next().unwrap_or("").trim();
            if let Some((k, _)) = stripped.split_once('=')
                && k.trim().trim_matches('"') == key.trim_matches('"')
            {
                let leading: String = line.chars().take_while(|c| c.is_whitespace()).collect();
                let comment = line
                    .find('#')
                    .map(|p| &line[p..])
                    .filter(|_| stripped.contains('='))
                    .unwrap_or("");
                *line = if comment.is_empty() {
                    format!("{leading}{key} = {value}")
                } else {
                    format!("{leading}{key} = {value}  {comment}")
                };
                found = true;
            }
        }
    }
    if !found {
        lines.insert(section_end, format!("{key} = {value}"));
    }

    let mut output = lines.join("\n");
    if !output.ends_with('\n') {
        output.push('\n');
    }
    validated_atomic_write(config_path, &output)?;
    Ok(())
}

/// Write a TOML multi-line string by replacing the existing key in-place.
///
/// Handles both single-quoted (`key = "..."`) and triple-quoted (`key = """..."""`)
/// values. If the key doesn't exist in the section, appends it.
pub fn write_toml_multiline_string(
    config_path: &Path,
    section: &str,
    key: &str,
    value: &str,
) -> Result<(), std::io::Error> {
    let mut content = std::fs::read_to_string(config_path)?;
    ensure_section(&mut content, section);
    let mut lines: Vec<String> = content.lines().map(String::from).collect();

    let mut in_section = false;
    let mut found = false;
    let mut skip_until_close = false;
    let mut i = 0;

    while i < lines.len() {
        let trimmed = lines[i].trim();

        // Track triple-quoted blocks to skip
        if skip_until_close {
            if trimmed.contains("\"\"\"") {
                skip_until_close = false;
                lines.remove(i);
            } else {
                lines.remove(i);
            }
            continue;
        }

        if trimmed.starts_with('[') {
            in_section = trimmed == section || trimmed.starts_with(&format!("{section}]"));
        }

        if in_section {
            let stripped = trimmed.split('#').next().unwrap_or("").trim();
            if let Some((k, _)) = stripped.split_once('=')
                && k.trim().trim_matches('"') == key.trim_matches('"')
            {
                found = true;
                let leading: String = lines[i].chars().take_while(|c| c.is_whitespace()).collect();
                // Check if this is a triple-quoted string that spans lines
                if stripped.contains("\"\"\"") && stripped.matches("\"\"\"").count() < 2 {
                    // Opening triple-quote — remove this line and skip to close
                    lines.remove(i);
                    skip_until_close = true;
                    // Insert the replacement
                    let safe = value.replace("\"\"\"", "\"\"\\\"");
                    lines.insert(i, format!("{leading}{key} = \"\"\"\n{safe}\"\"\""));
                    i += 1;
                    continue;
                } else {
                    // Single-line value — replace in place
                    let safe = value.replace("\"\"\"", "\"\"\\\"");
                    lines[i] = format!("{leading}{key} = \"\"\"\n{safe}\"\"\"");
                }
                break;
            }
        }

        i += 1;
    }

    // If not found, append to the section
    if !found {
        let safe = value.replace("\"\"\"", "\"\"\\\"");
        let insert_line = format!("{key} = \"\"\"\n{safe}\"\"\"");

        // Find the section and append after it
        let mut section_end = lines.len();
        let mut found_section = false;
        for (j, line) in lines.iter().enumerate() {
            let t = line.trim();
            if t == section || t.starts_with(&format!("{section}]")) {
                found_section = true;
                continue;
            }
            if found_section && t.starts_with('[') {
                section_end = j;
                break;
            }
        }
        lines.insert(section_end, insert_line);
    }

    let mut output = lines.join("\n");
    if !output.ends_with('\n') {
        output.push('\n');
    }
    validated_atomic_write(config_path, &output)?;
    Ok(())
}

/// Write a TOML string array by replacing the existing key in-place.
///
/// Finds `key = [...]` in the given section and replaces it with the new values.
/// If the key doesn't exist, appends it to the section.
pub fn write_toml_string_array(config_path: &Path, section: &str, key: &str, values: &[String]) {
    let Ok(mut content) = std::fs::read_to_string(config_path) else {
        return;
    };
    ensure_section(&mut content, section);
    let mut lines: Vec<String> = content.lines().map(String::from).collect();

    let formatted = if values.is_empty() {
        format!("{key} = []")
    } else if values.len() <= 3 && values.iter().all(|v| v.len() < 30) {
        let items: Vec<String> = values.iter().map(|v| format!("\"{v}\"")).collect();
        format!("{key} = [{}]", items.join(", "))
    } else {
        let mut s = format!("{key} = [\n");
        for v in values {
            s.push_str(&format!("    \"{v}\",\n"));
        }
        s.push(']');
        s
    };

    let mut in_section = false;
    let mut found = false;
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim();
        if trimmed.starts_with('[') {
            in_section = trimmed == section;
        }
        if in_section {
            let stripped = trimmed.split('#').next().unwrap_or("").trim();
            if let Some((k, _)) = stripped.split_once('=')
                && k.trim().trim_matches('"') == key.trim_matches('"')
            {
                let leading: String = lines[i].chars().take_while(|c| c.is_whitespace()).collect();
                lines.remove(i);
                while i < lines.len() {
                    let t = lines[i].trim();
                    if t.starts_with('"') || t == "]" || t == "]," {
                        lines.remove(i);
                    } else {
                        break;
                    }
                }
                for (j, new_line) in formatted.lines().enumerate() {
                    lines.insert(i + j, format!("{leading}{new_line}"));
                }
                found = true;
                break;
            }
        }
        i += 1;
    }

    if !found {
        let mut section_end = lines.len();
        in_section = false;
        for (idx, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            if trimmed == section {
                in_section = true;
                continue;
            }
            if in_section && trimmed.starts_with('[') {
                section_end = idx;
                break;
            }
            if in_section {
                section_end = idx + 1;
            }
        }
        for (j, new_line) in formatted.lines().enumerate() {
            lines.insert(section_end + j, new_line.to_string());
        }
    }

    let mut output = lines.join("\n");
    if !output.ends_with('\n') {
        output.push('\n');
    }
    let _ = validated_atomic_write(config_path, &output);
}

/// Toggle the `enabled` field for a named schedule in `[[schedules]]` arrays.
///
/// Scans for `[[schedules]]` blocks, matches by `name = "schedule_name"`,
/// and sets or appends the `enabled` key.
pub fn toggle_schedule_enabled(
    config_path: &Path,
    schedule_name: &str,
    enabled: bool,
) -> Result<(), std::io::Error> {
    let content = std::fs::read_to_string(config_path)?;
    let mut lines: Vec<String> = content.lines().map(String::from).collect();
    let val_str = if enabled { "true" } else { "false" };

    let mut in_schedule_block = false;
    let mut found_name = false;

    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim();

        if trimmed == "[[schedules]]" {
            // Previous block had matching name but no `enabled` key — insert it
            if found_name {
                lines.insert(i, format!("enabled = {val_str}"));
                return write_lines(config_path, &lines);
            }
            in_schedule_block = true;
            found_name = false;
        } else if in_schedule_block && trimmed.starts_with('[') {
            if found_name {
                lines.insert(i, format!("enabled = {val_str}"));
                return write_lines(config_path, &lines);
            }
            in_schedule_block = false;
        }

        if in_schedule_block {
            let stripped = trimmed.split('#').next().unwrap_or("").trim();
            if let Some((k, v)) = stripped.split_once('=') {
                let k = k.trim();
                let v = v.trim().trim_matches('"');
                if k == "name" && v == schedule_name {
                    found_name = true;
                }
                if k == "enabled" && found_name {
                    let leading: String =
                        lines[i].chars().take_while(|c| c.is_whitespace()).collect();
                    lines[i] = format!("{leading}enabled = {val_str}");
                    return write_lines(config_path, &lines);
                }
            }
        }
        i += 1;
    }

    // Last block had matching name but no `enabled` key
    if found_name {
        lines.push(format!("enabled = {val_str}"));
        return write_lines(config_path, &lines);
    }

    Ok(())
}

fn write_lines(config_path: &Path, lines: &[String]) -> Result<(), std::io::Error> {
    let mut output = lines.join("\n");
    if !output.ends_with('\n') {
        output.push('\n');
    }
    validated_atomic_write(config_path, &output)
}

pub fn write_app_access(
    config_path: &Path,
    binary: &str,
    access: &str,
) -> Result<(), std::io::Error> {
    let serde_access = if access == "View Only" {
        "ViewOnly"
    } else {
        access
    };
    // We MUST quote the binary name if it contains dots or special characters
    // to prevent TOML from interpreting it as nested child tables (`org: {gnome: {Terminal...}}`)
    let quoted_binary = format!("\"{}\"", binary.replace('"', "\\\""));
    write_toml_string_create(
        config_path,
        "[desktop.app_access]",
        &quoted_binary,
        serde_access,
    )
}

/// Resolve the TOML section name for an integration kind.
pub fn integration_section_name(kind: IntegrationKind) -> &'static str {
    match kind {
        IntegrationKind::Email => "email",
        IntegrationKind::Telegram => "telegram",
        IntegrationKind::Matrix => "matrix",
        IntegrationKind::Signal => "signal",
        IntegrationKind::Sms => "sms",
        IntegrationKind::Calendar => "calendar",
        IntegrationKind::Contacts => "contacts",
        IntegrationKind::Vault => "vault",
        IntegrationKind::Finance => "finance",
        IntegrationKind::Desktop => "desktop",
        IntegrationKind::DevTools => "devtools",
    }
}

/// Write an integration config section to config.toml.
///
/// Appends a new `[section]` block with the collected key-value pairs.
/// If the section already exists, it is removed first (overwrite semantics).
pub fn write_integration_config(
    config_path: &Path,
    kind: IntegrationKind,
    fields: &[(String, String)],
) -> Result<(), std::io::Error> {
    let section_name = integration_section_name(kind);

    // Remove existing section if present (enables reconfiguration).
    let _ = remove_integration_config(config_path, kind);

    let mut content = std::fs::read_to_string(config_path)?;

    if !content.ends_with('\n') {
        content.push('\n');
    }
    content.push('\n');
    content.push_str(&format!("[{section_name}]\n"));
    for (key, value) in fields {
        if value.is_empty() {
            continue;
        }
        // Write booleans and numbers without quotes.
        // Comma-separated values are written as TOML arrays.
        if value == "true" || value == "false" || value.parse::<u16>().is_ok() {
            content.push_str(&format!("{key} = {value}\n"));
        } else if value.contains(',') {
            let items: Vec<&str> = value
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect();
            let array = items
                .iter()
                .map(|s| format!("\"{s}\""))
                .collect::<Vec<_>>()
                .join(", ");
            content.push_str(&format!("{key} = [{array}]\n"));
        } else {
            let safe: String = value
                .chars()
                .filter(|c| !c.is_control())
                .map(|c| match c {
                    '"' => "\\\"".to_string(),
                    '\\' => "\\\\".to_string(),
                    other => other.to_string(),
                })
                .collect();
            content.push_str(&format!("{key} = \"{safe}\"\n"));
        }
    }

    validated_atomic_write(config_path, &content)?;
    Ok(())
}

/// Remove an integration's `[section]` block from config.toml.
///
/// Finds the `[section_name]` header and removes all lines up to the next
/// section header or end of file. Also removes one preceding blank line.
pub fn remove_integration_config(
    config_path: &Path,
    kind: IntegrationKind,
) -> Result<(), std::io::Error> {
    let section_name = integration_section_name(kind);
    let content = std::fs::read_to_string(config_path)?;
    let section_header = format!("[{section_name}]");

    // Find the line index of the section header.
    let lines: Vec<&str> = content.lines().collect();
    let Some(start) = lines.iter().position(|l| l.trim() == section_header) else {
        return Ok(()); // Section doesn't exist — nothing to remove.
    };

    // Find end: next `[` header or EOF.
    // Skip any headers that are sub-tables of this section (e.g. `[desktop.interaction]`).
    let end = lines[start + 1..]
        .iter()
        .position(|l| {
            let t = l.trim();
            if !t.starts_with('[') {
                return false;
            }
            // If it's a sub-table of the section we're removing (e.g., `[desktop.interaction]`), keep going
            let is_subtable_or_array = t.starts_with(&format!("[{section_name}."))
                || t.starts_with(&format!("[[{section_name}."));
            !is_subtable_or_array
        })
        .map(|offset| start + 1 + offset)
        .unwrap_or(lines.len());

    // Also remove one preceding blank line if present.
    let actual_start = if start > 0 && lines[start - 1].trim().is_empty() {
        start - 1
    } else {
        start
    };

    let mut result: Vec<&str> = Vec::new();
    result.extend_from_slice(&lines[..actual_start]);
    result.extend_from_slice(&lines[end..]);

    // Trim trailing blank lines.
    while result.last().is_some_and(|l| l.trim().is_empty()) {
        result.pop();
    }

    let mut output = result.join("\n");
    if !output.is_empty() {
        output.push('\n');
    }

    validated_atomic_write(config_path, &output)?;
    Ok(())
}

// ── Schedule CRUD Helpers ─────────────────────────────────────

/// Check if a schedule with the given name exists in config.toml.
pub fn schedule_exists(config_path: &Path, name: &str) -> bool {
    let Ok(content) = std::fs::read_to_string(config_path) else {
        return false;
    };
    let mut in_schedule = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == "[[schedules]]" {
            in_schedule = true;
            continue;
        }
        if trimmed.starts_with('[') && in_schedule {
            in_schedule = false;
        }
        if in_schedule {
            let stripped = trimmed.split('#').next().unwrap_or("").trim();
            if let Some((k, v)) = stripped.split_once('=')
                && k.trim() == "name"
                && v.trim().trim_matches('"') == name
            {
                return true;
            }
        }
    }
    false
}

/// Append a new `[[schedules]]` block to config.toml.
pub fn append_schedule(
    config_path: &Path,
    name: &str,
    cron: &str,
    prompt: &str,
    notify: bool,
) -> Result<(), std::io::Error> {
    let mut content = std::fs::read_to_string(config_path)?;
    if !content.ends_with('\n') {
        content.push('\n');
    }

    let safe_name = sanitize_toml_value(name);
    let safe_cron = sanitize_toml_value(cron);
    let safe_prompt = sanitize_toml_value(prompt);

    content.push_str(&format!(
        "\n[[schedules]]\n\
         name = \"{safe_name}\"\n\
         cron = \"{safe_cron}\"\n\
         agent = \"assistant\"\n\
         prompt = \"{safe_prompt}\"\n\
         notify = {notify}\n\
         enabled = true\n"
    ));

    validated_atomic_write(config_path, &content)
}

/// Edit a single field within a named `[[schedules]]` block.
///
/// The `value` should be a TOML-ready literal (e.g., `"\"new cron\""` for
/// strings, `"true"` for booleans). If the field doesn't exist in the block,
/// it is appended before the next section header.
pub fn edit_schedule_field(
    config_path: &Path,
    schedule_name: &str,
    field: &str,
    value: &str,
) -> Result<(), std::io::Error> {
    let content = std::fs::read_to_string(config_path)?;
    let mut lines: Vec<String> = content.lines().map(String::from).collect();

    let mut in_schedule = false;
    let mut found_name = false;
    let mut i = 0;

    while i < lines.len() {
        let trimmed = lines[i].trim();

        if trimmed == "[[schedules]]" {
            // If we were tracking a previous block with the right name but
            // didn't find the field, insert before this new block header.
            if found_name {
                lines.insert(i, format!("{field} = {value}"));
                return write_lines(config_path, &lines);
            }
            in_schedule = true;
            found_name = false;
        } else if in_schedule && trimmed.starts_with('[') {
            if found_name {
                lines.insert(i, format!("{field} = {value}"));
                return write_lines(config_path, &lines);
            }
            in_schedule = false;
        }

        if in_schedule {
            let stripped = trimmed.split('#').next().unwrap_or("").trim();
            if let Some((k, v)) = stripped.split_once('=') {
                let k = k.trim();
                let v = v.trim().trim_matches('"');
                if k == "name" && v == schedule_name {
                    found_name = true;
                }
                if k == field && found_name {
                    let leading: String =
                        lines[i].chars().take_while(|c| c.is_whitespace()).collect();
                    lines[i] = format!("{leading}{field} = {value}");
                    return write_lines(config_path, &lines);
                }
            }
        }
        i += 1;
    }

    // Last block matched but field wasn't found — append at end
    if found_name {
        lines.push(format!("{field} = {value}"));
        return write_lines(config_path, &lines);
    }

    Ok(())
}

/// Remove an entire `[[schedules]]` block by name.
pub fn remove_schedule(config_path: &Path, name: &str) -> Result<(), std::io::Error> {
    let content = std::fs::read_to_string(config_path)?;
    let lines: Vec<&str> = content.lines().collect();

    // First pass: find the block boundaries for the named schedule.
    let mut block_start = None;
    let mut block_end = None;
    let mut in_schedule = false;
    let mut found_name = false;

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed == "[[schedules]]" {
            if found_name {
                // We already found our block — its end is here
                block_end = Some(i);
                break;
            }
            in_schedule = true;
            found_name = false;
            block_start = Some(i);
        } else if in_schedule && trimmed.starts_with('[') {
            if found_name {
                block_end = Some(i);
                break;
            }
            in_schedule = false;
            block_start = None;
        }

        if in_schedule {
            let stripped = trimmed.split('#').next().unwrap_or("").trim();
            if let Some((k, v)) = stripped.split_once('=')
                && k.trim() == "name"
                && v.trim().trim_matches('"') == name
            {
                found_name = true;
            }
        }
    }

    if !found_name {
        return Ok(());
    }

    let start = match block_start {
        Some(s) => s,
        None => return Ok(()),
    };
    let end = block_end.unwrap_or(lines.len());

    // Also remove a blank line before the block header (cosmetic)
    let start = if start > 0 && lines[start - 1].trim().is_empty() {
        start - 1
    } else {
        start
    };

    let mut output: Vec<String> = Vec::with_capacity(lines.len());
    for (i, line) in lines.iter().enumerate() {
        if i >= start && i < end {
            continue;
        }
        output.push((*line).to_string());
    }

    write_lines(config_path, &output)
}

/// Sanitize a string for use in a TOML double-quoted value.
fn sanitize_toml_value(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .map(|c| match c {
            '"' => "\\\"".to_string(),
            '\\' => "\\\\".to_string(),
            other => other.to_string(),
        })
        .collect()
}

// ── Settings Reload ───────────────────────────────────────────

/// Reload the settings snapshot from disk.
///
/// Parses `config.toml` as both `AivyxConfig` (core) and `PaConfig` (PA)
/// and assembles the flat `SettingsSnapshot`.
pub fn reload_settings_snapshot(config_path: &Path) -> Result<SettingsSnapshot, String> {
    let toml_str = std::fs::read_to_string(config_path)
        .map_err(|e| format!("Cannot read {}: {e}", config_path.display()))?;
    let core_config: AivyxConfig =
        toml::from_str(&toml_str).map_err(|e| format!("Core config parse error: {e}"))?;
    let pa_config: PaConfig = toml::from_str::<PaConfig>(&toml_str)
        .map_err(|e| format!("PA config parse error: {e}"))?
        .with_auto_desktop();

    let agent_cfg_ref = pa_config.agent.as_ref();
    let hb_ref = pa_config.heartbeat.as_ref();
    let loop_ref = pa_config.loop_config.as_ref();
    let persona_ref = pa_config.persona.as_ref();
    let td_ref = pa_config.tool_discovery.as_ref();
    let style_ref = pa_config.style.as_ref();

    Ok(SettingsSnapshot {
        agent_name: agent_cfg_ref
            .map(|a| a.name.clone())
            .unwrap_or_else(|| "assistant".into()),
        agent_persona: agent_cfg_ref
            .map(|a| a.persona.clone())
            .unwrap_or_else(|| "assistant".into()),
        agent_max_tokens: agent_cfg_ref.map(|a| a.max_tokens).unwrap_or(4096),
        agent_skills: agent_cfg_ref.map(|a| a.skills.clone()).unwrap_or_default(),
        has_custom_soul: agent_cfg_ref.and_then(|a| a.soul.as_ref()).is_some(),
        provider_label: match &core_config.provider {
            aivyx_config::ProviderConfig::Claude { .. } => "Claude".into(),
            aivyx_config::ProviderConfig::OpenAI { .. } => "OpenAI".into(),
            aivyx_config::ProviderConfig::Gemini { .. } => "Gemini".into(),
            aivyx_config::ProviderConfig::Ollama { .. } => "Ollama".into(),
            aivyx_config::ProviderConfig::OpenAICompatible { .. } => "OpenAI-Compatible".into(),
        },
        model_name: core_config.provider.model_name().to_string(),
        provider_base_url: match &core_config.provider {
            aivyx_config::ProviderConfig::Ollama { base_url, .. } => Some(base_url.clone()),
            aivyx_config::ProviderConfig::OpenAICompatible { base_url, .. } => {
                Some(base_url.clone())
            }
            _ => None,
        },
        embedding_model: core_config.embedding.as_ref().map(|e| match e {
            aivyx_config::EmbeddingConfig::Ollama { model, .. } => model.clone(),
            aivyx_config::EmbeddingConfig::OpenAI { model, .. } => model.clone(),
        }),
        embedding_dimensions: core_config.embedding.as_ref().map(|e| match e {
            aivyx_config::EmbeddingConfig::Ollama { dimensions, .. } => *dimensions,
            aivyx_config::EmbeddingConfig::OpenAI { dimensions, .. } => *dimensions,
        }),
        max_memories: core_config.memory.max_memories,
        profile_extraction_threshold: core_config.memory.profile_extraction_threshold,
        session_max_age_hours: core_config.memory.session_max_age_hours,
        use_graph_recall: core_config.memory.use_graph_recall,
        autonomy_tier: format!("{:?}", core_config.autonomy.default_tier),
        max_tool_calls_per_min: core_config.autonomy.max_tool_calls_per_minute,
        max_cost_usd: core_config.autonomy.max_cost_per_session_usd,
        max_retries: core_config.autonomy.max_retries,
        require_approval_destructive: core_config.autonomy.require_approval_for_destructive,
        escalation_threshold: core_config.autonomy.escalation_confidence_threshold,
        scope_overrides: core_config
            .autonomy
            .scope_overrides
            .iter()
            .map(|o| (o.scope.clone(), format!("{:?}", o.tier)))
            .collect(),
        persona_dimensions: persona_ref.map(|p| PaPersonaDimensions {
            formality: p.formality,
            verbosity: p.verbosity,
            warmth: p.warmth,
            humor: p.humor,
            confidence: p.confidence,
            curiosity: p.curiosity,
            tone: p.tone.clone(),
            uses_emoji: p.uses_emoji,
            uses_analogies: p.uses_analogies,
            asks_followups: p.asks_followups,
            admits_uncertainty: p.admits_uncertainty,
        }),
        tool_discovery_mode: td_ref.map(|t| t.mode.clone()),
        tool_discovery_top_k: td_ref.map(|t| t.top_k),
        tool_discovery_threshold: td_ref.map(|t| t.threshold),
        tool_discovery_always_include: td_ref.map(|t| t.always_include.clone()).unwrap_or_default(),
        heartbeat_enabled: hb_ref.map(|h| h.enabled).unwrap_or(true),
        heartbeat_interval: hb_ref.map(|h| h.interval_minutes).unwrap_or(30),
        heartbeat_can_reflect: hb_ref.map(|h| h.can_reflect).unwrap_or(false),
        heartbeat_can_consolidate: hb_ref.map(|h| h.can_consolidate_memory).unwrap_or(false),
        heartbeat_can_analyze_failures: hb_ref.map(|h| h.can_analyze_failures).unwrap_or(false),
        heartbeat_can_extract_knowledge: hb_ref.map(|h| h.can_extract_knowledge).unwrap_or(false),
        heartbeat_can_plan_review: hb_ref.map(|h| h.can_plan_review).unwrap_or(false),
        heartbeat_can_strategy_review: hb_ref.map(|h| h.can_strategy_review).unwrap_or(false),
        heartbeat_can_track_mood: hb_ref.map(|h| h.can_track_mood).unwrap_or(false),
        heartbeat_can_encourage: hb_ref.map(|h| h.can_encourage).unwrap_or(false),
        heartbeat_can_track_milestones: hb_ref.map(|h| h.can_track_milestones).unwrap_or(false),
        heartbeat_notification_pacing: hb_ref.map(|h| h.notification_pacing).unwrap_or(false),
        heartbeat_max_notifications_per_hour: hb_ref
            .map(|h| h.max_notifications_per_hour)
            .unwrap_or(5),
        loop_check_interval: loop_ref.map(|l| l.check_interval_minutes).unwrap_or(15),
        morning_briefing: loop_ref.map(|l| l.morning_briefing).unwrap_or(true),
        briefing_hour: loop_ref.map(|l| l.briefing_hour).unwrap_or(8),
        schedules: pa_config
            .schedules
            .iter()
            .map(|s| (s.name.clone(), s.cron.clone(), s.enabled))
            .collect(),
        email_configured: pa_config.email.is_some(),
        email_address: pa_config.email.as_ref().map(|e| e.address.clone()),
        telegram_configured: pa_config.telegram.is_some(),
        matrix_configured: pa_config.matrix.is_some(),
        matrix_homeserver: pa_config.matrix.as_ref().map(|m| m.homeserver.clone()),
        signal_configured: pa_config.signal.is_some(),
        sms_configured: pa_config.sms.is_some(),
        calendar_configured: pa_config.calendar.is_some(),
        contacts_configured: pa_config.contacts.is_some(),
        vault_configured: pa_config.vault.is_some(),
        finance_configured: pa_config.finance.is_some(),
        desktop_configured: pa_config.desktop.is_some(),
        desktop_app_access: {
            let dc = pa_config.desktop.as_ref();
            let mut entries = Vec::new();
            if dc.is_some() {
                // Scan installed apps and merge with configured access levels
                let scanned = aivyx_actions::desktop::scanner::scan_applications();
                let default_config = aivyx_actions::desktop::DesktopConfig::default();
                let config = dc.unwrap_or(&default_config);
                for (bin, app) in &scanned {
                    let access = aivyx_actions::desktop::resolve_app_access(bin, config);
                    entries.push((bin.clone(), app.name.clone(), access.to_string()));
                }
            }
            entries
        },
        devtools_configured: pa_config.devtools.is_some(),
        voice_enabled: pa_config.voice.as_ref().map(|v| v.enabled).unwrap_or(true),
        stt_model_path: pa_config
            .voice
            .as_ref()
            .and_then(|v| v.stt_model_path.clone()),
        tts_model_path: pa_config
            .voice
            .as_ref()
            .and_then(|v| v.tts_model_path.clone()),
        webhook_configured: pa_config.webhook.is_some(),
        triage_configured: pa_config.triage.is_some(),
        mcp_servers: pa_config
            .mcp_servers
            .iter()
            .map(|s| s.name.clone())
            .collect(),
        style_tone: style_ref.and_then(|s| s.tone.as_ref().cloned()),
        style_detail: style_ref.and_then(|s| s.detail_level.as_ref().cloned()),
        style_prefs: style_ref.map(|s| s.preferences.clone()).unwrap_or_default(),
        resilience_circuit_breaker: pa_config
            .resilience
            .as_ref()
            .map(|r| r.circuit_breaker)
            .unwrap_or(false),
        resilience_cache_enabled: pa_config
            .resilience
            .as_ref()
            .map(|r| r.cache_enabled)
            .unwrap_or(false),
        resilience_fallback_count: pa_config
            .resilience
            .as_ref()
            .map(|r| r.fallback_providers.len())
            .unwrap_or(0),
        consolidation_merge_threshold: pa_config
            .consolidation
            .as_ref()
            .map(|c| c.merge_threshold)
            .unwrap_or(0.85),
        consolidation_stale_days: pa_config
            .consolidation
            .as_ref()
            .map(|c| c.stale_days)
            .unwrap_or(90),
        consolidation_mine_patterns: pa_config
            .consolidation
            .as_ref()
            .map(|c| c.mine_patterns)
            .unwrap_or(true),
        mission_default_mode: pa_config
            .missions
            .as_ref()
            .map(|m| m.default_mode.clone())
            .unwrap_or_else(|| "sequential".into()),
        mission_recipe_dir: pa_config
            .missions
            .as_ref()
            .and_then(|m| m.recipe_dir.clone())
            .unwrap_or_else(|| "~/.aivyx/recipes".into()),
        mission_experiment_tracking: pa_config
            .missions
            .as_ref()
            .map(|m| m.experiment_tracking)
            .unwrap_or(false),
        abuse_detection_enabled: pa_config
            .abuse_detection
            .as_ref()
            .map(|a| a.enabled)
            .unwrap_or(false),
        routing_enabled: pa_config
            .routing
            .as_ref()
            .map(|r| r.enabled)
            .unwrap_or(false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn temp_config(content: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f
    }

    #[test]
    fn replace_single_line_string() {
        let f =
            temp_config("[agent]\nname = \"test\"\nsoul = \"old soul\"\npersona = \"assistant\"\n");
        write_toml_multiline_string(f.path(), "[agent]", "soul", "new narrative\nline two")
            .unwrap();
        let content = std::fs::read_to_string(f.path()).unwrap();
        assert!(content.contains("soul = \"\"\"\nnew narrative\nline two\"\"\""));
        assert!(!content.contains("old soul"));
    }

    #[test]
    fn replace_triple_quoted_string() {
        let f = temp_config(
            "[agent]\nname = \"test\"\nsoul = \"\"\"\nold soul\nline two\"\"\"\npersona = \"assistant\"\n",
        );
        write_toml_multiline_string(f.path(), "[agent]", "soul", "replaced").unwrap();
        let content = std::fs::read_to_string(f.path()).unwrap();
        assert!(content.contains("soul = \"\"\"\nreplaced\"\"\""));
        assert!(!content.contains("old soul"));
    }

    #[test]
    fn append_missing_key() {
        let f = temp_config(
            "[agent]\nname = \"test\"\npersona = \"assistant\"\n\n[loop]\ncheck_interval_minutes = 15\n",
        );
        write_toml_multiline_string(f.path(), "[agent]", "soul", "appended soul").unwrap();
        let content = std::fs::read_to_string(f.path()).unwrap();
        assert!(content.contains("soul = \"\"\"\nappended soul\"\"\""));
        // Should be before the [loop] section
        let soul_pos = content.find("soul").unwrap();
        let loop_pos = content.find("[loop]").unwrap();
        assert!(soul_pos < loop_pos);
    }

    // ── Schedule CRUD tests ──────────────────────────────────────

    const BASE_CONFIG: &str = "\
[agent]
name = \"assistant\"

[[schedules]]
name = \"morning-digest\"
cron = \"0 7 * * *\"
agent = \"assistant\"
prompt = \"Generate a morning digest\"
notify = true
enabled = true
";

    #[test]
    fn schedule_exists_finds_present() {
        let f = temp_config(BASE_CONFIG);
        assert!(schedule_exists(f.path(), "morning-digest"));
    }

    #[test]
    fn schedule_exists_rejects_absent() {
        let f = temp_config(BASE_CONFIG);
        assert!(!schedule_exists(f.path(), "nonexistent"));
    }

    #[test]
    fn append_schedule_adds_block() {
        let f = temp_config(BASE_CONFIG);
        append_schedule(
            f.path(),
            "evening-review",
            "0 18 * * *",
            "Review the day",
            false,
        )
        .unwrap();
        let content = std::fs::read_to_string(f.path()).unwrap();
        assert!(content.contains("name = \"evening-review\""));
        assert!(content.contains("cron = \"0 18 * * *\""));
        assert!(content.contains("prompt = \"Review the day\""));
        assert!(content.contains("notify = false"));
        assert!(content.contains("enabled = true"));
        // Original schedule still present
        assert!(content.contains("name = \"morning-digest\""));
    }

    #[test]
    fn edit_schedule_field_updates_cron() {
        let f = temp_config(BASE_CONFIG);
        edit_schedule_field(f.path(), "morning-digest", "cron", "\"0 8 * * *\"").unwrap();
        let content = std::fs::read_to_string(f.path()).unwrap();
        assert!(content.contains("cron = \"0 8 * * *\""));
        assert!(!content.contains("cron = \"0 7 * * *\""));
    }

    #[test]
    fn edit_schedule_field_appends_missing() {
        // Config block without a 'nexus' key — should be appended
        let f = temp_config(BASE_CONFIG);
        edit_schedule_field(f.path(), "morning-digest", "nexus", "true").unwrap();
        let content = std::fs::read_to_string(f.path()).unwrap();
        assert!(content.contains("nexus = true"));
    }

    #[test]
    fn remove_schedule_deletes_block() {
        let f = temp_config(BASE_CONFIG);
        remove_schedule(f.path(), "morning-digest").unwrap();
        let content = std::fs::read_to_string(f.path()).unwrap();
        assert!(!content.contains("morning-digest"));
        assert!(!content.contains("[[schedules]]"));
        // Agent section still present
        assert!(content.contains("[agent]"));
    }

    #[test]
    fn remove_schedule_preserves_others() {
        let two_schedules = "\
[agent]
name = \"assistant\"

[[schedules]]
name = \"first\"
cron = \"0 7 * * *\"
agent = \"assistant\"
prompt = \"First task\"

[[schedules]]
name = \"second\"
cron = \"0 18 * * *\"
agent = \"assistant\"
prompt = \"Second task\"
";
        let f = temp_config(two_schedules);
        remove_schedule(f.path(), "first").unwrap();
        let content = std::fs::read_to_string(f.path()).unwrap();
        assert!(!content.contains("\"first\""));
        assert!(content.contains("\"second\""));
        assert!(content.contains("[[schedules]]"));
    }

    #[test]
    fn remove_nonexistent_schedule_is_noop() {
        let f = temp_config(BASE_CONFIG);
        let before = std::fs::read_to_string(f.path()).unwrap();
        remove_schedule(f.path(), "nonexistent").unwrap();
        let after = std::fs::read_to_string(f.path()).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn edit_schedule_field_wrong_name_is_noop() {
        let f = temp_config(BASE_CONFIG);
        let before = std::fs::read_to_string(f.path()).unwrap();
        edit_schedule_field(f.path(), "nonexistent", "cron", "\"* * * * *\"").unwrap();
        let after = std::fs::read_to_string(f.path()).unwrap();
        assert_eq!(before, after);
    }

    // ── Config validation tests ──────────────────────────────────

    #[test]
    fn validated_write_accepts_good_toml() {
        let f = temp_config("[agent]\nname = \"test\"\n");
        let new_content = "[agent]\nname = \"updated\"\n";
        validated_atomic_write(f.path(), new_content).unwrap();
        let content = std::fs::read_to_string(f.path()).unwrap();
        assert!(content.contains("updated"));
    }

    #[test]
    fn validated_write_rejects_bad_toml() {
        let f = temp_config("[agent]\nname = \"test\"\n");
        let bad_content = "[agent\nname = broken toml";
        let result = validated_atomic_write(f.path(), bad_content);
        assert!(result.is_err());
        // Original file must be preserved
        let content = std::fs::read_to_string(f.path()).unwrap();
        assert!(content.contains("name = \"test\""));
    }

    #[test]
    fn validated_write_rollback_preserves_original() {
        let original = "[heartbeat]\nenabled = true\ninterval_minutes = 30\n";
        let f = temp_config(original);

        // Simulate a write that would corrupt the config
        let corrupt = "[heartbeat]\nenabled = true\n[[[invalid_array_header\n";
        assert!(validated_atomic_write(f.path(), corrupt).is_err());

        // Original must be intact
        let content = std::fs::read_to_string(f.path()).unwrap();
        assert_eq!(content, original);
    }

    #[test]
    fn write_app_access_creates_section_and_key() {
        let f = temp_config("[desktop]\nclipboard = true\n");
        write_app_access(f.path(), "firefox", "Full").unwrap();
        let content = std::fs::read_to_string(f.path()).unwrap();
        assert!(content.contains("[desktop.app_access]"));
        assert!(content.contains("\"firefox\" = \"Full\""));
    }

    #[test]
    fn write_app_access_updates_existing_key() {
        let f = temp_config(
            "[desktop]\nclipboard = true\n\n[desktop.app_access]\n\"firefox\" = \"Full\"\n",
        );
        write_app_access(f.path(), "firefox", "Blocked").unwrap();
        let content = std::fs::read_to_string(f.path()).unwrap();
        assert!(content.contains("\"firefox\" = \"Blocked\""));
        assert!(!content.contains("\"firefox\" = \"Full\""));
    }

    /// write_integration_config must write booleans unquoted and comma-separated
    /// values as TOML arrays, not quoted strings.
    #[test]
    fn write_integration_config_types() {
        let f = temp_config("[agent]\nname = \"test\"\npersona = \"assistant\"\n");
        // Desktop: booleans
        write_integration_config(
            f.path(),
            IntegrationKind::Desktop,
            &[
                ("clipboard".into(), "true".into()),
                ("windows".into(), "false".into()),
                ("notifications".into(), "true".into()),
            ],
        )
        .unwrap();
        let content = std::fs::read_to_string(f.path()).unwrap();
        assert!(
            content.contains("clipboard = true\n"),
            "bool should be unquoted: {content}"
        );
        assert!(
            content.contains("windows = false\n"),
            "bool should be unquoted: {content}"
        );
        // Vault: comma-separated → array
        let _ = remove_integration_config(f.path(), IntegrationKind::Desktop);
        write_integration_config(
            f.path(),
            IntegrationKind::Vault,
            &[
                ("path".into(), "~/docs".into()),
                ("extensions".into(), "md,txt,pdf".into()),
            ],
        )
        .unwrap();
        let content = std::fs::read_to_string(f.path()).unwrap();
        assert!(
            content.contains(r#"extensions = ["md", "txt", "pdf"]"#),
            "comma list should be array: {content}"
        );
        // Verify the resulting config parses
        let pa: crate::config::PaConfig = toml::from_str(&content).unwrap();
        let vault = pa.vault.expect("vault section missing");
        assert_eq!(vault.extensions, vec!["md", "txt", "pdf"]);
    }

    /// Regression test: a realistic full config (Jarvis coder persona, Ollama) must
    /// parse through `reload_settings_snapshot` without returning `None`.
    #[test]
    fn reload_snapshot_full_coder_config() {
        let config = r#"
# ═══════════════════════════════════════════
#  Aivyx Personal Assistant — Configuration
# ═══════════════════════════════════════════
# Generated by `aivyx genesis`

[provider]
type = "Ollama"
base_url = "http://localhost:11434"
model = "qwen3.5:9b"

[autonomy]
default_tier = "Trust"

[memory]
max_memories = 10000

[agent]
name = "Jarvis"
persona = "coder"
max_tokens = 4096
skills = []

[heartbeat]
enabled = true
interval_minutes = 30
can_reflect = true
can_consolidate_memory = true
can_analyze_failures = true
can_extract_knowledge = true
can_plan_review = true
can_strategy_review = true
can_track_mood = true
can_encourage = true
can_track_milestones = true
notification_pacing = true
max_notifications_per_hour = 5

[loop]
check_interval_minutes = 15
morning_briefing = true
briefing_hour = 8

[[initial_goals]]
title = "Learn user's coding style"
description = "Study code patterns, naming conventions, commit style, and preferred tools to provide increasingly tailored assistance."
priority = "high"

[[initial_goals]]
title = "Build project knowledge"
description = "Map repositories, dependencies, architecture patterns, and key files to provide context-aware code suggestions."
priority = "medium"

[[initial_goals]]
title = "Develop tool mastery"
description = "Become proficient with every available tool, especially code analysis, search, file operations, and git integration."
priority = "medium"

[[schedules]]
name = "daily-code-review"
cron = "0 9 * * *"
agent = "Jarvis"
prompt = "Review recent git commits and code changes. Note any patterns, potential issues, or improvements worth discussing."
notify = true
enabled = true

[[schedules]]
name = "dependency-check"
cron = "0 10 * * 1"
agent = "Jarvis"
prompt = "Check project dependencies for updates, security advisories, and deprecation notices. Summarize findings."
notify = true
enabled = true

[[schedules]]
name = "weekly-retrospective"
cron = "0 17 * * 5"
agent = "Jarvis"
prompt = "Review this week's development activity. Summarize completed work, ongoing tasks, and suggest priorities for next week."
notify = true
enabled = true

[desktop]
clipboard = true
windows = true
notifications = true
"#;
        let f = temp_config(config);
        let snapshot = reload_settings_snapshot(f.path());
        let s = snapshot.unwrap_or_else(|e| panic!("reload_settings_snapshot failed: {e}"));
        assert_eq!(s.agent_name, "Jarvis");
        assert_eq!(s.agent_persona, "coder");
        assert_eq!(s.provider_label, "Ollama");
        assert_eq!(s.model_name, "qwen3.5:9b");
        assert_eq!(s.autonomy_tier, "Trust");
        assert!(s.heartbeat_enabled);
        assert_eq!(s.schedules.len(), 3);
        assert!(s.desktop_configured);
    }
}
