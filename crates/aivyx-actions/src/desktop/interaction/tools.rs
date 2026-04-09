#![allow(
    unsafe_op_in_unsafe_fn,
    unused_imports,
    unreachable_code,
    unused_variables,
    dead_code,
    clippy::all
)]
#![allow(unused_variables, unreachable_code)]

//! Action implementations for all 14 interaction tools.
//!
//! Each tool is a struct implementing `crate::Action`. They delegate to the
//! `BackendRouter` / specific backends in `InteractionContext`.

use std::sync::Arc;

use crate::Action;
use aivyx_core::{AivyxError, Result};

use super::{
    ALLOWED_URL_SCHEMES, ElementQuery, InteractionContext, MAX_JS_INPUT_BYTES, MAX_TYPE_TEXT_BYTES,
    ScrollDirection, WindowRef,
};
// InputBackend and UiBackend traits are in scope via parent module for dyn dispatch.
#[allow(unused_imports)]
use super::{InputBackend, UiBackend};

// ══════════════════════════════════════════════════════════════════
// Semantic UI Tools (AT-SPI2 / ydotool fallback)
// ═════���════════════════════════════════════════════════���═══════════

// ── ui_inspect ────────────��──────────────────────────────────────

pub struct UiInspect {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for UiInspect {
    fn name(&self) -> &str {
        "ui_inspect"
    }

    fn description(&self) -> &str {
        "Read the accessibility tree of a window. Returns structured JSON of UI \
         elements (buttons, labels, text fields, menus) with roles, names, and states. \
         Requires the accessibility backend or AT-SPI2 support."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "window": {
                    "type": "string",
                    "description": "Window title (substring match). Omit for active window."
                },
                "window_id": {
                    "type": "string",
                    "description": "Window ID (overrides window title if both given)."
                },
                "max_depth": {
                    "type": "integer",
                    "description": "Maximum tree depth to traverse (default: 5).",
                    "default": 5
                }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let window = WindowRef::from_input(input["window"].as_str(), input["window_id"].as_str());
        let max_depth = input["max_depth"].as_u64().unwrap_or(5) as u32;

        self.ctx.enforce_access(&window, false).await?;
        let backend = self.ctx.router.route(&window).await;
        let tree = backend.inspect(&window, max_depth).await?;

        Ok(serde_json::json!({
            "backend": backend.name(),
            "element_count": count_nodes(&tree),
            "tree": tree,
        }))
    }
}

fn count_nodes(nodes: &[super::UiTreeNode]) -> usize {
    nodes
        .iter()
        .fold(0, |acc, n| acc + 1 + count_nodes(&n.children))
}

// ── ui_find_element ──────────────────────────────────────────────

pub struct UiFindElement {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for UiFindElement {
    fn name(&self) -> &str {
        "ui_find_element"
    }

    fn description(&self) -> &str {
        "Find a specific UI element by role, name, or text content. Returns element \
         paths that can be used with ui_click and ui_type_text."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "window": {
                    "type": "string",
                    "description": "Window title (substring match). Omit for active window."
                },
                "window_id": {
                    "type": "string",
                    "description": "Window ID."
                },
                "role": {
                    "type": "string",
                    "description": "Element role filter (e.g., 'button', 'text_field', 'menu_item')."
                },
                "name": {
                    "type": "string",
                    "description": "Element name filter (substring match, case-insensitive)."
                },
                "text": {
                    "type": "string",
                    "description": "Element text content filter (substring match)."
                }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let window = WindowRef::from_input(input["window"].as_str(), input["window_id"].as_str());
        let query = ElementQuery {
            role: input["role"].as_str().map(String::from),
            name: input["name"].as_str().map(String::from),
            text: input["text"].as_str().map(String::from),
        };

        if query.role.is_none() && query.name.is_none() && query.text.is_none() {
            return Err(AivyxError::Validation(
                "At least one of role, name, or text must be provided".into(),
            ));
        }

        self.ctx.enforce_access(&window, false).await?;
        let backend = self.ctx.router.route(&window).await;
        let elements = backend.find_element(&window, &query).await?;

        Ok(serde_json::json!({
            "backend": backend.name(),
            "count": elements.len(),
            "elements": elements,
        }))
    }
}

// ── ui_click ─────────────────────────────────────────────────────

pub struct UiClick {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for UiClick {
    fn name(&self) -> &str {
        "ui_click"
    }

    fn description(&self) -> &str {
        "Click a UI element found by ui_find_element (using its element path), \
         or search for it directly by role+name. Falls back to coordinate-based \
         clicking via ydotool if the accessibility backend can't perform the action."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "window": {
                    "type": "string",
                    "description": "Window title (substring match)."
                },
                "element": {
                    "type": "string",
                    "description": "Element path from ui_find_element result."
                },
                "role": {
                    "type": "string",
                    "description": "Element role (used with name for direct lookup)."
                },
                "name": {
                    "type": "string",
                    "description": "Element name (used with role for direct lookup)."
                }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let window = WindowRef::from_input(input["window"].as_str(), None);

        // Either use element path or do a find-then-click.
        let element = if let Some(path) = input["element"].as_str() {
            super::UiElement {
                path: path.to_string(),
                role: String::new(),
                name: String::new(),
                text: None,
                bounds: None,
                states: Vec::new(),
            }
        } else {
            let query = ElementQuery {
                role: input["role"].as_str().map(String::from),
                name: input["name"].as_str().map(String::from),
                text: None,
            };
            if query.role.is_none() && query.name.is_none() {
                return Err(AivyxError::Validation(
                    "Provide element path, or role+name to find the element".into(),
                ));
            }
            let backend = self.ctx.router.route(&window).await;
            let elements = backend.find_element(&window, &query).await?;
            elements
                .into_iter()
                .next()
                .ok_or_else(|| AivyxError::Other("No matching element found".into()))?
        };

        self.ctx.enforce_access(&window, true).await?;
        let backend = self.ctx.router.route(&window).await;
        // Try semantic click; fall back to ydotool coordinate click.
        match backend.click(&element).await {
            Ok(()) => Ok(serde_json::json!({
                "status": "clicked",
                "backend": backend.name(),
                "element": element.name,
            })),
            Err(e) if element.bounds.is_some() => {
                // Fallback to coordinate-based click via input backend.
                let b = element.bounds.unwrap();
                let cx = b[0] + b[2] / 2;
                let cy = b[1] + b[3] / 2;
                self.ctx.router.input_backend().click_at(cx, cy).await?;
                Ok(serde_json::json!({
                    "status": "clicked",
                    "backend": "input (fallback)",
                    "element": element.name,
                    "note": format!("Semantic click failed ({e}), used coordinate fallback"),
                }))
            }
            Err(e) => Err(e),
        }
    }
}

// ── ui_type_text ───��───────────────────���─────────────────────────

pub struct UiTypeText {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for UiTypeText {
    fn name(&self) -> &str {
        "ui_type_text"
    }

    fn description(&self) -> &str {
        "Type text into a focused text field or a specific UI element. \
         Uses the accessibility backend when available, falls back to \
         ydotool keyboard simulation."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "Text to type."
                },
                "element": {
                    "type": "string",
                    "description": "Element path (optional — types into focused element if omitted)."
                }
            },
            "required": ["text"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let text = input["text"]
            .as_str()
            .ok_or_else(|| AivyxError::Validation("text is required".into()))?;

        if text.len() > MAX_TYPE_TEXT_BYTES {
            return Err(AivyxError::Validation(format!(
                "Text too large ({} bytes, max {MAX_TYPE_TEXT_BYTES})",
                text.len()
            )));
        }

        let element = input["element"].as_str().map(|path| super::UiElement {
            path: path.to_string(),
            role: String::new(),
            name: String::new(),
            text: None,
            bounds: None,
            states: Vec::new(),
        });

        let window = WindowRef::Active;
        self.ctx.enforce_access(&window, true).await?;
        let backend = self.ctx.router.route(&window).await;

        match backend.type_text(element.as_ref(), text).await {
            Ok(()) => Ok(serde_json::json!({
                "status": "typed",
                "backend": backend.name(),
                "length": text.len(),
            })),
            Err(_) => {
                // Fallback to input backend.
                self.ctx.router.input_backend().type_string(text).await?;
                Ok(serde_json::json!({
                    "status": "typed",
                    "backend": "input (fallback)",
                    "length": text.len(),
                }))
            }
        }
    }
}

// ── ui_read_text ─────────────────────���───────────────────────────

pub struct UiReadText {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for UiReadText {
    fn name(&self) -> &str {
        "ui_read_text"
    }

    fn description(&self) -> &str {
        "Read text content from a UI element or the active focus. \
         Requires the accessibility backend (AT-SPI2)."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "element": {
                    "type": "string",
                    "description": "Element path from ui_find_element."
                },
                "window": {
                    "type": "string",
                    "description": "Window title."
                }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let element_path = input["element"]
            .as_str()
            .ok_or_else(|| AivyxError::Validation("element path is required".into()))?;

        let element = super::UiElement {
            path: element_path.to_string(),
            role: String::new(),
            name: String::new(),
            text: None,
            bounds: None,
            states: Vec::new(),
        };

        let window = WindowRef::from_input(input["window"].as_str(), None);
        self.ctx.enforce_access(&window, false).await?;
        let backend = self.ctx.router.route(&window).await;
        let text = backend.read_text(&element).await?;

        Ok(serde_json::json!({
            "backend": backend.name(),
            "text": text,
        }))
    }
}

// ── ui_key_combo ─────��────────────────────���──────────────────────

pub struct UiKeyCombo {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for UiKeyCombo {
    fn name(&self) -> &str {
        "ui_key_combo"
    }

