#![allow(unsafe_op_in_unsafe_fn, unused_imports, unreachable_code, unused_variables, dead_code, clippy::all)]
//! Windows media control — System Media Transport Controls (SMTC).
//!
//! Provides media player detection and control (play/pause/next/previous)
//! via the Windows SMTC session manager. This is the Windows equivalent of
//! `dbus.rs` (D-Bus MPRIS) on Linux.
//!
//! SMTC is the unified media interface on Windows 10+ — any app that shows
//! media controls in the volume flyout (Spotify, Chrome, VLC, etc.) exposes
//! an SMTC session.

use aivyx_core::{AivyxError, Result};

/// A discovered media player session.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MediaSession {
    pub name: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub status: String,
}

/// List active media sessions.
///
/// Uses the GlobalSystemMediaTransportControlsSessionManager WinRT API.
pub async fn list_sessions() -> Result<Vec<MediaSession>> {
    #[cfg(target_os = "windows")]
    {
        use windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager;

        let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
            .map_err(|e| AivyxError::Other(format!("SMTC RequestAsync: {e}")))?
            .get()
            .map_err(|e| AivyxError::Other(format!("SMTC get manager: {e}")))?;

        let sessions = manager
            .GetSessions()
            .map_err(|e| AivyxError::Other(format!("SMTC GetSessions: {e}")))?;

        let mut results = Vec::new();
        for i in 0..sessions.Size().unwrap_or(0) {
            let session = match sessions.GetAt(i) {
                Ok(s) => s,
                Err(_) => continue,
            };

            let source_id = session
                .SourceAppUserModelId()
                .map(|s: windows::core::HSTRING| s.to_string())
                .unwrap_or_else(|_| "unknown".into());

            let info = session
                .TryGetMediaPropertiesAsync()
                .ok()
                .and_then(|a: windows::core::IReference<bool>| a.get().ok());

            let (title, artist) = if let Some(ref props) = info {
                (
                    props.Title().ok().map(|s: windows::core::HSTRING| s.to_string()),
                    props.Artist().ok().map(|s: windows::core::HSTRING| s.to_string()),
                )
            } else {
                (None, None)
            };

            let playback_info = session.GetPlaybackInfo().ok();
            let status = playback_info
                .and_then(|pi| pi.PlaybackStatus().ok())
                .map(|s| format!("{s:?}"))
                .unwrap_or_else(|| "unknown".into());

            results.push(MediaSession {
                name: source_id,
                title,
                artist,
                status,
            });
        }

        Ok(results)
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err(AivyxError::Other(
            "win_media: only available on Windows".into(),
        ))
    }
}

/// Send a control command to a media session.
///
/// `session_name`: app model ID or "current" for the active session.
/// `action`: "play", "pause", "toggle", "next", "previous", "stop"
pub async fn control(session_name: &str, action: &str) -> Result<serde_json::Value> {
    #[cfg(target_os = "windows")]
    {
        use windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager;

        let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
            .map_err(|e| AivyxError::Other(format!("SMTC RequestAsync: {e}")))?
            .get()
            .map_err(|e| AivyxError::Other(format!("SMTC get manager: {e}")))?;

        let session = if session_name == "current" || session_name.is_empty() {
            manager
                .GetCurrentSession()
                .map_err(|e| AivyxError::Other(format!("SMTC: no active session: {e}")))?
        } else {
            // Find session by name.
            let sessions = manager
                .GetSessions()
                .map_err(|e| AivyxError::Other(format!("SMTC GetSessions: {e}")))?;
            let mut found = None;
            for i in 0..sessions.Size().unwrap_or(0) {
                if let Ok(s) = sessions.GetAt(i) {
                    let id = s
                        .SourceAppUserModelId()
                        .map(|s: windows::core::HSTRING| s.to_string())
                        .unwrap_or_default();
                    if id.to_lowercase().contains(&session_name.to_lowercase()) {
                        found = Some(s);
                        break;
                    }
                }
            }
            found.ok_or_else(|| {
                AivyxError::Other(format!("SMTC: session '{session_name}' not found"))
            })?
        };

        match action {
            "play" => {
                session
                    .TryPlayAsync()
                    .map_err(|e| AivyxError::Other(format!("TryPlayAsync: {e}")))?
                    .get()
                    .map_err(|e| AivyxError::Other(format!("play: {e}")))?;
            }
            "pause" => {
                session
                    .TryPauseAsync()
                    .map_err(|e| AivyxError::Other(format!("TryPauseAsync: {e}")))?
                    .get()
                    .map_err(|e| AivyxError::Other(format!("pause: {e}")))?;
            }
            "toggle" => {
                session
                    .TryTogglePlayPauseAsync()
                    .map_err(|e| AivyxError::Other(format!("TryTogglePlayPauseAsync: {e}")))?
                    .get()
                    .map_err(|e| AivyxError::Other(format!("toggle: {e}")))?;
            }
            "next" => {
                session
                    .TrySkipNextAsync()
                    .map_err(|e| AivyxError::Other(format!("TrySkipNextAsync: {e}")))?
                    .get()
                    .map_err(|e| AivyxError::Other(format!("next: {e}")))?;
            }
            "previous" => {
                session
                    .TrySkipPreviousAsync()
                    .map_err(|e| AivyxError::Other(format!("TrySkipPreviousAsync: {e}")))?
                    .get()
                    .map_err(|e| AivyxError::Other(format!("previous: {e}")))?;
            }
            "stop" => {
                session
                    .TryStopAsync()
                    .map_err(|e| AivyxError::Other(format!("TryStopAsync: {e}")))?
                    .get()
                    .map_err(|e| AivyxError::Other(format!("stop: {e}")))?;
            }
            _ => {
                return Err(AivyxError::Validation(format!(
                    "Unknown media action: '{action}'. Valid: play, pause, toggle, next, previous, stop"
                )));
            }
        }

        // Return current state after the action.
        let info = session
            .TryGetMediaPropertiesAsync()
            .ok()
            .and_then(|a: windows::core::IReference<bool>| a.get().ok());

        let title = info
            .as_ref()
            .and_then(|p| p.Title().ok())
            .map(|s: windows::core::HSTRING| s.to_string());
        let artist = info
            .as_ref()
            .and_then(|p| p.Artist().ok())
            .map(|s: windows::core::HSTRING| s.to_string());

        Ok(serde_json::json!({
            "action": action,
            "title": title,
            "artist": artist,
        }))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (session_name, action);
        Err(AivyxError::Other(
            "win_media: only available on Windows".into(),
        ))
    }
}

/// Get info about the currently playing media.
pub async fn media_info() -> Result<serde_json::Value> {
    #[cfg(target_os = "windows")]
    {
        let sessions = list_sessions().await?;
        Ok(serde_json::json!({
            "sessions": sessions,
        }))
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err(AivyxError::Other(
            "win_media: only available on Windows".into(),
        ))
    }
}

// ── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_session_serialize() {
        let session = MediaSession {
            name: "Spotify.exe".into(),
            title: Some("Song Title".into()),
            artist: Some("Artist Name".into()),
            status: "Playing".into(),
        };
        let json = serde_json::to_value(&session).unwrap();
        assert_eq!(json["name"], "Spotify.exe");
        assert_eq!(json["status"], "Playing");
    }

    #[test]
    fn media_on_non_windows() {
        #[cfg(not(target_os = "windows"))]
        {
            let rt = tokio::runtime::Runtime::new().unwrap();
            assert!(rt.block_on(list_sessions()).is_err());
            assert!(rt.block_on(control("current", "play")).is_err());
            assert!(rt.block_on(media_info()).is_err());
        }
    }
}
