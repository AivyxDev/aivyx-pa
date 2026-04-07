//! Deep application interaction — semantic UI automation, browser control,
//! media playback, and universal input injection.
//!
//! Multi-backend architecture with platform-specific implementations:
//!
//! **Linux:**
//! - **AT-SPI2** (`accessibility` feature) — GTK/Qt/Electron via accessibility tree
//! - **CDP** (`browser-automation` feature) — Chromium browser via DevTools Protocol
//! - **D-Bus MPRIS** (`media-control` feature) — media player control
//! - **ydotool** (always available) — universal Wayland-compatible input fallback
//!
//! **Windows:**
//! - **UI Automation** (`windows-automation` feature) — Win32 UIA for native app inspection
//! - **CDP** (`browser-automation` feature) — Chromium browser via DevTools Protocol (shared)
//! - **SMTC** (`windows-automation` feature) — System Media Transport Controls
//! - **SendInput** (always available) — Win32 input injection fallback
//!
//! Gated behind `[desktop.interaction]` config. When absent or `enabled = false`,
//! no interaction tools are registered.

// ── Cross-platform modules ──────────────────────────────────────
pub mod cdp;
pub mod documents;
pub mod tools;

// ── Linux-specific backends ─────────────────────────────────────
#[cfg(target_os = "linux")]
#[cfg(feature = "accessibility")]
pub mod atspi;
#[cfg(target_os = "linux")]
pub mod dbus;
#[cfg(target_os = "linux")]
pub mod desktop_info;
#[cfg(target_os = "linux")]
pub mod screen_ocr;
#[cfg(target_os = "linux")]
pub mod screenshot;
#[cfg(target_os = "linux")]
pub mod system_ctl;
#[cfg(target_os = "linux")]
pub mod window_manage;
#[cfg(target_os = "linux")]
pub mod ydotool;

// ── Windows-specific backends ───────────────────────────────────
#[cfg(target_os = "windows")]
pub mod win_input;
#[cfg(target_os = "windows")]
#[cfg(feature = "windows-automation")]
pub mod win_uia;
#[cfg(target_os = "windows")]
pub mod win_screenshot;
#[cfg(target_os = "windows")]
pub mod win_window;
#[cfg(target_os = "windows")]
pub mod win_system;
#[cfg(target_os = "windows")]
pub mod win_ocr;
#[cfg(target_os = "windows")]
pub mod win_media;
#[cfg(target_os = "windows")]
pub mod win_desktop_info;

use aivyx_core::{AivyxError, Result};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// ── Config ────────────────────────────────────────────────────────

/// Top-level interaction configuration, deserialized from `[desktop.interaction]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InteractionConfig {
    /// Master switch — all interaction tools disabled when false.
    #[serde(default)]
    pub enabled: bool,

    /// AT-SPI2 accessibility backend config.
    #[serde(default)]
    pub accessibility: AccessibilityConfig,

    /// Chrome DevTools Protocol backend config.
    #[serde(default)]
    pub browser: BrowserConfig,

    /// D-Bus MPRIS media control config.
    #[serde(default)]
    pub media: MediaConfig,

    /// ydotool input injection config.
    #[serde(default)]
    pub input: InputConfig,
}

impl Default for InteractionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            accessibility: AccessibilityConfig::default(),
            browser: BrowserConfig::default(),
            media: MediaConfig::default(),
            input: InputConfig::default(),
        }
    }
}

/// AT-SPI2 accessibility backend configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessibilityConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for AccessibilityConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// Chrome DevTools Protocol backend configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Chrome remote debugging port.
    #[serde(default = "default_debug_port")]
    pub debug_port: u16,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            debug_port: 9222,
        }
    }
}

fn default_debug_port() -> u16 {
    9222
}

/// D-Bus MPRIS media control configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for MediaConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// ydotool input injection configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

fn default_true() -> bool {
    true
}

// ── Shared types ─────────────────────────────────────────────────