    fn description(&self) -> &str {
        "Send a keyboard shortcut (e.g., ctrl+s, alt+F4, super). Uses ydotool \
         for reliable key injection on both X11 and Wayland. The ydotoold daemon \
         must be running."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "keys": {
                    "type": "string",
                    "description": "Key combination (e.g., 'ctrl+s', 'alt+F4', 'super', 'ctrl+shift+t')."
                }
            },
            "required": ["keys"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let keys = input["keys"]
            .as_str()
            .ok_or_else(|| AivyxError::Validation("keys is required".into()))?;

        if keys.trim().is_empty() {
            return Err(AivyxError::Validation("keys must not be empty".into()));
        }

        self.ctx.enforce_access(&WindowRef::Active, true).await?;
        self.ctx.router.input_backend().key_combo(keys).await?;

        Ok(serde_json::json!({
            "status": "sent",
            "keys": keys,
        }))
    }
}

// ══════════════════════════════════════════════════════════════════
// Browser Automation Tools (CDP)
// ═══���══════════════════���═══════════════════════════════════════════

fn require_cdp(ctx: &InteractionContext) -> Result<&super::cdp::CdpBackend> {
    ctx.router.cdp().ok_or_else(|| {
        AivyxError::Other(
            "Browser automation is disabled. Enable [desktop.interaction.browser] in config."
                .into(),
        )
    })
}

// ── browser_navigate ─────���─────────────────────────��─────────────

pub struct BrowserNavigate {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for BrowserNavigate {
    fn name(&self) -> &str {
        "browser_navigate"
    }

    fn description(&self) -> &str {
        "Navigate a browser tab to a URL. The browser must be running with \
         --remote-debugging-port enabled. Only http://, https://, and file:// \
         URLs are allowed."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "URL to navigate to."
                },
                "tab": {
                    "type": "integer",
                    "description": "Tab index (default: 0 = active tab).",
                    "default": 0
                }
            },
            "required": ["url"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let url = input["url"]
            .as_str()
            .ok_or_else(|| AivyxError::Validation("url is required".into()))?;
        let tab = input["tab"].as_u64().unwrap_or(0) as usize;

        if !ALLOWED_URL_SCHEMES.iter().any(|s| url.starts_with(s)) {
            return Err(AivyxError::Validation(format!(
                "URL scheme not allowed. Only http://, https://, file:// permitted. Got: {url}"
            )));
        }

        let cdp = require_cdp(&self.ctx)?;
        let result = cdp.navigate(url, tab).await?;

        Ok(serde_json::json!({
            "status": "navigated",
            "url": url,
            "tab": tab,
            "detail": result,
        }))
    }
}

// ── browser_query ────��──────────────────────────────────────��────

pub struct BrowserQuery {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for BrowserQuery {
    fn name(&self) -> &str {
        "browser_query"
    }

    fn description(&self) -> &str {
        "Query DOM elements in a browser tab by CSS selector. Returns matching \
         elements with their text content and attributes."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "selector": {
                    "type": "string",
                    "description": "CSS selector (e.g., 'button.submit', '#login-form input')."
                },
                "tab": {
                    "type": "integer",
                    "description": "Tab index (default: 0).",
                    "default": 0
                }
            },
            "required": ["selector"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let selector = input["selector"]
            .as_str()
            .ok_or_else(|| AivyxError::Validation("selector is required".into()))?;
        let tab = input["tab"].as_u64().unwrap_or(0) as usize;

        let cdp = require_cdp(&self.ctx)?;
        cdp.query_selector(selector, tab).await
    }
}

// ── browser_click ──────────────────────────────────────��─────────

pub struct BrowserClick {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for BrowserClick {
    fn name(&self) -> &str {
        "browser_click"
    }

    fn description(&self) -> &str {
        "Click a DOM element in the browser by CSS selector. Scrolls the element \
         into view and clicks it."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "selector": {
                    "type": "string",
                    "description": "CSS selector for the element to click."
                },
                "tab": {
                    "type": "integer",
                    "description": "Tab index (default: 0).",
                    "default": 0
                }
            },
            "required": ["selector"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let selector = input["selector"]
            .as_str()
            .ok_or_else(|| AivyxError::Validation("selector is required".into()))?;
        let tab = input["tab"].as_u64().unwrap_or(0) as usize;

        let cdp = require_cdp(&self.ctx)?;
        let result = cdp.click_selector(selector, tab).await?;

        Ok(serde_json::json!({
            "status": "clicked",
            "selector": selector,
            "detail": result,
        }))
    }
}

// ── browser_type ───���────────────────────────────────��────────────

pub struct BrowserType {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for BrowserType {
    fn name(&self) -> &str {
        "browser_type"
    }

    fn description(&self) -> &str {
        "Type text into a form field in the browser, selected by CSS selector. \
         Focuses the element first, then dispatches keyboard events."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "selector": {
                    "type": "string",
                    "description": "CSS selector for the input field."
                },
                "text": {
                    "type": "string",
                    "description": "Text to type into the field."
                },
                "tab": {
                    "type": "integer",
                    "description": "Tab index (default: 0).",
                    "default": 0
                }
            },
            "required": ["selector", "text"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let selector = input["selector"]
            .as_str()
            .ok_or_else(|| AivyxError::Validation("selector is required".into()))?;
        let text = input["text"]
            .as_str()
            .ok_or_else(|| AivyxError::Validation("text is required".into()))?;
        let tab = input["tab"].as_u64().unwrap_or(0) as usize;

        if text.len() > MAX_TYPE_TEXT_BYTES {
            return Err(AivyxError::Validation(format!(
                "Text too large ({} bytes, max {MAX_TYPE_TEXT_BYTES})",
                text.len()
            )));
        }

        let cdp = require_cdp(&self.ctx)?;
        let result = cdp.type_text(selector, text, tab).await?;

        Ok(serde_json::json!({
            "status": "typed",
            "selector": selector,
            "length": text.len(),
            "detail": result,
        }))
    }
}

// ── browser_read_page ────────────────────────────────────────────

pub struct BrowserReadPage {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for BrowserReadPage {
    fn name(&self) -> &str {
        "browser_read_page"
    }

    fn description(&self) -> &str {
        "Read the visible text content of a browser page or a specific element. \
         Returns the innerText, capped at 64KB."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "selector": {
                    "type": "string",
                    "description": "CSS selector to scope reading (optional — reads full page if omitted)."
                },
                "tab": {
                    "type": "integer",
                    "description": "Tab index (default: 0).",
                    "default": 0
                }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let selector = input["selector"].as_str();
        let tab = input["tab"].as_u64().unwrap_or(0) as usize;

        let cdp = require_cdp(&self.ctx)?;
        let text = cdp.read_page(selector, tab).await?;

        Ok(serde_json::json!({
            "text": text,
            "length": text.len(),
        }))
    }
}

// ── browser_screenshot ───────────────────────────────────────────

pub struct BrowserScreenshot {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for BrowserScreenshot {
    fn name(&self) -> &str {
        "browser_screenshot"
    }

    fn description(&self) -> &str {
        "Take a screenshot of the current browser page. Returns a base64-encoded \
         image. Useful for visual inspection of page state."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "format": {
                    "type": "string",
                    "enum": ["png", "jpeg"],
                    "description": "Image format (default: png).",
                    "default": "png"
                },
                "tab": {
                    "type": "integer",
                    "description": "Tab index (default: 0).",
                    "default": 0
                }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let format = input["format"].as_str().unwrap_or("png");
        let tab = input["tab"].as_u64().unwrap_or(0) as usize;

        if format != "png" && format != "jpeg" {
            return Err(AivyxError::Validation(
                "format must be 'png' or 'jpeg'".into(),
            ));
        }

        let cdp = require_cdp(&self.ctx)?;
        let data = cdp.screenshot(format, tab).await?;

        Ok(serde_json::json!({
            "format": format,
            "data_base64": data,
            "size_bytes": data.len(),
        }))
    }
}

// ═════════════════════════════════════════════════════��════════════
// Media & System Tools (D-Bus MPRIS)
// ══════���═══════════════════════════════════════════════════════════

// ── media_control ─────────────────────────────────────────���──────

pub struct MediaControl {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for MediaControl {
    fn name(&self) -> &str {
        "media_control"
    }

    fn description(&self) -> &str {
        "Control the active media player — play, pause, toggle, next, previous, stop. \
         Uses D-Bus MPRIS2, supported by most Linux media players (VLC, Spotify, \
         Firefox, Chromium, mpv, etc.)."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["play", "pause", "toggle", "next", "previous", "stop"],
                    "description": "Media action to perform."
                },
                "player": {
                    "type": "string",
                    "description": "Player name (e.g., 'spotify', 'vlc'). Omit to auto-detect."
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        if !self.ctx.config.media.enabled {
            return Err(AivyxError::Other(
                "Media control is disabled. Enable [desktop.interaction.media] in config.".into(),
            ));
        }

        let action = input["action"]
            .as_str()
            .ok_or_else(|| AivyxError::Validation("action is required".into()))?;

        #[cfg(target_os = "linux")]
        {
            if !super::dbus::VALID_ACTIONS.contains(&action) {
                return Err(AivyxError::Validation(format!(
                    "Invalid action: '{action}'. Valid: {}",
                    super::dbus::VALID_ACTIONS.join(", ")
                )));
            }

            let player_name = input["player"].as_str();
            let player_bus = super::dbus::resolve_player(player_name).await?;
            let result = super::dbus::control(action, &player_bus).await?;

            Ok(serde_json::json!({
                "status": "ok",
                "detail": result,
            }))
        }

        #[cfg(target_os = "windows")]
        {
            const VALID_ACTIONS: &[&str] = &["play", "pause", "toggle", "next", "previous", "stop"];
            if !VALID_ACTIONS.contains(&action) {
                return Err(AivyxError::Validation(format!(
                    "Invalid action: '{action}'. Valid: {}",
                    VALID_ACTIONS.join(", ")
                )));
            }

            let session = input["player"].as_str().unwrap_or("current");
            let result = super::win_media::control(session, action).await?;

            Ok(serde_json::json!({
                "status": "ok",
                "detail": result,
            }))
        }

        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        {
            Err(AivyxError::Other(
                "Media control is not supported on this platform".into(),
            ))
        }
    }
}

// ── media_info ─────────────────────────────────────���─────────────

pub struct MediaInfo {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for MediaInfo {
    fn name(&self) -> &str {
        "media_info"
    }

