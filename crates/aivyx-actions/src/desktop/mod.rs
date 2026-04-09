//! Desktop interaction — launch apps, clipboard, window management, notifications.
//!
//! All tools use subprocess calls to standard Linux CLI tools (`xdg-open`,
//! `xclip`/`wl-copy`, `wmctrl`, `xdotool`, `notify-send`). Zero additional
//! crate dependencies.
//!
//! Auto-enabled when a display server ($DISPLAY / $WAYLAND_DISPLAY) is
//! detected. Can be configured via an explicit `[desktop]` section.

pub mod clipboard;
pub mod interaction;
pub mod notify;
pub mod open;
pub mod scanner;
pub mod windows;

use aivyx_core::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Maximum output size from any desktop subprocess (64 KB).
const MAX_OUTPUT_BYTES: usize = 64 * 1024;

/// Default subprocess timeout (seconds).
const DEFAULT_TIMEOUT_SECS: u64 = 5;

// ── Config ────────────────────────────────────────────────────────

/// Per-application access level — controls what the agent can do with each app.
///
/// ```toml
/// [desktop.app_access]
/// firefox = "Full"
/// alacritty = "Interact"
/// nautilus = "ViewOnly"
/// steam = "Blocked"
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AppAccess {
    /// Agent cannot interact with or launch the app at all.
    Blocked,
    /// Agent can see the app in window lists and take screenshots, but
    /// cannot launch, click, type, or close it.
    ViewOnly,
    /// Agent can launch the app and interact with it (click, type, inspect),
    /// but cannot close or force-manage its windows.
    Interact,
    /// Full access — launch, interact, close, resize, manage.
    Full,
}

impl Default for AppAccess {
    fn default() -> Self {
        Self::Full
    }
}

impl std::fmt::Display for AppAccess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Blocked => write!(f, "Blocked"),
            Self::ViewOnly => write!(f, "View Only"),
            Self::Interact => write!(f, "Interact"),
            Self::Full => write!(f, "Full"),
        }
    }
}

impl AppAccess {
    /// All variants in order, for cycling through in the TUI.
    pub const ALL: [AppAccess; 4] = [
        AppAccess::Blocked,
        AppAccess::ViewOnly,
        AppAccess::Interact,
        AppAccess::Full,
    ];

    /// Cycle to the next access level.
    pub fn next(self) -> Self {
        match self {
            Self::Blocked => Self::ViewOnly,
            Self::ViewOnly => Self::Interact,
            Self::Interact => Self::Full,
            Self::Full => Self::Blocked,
        }
    }

    /// Cycle to the previous access level.
    pub fn prev(self) -> Self {
        match self {
            Self::Blocked => Self::Full,
            Self::Full => Self::Interact,
            Self::Interact => Self::ViewOnly,
            Self::ViewOnly => Self::Blocked,
        }
    }

    /// Can the agent launch this app via open_application?
    pub fn can_launch(self) -> bool {
        matches!(self, Self::Interact | Self::Full)
    }

    /// Can the agent see the app in window lists / take screenshots?
    pub fn can_view(self) -> bool {
        !matches!(self, Self::Blocked)
    }

    /// Can the agent click, type, inspect the app's UI?
    pub fn can_interact(self) -> bool {
        matches!(self, Self::Interact | Self::Full)
    }

    /// Can the agent close/resize/manage the app's windows?
    pub fn can_manage(self) -> bool {
        matches!(self, Self::Full)
    }
}

/// Desktop interaction configuration, deserialized from `[desktop]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopConfig {
    /// Strict allowlist of app names the agent may launch.
    /// Empty = all non-denied apps allowed.
    #[serde(default)]
    pub allowed_apps: Vec<String>,

    /// Additional denied app names beyond the built-in denylist.
    #[serde(default)]
    pub denied_apps: Vec<String>,

    /// Per-application access levels. Apps not listed default to `Full`.
    /// This takes precedence over allowed_apps/denied_apps for listed apps.
    #[serde(default)]
    pub app_access: HashMap<String, AppAccess>,

    /// Whether clipboard tools are registered (default: true).
    #[serde(default = "default_true")]
    pub clipboard: bool,

    /// Whether window management tools are registered (default: true).
    #[serde(default = "default_true")]
    pub windows: bool,

    /// Whether the notification tool is registered (default: true).
    #[serde(default = "default_true")]
    pub notifications: bool,

    /// Deep application interaction — AT-SPI2, CDP, D-Bus, ydotool.
    /// `None` if `[desktop.interaction]` section is missing — no interaction tools.
    pub interaction: Option<interaction::InteractionConfig>,
}