/// Reference to a window by title or X11/Wayland ID.
#[derive(Debug, Clone)]
pub enum WindowRef {
    /// Active/focused window.
    Active,
    /// Match by (substring of) window title.
    Title(String),
    /// Match by numeric window ID.
    Id(String),
}

impl WindowRef {
    /// Parse from optional tool input fields.
    pub fn from_input(
        window: Option<&str>,
        window_id: Option<&str>,
    ) -> Self {
        if let Some(id) = window_id {
            WindowRef::Id(id.to_string())
        } else if let Some(title) = window {
            WindowRef::Title(title.to_string())
        } else {
            WindowRef::Active
        }
    }
}

/// A UI element discovered via accessibility or DOM inspection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiElement {
    /// Hierarchical path (e.g., "0/3/1") for re-locating the element.
    pub path: String,
    /// Role (e.g., "button", "text_field", "menu_item", "label").
    pub role: String,
    /// Accessible name or DOM id/aria-label.
    pub name: String,
    /// Text content if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Bounding box [x, y, width, height] if available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bounds: Option<[i32; 4]>,
    /// State flags (e.g., "focused", "checked", "disabled").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub states: Vec<String>,
}

/// A tree node in the accessibility/DOM tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiTreeNode {
    pub element: UiElement,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<UiTreeNode>,
}

/// Query for finding UI elements.
#[derive(Debug, Clone, Default)]
pub struct ElementQuery {
    /// Filter by role (e.g., "button").
    pub role: Option<String>,
    /// Filter by name (substring match, case-insensitive).
    pub name: Option<String>,
    /// Filter by text content (substring match, case-insensitive).
    pub text: Option<String>,
}

/// Scroll direction for `ui_scroll` and `browser_scroll`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollDirection {
    Up,
    Down,
    Left,
    Right,
}

impl ScrollDirection {
    /// Parse from a string (case-insensitive).
    pub fn parse(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "up" => Ok(Self::Up),
            "down" => Ok(Self::Down),
            "left" => Ok(Self::Left),
            "right" => Ok(Self::Right),
            _ => Err(AivyxError::Validation(format!(
                "Invalid scroll direction: '{s}'. Valid: up, down, left, right"
            ))),
        }
    }
}

/// Resolved window metadata for smart backend routing.
#[derive(Debug, Clone, Default)]
pub struct WindowInfo {
    /// Window class (e.g., "google-chrome", "gnome-calculator", "org.gnome.Nautilus").
    pub class: Option<String>,
    /// Window title.
    pub title: Option<String>,
    /// Process ID of the window.
    pub pid: Option<u32>,
}

// ── InputBackend trait ──────────────────────────────────────────

/// Platform-agnostic input injection interface.
///
/// Implemented by `YdotoolBackend` (Linux) and `WinInputBackend` (Windows).
/// Tools call this via `router.input_backend()` for low-level input operations
/// that aren't covered by the semantic `UiBackend` trait (key combos, coordinate
/// clicks, multi-click, drag, etc.).
#[async_trait::async_trait]
pub(crate) trait InputBackend: Send + Sync {
    /// Send a key combination (e.g., "ctrl+s", "alt+F4").
    async fn key_combo(&self, keys: &str) -> Result<()>;

    /// Type a string of text via keyboard simulation.
    async fn type_string(&self, text: &str) -> Result<()>;

    /// Left-click at absolute screen coordinates.
    async fn click_at(&self, x: i32, y: i32) -> Result<()>;

    /// Double-click at absolute screen coordinates.
    async fn double_click_at(&self, x: i32, y: i32) -> Result<()>;

    /// Middle-click at absolute screen coordinates.
    async fn middle_click_at(&self, x: i32, y: i32) -> Result<()>;

    /// Right-click at absolute screen coordinates.
    async fn right_click_at(&self, x: i32, y: i32) -> Result<()>;

    /// Move the mouse to absolute screen coordinates.
    async fn mouse_move_to(&self, x: i32, y: i32) -> Result<()>;