    fn description(&self) -> &str {
        "Get current playback info from the active media player — title, artist, \
         album, and playback status. Uses D-Bus MPRIS2."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "player": {
                    "type": "string",
                    "description": "Player name (e.g., 'spotify'). Omit to auto-detect."
                }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        if !self.ctx.config.media.enabled {
            return Err(AivyxError::Other(
                "Media control is disabled. Enable [desktop.interaction.media] in config.".into(),
            ));
        }

        #[cfg(target_os = "linux")]
        {
            let player_name = input["player"].as_str();
            let player_bus = super::dbus::resolve_player(player_name).await?;
            super::dbus::get_metadata(&player_bus).await
        }

        #[cfg(target_os = "windows")]
        {
            let _ = input; // player selection not needed — SMTC returns all sessions.
            super::win_media::media_info().await
        }

        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        {
            let _ = input;
            Err(AivyxError::Other(
                "Media info is not supported on this platform".into(),
            ))
        }
    }
}

// ══════════════════════════════════════════════════════════════════
// Extended Interaction Tools (Phase 2 expansion)
// ══════════════════════════════════════════════════════════════════

// ── ui_scroll ───────────────────────────────────────────────────

pub struct UiScroll {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for UiScroll {
    fn name(&self) -> &str {
        "ui_scroll"
    }

    fn description(&self) -> &str {
        "Scroll within any application window. Uses mouse wheel injection via \
         ydotool. Direction can be up, down, left, or right."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "direction": {
                    "type": "string",
                    "enum": ["up", "down", "left", "right"],
                    "description": "Scroll direction."
                },
                "amount": {
                    "type": "integer",
                    "description": "Scroll units (default: 3). Each unit is approx one mouse wheel click.",
                    "default": 3
                },
                "window": {
                    "type": "string",
                    "description": "Window title (optional — scrolls wherever cursor is)."
                }
            },
            "required": ["direction"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let direction_str = input["direction"]
            .as_str()
            .ok_or_else(|| AivyxError::Validation("direction is required".into()))?;
        let direction = ScrollDirection::parse(direction_str)?;
        let amount = input["amount"].as_u64().unwrap_or(3) as u32;
        let window = WindowRef::from_input(input["window"].as_str(), None);
        self.ctx.enforce_access(&window, true).await?;
        let backend = self.ctx.router.route(&window).await;
        backend.scroll(&window, direction, amount).await?;

        Ok(serde_json::json!({
            "status": "scrolled",
            "direction": direction_str,
            "amount": amount,
            "backend": backend.name(),
        }))
    }
}

// ── ui_right_click ──────────────────────────────────────────────

pub struct UiRightClick {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for UiRightClick {
    fn name(&self) -> &str {
        "ui_right_click"
    }

    fn description(&self) -> &str {
        "Right-click a UI element to open its context menu. Accepts an element \
         path from ui_find_element, role+name for lookup, or x+y coordinates."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "window": { "type": "string", "description": "Window title (optional)." },
                "element": { "type": "string", "description": "Element path from ui_find_element." },
                "role": { "type": "string", "description": "Element role (for lookup)." },
                "name": { "type": "string", "description": "Element name (for lookup)." },
                "x": { "type": "integer", "description": "Absolute X coordinate (if no element)." },
                "y": { "type": "integer", "description": "Absolute Y coordinate (if no element)." }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        if let (Some(x), Some(y)) = (input["x"].as_i64(), input["y"].as_i64()) {
            self.ctx
                .router
                .input_backend()
                .right_click_at(x as i32, y as i32)
                .await?;
            return Ok(serde_json::json!({
                "status": "right_clicked",
                "backend": "input",
                "x": x,
                "y": y,
            }));
        }

        let window = WindowRef::from_input(input["window"].as_str(), None);
        let element = resolve_element_input(&self.ctx, &input, &window).await?;
        self.ctx.enforce_access(&window, true).await?;
        let backend = self.ctx.router.route(&window).await;
        backend.right_click(&element).await?;

        Ok(serde_json::json!({
            "status": "right_clicked",
            "backend": backend.name(),
            "element": element.name,
        }))
    }
}

// ── ui_hover ────────────────────────────────────────────────────

pub struct UiHover {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for UiHover {
    fn name(&self) -> &str {
        "ui_hover"
    }

    fn description(&self) -> &str {
        "Hover over a UI element to trigger tooltips or hover menus. \
         Accepts an element path, role+name, or x+y coordinates."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "window": { "type": "string", "description": "Window title (optional)." },
                "element": { "type": "string", "description": "Element path." },
                "role": { "type": "string", "description": "Element role (for lookup)." },
                "name": { "type": "string", "description": "Element name (for lookup)." },
                "x": { "type": "integer", "description": "Absolute X coordinate." },
                "y": { "type": "integer", "description": "Absolute Y coordinate." }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        if let (Some(x), Some(y)) = (input["x"].as_i64(), input["y"].as_i64()) {
            self.ctx
                .router
                .input_backend()
                .mouse_move_to(x as i32, y as i32)
                .await?;
            return Ok(serde_json::json!({
                "status": "hovered",
                "backend": "input",
                "x": x,
                "y": y,
            }));
        }

        let window = WindowRef::from_input(input["window"].as_str(), None);
        let element = resolve_element_input(&self.ctx, &input, &window).await?;
        self.ctx.enforce_access(&window, false).await?;
        let backend = self.ctx.router.route(&window).await;
        backend.hover(&element).await?;

        Ok(serde_json::json!({
            "status": "hovered",
            "backend": backend.name(),
            "element": element.name,
        }))
    }
}

// ── ui_drag ─────────────────────────────────────────────────────

pub struct UiDrag {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for UiDrag {
    fn name(&self) -> &str {
        "ui_drag"
    }

    fn description(&self) -> &str {
        "Drag from one position to another. Useful for drag-and-drop, sliders, \
         and resizing. Accepts element paths or x+y coordinates for source and \
         destination."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "from_x": { "type": "integer", "description": "Source X coordinate." },
                "from_y": { "type": "integer", "description": "Source Y coordinate." },
                "to_x": { "type": "integer", "description": "Destination X coordinate." },
                "to_y": { "type": "integer", "description": "Destination Y coordinate." },
                "from_element": { "type": "string", "description": "Source element path." },
                "to_element": { "type": "string", "description": "Destination element path." }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        if let (Some(fx), Some(fy), Some(tx), Some(ty)) = (
            input["from_x"].as_i64(),
            input["from_y"].as_i64(),
            input["to_x"].as_i64(),
            input["to_y"].as_i64(),
        ) {
            self.ctx
                .router
                .input_backend()
                .drag(fx as i32, fy as i32, tx as i32, ty as i32)
                .await?;
            return Ok(serde_json::json!({
                "status": "dragged",
                "backend": "input",
                "from": [fx, fy],
                "to": [tx, ty],
            }));
        }

        let from_path = input["from_element"].as_str().ok_or_else(|| {
            AivyxError::Validation(
                "Provide from_x+from_y+to_x+to_y coordinates, or from_element+to_element paths"
                    .into(),
            )
        })?;
        let to_path = input["to_element"].as_str().ok_or_else(|| {
            AivyxError::Validation("to_element is required for element-based drag".into())
        })?;

        let from = super::UiElement {
            path: from_path.to_string(),
            role: String::new(),
            name: String::new(),
            text: None,
            bounds: None,
            states: Vec::new(),
        };
        let to = super::UiElement {
            path: to_path.to_string(),
            role: String::new(),
            name: String::new(),
            text: None,
            bounds: None,
            states: Vec::new(),
        };

        let window = WindowRef::Active;
        self.ctx.enforce_access(&window, true).await?;
        let backend = self.ctx.router.route(&window).await;
        backend.drag(&from, &to).await?;

        Ok(serde_json::json!({
            "status": "dragged",
            "backend": backend.name(),
            "from_element": from_path,
            "to_element": to_path,
        }))
    }
}

// ── ui_mouse_move ───────────────────────────────────────────────

pub struct UiMouseMove {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for UiMouseMove {
    fn name(&self) -> &str {
        "ui_mouse_move"
    }

    fn description(&self) -> &str {
        "Move the mouse cursor to absolute screen coordinates. Uses ydotool \
         for precise cursor positioning on both X11 and Wayland."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "x": { "type": "integer", "description": "Absolute X coordinate." },
                "y": { "type": "integer", "description": "Absolute Y coordinate." }
            },
            "required": ["x", "y"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let x = input["x"]
            .as_i64()
            .ok_or_else(|| AivyxError::Validation("x is required".into()))? as i32;
        let y = input["y"]
            .as_i64()
            .ok_or_else(|| AivyxError::Validation("y is required".into()))? as i32;

        self.ctx.router.input_backend().mouse_move_to(x, y).await?;

        Ok(serde_json::json!({
            "status": "moved",
            "x": x,
            "y": y,
        }))
    }
}

// ── window_screenshot ───────────────────────────────────────────

pub struct WindowScreenshot {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for WindowScreenshot {
    fn name(&self) -> &str {
        "window_screenshot"
    }