fn default_true() -> bool {
    true
}

impl Default for DesktopConfig {
    fn default() -> Self {
        Self {
            allowed_apps: Vec::new(),
            denied_apps: Vec::new(),
            app_access: HashMap::new(),
            clipboard: true,
            windows: true,
            notifications: true,
            interaction: None,
        }
    }
}

// ── Display server detection ──────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayServer {
    X11,
    Wayland,
    Unknown,
}

/// Detect the running display server from environment variables.
pub fn detect_display_server() -> DisplayServer {
    if std::env::var("WAYLAND_DISPLAY").is_ok() {
        DisplayServer::Wayland
    } else if std::env::var("DISPLAY").is_ok() {
        DisplayServer::X11
    } else {
        DisplayServer::Unknown
    }
}

// ── App denylist ──────────────────────────────────────────────────

/// Apps that are always denied regardless of config.
const DENIED_APPS: &[&str] = &[
    // Shells (direct shell bypass)
    "bash",
    "sh",
    "zsh",
    "fish",
    "dash",
    "csh",
    "tcsh",
    // Package managers
    "apt",
    "apt-get",
    "dnf",
    "yum",
    "pacman",
    "snap",
    "flatpak",
    "nix",
    // Privilege escalation
    "sudo",
    "su",
    "pkexec",
    "gksu",
    "doas",
    // System administration
    "systemctl",
    "journalctl",
    "dd",
    "mkfs",
    "fdisk",
    "parted",
    "rm",
    "shred",
    // Network exfiltration
    "nc",
    "ncat",
    "socat",
];

/// Terminal emulators — allowed by default (opening a terminal for the user
/// to see is a UX action, not a privilege escalation). The agent already has
/// `run_command` for shell access. Can be re-blocked via config `denied_apps`.
#[allow(dead_code)]
const TERMINAL_EMULATORS: &[&str] = &[
    "xterm",
    "konsole",
    "gnome-terminal",
    "alacritty",
    "kitty",
    "foot",
    "wezterm",
    "tilix",
    "terminator",
    "urxvt",
    "st",
];

/// Sensitive path fragments — files that should not be opened.
const DENIED_PATH_FRAGMENTS: &[&str] = &[
    "/etc/shadow",
    "/.ssh/",
    "/.gnupg/",
    "/.aws/credentials",
    "/.config/gcloud",
];

/// Resolve the effective access level for an application.
///
/// Precedence: built-in denylist → per-app `app_access` → config `denied_apps`
/// → config `allowed_apps` → default (Full).
pub fn resolve_app_access(app: &str, config: &DesktopConfig) -> AppAccess {
    let lower = app.to_lowercase();
    let bin_name = lower.rsplit('/').next().unwrap_or(&lower);

    // Built-in denylist is absolute (shells, sudo, etc.)
    for &denied in DENIED_APPS {
        if bin_name == denied {
            return AppAccess::Blocked;
        }
    }

    // Per-app access level takes priority over legacy deny/allow lists
    if let Some(&access) = config.app_access.get(bin_name) {
        return access;
    }
    // Also check original case
    if let Some(&access) = config.app_access.get(app) {
        return access;
    }

    // Legacy config denylist
    for denied in &config.denied_apps {
        if bin_name == denied.to_lowercase() {
            return AppAccess::Blocked;
        }
    }

    // Legacy allowlist mode
    if !config.allowed_apps.is_empty()
        && !config
            .allowed_apps
            .iter()
            .any(|a| a.eq_ignore_ascii_case(bin_name))
    {
        return AppAccess::Blocked;
    }

    AppAccess::Full
}