    /// Scroll the mouse wheel in a direction.
    async fn scroll(&self, direction: ScrollDirection, amount: u32) -> Result<()>;

    /// Drag from one position to another (left-button hold + move + release).
    async fn drag(&self, from_x: i32, from_y: i32, to_x: i32, to_y: i32) -> Result<()>;

    /// Ctrl+click at multiple coordinates (for multi-select).
    async fn multi_click_at(&self, positions: &[(i32, i32)]) -> Result<()>;
}

// ── UiBackend trait ──────────────────────────────────────────────

/// Internal trait unifying AT-SPI2, CDP, and ydotool backends.
///
/// Not public — the `BackendRouter` selects the appropriate backend and
/// the tool implementations call through the router.
#[async_trait::async_trait]
pub(crate) trait UiBackend: Send + Sync {
    fn name(&self) -> &str;

    /// Inspect the accessibility/DOM tree of a window.
    async fn inspect(
        &self,
        window: &WindowRef,
        max_depth: u32,
    ) -> Result<Vec<UiTreeNode>>;

    /// Find elements matching a query.
    async fn find_element(
        &self,
        window: &WindowRef,
        query: &ElementQuery,
    ) -> Result<Vec<UiElement>>;

    /// Click an element (by path or coordinates).
    async fn click(&self, element: &UiElement) -> Result<()>;

    /// Type text into an element or the focused field.
    async fn type_text(
        &self,
        element: Option<&UiElement>,
        text: &str,
    ) -> Result<()>;

    /// Read text content from an element.
    async fn read_text(&self, element: &UiElement) -> Result<String>;

    /// Scroll within a window or element.
    async fn scroll(
        &self,
        _window: &WindowRef,
        _direction: ScrollDirection,
        _amount: u32,
    ) -> Result<()> {
        Err(AivyxError::Other(format!(
            "{} does not support scroll",
            self.name()
        )))
    }

    /// Double-click an element.
    async fn double_click(&self, _element: &UiElement) -> Result<()> {
        Err(AivyxError::Other(format!(
            "{} does not support double-click",
            self.name()
        )))
    }

    /// Middle-click an element.
    async fn middle_click(&self, _element: &UiElement) -> Result<()> {
        Err(AivyxError::Other(format!(
            "{} does not support middle-click",
            self.name()
        )))
    }

    /// Right-click an element (context menu).
    async fn right_click(&self, _element: &UiElement) -> Result<()> {
        Err(AivyxError::Other(format!(
            "{} does not support right-click",
            self.name()
        )))
    }

    /// Hover over an element (trigger tooltips, hover menus).
    async fn hover(&self, _element: &UiElement) -> Result<()> {
        Err(AivyxError::Other(format!(
            "{} does not support hover",
            self.name()
        )))
    }

    /// Drag from one element/position to another.
    async fn drag(&self, _from: &UiElement, _to: &UiElement) -> Result<()> {
        Err(AivyxError::Other(format!(
            "{} does not support drag",
            self.name()
        )))
    }

    /// Move the mouse to absolute screen coordinates.
    async fn mouse_move(&self, _x: i32, _y: i32) -> Result<()> {
        Err(AivyxError::Other(format!(
            "{} does not support mouse_move",
            self.name()
        )))
    }

    /// Take a screenshot of a window. Returns base64-encoded image data.
    async fn screenshot_window(&self, _window: &WindowRef) -> Result<String> {
        Err(AivyxError::Other(format!(
            "{} does not support window screenshots",
            self.name()
        )))
    }
}

// ── BackendRouter (Linux) ────────────────────────────────────────

#[cfg(target_os = "linux")]
/// Routes UI operations to the best available backend for a given window.
pub struct BackendRouter {
    #[cfg(feature = "accessibility")]
    pub(crate) atspi: Option<atspi::AtSpiBackend>,
    pub(crate) cdp: Option<cdp::CdpBackend>,
    pub(crate) ydotool: ydotool::YdotoolBackend,
}