    fn description(&self) -> &str {
        "Take a screenshot of a specific window, screen region, or the full \
         screen. Returns base64-encoded image data. Uses grim (Wayland) or \
         import (X11). Provide `region` for a precise screen area."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "window": {
                    "type": "string",
                    "description": "Window title (optional — screenshots full screen if omitted)."
                },
                "region": {
                    "type": "string",
                    "description": "Screen region as 'x,y widthxheight' (e.g., '100,200 800x600'). Overrides window."
                },
                "format": {
                    "type": "string",
                    "enum": ["png", "jpeg"],
                    "description": "Image format (default: png).",
                    "default": "png"
                }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let format = input["format"].as_str().unwrap_or("png");
        if format != "png" && format != "jpeg" {
            return Err(AivyxError::Validation(
                "format must be 'png' or 'jpeg'".into(),
            ));
        }

        #[cfg(target_os = "linux")]
        {
            // Region takes precedence over window.
            let geometry = if let Some(region) = input["region"].as_str() {
                if !region.contains('x') || region.is_empty() {
                    return Err(AivyxError::Validation(
                        "region must be in 'x,y widthxheight' format (e.g., '100,200 800x600')"
                            .into(),
                    ));
                }
                Some(region.to_string())
            } else {
                let window_title = input["window"].as_str();
                super::screenshot::get_window_geometry(window_title).await?
            };

            let data = super::screenshot::capture_window(geometry.as_deref(), format).await?;

            Ok(serde_json::json!({
                "format": format,
                "data_base64": data,
                "size_bytes": data.len(),
            }))
        }

        #[cfg(target_os = "windows")]
        {
            let data = if let Some(window_title) = input["window"].as_str() {
                super::win_screenshot::capture_window_by_title(window_title).await?
            } else {
                // Region or full screen — capture full screen for now.
                super::win_screenshot::capture_screen().await?
            };

            Ok(serde_json::json!({
                "format": format,
                "data_base64": data,
                "size_bytes": data.len(),
            }))
        }

        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        {
            Err(AivyxError::Other(
                "Screenshots not supported on this platform".into(),
            ))
        }
    }
}

// ── browser_scroll ──────────────────────────────────────────────

pub struct BrowserScroll {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for BrowserScroll {
    fn name(&self) -> &str {
        "browser_scroll"
    }

    fn description(&self) -> &str {
        "Scroll within a browser page. Can scroll the whole page or a specific \
         element selected by CSS selector. Uses Chrome DevTools Protocol."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "direction": {
                    "type": "string",
                    "enum": ["up", "down", "left", "right"],
                    "description": "Scroll direction."
                },
                "amount": {
                    "type": "integer",
                    "description": "Scroll units (default: 3). Each unit is approx 100px.",
                    "default": 3
                },
                "selector": {
                    "type": "string",
                    "description": "CSS selector to scope scrolling (optional — scrolls page if omitted)."
                },
                "tab": {
                    "type": "integer",
                    "description": "Tab index (default: 0).",
                    "default": 0
                }
            },
            "required": ["direction"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let direction_str = input["direction"]
            .as_str()
            .ok_or_else(|| AivyxError::Validation("direction is required".into()))?;
        let direction = ScrollDirection::parse(direction_str)?;
        let amount = input["amount"].as_u64().unwrap_or(3) as u32;
        let selector = input["selector"].as_str();
        let tab = input["tab"].as_u64().unwrap_or(0) as usize;

        let cdp = require_cdp(&self.ctx)?;
        let result = cdp.scroll_page(direction, amount, selector, tab).await?;

        Ok(serde_json::json!({
            "status": "scrolled",
            "direction": direction_str,
            "amount": amount,
            "detail": result,
        }))
    }
}

// ── browser_execute_js ──────────────────────────────────────────

pub struct BrowserExecuteJs {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for BrowserExecuteJs {
    fn name(&self) -> &str {
        "browser_execute_js"
    }

    fn description(&self) -> &str {
        "Execute arbitrary JavaScript in a browser tab. Returns the evaluated \
         result. Useful for complex page interactions, data extraction, or \
         automation that other browser tools don't cover. Max 10KB expression."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "expression": {
                    "type": "string",
                    "description": "JavaScript expression to evaluate."
                },
                "tab": {
                    "type": "integer",
                    "description": "Tab index (default: 0).",
                    "default": 0
                }
            },
            "required": ["expression"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let expression = input["expression"]
            .as_str()
            .ok_or_else(|| AivyxError::Validation("expression is required".into()))?;
        let tab = input["tab"].as_u64().unwrap_or(0) as usize;

        if expression.len() > MAX_JS_INPUT_BYTES {
            return Err(AivyxError::Validation(format!(
                "JavaScript expression too large ({} bytes, max {MAX_JS_INPUT_BYTES})",
                expression.len()
            )));
        }

        let cdp = require_cdp(&self.ctx)?;
        let result = cdp.execute_js(expression, tab).await?;

        Ok(serde_json::json!({
            "status": "executed",
            "result": result,
        }))
    }
}

// ── Helper: resolve element from tool input ─────────────────────

/// Resolve a UiElement from tool input that accepts element path OR role+name.
async fn resolve_element_input(
    ctx: &InteractionContext,
    input: &serde_json::Value,
    window: &WindowRef,
) -> Result<super::UiElement> {
    if let Some(path) = input["element"].as_str() {
        return Ok(super::UiElement {
            path: path.to_string(),
            role: String::new(),
            name: String::new(),
            text: None,
            bounds: None,
            states: Vec::new(),
        });
    }

    let query = ElementQuery {
        role: input["role"].as_str().map(String::from),
        name: input["name"].as_str().map(String::from),
        text: None,
    };

    if query.role.is_none() && query.name.is_none() {
        return Err(AivyxError::Validation(
            "Provide element path, role+name, or x+y coordinates".into(),
        ));
    }

    let backend = ctx.router.route(window).await;
    let elements = backend.find_element(window, &query).await?;
    elements
        .into_iter()
        .next()
        .ok_or_else(|| AivyxError::Other("No matching element found".into()))
}

// ══════════════════════════════════════════════════════════════════
// Tier 1+2 Expansion Tools
// ══════════════════════════════════════════════════════════════════

// ── ui_double_click ─────────────────────────────────────────────

pub struct UiDoubleClick {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for UiDoubleClick {
    fn name(&self) -> &str {
        "ui_double_click"
    }

    fn description(&self) -> &str {
        "Double-click a UI element. Useful for opening files in file managers, \
         selecting words in text editors, and other double-click interactions. \
         Accepts element path, role+name, or x+y coordinates."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "window": { "type": "string", "description": "Window title (optional)." },
                "element": { "type": "string", "description": "Element path from ui_find_element." },
                "role": { "type": "string", "description": "Element role (for lookup)." },
                "name": { "type": "string", "description": "Element name (for lookup)." },
                "x": { "type": "integer", "description": "Absolute X coordinate." },
                "y": { "type": "integer", "description": "Absolute Y coordinate." }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        if let (Some(x), Some(y)) = (input["x"].as_i64(), input["y"].as_i64()) {
            self.ctx
                .router
                .input_backend()
                .double_click_at(x as i32, y as i32)
                .await?;
            return Ok(serde_json::json!({
                "status": "double_clicked", "backend": "input", "x": x, "y": y,
            }));
        }

        let window = WindowRef::from_input(input["window"].as_str(), None);
        let element = resolve_element_input(&self.ctx, &input, &window).await?;
        self.ctx.enforce_access(&window, true).await?;
        let backend = self.ctx.router.route(&window).await;
        backend.double_click(&element).await?;

        Ok(serde_json::json!({
            "status": "double_clicked", "backend": backend.name(), "element": element.name,
        }))
    }
}

// ── ui_middle_click ─────────────────────────────────────────────

pub struct UiMiddleClick {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for UiMiddleClick {
    fn name(&self) -> &str {
        "ui_middle_click"
    }

    fn description(&self) -> &str {
        "Middle-click a UI element. On Linux, middle-click typically pastes the \
         primary selection. Also used to open links in new tabs in browsers."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "window": { "type": "string", "description": "Window title (optional)." },
                "element": { "type": "string", "description": "Element path." },
                "role": { "type": "string", "description": "Element role (for lookup)." },
                "name": { "type": "string", "description": "Element name (for lookup)." },
                "x": { "type": "integer", "description": "Absolute X coordinate." },
                "y": { "type": "integer", "description": "Absolute Y coordinate." }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        if let (Some(x), Some(y)) = (input["x"].as_i64(), input["y"].as_i64()) {
            self.ctx
                .router
                .input_backend()
                .middle_click_at(x as i32, y as i32)
                .await?;
            return Ok(serde_json::json!({
                "status": "middle_clicked", "backend": "input", "x": x, "y": y,
            }));
        }

        let window = WindowRef::from_input(input["window"].as_str(), None);
        let element = resolve_element_input(&self.ctx, &input, &window).await?;
        self.ctx.enforce_access(&window, true).await?;
        let backend = self.ctx.router.route(&window).await;
        backend.middle_click(&element).await?;

        Ok(serde_json::json!({
            "status": "middle_clicked", "backend": backend.name(), "element": element.name,
        }))
    }
}

// ── browser_list_tabs ───────────────────────────────────────────

pub struct BrowserListTabs {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for BrowserListTabs {
    fn name(&self) -> &str {
        "browser_list_tabs"
    }

    fn description(&self) -> &str {
        "List all open browser tabs with their titles and URLs. Returns tab \
         index, title, and URL for each page tab."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }

    async fn execute(&self, _input: serde_json::Value) -> Result<serde_json::Value> {
        let cdp = require_cdp(&self.ctx)?;
        let tabs = cdp.list_tabs_info().await?;
        let count = tabs.as_array().map(|a| a.len()).unwrap_or(0);
        Ok(serde_json::json!({ "count": count, "tabs": tabs }))
    }
}

// ── browser_new_tab ─────────────────────────────────────────────

pub struct BrowserNewTab {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for BrowserNewTab {
    fn name(&self) -> &str {
        "browser_new_tab"
    }

