//! D-Bus MPRIS2 media control — play, pause, skip, and query media players.
//!
//! Uses `zbus` (when the `media-control` feature is enabled) or falls back
//! to subprocess calls to `dbus-send` / `playerctl`.

use aivyx_core::{AivyxError, Result};

/// Valid media control actions.
pub const VALID_ACTIONS: &[&str] = &[
    "play", "pause", "toggle", "next", "previous", "stop",
];

/// MPRIS2 D-Bus interface name.
const MPRIS_PLAYER_IFACE: &str = "org.mpris.MediaPlayer2.Player";
const MPRIS_BUS_PREFIX: &str = "org.mpris.MediaPlayer2.";

/// Discover available MPRIS2 media players on the session bus.
pub async fn list_players() -> Result<Vec<String>> {
    let output = run_dbus_cmd(&[
        "dbus-send",
        "--session",
        "--dest=org.freedesktop.DBus",
        "--type=method_call",
        "--print-reply",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus.ListNames",
    ])
    .await?;

    let players: Vec<String> = output
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.starts_with("string \"org.mpris.MediaPlayer2.") {
                let name = trimmed
                    .trim_start_matches("string \"")
                    .trim_end_matches('"');
                Some(name.to_string())
            } else {
                None
            }
        })
        .collect();

    Ok(players)
}

/// Get the "friendly" name of a player from its bus name.
/// e.g., "org.mpris.MediaPlayer2.spotify" → "spotify"
pub fn player_short_name(bus_name: &str) -> &str {
    bus_name
        .strip_prefix(MPRIS_BUS_PREFIX)
        .unwrap_or(bus_name)
}

/// Resolve the target player bus name. If `player` is None, pick the first available.
pub async fn resolve_player(player: Option<&str>) -> Result<String> {
    let players = list_players().await?;

    if players.is_empty() {
        return Err(AivyxError::Other(
            "No active media players found on D-Bus".into(),
        ));
    }

    if let Some(name) = player {
        // Match by short name or full bus name.
        let lower = name.to_lowercase();
        let found = players.iter().find(|p| {
            p.to_lowercase() == lower
                || player_short_name(p).to_lowercase() == lower
        });
        match found {
            Some(p) => Ok(p.clone()),
            None => {
                let available: Vec<&str> = players.iter().map(|p| player_short_name(p)).collect();
                Err(AivyxError::Other(format!(
                    "Player '{name}' not found. Available: {}",
                    available.join(", ")
                )))
            }
        }
    } else {
        // Return the first player.
        Ok(players.into_iter().next().unwrap())
    }
}

/// Send a media control command to a player.
pub async fn control(action: &str, player_bus: &str) -> Result<String> {
    let method = match action {
        "play" => "Play",
        "pause" => "Pause",
        "toggle" => "PlayPause",
        "next" => "Next",
        "previous" => "Previous",
        "stop" => "Stop",
        _ => {
            return Err(AivyxError::Other(format!(
                "Invalid media action: '{action}'. Valid: {}",
                VALID_ACTIONS.join(", ")
            )));
        }
    };

    let dest = format!("--dest={player_bus}");
    let member = format!("{MPRIS_PLAYER_IFACE}.{method}");

    run_dbus_cmd(&[
        "dbus-send",
        "--session",
        "--type=method_call",
        "--print-reply",
        &dest,
        "/org/mpris/MediaPlayer2",
        &member,
    ])
    .await?;

    Ok(format!(
        "{action} sent to {}",
        player_short_name(player_bus)
    ))
}

/// Get current playback metadata from a player.
pub async fn get_metadata(player_bus: &str) -> Result<serde_json::Value> {
    let dest = format!("--dest={player_bus}");

    // Get PlaybackStatus.
    let status_output = run_dbus_cmd(&[
        "dbus-send",
        "--session",
        "--type=method_call",
        "--print-reply",
        &dest,
        "/org/mpris/MediaPlayer2",
        "org.freedesktop.DBus.Properties.Get",
        &format!("string:{MPRIS_PLAYER_IFACE}"),
        "string:PlaybackStatus",
    ])
    .await
    .unwrap_or_default();

    let status = extract_variant_string(&status_output).unwrap_or("Unknown".into());

    // Get Metadata (title, artist, album).
    let meta_output = run_dbus_cmd(&[
        "dbus-send",
        "--session",
        "--type=method_call",
        "--print-reply",
        &dest,
        "/org/mpris/MediaPlayer2",
        "org.freedesktop.DBus.Properties.Get",
        &format!("string:{MPRIS_PLAYER_IFACE}"),
        "string:Metadata",
    ])
    .await
    .unwrap_or_default();

    let title = extract_metadata_field(&meta_output, "xesam:title").unwrap_or_default();
    let artist = extract_metadata_field(&meta_output, "xesam:artist").unwrap_or_default();
    let album = extract_metadata_field(&meta_output, "xesam:album").unwrap_or_default();

    Ok(serde_json::json!({
        "player": player_short_name(player_bus),
        "status": status,
        "title": title,
        "artist": artist,
        "album": album,
    }))
}