#[cfg(target_os = "linux")]
impl BackendRouter {
    /// Create a router from the interaction config.
    pub fn new(config: &InteractionConfig) -> Self {
        Self {
            #[cfg(feature = "accessibility")]
            atspi: if config.accessibility.enabled {
                Some(atspi::AtSpiBackend::new())
            } else {
                None
            },
            cdp: if config.browser.enabled {
                Some(cdp::CdpBackend::new(config.browser.debug_port))
            } else {
                None
            },
            ydotool: ydotool::YdotoolBackend::new(),
        }
    }

    /// Pick the best backend for a window. Falls back to ydotool.
    pub(crate) async fn route(&self, window: &WindowRef) -> &dyn UiBackend {
        // Try to detect window class for smart routing.
        if let Some(ref cdp_backend) = self.cdp {
            if let Ok(Some(class)) = get_window_class(window).await {
                let lower = class.to_lowercase();
                if is_browser_class(&lower) {
                    return cdp_backend;
                }
            }
        }

        #[cfg(feature = "accessibility")]
        if let Some(ref backend) = self.atspi {
            return backend;
        }
        &self.ydotool
    }

    /// Get the CDP backend (for browser-specific tools).
    pub(crate) fn cdp(&self) -> Option<&cdp::CdpBackend> {
        self.cdp.as_ref()
    }

    /// Get the input injection backend directly (for key combos, coordinate clicks, etc.).
    pub(crate) fn input_backend(&self) -> &dyn InputBackend {
        &self.ydotool
    }
}

// ── BackendRouter (Windows) ─────────────────────────────────────

#[cfg(target_os = "windows")]
/// Routes UI operations to the best available backend for a given window.
pub struct BackendRouter {
    #[cfg(feature = "windows-automation")]
    pub(crate) uia: Option<win_uia::UiaBackend>,
    pub(crate) cdp: Option<cdp::CdpBackend>,
    pub(crate) win_input: win_input::WinInputBackend,
}

#[cfg(target_os = "windows")]
impl BackendRouter {
    /// Create a router from the interaction config.
    pub fn new(config: &InteractionConfig) -> Self {
        Self {
            #[cfg(feature = "windows-automation")]
            uia: if config.accessibility.enabled {
                win_uia::UiaBackend::new().ok()
            } else {
                None
            },
            cdp: if config.browser.enabled {
                Some(cdp::CdpBackend::new(config.browser.debug_port))
            } else {
                None
            },
            win_input: win_input::WinInputBackend::new(),
        }
    }

    /// Pick the best backend for a window. Falls back to SendInput.
    ///
    /// Routing logic:
    /// 1. Get window class (via Win32 GetClassName)
    /// 2. If class matches a known browser → CDP (if available)
    /// 3. If UI Automation available → UIA
    /// 4. Fallback → SendInput
    pub(crate) async fn route(&self, window: &WindowRef) -> &dyn UiBackend {
        if let Some(ref cdp_backend) = self.cdp {
            if let Ok(Some(class)) = get_window_class(window).await {
                let lower = class.to_lowercase();
                if is_browser_class(&lower) {
                    return cdp_backend;
                }
            }
        }

        #[cfg(feature = "windows-automation")]
        if let Some(ref backend) = self.uia {
            return backend;
        }
        &self.win_input
    }

    /// Get the CDP backend (for browser-specific tools).
    pub(crate) fn cdp(&self) -> Option<&cdp::CdpBackend> {
        self.cdp.as_ref()
    }

    /// Get the input injection backend directly (for key combos, coordinate clicks, etc.).
    pub(crate) fn input_backend(&self) -> &dyn InputBackend {
        &self.win_input
    }
}

// ── InteractionContext ───────────────────────────────────────────

/// Shared state for all interaction tools. Passed via `Arc`.
pub struct InteractionContext {
    pub config: InteractionConfig,
    pub router: BackendRouter,
}