    fn description(&self) -> &str {
        "Open a new browser tab, optionally navigating to a URL."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "URL to open (optional — opens blank tab if omitted)."
                }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let url = input["url"].as_str().unwrap_or("about:blank");
        if url != "about:blank" && !ALLOWED_URL_SCHEMES.iter().any(|s| url.starts_with(s)) {
            return Err(AivyxError::Validation(format!(
                "URL scheme not allowed. Only http://, https://, file:// permitted. Got: {url}"
            )));
        }
        let cdp = require_cdp(&self.ctx)?;
        let result = cdp.new_tab(url).await?;
        Ok(serde_json::json!({ "status": "opened", "url": url, "detail": result }))
    }
}

// ── browser_close_tab ───────────────────────────────────────────

pub struct BrowserCloseTab {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for BrowserCloseTab {
    fn name(&self) -> &str {
        "browser_close_tab"
    }

    fn description(&self) -> &str {
        "Close a browser tab by index."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "tab": {
                    "type": "integer",
                    "description": "Tab index to close (default: 0 = active tab).",
                    "default": 0
                }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let tab = input["tab"].as_u64().unwrap_or(0) as usize;
        let cdp = require_cdp(&self.ctx)?;
        cdp.close_tab(tab).await?;
        Ok(serde_json::json!({ "status": "closed", "tab": tab }))
    }
}

// ── browser_wait_for ────────────────────────────────────────────

pub struct BrowserWaitFor {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for BrowserWaitFor {
    fn name(&self) -> &str {
        "browser_wait_for"
    }

    fn description(&self) -> &str {
        "Wait for a DOM element to appear on the page. Polls with a CSS \
         selector until the element exists or the timeout is reached. Essential \
         for waiting after navigation or dynamic content loading."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "selector": {
                    "type": "string",
                    "description": "CSS selector to wait for."
                },
                "timeout": {
                    "type": "integer",
                    "description": "Max wait time in milliseconds (default: 5000).",
                    "default": 5000
                },
                "tab": {
                    "type": "integer",
                    "description": "Tab index (default: 0).",
                    "default": 0
                }
            },
            "required": ["selector"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let selector = input["selector"]
            .as_str()
            .ok_or_else(|| AivyxError::Validation("selector is required".into()))?;
        let timeout_ms = input["timeout"].as_u64().unwrap_or(5000);
        let tab = input["tab"].as_u64().unwrap_or(0) as usize;

        if timeout_ms > 30000 {
            return Err(AivyxError::Validation(
                "timeout must be 30000ms or less".into(),
            ));
        }

        let cdp = require_cdp(&self.ctx)?;
        let found = cdp.wait_for_selector(selector, timeout_ms, tab).await?;

        Ok(serde_json::json!({
            "status": if found { "found" } else { "timeout" },
            "selector": selector,
            "timeout_ms": timeout_ms,
        }))
    }
}

// ── window_manage ───────────────────────────────────────────────

pub struct WindowManage {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for WindowManage {
    fn name(&self) -> &str {
        "window_manage"
    }

    fn description(&self) -> &str {
        "Manage desktop windows: minimize, maximize, restore, close, resize, \
         move, or toggle fullscreen. Works with the active window or a window \
         matched by title."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["minimize", "maximize", "restore", "close", "fullscreen",
                             "resize", "move"],
                    "description": "Window management action."
                },
                "window": {
                    "type": "string",
                    "description": "Window title (optional — uses active window)."
                },
                "width": {
                    "type": "integer",
                    "description": "New width (for resize action)."
                },
                "height": {
                    "type": "integer",
                    "description": "New height (for resize action)."
                },
                "x": {
                    "type": "integer",
                    "description": "New X position (for move action)."
                },
                "y": {
                    "type": "integer",
                    "description": "New Y position (for move action)."
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let action = input["action"]
            .as_str()
            .ok_or_else(|| AivyxError::Validation("action is required".into()))?;
        let window = input["window"].as_str();

        #[cfg(target_os = "linux")]
        let result = super::window_manage::manage_window(action, window, &input).await?;

        #[cfg(target_os = "windows")]
        let result = super::win_window::manage_window(action, window, &input).await?;

        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        let result: String = return Err(AivyxError::Other(
            "Window management not supported on this platform".into(),
        ));

        Ok(serde_json::json!({
            "status": "ok",
            "action": action,
            "detail": result,
        }))
    }
}

// ── system_volume ───────────────────────────────────────────────

pub struct SystemVolume {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for SystemVolume {
    fn name(&self) -> &str {
        "system_volume"
    }

    fn description(&self) -> &str {
        "Control system audio volume. Set absolute percentage, adjust up/down, \
         mute, or unmute. Uses wpctl (PipeWire) or pactl (PulseAudio)."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["set", "up", "down", "mute", "unmute", "toggle_mute", "get"],
                    "description": "Volume action."
                },
                "value": {
                    "type": "integer",
                    "description": "Percentage value (for set: 0-100, for up/down: increment)."
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let action = input["action"]
            .as_str()
            .ok_or_else(|| AivyxError::Validation("action is required".into()))?;
        let value = input["value"].as_u64().map(|v| v as u32);

        #[cfg(target_os = "linux")]
        {
            super::system_ctl::volume_control(action, value).await
        }

        #[cfg(target_os = "windows")]
        {
            super::win_system::volume_control(action, value).await
        }

        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        {
            let _ = (action, value);
            Err(AivyxError::Other(
                "Volume control not supported on this platform".into(),
            ))
        }
    }
}

// ── system_brightness ───────────────────────────────────────────

pub struct SystemBrightness {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for SystemBrightness {
    fn name(&self) -> &str {
        "system_brightness"
    }

    fn description(&self) -> &str {
        "Control screen brightness. Set absolute percentage, adjust up/down, \
         or get current value. Uses brightnessctl."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["set", "up", "down", "get"],
                    "description": "Brightness action."
                },
                "value": {
                    "type": "integer",
                    "description": "Percentage value (for set: 0-100, for up/down: increment)."
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let action = input["action"]
            .as_str()
            .ok_or_else(|| AivyxError::Validation("action is required".into()))?;
        let value = input["value"].as_u64().map(|v| v as u32);

        #[cfg(target_os = "linux")]
        {
            super::system_ctl::brightness_control(action, value).await
        }

        #[cfg(target_os = "windows")]
        {
            super::win_system::brightness_control(action, value).await
        }

        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        {
            let _ = (action, value);
            Err(AivyxError::Other(
                "Brightness control not supported on this platform".into(),
            ))
        }
    }
}

// ── ui_select_option ────────────────────────────────────────────

pub struct UiSelectOption {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for UiSelectOption {
    fn name(&self) -> &str {
        "ui_select_option"
    }

    fn description(&self) -> &str {
        "Select an option from a dropdown, combo box, or list. Works by finding \
         the dropdown element, clicking to open it, then finding and clicking \
         the target option. For browser dropdowns, uses JavaScript select."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "selector": {
                    "type": "string",
                    "description": "CSS selector for <select> element (browser) or element path (native)."
                },
                "value": {
                    "type": "string",
                    "description": "Option value to select."
                },
                "text": {
                    "type": "string",
                    "description": "Option visible text to select (alternative to value)."
                },
                "tab": {
                    "type": "integer",
                    "description": "Browser tab index (default: 0).",
                    "default": 0
                }
            },
            "required": ["selector"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let selector = input["selector"]
            .as_str()
            .ok_or_else(|| AivyxError::Validation("selector is required".into()))?;
        let value = input["value"].as_str();
        let text = input["text"].as_str();
        let tab = input["tab"].as_u64().unwrap_or(0) as usize;

        if value.is_none() && text.is_none() {
            return Err(AivyxError::Validation(
                "Provide value or text to select".into(),
            ));
        }

        // Use CDP for browser select elements.
        let cdp = require_cdp(&self.ctx)?;
        let result = cdp.select_option(selector, value, text, tab).await?;

        Ok(serde_json::json!({
            "status": "selected",
            "selector": selector,
            "detail": result,
        }))
    }
}

// ── ui_clear_field ──────────────────────────────────────────────

pub struct UiClearField {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for UiClearField {
    fn name(&self) -> &str {
        "ui_clear_field"
    }

    fn description(&self) -> &str {
        "Clear a text input field. For browser fields, uses JavaScript. For \
         native fields, sends Ctrl+A then Delete via ydotool. Useful before \
         typing new content into an existing field."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "selector": {
                    "type": "string",
                    "description": "CSS selector (browser) or element path (native)."
                },
                "tab": {
                    "type": "integer",
                    "description": "Browser tab index (default: 0).",
                    "default": 0
                }
            },
            "required": ["selector"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let selector = input["selector"]
            .as_str()
            .ok_or_else(|| AivyxError::Validation("selector is required".into()))?;
        let tab = input["tab"].as_u64().unwrap_or(0) as usize;

        // Try browser clear first.
        if let Ok(cdp) = require_cdp(&self.ctx) {
            let result = cdp.clear_field(selector, tab).await?;
            return Ok(serde_json::json!({
                "status": "cleared", "backend": "cdp", "detail": result,
            }));
        }

        // Native fallback: Ctrl+A then Delete.
        self.ctx.router.input_backend().key_combo("ctrl+a").await?;
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        self.ctx.router.input_backend().key_combo("delete").await?;

        Ok(serde_json::json!({
            "status": "cleared", "backend": "input",
        }))
    }
}

// ── notification_list ───────────────────────────────────────────

pub struct NotificationList {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for NotificationList {
    fn name(&self) -> &str {
        "notification_list"
    }