// ── Helpers ──────────────────────────────────────────────────────

/// Run a dbus-send command and capture stdout.
async fn run_dbus_cmd(args: &[&str]) -> Result<String> {
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::process::Command::new(args[0])
            .args(&args[1..])
            .output(),
    )
    .await;

    match result {
        Ok(Ok(output)) => {
            if output.status.success() {
                Ok(String::from_utf8_lossy(&output.stdout).into_owned())
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                Err(AivyxError::Other(format!(
                    "D-Bus command failed: {stderr}"
                )))
            }
        }
        Ok(Err(e)) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                Err(AivyxError::Other(
                    "dbus-send not found. Install dbus-tools (usually pre-installed on Linux)."
                        .into(),
                ))
            } else {
                Err(AivyxError::Other(format!("D-Bus command error: {e}")))
            }
        }
        Err(_) => Err(AivyxError::Other("D-Bus command timed out".into())),
    }
}

/// Extract a string from a D-Bus Properties.Get variant reply.
fn extract_variant_string(output: &str) -> Option<String> {
    // dbus-send replies look like:
    //    variant       string "Playing"
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("string \"") || trimmed.starts_with("variant") {
            if let Some(start) = trimmed.find('"') {
                let rest = &trimmed[start + 1..];
                if let Some(end) = rest.find('"') {
                    return Some(rest[..end].to_string());
                }
            }
        }
    }
    None
}

/// Extract a metadata field from dbus-send Metadata reply.
fn extract_metadata_field(output: &str, field: &str) -> Option<String> {
    // Look for the field key, then the next string value.
    let mut found_key = false;
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.contains(field) {
            found_key = true;
            continue;
        }
        if found_key {
            if let Some(val) = extract_variant_string(trimmed) {
                return Some(val);
            }
            // Also check for array entries.
            if trimmed.starts_with("string \"") {
                if let Some(start) = trimmed.find('"') {
                    let rest = &trimmed[start + 1..];
                    if let Some(end) = rest.find('"') {
                        return Some(rest[..end].to_string());
                    }
                }
            }
            // If it's a different dict entry, stop.
            if trimmed.starts_with("dict entry(") {
                return None;
            }
        }
    }
    None
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_short_name_strips_prefix() {
        assert_eq!(
            player_short_name("org.mpris.MediaPlayer2.spotify"),
            "spotify"
        );
        assert_eq!(
            player_short_name("org.mpris.MediaPlayer2.vlc"),
            "vlc"
        );
        assert_eq!(player_short_name("custom"), "custom");
    }

    #[test]
    fn valid_actions_list() {
        assert!(VALID_ACTIONS.contains(&"play"));
        assert!(VALID_ACTIONS.contains(&"toggle"));
        assert!(!VALID_ACTIONS.contains(&"rewind"));
    }

    #[test]
    fn extract_variant_string_parses() {
        let output = r#"   variant       string "Playing"
"#;
        assert_eq!(
            extract_variant_string(output),
            Some("Playing".to_string())
        );
    }

    #[test]
    fn extract_variant_string_empty() {
        assert_eq!(extract_variant_string(""), None);
        assert_eq!(extract_variant_string("no quotes here"), None);
    }

    #[test]
    fn extract_metadata_field_works() {
        let output = r#"      dict entry(
         string "xesam:title"
         variant             string "Never Gonna Give You Up"
      )
      dict entry(
         string "xesam:artist"
         variant             array [
               string "Rick Astley"
            ]
      )
"#;
        assert_eq!(
            extract_metadata_field(output, "xesam:title"),
            Some("Never Gonna Give You Up".to_string())
        );
        assert_eq!(
            extract_metadata_field(output, "xesam:artist"),
            Some("Rick Astley".to_string())
        );
    }
}