impl InteractionContext {
    pub fn new(config: InteractionConfig) -> Arc<Self> {
        let router = BackendRouter::new(&config);
        Arc::new(Self { config, router })
    }
}

// ── Window class detection (cross-platform) ─────────────────────

/// Browser class name patterns for routing to CDP.
const BROWSER_CLASS_PATTERNS: &[&str] = &[
    "chromium",
    "chrome",
    "google-chrome",
    "brave",
    "edge",
    "microsoft-edge",
    "firefox",
    "electron",
    "vivaldi",
    "opera",
];

/// Check if a window class (lowercase) matches a known browser.
fn is_browser_class(class: &str) -> bool {
    BROWSER_CLASS_PATTERNS
        .iter()
        .any(|pattern| class.contains(pattern))
}

// ── Window class detection (Linux) ──────────────────────────────

#[cfg(target_os = "linux")]
async fn get_window_class(window: &WindowRef) -> Result<Option<String>> {
    match window {
        WindowRef::Active => {}
        WindowRef::Title(_) => {}
        WindowRef::Id(_) => return Ok(None),
    }

    if let Ok(class) = get_class_xdotool(window).await {
        return Ok(Some(class));
    }
    if let Ok(class) = get_class_hyprctl().await {
        return Ok(Some(class));
    }
    Ok(None)
}

#[cfg(target_os = "linux")]
async fn get_class_xdotool(window: &WindowRef) -> Result<String> {
    let output = match window {
        WindowRef::Active => {
            tokio::process::Command::new("xdotool")
                .args(["getactivewindow", "getwindowclassname"])
                .output()
                .await
        }
        WindowRef::Title(title) => {
            let id_output = tokio::process::Command::new("xdotool")
                .args(["search", "--name", title])
                .output()
                .await
                .map_err(|e| AivyxError::Other(format!("xdotool search: {e}")))?;

            if !id_output.status.success() {
                return Err(AivyxError::Other("xdotool: window not found".into()));
            }

            let window_id = String::from_utf8_lossy(&id_output.stdout)
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .to_string();

            if window_id.is_empty() {
                return Err(AivyxError::Other("xdotool: empty window ID".into()));
            }

            tokio::process::Command::new("xdotool")
                .args(["getwindowclassname", &window_id])
                .output()
                .await
        }
        WindowRef::Id(_) => unreachable!(),
    };

    let output = output.map_err(|e| AivyxError::Other(format!("xdotool: {e}")))?;

    if !output.status.success() {
        return Err(AivyxError::Other("xdotool getwindowclassname failed".into()));
    }

    let class = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if class.is_empty() {
        return Err(AivyxError::Other("xdotool: empty class name".into()));
    }

    Ok(class)
}