    fn description(&self) -> &str {
        "List recent desktop notifications. Reads from the notification history \
         daemon (dunstctl, swaync, or makoctl). Returns notification summaries, \
         bodies, and app names."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "count": {
                    "type": "integer",
                    "description": "Maximum notifications to return (default: 10).",
                    "default": 10
                }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let count = input["count"].as_u64().unwrap_or(10) as usize;
        #[cfg(target_os = "linux")]
        {
            super::system_ctl::list_notifications(count).await
        }

        #[cfg(target_os = "windows")]
        {
            let _ = count;
            // Windows doesn't have a unified notification history API.
            // Return an informative message rather than an error.
            Ok(serde_json::json!({
                "notifications": [],
                "note": "Windows does not provide a notification history API. Use Action Center manually.",
            }))
        }

        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        {
            let _ = count;
            Err(AivyxError::Other(
                "Notification listing not supported on this platform".into(),
            ))
        }
    }
}

// ── file_manager_show ───────────────────────────────────────────

pub struct FileManagerShow {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for FileManagerShow {
    fn name(&self) -> &str {
        "file_manager_show"
    }

    fn description(&self) -> &str {
        "Open a file or folder in the system file manager. Can reveal a specific \
         file (highlight it in the parent folder) or open a directory. Uses \
         D-Bus org.freedesktop.FileManager1 or xdg-open as fallback."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute path to a file or directory."
                },
                "reveal": {
                    "type": "boolean",
                    "description": "If true, opens the parent folder and highlights the file (default: false).",
                    "default": false
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let path = input["path"]
            .as_str()
            .ok_or_else(|| AivyxError::Validation("path is required".into()))?;
        let reveal = input["reveal"].as_bool().unwrap_or(false);

        #[cfg(target_os = "linux")]
        let result = super::system_ctl::file_manager_show(path, reveal).await?;

        #[cfg(target_os = "windows")]
        let result = super::win_system::file_manager_show(path, reveal).await?;

        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        let result: String = return Err(AivyxError::Other(
            "File manager not supported on this platform".into(),
        ));

        Ok(serde_json::json!({
            "status": "opened",
            "path": path,
            "reveal": reveal,
            "detail": result,
        }))
    }
}

// ══════════════════════════════════════════════════════════════════
// Tier 3+4+5 — High-Impact Expansion
// ══════════════════════════════════════════════════════════════════

// ── screen_ocr ─────────────────────────────────────────────────

pub struct ScreenOcr {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for ScreenOcr {
    fn name(&self) -> &str {
        "screen_ocr"
    }

    fn description(&self) -> &str {
        "Extract text from a screen region using OCR (Tesseract). This is the \
         fallback for reading text from apps that don't expose accessibility data \
         — games, remote desktops, images, Electron apps. Specify a screen region \
         in 'x,y widthxheight' format."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "region": {
                    "type": "string",
                    "description": "Screen region as 'x,y widthxheight' (e.g., '100,200 800x600')."
                },
                "language": {
                    "type": "string",
                    "description": "OCR language (default: eng). E.g., deu, fra, jpn, chi_sim.",
                    "default": "eng"
                }
            },
            "required": ["region"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let region = input["region"]
            .as_str()
            .ok_or_else(|| AivyxError::Validation("region is required".into()))?;
        let language = input["language"].as_str();

        #[cfg(target_os = "linux")]
        let text = super::screen_ocr::ocr_region(region, language).await?;

        #[cfg(target_os = "windows")]
        let text = {
            let _ = language; // Windows OCR uses system language packs, not a language param.
            // Parse "x,y widthxheight" format into (x, y, w, h).
            let parts: Vec<&str> = region.split_whitespace().collect();
            if parts.len() != 2 {
                return Err(AivyxError::Validation(
                    "region must be in 'x,y widthxheight' format".into(),
                ));
            }
            let coords: Vec<i32> = parts[0]
                .split(',')
                .map(|s| s.trim().parse::<i32>())
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|_| AivyxError::Validation("Invalid region coordinates".into()))?;
            let dims: Vec<i32> = parts[1]
                .split('x')
                .map(|s| s.trim().parse::<i32>())
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|_| AivyxError::Validation("Invalid region dimensions".into()))?;
            if coords.len() != 2 || dims.len() != 2 {
                return Err(AivyxError::Validation(
                    "region must be in 'x,y widthxheight' format".into(),
                ));
            }
            super::win_ocr::ocr_region(coords[0], coords[1], dims[0], dims[1]).await?
        };

        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        let text: String = return Err(AivyxError::Other(
            "OCR not supported on this platform".into(),
        ));

        Ok(serde_json::json!({
            "status": "ok",
            "region": region,
            "text": text,
            "char_count": text.len(),
        }))
    }
}

// ── list_running_apps ──────────────────────────────────────────

pub struct ListRunningApps {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for ListRunningApps {
    fn name(&self) -> &str {
        "list_running_apps"
    }

    fn description(&self) -> &str {
        "List all running GUI applications on the desktop. Returns window titles, \
         classes, PIDs, and workspace assignments. Useful for understanding what's \
         currently open before interacting with specific windows."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }

    async fn execute(&self, _input: serde_json::Value) -> Result<serde_json::Value> {
        #[cfg(target_os = "linux")]
        {
            super::desktop_info::list_running_apps().await
        }

        #[cfg(target_os = "windows")]
        {
            super::win_desktop_info::list_running_apps().await
        }

        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        {
            Err(AivyxError::Other("Not supported on this platform".into()))
        }
    }
}

// ── desktop_workspace ──────────────────────────────────────────

pub struct DesktopWorkspace {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for DesktopWorkspace {
    fn name(&self) -> &str {
        "desktop_workspace"
    }

    fn description(&self) -> &str {
        "Manage desktop workspaces/virtual desktops. List workspaces, get the \
         current workspace, switch to another, or move a window between workspaces."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "current", "switch", "move_window"],
                    "description": "Workspace action."
                },
                "target": {
                    "type": "string",
                    "description": "Target workspace name or number (for switch/move_window)."
                },
                "window": {
                    "type": "string",
                    "description": "Window title (for move_window — defaults to active window)."
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let action = input["action"]
            .as_str()
            .ok_or_else(|| AivyxError::Validation("action is required".into()))?;
        let target = input["target"].as_str();
        let window = input["window"].as_str();

        #[cfg(target_os = "linux")]
        {
            super::desktop_info::workspace_control(action, target, window).await
        }

        #[cfg(target_os = "windows")]
        {
            let _ = (target, window);
            // Windows virtual desktops have limited API access.
            // Windows 10/11 has IVirtualDesktopManager but it's undocumented COM.
            match action {
                "list" | "current" => Ok(serde_json::json!({
                    "note": "Windows virtual desktop API is limited. Use Win+Tab or Win+Ctrl+Arrow to manage desktops.",
                })),
                _ => Err(AivyxError::Other(
                    "Windows virtual desktop management requires undocumented COM APIs. Use keyboard shortcuts instead.".into(),
                )),
            }
        }

        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        {
            let _ = (action, target, window);
            Err(AivyxError::Other("Not supported on this platform".into()))
        }
    }
}

// ── browser_pdf ────────────────────────────────────────────────

pub struct BrowserPdf {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for BrowserPdf {
    fn name(&self) -> &str {
        "browser_pdf"
    }

    fn description(&self) -> &str {
        "Save the current browser page as a PDF document. Returns base64-encoded \
         PDF data. Useful for archiving articles, receipts, or documentation."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "tab": {
                    "type": "integer",
                    "description": "Tab index (default: 0).",
                    "default": 0
                },
                "landscape": {
                    "type": "boolean",
                    "description": "Landscape orientation (default: false).",
                    "default": false
                }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let tab = input["tab"].as_u64().unwrap_or(0) as usize;
        let landscape = input["landscape"].as_bool().unwrap_or(false);

        let cdp = require_cdp(&self.ctx)?;
        let data = cdp.print_to_pdf(tab, landscape).await?;

        Ok(serde_json::json!({
            "status": "ok",
            "format": "pdf",
            "data_base64": data,
            "size_bytes": data.len(),
            "landscape": landscape,
        }))
    }
}

// ── browser_find_text ──────────────────────────────────────────

pub struct BrowserFindText {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for BrowserFindText {
    fn name(&self) -> &str {
        "browser_find_text"
    }

    fn description(&self) -> &str {
        "Find text on a browser page (Ctrl+F equivalent). Returns the number \
         of matches and up to 10 surrounding context snippets. Case-insensitive."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Text to search for on the page."
                },
                "tab": {
                    "type": "integer",
                    "description": "Tab index (default: 0).",
                    "default": 0
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let query = input["query"]
            .as_str()
            .ok_or_else(|| AivyxError::Validation("query is required".into()))?;

        if query.is_empty() {
            return Err(AivyxError::Validation("query must not be empty".into()));
        }
        if query.len() > 1000 {
            return Err(AivyxError::Validation(
                "query too long (max 1000 characters)".into(),
            ));
        }

        let tab = input["tab"].as_u64().unwrap_or(0) as usize;
        let cdp = require_cdp(&self.ctx)?;
        let result = cdp.find_text_on_page(query, tab).await?;

        Ok(serde_json::json!({
            "query": query,
            "count": result["count"],
            "matches": result["matches"],
        }))
    }
}

// ── ui_multi_select ────────────────────────────────────────────

pub struct UiMultiSelect {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for UiMultiSelect {
    fn name(&self) -> &str {
        "ui_multi_select"
    }