/// Check whether an app name is denied for launching. Returns the matched reason if denied.
///
/// This is the legacy API used by `open_application`. It checks whether the
/// resolved access level permits launching.
pub fn is_app_denied(app: &str, config: &DesktopConfig) -> Option<String> {
    let access = resolve_app_access(app, config);
    if access.can_launch() {
        None
    } else {
        let reason = match access {
            AppAccess::Blocked => "blocked",
            AppAccess::ViewOnly => "view-only access (cannot launch)",
            _ => "access denied",
        };
        Some(format!("{reason}: {app}"))
    }
}

/// Check whether a file path contains sensitive fragments.
pub fn is_path_denied(path: &str) -> Option<&'static str> {
    let lower = path.to_lowercase();
    DENIED_PATH_FRAGMENTS
        .iter()
        .find(|frag| lower.contains(&frag.to_lowercase()))
        .copied()
}

// ── Shared subprocess runner ──────────────────────────────────────

/// Output from a desktop subprocess.
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

/// Run a desktop subprocess with timeout and output capping.
pub async fn run_desktop_command(
    program: &str,
    args: &[&str],
    timeout_secs: u64,
) -> Result<CommandOutput> {
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        tokio::process::Command::new(program).args(args).output(),
    )
    .await;

    match result {
        Ok(Ok(output)) => Ok(CommandOutput {
            stdout: cap_output(&output.stdout),
            stderr: cap_output(&output.stderr),
            exit_code: output.status.code().unwrap_or(-1),
        }),
        Ok(Err(e)) => Err(aivyx_core::AivyxError::Other(format!(
            "Failed to run {program}: {e}"
        ))),
        Err(_) => Err(aivyx_core::AivyxError::Other(format!(
            "{program} timed out after {timeout_secs}s"
        ))),
    }
}

/// Truncate output bytes to the cap, appending notice if truncated.
fn cap_output(bytes: &[u8]) -> String {
    if bytes.len() <= MAX_OUTPUT_BYTES {
        String::from_utf8_lossy(bytes).into_owned()
    } else {
        let truncated = String::from_utf8_lossy(&bytes[..MAX_OUTPUT_BYTES]);
        format!("{truncated}\n...[output truncated at {MAX_OUTPUT_BYTES} bytes]")
    }
}

// ── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denies_shell_commands() {
        let config = DesktopConfig::default();
        assert!(is_app_denied("bash", &config).is_some());
        assert!(is_app_denied("sh", &config).is_some());
        assert!(is_app_denied("zsh", &config).is_some());
        assert!(is_app_denied("/usr/bin/bash", &config).is_some());
    }

    #[test]
    fn allows_terminal_emulators() {
        // Terminal emulators are allowed by default — opening a terminal
        // for the user is a UX action, not a privilege escalation.
        // Users can re-block them via config denied_apps if desired.
        let config = DesktopConfig::default();
        assert!(is_app_denied("alacritty", &config).is_none());
        assert!(is_app_denied("kitty", &config).is_none());
        assert!(is_app_denied("gnome-terminal", &config).is_none());
        assert!(is_app_denied("Konsole", &config).is_none());
    }

    #[test]
    fn config_can_reblock_terminals() {
        let config = DesktopConfig {
            denied_apps: vec!["alacritty".into(), "kitty".into()],
            ..DesktopConfig::default()
        };
        assert!(is_app_denied("alacritty", &config).is_some());
        assert!(is_app_denied("kitty", &config).is_some());
        // Others not in config denylist are still allowed
        assert!(is_app_denied("gnome-terminal", &config).is_none());
    }

    #[test]
    fn denies_privilege_escalation() {
        let config = DesktopConfig::default();
        assert!(is_app_denied("sudo", &config).is_some());
        assert!(is_app_denied("pkexec", &config).is_some());
    }

    #[test]
    fn allows_normal_apps() {
        let config = DesktopConfig::default();
        assert!(is_app_denied("firefox", &config).is_none());
        assert!(is_app_denied("gimp", &config).is_none());
        assert!(is_app_denied("libreoffice", &config).is_none());
        assert!(is_app_denied("code", &config).is_none());
    }

    #[test]
    fn custom_denylist() {
        let config = DesktopConfig {
            denied_apps: vec!["dangerous-app".into()],
            ..Default::default()
        };
        assert!(is_app_denied("dangerous-app", &config).is_some());
        assert!(is_app_denied("firefox", &config).is_none());
    }

    #[test]
    fn allowlist_mode() {
        let config = DesktopConfig {
            allowed_apps: vec!["firefox".into(), "gimp".into()],
            ..Default::default()
        };
        assert!(is_app_denied("firefox", &config).is_none());
        assert!(is_app_denied("gimp", &config).is_none());
        assert!(is_app_denied("chrome", &config).is_some()); // not in allowlist
        // Denylist still applies even if in allowlist
        assert!(is_app_denied("bash", &config).is_some());
    }

    #[test]
    fn path_denylist() {
        assert!(is_path_denied("/etc/shadow").is_some());
        assert!(is_path_denied("/home/user/.ssh/id_rsa").is_some());
        assert!(is_path_denied("/home/user/.gnupg/private-keys-v1.d/key").is_some());
        assert!(is_path_denied("/home/user/Documents/report.pdf").is_none());
    }

    #[test]
    fn display_server_detection() {
        // We can't reliably test env-dependent detection in CI,
        // but we can verify the function doesn't panic
        let _ds = detect_display_server();
    }

    #[test]
    fn app_access_per_app_overrides() {
        let mut config = DesktopConfig::default();
        config
            .app_access
            .insert("firefox".into(), AppAccess::ViewOnly);
        config.app_access.insert("gimp".into(), AppAccess::Blocked);
        config
            .app_access
            .insert("alacritty".into(), AppAccess::Interact);

        assert_eq!(resolve_app_access("firefox", &config), AppAccess::ViewOnly);
        assert_eq!(resolve_app_access("gimp", &config), AppAccess::Blocked);
        assert_eq!(
            resolve_app_access("alacritty", &config),
            AppAccess::Interact
        );
        // Not in app_access → default Full
        assert_eq!(resolve_app_access("code", &config), AppAccess::Full);
        // Built-in denylist still overrides
        assert_eq!(resolve_app_access("bash", &config), AppAccess::Blocked);
    }

    #[test]
    fn app_access_launch_permissions() {
        // ViewOnly → cannot launch
        assert!(!AppAccess::ViewOnly.can_launch());
        assert!(AppAccess::ViewOnly.can_view());
        assert!(!AppAccess::ViewOnly.can_interact());
        assert!(!AppAccess::ViewOnly.can_manage());

        // Interact → can launch + interact, no manage
        assert!(AppAccess::Interact.can_launch());
        assert!(AppAccess::Interact.can_view());
        assert!(AppAccess::Interact.can_interact());
        assert!(!AppAccess::Interact.can_manage());

        // Full → everything
        assert!(AppAccess::Full.can_launch());
        assert!(AppAccess::Full.can_view());
        assert!(AppAccess::Full.can_interact());
        assert!(AppAccess::Full.can_manage());

        // Blocked → nothing
        assert!(!AppAccess::Blocked.can_launch());
        assert!(!AppAccess::Blocked.can_view());
        assert!(!AppAccess::Blocked.can_interact());
        assert!(!AppAccess::Blocked.can_manage());
    }

    #[test]
    fn app_access_view_only_blocks_launch() {
        let mut config = DesktopConfig::default();
        config
            .app_access
            .insert("steam".into(), AppAccess::ViewOnly);

        // is_app_denied returns Some for ViewOnly (can't launch)
        assert!(is_app_denied("steam", &config).is_some());
        // But can still view
        assert!(resolve_app_access("steam", &config).can_view());
    }

    #[test]
    fn app_access_cycles() {
        assert_eq!(AppAccess::Blocked.next(), AppAccess::ViewOnly);
        assert_eq!(AppAccess::ViewOnly.next(), AppAccess::Interact);
        assert_eq!(AppAccess::Interact.next(), AppAccess::Full);
        assert_eq!(AppAccess::Full.next(), AppAccess::Blocked);

        assert_eq!(AppAccess::Full.prev(), AppAccess::Interact);
        assert_eq!(AppAccess::Blocked.prev(), AppAccess::Full);
    }
}