#[cfg(target_os = "linux")]
async fn get_class_hyprctl() -> Result<String> {
    let output = tokio::process::Command::new("hyprctl")
        .args(["activewindow", "-j"])
        .output()
        .await
        .map_err(|e| AivyxError::Other(format!("hyprctl: {e}")))?;

    if !output.status.success() {
        return Err(AivyxError::Other("hyprctl failed".into()));
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| AivyxError::Other(format!("hyprctl parse: {e}")))?;

    json["class"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .ok_or_else(|| AivyxError::Other("hyprctl: no class field".into()))
}

// ── Window class detection (Windows) ────────────────────────────

#[cfg(target_os = "windows")]
async fn get_window_class(window: &WindowRef) -> Result<Option<String>> {
    match window {
        WindowRef::Active => {}
        WindowRef::Title(_) => {}
        WindowRef::Id(_) => return Ok(None),
    }
    // Use Win32 GetClassName on the foreground window.
    if let Ok(class) = win_window::get_foreground_window_class() {
        return Ok(Some(class));
    }
    Ok(None)
}

// ── Constants ────────────────────────────────────────────────────

/// Maximum text input size (10 KB) to prevent clipboard bombing.
pub const MAX_TYPE_TEXT_BYTES: usize = 10 * 1024;

/// Maximum screenshot size (5 MB).
pub const MAX_SCREENSHOT_BYTES: usize = 5 * 1024 * 1024;

/// Allowed URL schemes for browser_navigate.
pub const ALLOWED_URL_SCHEMES: &[&str] = &["http://", "https://", "file://"];

/// Maximum JavaScript expression size for browser_execute_js (10 KB).
pub const MAX_JS_INPUT_BYTES: usize = 10 * 1024;

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_disabled() {
        let config = InteractionConfig::default();
        assert!(!config.enabled);
        assert!(config.accessibility.enabled);
        assert!(config.browser.enabled);
        assert!(config.media.enabled);
        assert!(config.input.enabled);
    }

    #[test]
    fn browser_config_defaults() {
        let config = BrowserConfig::default();
        assert_eq!(config.debug_port, 9222);
        assert!(config.enabled);
    }

    #[test]
    fn window_ref_from_input() {
        assert!(matches!(WindowRef::from_input(None, None), WindowRef::Active));
        assert!(matches!(
            WindowRef::from_input(Some("Firefox"), None),
            WindowRef::Title(t) if t == "Firefox"
        ));
        assert!(matches!(
            WindowRef::from_input(Some("Firefox"), Some("0x1234")),
            WindowRef::Id(id) if id == "0x1234"
        ));
    }

    #[test]
    fn url_scheme_validation() {
        let valid = |url: &str| ALLOWED_URL_SCHEMES.iter().any(|s| url.starts_with(s));
        assert!(valid("https://example.com"));
        assert!(valid("http://localhost:3000"));
        assert!(valid("file:///home/user/doc.html"));
        assert!(!valid("javascript:alert(1)"));
        assert!(!valid("ftp://evil.com"));
        assert!(!valid("data:text/html,<script>"));
    }

    #[test]
    fn element_query_default() {
        let q = ElementQuery::default();
        assert!(q.role.is_none());
        assert!(q.name.is_none());
        assert!(q.text.is_none());
    }

    #[test]
    fn scroll_direction_parse() {
        assert_eq!(ScrollDirection::parse("up").unwrap(), ScrollDirection::Up);
        assert_eq!(ScrollDirection::parse("DOWN").unwrap(), ScrollDirection::Down);
        assert_eq!(ScrollDirection::parse("Left").unwrap(), ScrollDirection::Left);
        assert_eq!(ScrollDirection::parse("right").unwrap(), ScrollDirection::Right);
        assert!(ScrollDirection::parse("diagonal").is_err());
        assert!(ScrollDirection::parse("").is_err());
    }

    #[test]
    fn browser_class_detection() {
        assert!(is_browser_class("google-chrome"));
        assert!(is_browser_class("chromium-browser"));
        assert!(is_browser_class("brave-browser"));
        assert!(is_browser_class("firefox"));
        assert!(is_browser_class("microsoft-edge"));
        assert!(is_browser_class("electron"));
        assert!(is_browser_class("vivaldi-stable"));
        assert!(is_browser_class("opera"));
        assert!(!is_browser_class("gnome-calculator"));
        assert!(!is_browser_class("nautilus"));
        assert!(!is_browser_class("libreoffice"));
        assert!(!is_browser_class(""));
    }

    #[test]
    fn window_info_default() {
        let info = WindowInfo::default();
        assert!(info.class.is_none());
        assert!(info.title.is_none());
        assert!(info.pid.is_none());
    }

    #[test]
    fn config_deserialize() {
        let toml = r#"
            enabled = true
            [accessibility]
            enabled = false
            [browser]
            enabled = true
            debug_port = 9333
            [media]
            enabled = true
            [input]
            enabled = false
        "#;
        let config: InteractionConfig = toml::from_str(toml).unwrap();
        assert!(config.enabled);
        assert!(!config.accessibility.enabled);
        assert!(config.browser.enabled);
        assert_eq!(config.browser.debug_port, 9333);
        assert!(config.media.enabled);
        assert!(!config.input.enabled);
    }
}