    fn description(&self) -> &str {
        "Ctrl+click multiple UI elements to select them simultaneously. Essential \
         for selecting multiple files in a file manager, multiple list items, or \
         multiple table rows. Provide a list of coordinate pairs."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "positions": {
                    "type": "array",
                    "description": "Array of [x, y] coordinate pairs to Ctrl+click.",
                    "items": {
                        "type": "array",
                        "items": { "type": "integer" },
                        "minItems": 2,
                        "maxItems": 2
                    }
                }
            },
            "required": ["positions"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let positions_val = input["positions"]
            .as_array()
            .ok_or_else(|| AivyxError::Validation("positions array is required".into()))?;

        if positions_val.is_empty() {
            return Err(AivyxError::Validation(
                "At least one position is required".into(),
            ));
        }

        if positions_val.len() > 100 {
            return Err(AivyxError::Validation(
                "Too many positions (max 100)".into(),
            ));
        }

        let mut positions: Vec<(i32, i32)> = Vec::new();
        for (i, pos) in positions_val.iter().enumerate() {
            let arr = pos
                .as_array()
                .ok_or_else(|| AivyxError::Validation(format!("Position {i} must be [x, y]")))?;
            if arr.len() != 2 {
                return Err(AivyxError::Validation(format!(
                    "Position {i} must be [x, y], got {} elements",
                    arr.len()
                )));
            }
            let x = arr[0].as_i64().ok_or_else(|| {
                AivyxError::Validation(format!("Position {i} x must be an integer"))
            })? as i32;
            let y = arr[1].as_i64().ok_or_else(|| {
                AivyxError::Validation(format!("Position {i} y must be an integer"))
            })? as i32;
            positions.push((x, y));
        }

        self.ctx
            .router
            .input_backend()
            .multi_click_at(&positions)
            .await?;

        Ok(serde_json::json!({
            "status": "multi_selected",
            "count": positions.len(),
            "backend": "input",
        }))
    }
}

// ══════════════════════════════════════════════════════════════════
// Document Tools
// ══════════════════════════════════════════════════════════════════

// ── doc_create_text ────────────────────────────────────────────

pub struct DocCreateText {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for DocCreateText {
    fn name(&self) -> &str {
        "doc_create_text"
    }

    fn description(&self) -> &str {
        "Create a text or markdown file. Supports .txt, .md, .html, .json, \
         .yaml, .toml, .xml, .csv, and other text formats. Parent directories \
         are created automatically."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute output path (e.g., /home/user/Documents/notes.md)."
                },
                "content": {
                    "type": "string",
                    "description": "File content (max 1MB)."
                }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let path = input["path"]
            .as_str()
            .ok_or_else(|| AivyxError::Validation("path is required".into()))?;
        let content = input["content"]
            .as_str()
            .ok_or_else(|| AivyxError::Validation("content is required".into()))?;

        let result = super::documents::create_text_file(path, content).await?;

        Ok(serde_json::json!({
            "status": "created",
            "path": path,
            "detail": result,
            "bytes": content.len(),
        }))
    }
}

// ── doc_create_spreadsheet ─────────────────────────────────────

pub struct DocCreateSpreadsheet {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for DocCreateSpreadsheet {
    fn name(&self) -> &str {
        "doc_create_spreadsheet"
    }

    fn description(&self) -> &str {
        "Create a spreadsheet from structured data. Writes CSV natively. \
         For .xlsx, .xls, or .ods output, converts via ssconvert (gnumeric) \
         or LibreOffice."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute output path (e.g., /home/user/data.csv or .xlsx)."
                },
                "headers": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Column headers."
                },
                "rows": {
                    "type": "array",
                    "items": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "description": "Data rows (array of arrays)."
                }
            },
            "required": ["path", "headers", "rows"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let path = input["path"]
            .as_str()
            .ok_or_else(|| AivyxError::Validation("path is required".into()))?;

        let headers: Vec<String> = input["headers"]
            .as_array()
            .ok_or_else(|| AivyxError::Validation("headers array is required".into()))?
            .iter()
            .map(|v| v.as_str().unwrap_or("").to_string())
            .collect();

        if headers.is_empty() {
            return Err(AivyxError::Validation("headers must not be empty".into()));
        }

        let rows: Vec<Vec<String>> = input["rows"]
            .as_array()
            .ok_or_else(|| AivyxError::Validation("rows array is required".into()))?
            .iter()
            .map(|row| {
                row.as_array()
                    .map(|cells| {
                        cells
                            .iter()
                            .map(|c| c.as_str().unwrap_or("").to_string())
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .collect();

        let result = super::documents::create_spreadsheet(path, &headers, &rows).await?;

        Ok(serde_json::json!({
            "status": "created",
            "path": path,
            "detail": result,
            "columns": headers.len(),
            "rows": rows.len(),
        }))
    }
}

// ── doc_create_pdf ─────────────────────────────────────────────

pub struct DocCreatePdf {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for DocCreatePdf {
    fn name(&self) -> &str {
        "doc_create_pdf"
    }

    fn description(&self) -> &str {
        "Create a PDF from markdown, HTML, LaTeX, or reStructuredText content. \
         Uses pandoc (primary) or weasyprint (HTML fallback). Write content \
         inline — no need to create a file first."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute output path for the PDF."
                },
                "content": {
                    "type": "string",
                    "description": "Document content (markdown, HTML, LaTeX, or RST)."
                },
                "format": {
                    "type": "string",
                    "enum": ["markdown", "md", "html", "latex", "tex", "rst"],
                    "description": "Source content format (default: markdown).",
                    "default": "markdown"
                }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let path = input["path"]
            .as_str()
            .ok_or_else(|| AivyxError::Validation("path is required".into()))?;
        let content = input["content"]
            .as_str()
            .ok_or_else(|| AivyxError::Validation("content is required".into()))?;
        let format = input["format"].as_str().unwrap_or("markdown");

        let result = super::documents::create_pdf(path, content, format).await?;

        Ok(serde_json::json!({
            "status": "created",
            "path": path,
            "format": "pdf",
            "source_format": format,
            "detail": result,
        }))
    }
}

// ── doc_edit_text ──────────────────────────────────────────────

pub struct DocEditText {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for DocEditText {
    fn name(&self) -> &str {
        "doc_edit_text"
    }

    fn description(&self) -> &str {
        "Edit a text file with structured operations: find_replace, insert_at \
         (line number), append, prepend, or delete_lines. Works on any text \
         file — markdown, code, config, etc."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute path to the text file to edit."
                },
                "operation": {
                    "type": "string",
                    "enum": ["find_replace", "insert_at", "append", "prepend", "delete_lines"],
                    "description": "Edit operation to perform."
                },
                "find": {
                    "type": "string",
                    "description": "Text to find (for find_replace)."
                },
                "replace": {
                    "type": "string",
                    "description": "Replacement text (for find_replace)."
                },
                "all": {
                    "type": "boolean",
                    "description": "Replace all occurrences (for find_replace, default: false).",
                    "default": false
                },
                "line": {
                    "type": "integer",
                    "description": "Line number (1-based, for insert_at)."
                },
                "text": {
                    "type": "string",
                    "description": "Text to insert/append/prepend."
                },
                "from": {
                    "type": "integer",
                    "description": "Start line (1-based, for delete_lines)."
                },
                "to": {
                    "type": "integer",
                    "description": "End line (1-based, inclusive, for delete_lines)."
                }
            },
            "required": ["path", "operation"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let path = input["path"]
            .as_str()
            .ok_or_else(|| AivyxError::Validation("path is required".into()))?;
        let operation = input["operation"]
            .as_str()
            .ok_or_else(|| AivyxError::Validation("operation is required".into()))?;

        let result = super::documents::edit_text_file(path, operation, &input).await?;

        Ok(serde_json::json!({
            "status": "edited",
            "path": path,
            "operation": operation,
            "detail": result,
        }))
    }
}

// ── doc_convert ────────────────────────────────────────────────

pub struct DocConvert {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for DocConvert {
    fn name(&self) -> &str {
        "doc_convert"
    }

    fn description(&self) -> &str {
        "Convert a document between formats. Supports: markdown, HTML, LaTeX, \
         RST, DOCX, ODT, EPUB, PDF (via pandoc). Also XLSX, XLS, ODS, PPTX \
         (via LibreOffice). Format is auto-detected from file extensions."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "input_path": {
                    "type": "string",
                    "description": "Absolute path to the source document."
                },
                "output_path": {
                    "type": "string",
                    "description": "Absolute path for the converted output."
                },
                "from_format": {
                    "type": "string",
                    "description": "Source format override (auto-detected from extension if omitted)."
                },
                "to_format": {
                    "type": "string",
                    "description": "Target format override (auto-detected from extension if omitted)."
                }
            },
            "required": ["input_path", "output_path"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let input_path = input["input_path"]
            .as_str()
            .ok_or_else(|| AivyxError::Validation("input_path is required".into()))?;
        let output_path = input["output_path"]
            .as_str()
            .ok_or_else(|| AivyxError::Validation("output_path is required".into()))?;
        let from_format = input["from_format"].as_str();
        let to_format = input["to_format"].as_str();

        let result =
            super::documents::convert_document(input_path, output_path, from_format, to_format)
                .await?;

        Ok(serde_json::json!({
            "status": "converted",
            "input": input_path,
            "output": output_path,
            "detail": result,
        }))
    }
}

// ── doc_read_pdf ───────────────────────────────────────────────

pub struct DocReadPdf {
    pub ctx: Arc<InteractionContext>,
}

#[async_trait::async_trait]
impl Action for DocReadPdf {
    fn name(&self) -> &str {
        "doc_read_pdf"
    }

    fn description(&self) -> &str {
        "Extract text from a PDF file using pdftotext. More accurate than OCR \
         for actual PDF documents. Optionally read only specific pages. Output \
         is capped at 256KB."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute path to the PDF file."
                },
                "first_page": {
                    "type": "integer",
                    "description": "First page to extract (1-based, optional)."
                },
                "last_page": {
                    "type": "integer",
                    "description": "Last page to extract (1-based, optional)."
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let path = input["path"]
            .as_str()
            .ok_or_else(|| AivyxError::Validation("path is required".into()))?;
        let first_page = input["first_page"].as_u64().map(|v| v as u32);
        let last_page = input["last_page"].as_u64().map(|v| v as u32);

        let text = super::documents::read_pdf(path, first_page, last_page).await?;

        Ok(serde_json::json!({
            "path": path,
            "text": text,
            "char_count": text.len(),
        }))
    }
}

// ══════════════════════════════════════════════════════════════════
// Tests
// ══════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    fn test_ctx() -> Arc<InteractionContext> {
        InteractionContext::new(
            super::super::InteractionConfig {
                enabled: true,
                ..Default::default()
            },
            crate::desktop::DesktopConfig::default(),
        )
    }

    #[tokio::test]
    async fn ui_key_combo_rejects_empty() {
        let tool = UiKeyCombo { ctx: test_ctx() };
        let result = tool.execute(serde_json::json!({"keys": ""})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn ui_type_text_rejects_oversized() {
        let tool = UiTypeText { ctx: test_ctx() };
        let big = "x".repeat(MAX_TYPE_TEXT_BYTES + 1);
        let result = tool.execute(serde_json::json!({"text": big})).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("too large"), "error: {err}");
    }

    #[tokio::test]
    async fn ui_type_text_requires_text() {
        let tool = UiTypeText { ctx: test_ctx() };
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn ui_find_element_requires_filter() {
        let tool = UiFindElement { ctx: test_ctx() };
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("role, name, or text"), "error: {err}");
    }

    #[tokio::test]
    async fn browser_navigate_rejects_bad_scheme() {
        let tool = BrowserNavigate { ctx: test_ctx() };
        let result = tool
            .execute(serde_json::json!({"url": "javascript:alert(1)"}))
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("scheme not allowed"), "error: {err}");
    }

    #[tokio::test]
    async fn browser_screenshot_rejects_bad_format() {
        let tool = BrowserScreenshot { ctx: test_ctx() };
        let result = tool.execute(serde_json::json!({"format": "bmp"})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn browser_type_rejects_oversized() {
        let tool = BrowserType { ctx: test_ctx() };
        let big = "x".repeat(MAX_TYPE_TEXT_BYTES + 1);
        let result = tool
            .execute(serde_json::json!({"selector": "input", "text": big}))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn media_control_rejects_invalid_action() {
        let tool = MediaControl { ctx: test_ctx() };
        let result = tool.execute(serde_json::json!({"action": "rewind"})).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Invalid action"), "error: {err}");
    }

    #[tokio::test]
    async fn ui_read_text_requires_element() {
        let tool = UiReadText { ctx: test_ctx() };
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err());
    }

    // ── New tool validation tests ────────────────────────────────

    #[tokio::test]
    async fn ui_scroll_requires_direction() {
        let tool = UiScroll { ctx: test_ctx() };
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn ui_scroll_rejects_bad_direction() {
        let tool = UiScroll { ctx: test_ctx() };
        let result = tool
            .execute(serde_json::json!({"direction": "diagonal"}))
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Invalid scroll direction"), "error: {err}");
    }

    #[tokio::test]
    async fn ui_right_click_requires_target() {
        let tool = UiRightClick { ctx: test_ctx() };
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn ui_hover_requires_target() {
        let tool = UiHover { ctx: test_ctx() };
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn ui_drag_requires_coordinates_or_elements() {
        let tool = UiDrag { ctx: test_ctx() };
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn ui_mouse_move_requires_coords() {
        let tool = UiMouseMove { ctx: test_ctx() };
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn window_screenshot_rejects_bad_format() {
        let tool = WindowScreenshot { ctx: test_ctx() };
        let result = tool.execute(serde_json::json!({"format": "bmp"})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn browser_scroll_requires_direction() {
        let tool = BrowserScroll { ctx: test_ctx() };
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn browser_execute_js_requires_expression() {
        let tool = BrowserExecuteJs { ctx: test_ctx() };
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn browser_execute_js_rejects_oversized() {
        let tool = BrowserExecuteJs { ctx: test_ctx() };
        let big = "x".repeat(MAX_JS_INPUT_BYTES + 1);
        let result = tool.execute(serde_json::json!({"expression": big})).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("too large"), "error: {err}");
    }

    // ── Tier 1+2 expansion validation tests ────────────────────

    #[tokio::test]
    async fn ui_double_click_requires_target() {
        let tool = UiDoubleClick { ctx: test_ctx() };
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn ui_middle_click_requires_target() {
        let tool = UiMiddleClick { ctx: test_ctx() };
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn browser_wait_for_requires_selector() {
        let tool = BrowserWaitFor { ctx: test_ctx() };
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn browser_wait_for_rejects_long_timeout() {
        let tool = BrowserWaitFor { ctx: test_ctx() };
        let result = tool
            .execute(serde_json::json!({"selector": "#btn", "timeout": 60000}))
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("30000ms"), "error: {err}");
    }

    #[tokio::test]
    async fn browser_new_tab_rejects_bad_scheme() {
        let tool = BrowserNewTab { ctx: test_ctx() };
        let result = tool
            .execute(serde_json::json!({"url": "javascript:alert(1)"}))
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("scheme not allowed"), "error: {err}");
    }

    #[tokio::test]
    async fn window_manage_requires_action() {
        let tool = WindowManage { ctx: test_ctx() };
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn system_volume_requires_action() {
        let tool = SystemVolume { ctx: test_ctx() };
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn system_brightness_requires_action() {
        let tool = SystemBrightness { ctx: test_ctx() };
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn ui_select_option_requires_selector() {
        let tool = UiSelectOption { ctx: test_ctx() };
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn ui_clear_field_requires_selector() {
        let tool = UiClearField { ctx: test_ctx() };
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn file_manager_show_requires_path() {
        let tool = FileManagerShow { ctx: test_ctx() };
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn window_screenshot_rejects_bad_region() {
        let tool = WindowScreenshot { ctx: test_ctx() };
        let result = tool.execute(serde_json::json!({"region": "invalid"})).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("widthxheight"), "error: {err}");
    }

    // ── Tier 3+4+5 validation tests ────────────────────────────

    #[tokio::test]
    async fn screen_ocr_requires_region() {
        let tool = ScreenOcr { ctx: test_ctx() };
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn screen_ocr_rejects_bad_region() {
        let tool = ScreenOcr { ctx: test_ctx() };
        let result = tool
            .execute(serde_json::json!({"region": "not valid"}))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn screen_ocr_rejects_oversized_region() {
        let tool = ScreenOcr { ctx: test_ctx() };
        let result = tool
            .execute(serde_json::json!({"region": "0,0 3000x2000"}))
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("too large"), "error: {err}");
    }

    #[tokio::test]
    async fn desktop_workspace_requires_action() {
        let tool = DesktopWorkspace { ctx: test_ctx() };
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn desktop_workspace_switch_requires_target() {
        let tool = DesktopWorkspace { ctx: test_ctx() };
        let result = tool.execute(serde_json::json!({"action": "switch"})).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("target workspace"), "error: {err}");
    }

    #[tokio::test]
    async fn browser_find_text_requires_query() {
        let tool = BrowserFindText { ctx: test_ctx() };
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn browser_find_text_rejects_empty() {
        let tool = BrowserFindText { ctx: test_ctx() };
        let result = tool.execute(serde_json::json!({"query": ""})).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("empty"), "error: {err}");
    }

    #[tokio::test]
    async fn browser_find_text_rejects_oversized() {
        let tool = BrowserFindText { ctx: test_ctx() };
        let big = "x".repeat(1500);
        let result = tool.execute(serde_json::json!({"query": big})).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("too long"), "error: {err}");
    }

    #[tokio::test]
    async fn ui_multi_select_requires_positions() {
        let tool = UiMultiSelect { ctx: test_ctx() };
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn ui_multi_select_rejects_empty_array() {
        let tool = UiMultiSelect { ctx: test_ctx() };
        let result = tool.execute(serde_json::json!({"positions": []})).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("At least one"), "error: {err}");
    }

    #[tokio::test]
    async fn ui_multi_select_rejects_bad_format() {
        let tool = UiMultiSelect { ctx: test_ctx() };
        let result = tool
            .execute(serde_json::json!({"positions": [[100]]}))
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("[x, y]"), "error: {err}");
    }

    // ── Document tool validation tests ─────────────────────────

    #[tokio::test]
    async fn doc_create_text_requires_path() {
        let tool = DocCreateText { ctx: test_ctx() };
        let result = tool.execute(serde_json::json!({"content": "hello"})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn doc_create_text_requires_content() {
        let tool = DocCreateText { ctx: test_ctx() };
        let result = tool
            .execute(serde_json::json!({"path": "/tmp/test.txt"}))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn doc_create_spreadsheet_requires_headers() {
        let tool = DocCreateSpreadsheet { ctx: test_ctx() };
        let result = tool
            .execute(serde_json::json!({
                "path": "/tmp/test.csv",
                "rows": [["a", "b"]]
            }))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn doc_create_spreadsheet_rejects_empty_headers() {
        let tool = DocCreateSpreadsheet { ctx: test_ctx() };
        let result = tool
            .execute(serde_json::json!({
                "path": "/tmp/test.csv",
                "headers": [],
                "rows": []
            }))
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("empty"), "error: {err}");
    }

    #[tokio::test]
    async fn doc_create_pdf_requires_path() {
        let tool = DocCreatePdf { ctx: test_ctx() };
        let result = tool
            .execute(serde_json::json!({"content": "# Hello"}))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn doc_edit_text_requires_operation() {
        let tool = DocEditText { ctx: test_ctx() };
        let result = tool
            .execute(serde_json::json!({"path": "/tmp/test.txt"}))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn doc_convert_requires_paths() {
        let tool = DocConvert { ctx: test_ctx() };
        let result = tool
            .execute(serde_json::json!({"input_path": "/tmp/test.md"}))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn doc_read_pdf_requires_path() {
        let tool = DocReadPdf { ctx: test_ctx() };
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn doc_read_pdf_rejects_nonexistent() {
        let tool = DocReadPdf { ctx: test_ctx() };
        let result = tool
            .execute(serde_json::json!({"path": "/tmp/nonexistent_aivyx_test.pdf"}))
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("does not exist"), "error: {err}");
    }
}
