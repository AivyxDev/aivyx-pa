#![allow(unsafe_op_in_unsafe_fn, unused_imports, unreachable_code, unused_variables, dead_code, clippy::all)]
//! Chrome DevTools Protocol backend — browser automation for Chromium-based browsers.
//!
//! Connects to Chrome/Chromium via the remote debugging port (default: 9222).
//! Provides DOM queries, navigation, form filling, screenshots, and page reading.
//!
//! The browser must be launched with `--remote-debugging-port=9222`.
//! Works on both X11 and Wayland.

use aivyx_core::{AivyxError, Result};

use super::{
    ElementQuery, MAX_JS_INPUT_BYTES, MAX_SCREENSHOT_BYTES, ScrollDirection, UiBackend, UiElement,
    UiTreeNode, WindowRef,
};

/// CDP-based browser automation backend.
pub struct CdpBackend {
    /// Chrome remote debugging port.
    debug_port: u16,
}

impl CdpBackend {
    pub fn new(debug_port: u16) -> Self {
        Self { debug_port }
    }

    /// Connect to Chrome and get the list of available pages/tabs.
    async fn list_tabs(&self) -> Result<Vec<CdpTab>> {
        let url = format!("http://127.0.0.1:{}/json", self.debug_port);
        let resp = reqwest::get(&url).await.map_err(|e| {
            AivyxError::Other(format!(
                "Cannot connect to Chrome on port {}. Launch Chrome with \
                 --remote-debugging-port={}. Error: {e}",
                self.debug_port, self.debug_port
            ))
        })?;

        let tabs: Vec<CdpTab> = resp
            .json()
            .await
            .map_err(|e| AivyxError::Other(format!("Failed to parse Chrome tab list: {e}")))?;

        Ok(tabs.into_iter().filter(|t| t.tab_type == "page").collect())
    }

    /// Get the WebSocket debugger URL for a tab by index.
    async fn tab_ws_url(&self, tab_index: usize) -> Result<String> {
        let tabs = self.list_tabs().await?;
        let tab = tabs.get(tab_index).ok_or_else(|| {
            AivyxError::Other(format!(
                "Tab index {tab_index} out of range (found {} tabs)",
                tabs.len()
            ))
        })?;
        tab.web_socket_debugger_url.clone().ok_or_else(|| {
            AivyxError::Other(format!("Tab '{}' has no WebSocket debugger URL", tab.title))
        })
    }

    /// Navigate a tab to a URL.
    pub async fn navigate(&self, url: &str, tab_index: usize) -> Result<String> {
        validate_url(url)?;
        let ws_url = self.tab_ws_url(tab_index).await?;
        // Send Page.navigate via CDP WebSocket.
        let result = cdp_send(&ws_url, "Page.navigate", &serde_json::json!({"url": url})).await?;
        Ok(format!(
            "Navigated tab {tab_index} to {url}. Frame: {result}"
        ))
    }

    /// Query DOM elements by CSS selector.
    pub async fn query_selector(
        &self,
        selector: &str,
        tab_index: usize,
    ) -> Result<serde_json::Value> {
        let ws_url = self.tab_ws_url(tab_index).await?;

        // Get the document root.
        let doc = cdp_send(&ws_url, "DOM.getDocument", &serde_json::json!({})).await?;
        let root_id = doc["root"]["nodeId"]
            .as_i64()
            .ok_or_else(|| AivyxError::Other("DOM.getDocument: missing root nodeId".into()))?;

        // Query all matching elements.
        let result = cdp_send(
            &ws_url,
            "DOM.querySelectorAll",
            &serde_json::json!({"nodeId": root_id, "selector": selector}),
        )
        .await?;

        let node_ids = result["nodeIds"].as_array().cloned().unwrap_or_default();

        // Resolve each node to get its outer HTML and text.
        let mut elements = Vec::new();
        for node_id in node_ids.iter().take(50) {
            let html = cdp_send(
                &ws_url,
                "DOM.getOuterHTML",
                &serde_json::json!({"nodeId": node_id}),
            )
            .await;
            if let Ok(h) = html {
                elements.push(serde_json::json!({
                    "nodeId": node_id,
                    "outerHTML": truncate_str(
                        h["outerHTML"].as_str().unwrap_or(""),
                        500,
                    ),
                }));
            }
        }

        Ok(serde_json::json!({
            "selector": selector,
            "count": elements.len(),
            "elements": elements,
        }))
    }

    /// Click a DOM element by CSS selector.
    pub async fn click_selector(&self, selector: &str, tab_index: usize) -> Result<String> {
        let ws_url = self.tab_ws_url(tab_index).await?;

        // Use Runtime.evaluate to click via JS (most reliable cross-browser).
        let js = format!(
            r#"(() => {{
                const el = document.querySelector({selector});
                if (!el) return 'Element not found: {raw_selector}';
                el.scrollIntoView({{block: 'center'}});
                el.click();
                return 'Clicked: ' + (el.textContent || el.tagName).substring(0, 100);
            }})()"#,
            selector = serde_json::to_string(selector).unwrap_or_default(),
            raw_selector = selector.replace('\'', "\\'"),
        );

        let result = cdp_send(
            &ws_url,
            "Runtime.evaluate",
            &serde_json::json!({"expression": js, "returnByValue": true}),
        )
        .await?;

        let value = result["result"]["value"]
            .as_str()
            .unwrap_or("click executed");
        Ok(value.to_string())
    }

    /// Type text into a focused element or one matched by selector.
    pub async fn type_text(&self, selector: &str, text: &str, tab_index: usize) -> Result<String> {
        let ws_url = self.tab_ws_url(tab_index).await?;

        // Focus the element first.
        let focus_js = format!(
            r#"(() => {{
                const el = document.querySelector({selector});
                if (!el) return 'not_found';
                el.focus();
                return 'focused';
            }})()"#,
            selector = serde_json::to_string(selector).unwrap_or_default(),
        );

        let focus_result = cdp_send(
            &ws_url,
            "Runtime.evaluate",
            &serde_json::json!({"expression": focus_js, "returnByValue": true}),
        )
        .await?;

        if focus_result["result"]["value"].as_str() == Some("not_found") {
            return Err(AivyxError::Other(format!("Element not found: {selector}")));
        }

        // Type each character via Input.dispatchKeyEvent.
        for ch in text.chars() {
            cdp_send(
                &ws_url,
                "Input.dispatchKeyEvent",
                &serde_json::json!({
                    "type": "keyDown",
                    "text": ch.to_string(),
                }),
            )
            .await?;
            cdp_send(
                &ws_url,
                "Input.dispatchKeyEvent",
                &serde_json::json!({
                    "type": "keyUp",
                    "text": ch.to_string(),
                }),
            )
            .await?;
        }

        Ok(format!("Typed {} characters into '{selector}'", text.len()))
    }

    /// Read visible text from the page or a specific element.
    pub async fn read_page(&self, selector: Option<&str>, tab_index: usize) -> Result<String> {
        let ws_url = self.tab_ws_url(tab_index).await?;

        let js = if let Some(sel) = selector {
            format!(
                r#"(() => {{
                    const el = document.querySelector({sel});
                    return el ? el.innerText : 'Element not found: {raw}';
                }})()"#,
                sel = serde_json::to_string(sel).unwrap_or_default(),
                raw = sel.replace('\'', "\\'"),
            )
        } else {
            "document.body.innerText".to_string()
        };

        let result = cdp_send(
            &ws_url,
            "Runtime.evaluate",
            &serde_json::json!({"expression": js, "returnByValue": true}),
        )
        .await?;

        let text = result["result"]["value"].as_str().unwrap_or("").to_string();

        // Cap at 64KB.
        Ok(truncate_str(&text, 64 * 1024).to_string())
    }

    /// Take a screenshot of the page.
    pub async fn screenshot(&self, format: &str, tab_index: usize) -> Result<String> {
        let ws_url = self.tab_ws_url(tab_index).await?;

        let result = cdp_send(
            &ws_url,
            "Page.captureScreenshot",
            &serde_json::json!({"format": format}),
        )
        .await?;

        let data = result["data"]
            .as_str()
            .ok_or_else(|| AivyxError::Other("Screenshot: no data returned".into()))?;

        if data.len() > MAX_SCREENSHOT_BYTES {
            return Err(AivyxError::Other(format!(
                "Screenshot too large ({} bytes, max {})",
                data.len(),
                MAX_SCREENSHOT_BYTES
            )));
        }

        Ok(data.to_string())
    }

    /// Scroll the page or a specific element.
    pub async fn scroll_page(
        &self,
        direction: ScrollDirection,
        amount: u32,
        selector: Option<&str>,
        tab_index: usize,
    ) -> Result<String> {
        let ws_url = self.tab_ws_url(tab_index).await?;
        let pixels = amount as i32 * 100; // Convert scroll units to pixels.

        let (dx, dy) = match direction {
            ScrollDirection::Up => (0, -pixels),
            ScrollDirection::Down => (0, pixels),
            ScrollDirection::Left => (-pixels, 0),
            ScrollDirection::Right => (pixels, 0),
        };

        let js = if let Some(sel) = selector {
            format!(
                r#"(() => {{
                    const el = document.querySelector({sel});
                    if (!el) return 'Element not found: {raw}';
                    el.scrollBy({dx}, {dy});
                    return 'Scrolled element by ({dx}, {dy})';
                }})()"#,
                sel = serde_json::to_string(sel).unwrap_or_default(),
                raw = sel.replace('\'', "\\'"),
                dx = dx,
                dy = dy,
            )
        } else {
            format!(
                r#"(() => {{
                    window.scrollBy({dx}, {dy});
                    return 'Scrolled page by ({dx}, {dy})';
                }})()"#,
                dx = dx,
                dy = dy,
            )
        };

        let result = cdp_send(
            &ws_url,
            "Runtime.evaluate",
            &serde_json::json!({"expression": js, "returnByValue": true}),
        )
        .await?;

        Ok(result["result"]["value"]
            .as_str()
            .unwrap_or("scroll executed")
            .to_string())
    }

    /// Right-click an element by CSS selector.
    pub async fn right_click_selector(&self, selector: &str, tab_index: usize) -> Result<String> {
        let ws_url = self.tab_ws_url(tab_index).await?;

        // Get element position via JS.
        let js = format!(
            r#"(() => {{
                const el = document.querySelector({sel});
                if (!el) return null;
                el.scrollIntoView({{block: 'center'}});
                const r = el.getBoundingClientRect();
                return JSON.stringify({{x: r.x + r.width/2, y: r.y + r.height/2}});
            }})()"#,
            sel = serde_json::to_string(selector).unwrap_or_default(),
        );

        let result = cdp_send(
            &ws_url,
            "Runtime.evaluate",
            &serde_json::json!({"expression": js, "returnByValue": true}),
        )
        .await?;

        let pos_str = result["result"]["value"]
            .as_str()
            .ok_or_else(|| AivyxError::Other(format!("Element not found: {selector}")))?;

        let pos: serde_json::Value = serde_json::from_str(pos_str)
            .map_err(|e| AivyxError::Other(format!("Position parse error: {e}")))?;

        let x = pos["x"].as_f64().unwrap_or(0.0);
        let y = pos["y"].as_f64().unwrap_or(0.0);

        // Dispatch right-click events: mousePressed + mouseReleased with button=right.
        cdp_send(
            &ws_url,
            "Input.dispatchMouseEvent",
            &serde_json::json!({
                "type": "mousePressed",
                "x": x,
                "y": y,
                "button": "right",
                "clickCount": 1,
            }),
        )
        .await?;

        cdp_send(
            &ws_url,
            "Input.dispatchMouseEvent",
            &serde_json::json!({
                "type": "mouseReleased",
                "x": x,
                "y": y,
                "button": "right",
                "clickCount": 1,
            }),
        )
        .await?;

        Ok(format!("Right-clicked at ({x}, {y})"))
    }

    /// Hover over an element by CSS selector.
    pub async fn hover_selector(&self, selector: &str, tab_index: usize) -> Result<String> {
        let ws_url = self.tab_ws_url(tab_index).await?;

        let js = format!(
            r#"(() => {{
                const el = document.querySelector({sel});
                if (!el) return null;
                el.scrollIntoView({{block: 'center'}});
                const r = el.getBoundingClientRect();
                return JSON.stringify({{x: r.x + r.width/2, y: r.y + r.height/2}});
            }})()"#,
            sel = serde_json::to_string(selector).unwrap_or_default(),
        );

        let result = cdp_send(
            &ws_url,
            "Runtime.evaluate",
            &serde_json::json!({"expression": js, "returnByValue": true}),
        )
        .await?;

        let pos_str = result["result"]["value"]
            .as_str()
            .ok_or_else(|| AivyxError::Other(format!("Element not found: {selector}")))?;

        let pos: serde_json::Value = serde_json::from_str(pos_str)
            .map_err(|e| AivyxError::Other(format!("Position parse error: {e}")))?;

        let x = pos["x"].as_f64().unwrap_or(0.0);
        let y = pos["y"].as_f64().unwrap_or(0.0);

        cdp_send(
            &ws_url,
            "Input.dispatchMouseEvent",
            &serde_json::json!({
                "type": "mouseMoved",
                "x": x,
                "y": y,
            }),
        )
        .await?;

        Ok(format!("Hovered at ({x}, {y})"))
    }

    /// Execute arbitrary JavaScript in a browser tab.
    pub async fn execute_js(
        &self,
        expression: &str,
        tab_index: usize,
    ) -> Result<serde_json::Value> {
        if expression.len() > MAX_JS_INPUT_BYTES {
            return Err(AivyxError::Validation(format!(
                "JavaScript expression too large ({} bytes, max {MAX_JS_INPUT_BYTES})",
                expression.len()
            )));
        }

        let ws_url = self.tab_ws_url(tab_index).await?;

        let result = cdp_send(
            &ws_url,
            "Runtime.evaluate",
            &serde_json::json!({
                "expression": expression,
                "returnByValue": true,
                "awaitPromise": true,
            }),
        )
        .await?;

        if let Some(exception) = result.get("exceptionDetails") {
            let msg = exception["text"]
                .as_str()
                .or_else(|| exception["exception"]["description"].as_str())
                .unwrap_or("unknown error");
            return Err(AivyxError::Other(format!("JS error: {msg}")));
        }

        Ok(result
            .get("result")
            .cloned()
            .unwrap_or(serde_json::Value::Null))
    }

    /// List all open browser tabs with their titles and URLs.
    pub async fn list_tabs_info(&self) -> Result<serde_json::Value> {
        let tabs = self.list_tabs().await?;
        let items: Vec<serde_json::Value> = tabs
            .iter()
            .enumerate()
            .map(|(i, t)| {
                serde_json::json!({
                    "index": i,
                    "title": t.title,
                    "url": t.url,
                })
            })
            .collect();
        Ok(serde_json::Value::Array(items))
    }

    /// Open a new browser tab with the given URL.
    pub async fn new_tab(&self, url: &str) -> Result<String> {
        // Chrome's /json/new endpoint creates a tab.
        let endpoint = format!("http://127.0.0.1:{}/json/new?{}", self.debug_port, url);
        let resp = reqwest::get(&endpoint)
            .await
            .map_err(|e| AivyxError::Other(format!("Failed to create new tab: {e}")))?;
        let tab: CdpTab = resp
            .json()
            .await
            .map_err(|e| AivyxError::Other(format!("Failed to parse new tab response: {e}")))?;
        Ok(format!("Opened new tab: {}", tab.title))
    }

    /// Close a browser tab by index.
    pub async fn close_tab(&self, tab_index: usize) -> Result<()> {
        let tabs = self.list_tabs().await?;
        let tab = tabs.get(tab_index).ok_or_else(|| {
            AivyxError::Other(format!(
                "Tab index {tab_index} out of range (found {} tabs)",
                tabs.len()
            ))
        })?;

        let endpoint = format!("http://127.0.0.1:{}/json/close/{}", self.debug_port, tab.id);
        reqwest::get(&endpoint)
            .await
            .map_err(|e| AivyxError::Other(format!("Failed to close tab: {e}")))?;
        Ok(())
    }

    /// Wait for a CSS selector to appear in the DOM. Polls every 200ms.
    pub async fn wait_for_selector(
        &self,
        selector: &str,
        timeout_ms: u64,
        tab_index: usize,
    ) -> Result<bool> {
        let ws_url = self.tab_ws_url(tab_index).await?;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);

        let js = format!(
            r#"!!document.querySelector({sel})"#,
            sel = serde_json::to_string(selector).unwrap_or_default(),
        );

        loop {
            let result = cdp_send(
                &ws_url,
                "Runtime.evaluate",
                &serde_json::json!({"expression": js, "returnByValue": true}),
            )
            .await?;

            if result["result"]["value"].as_bool() == Some(true) {
                return Ok(true);
            }

            if tokio::time::Instant::now() >= deadline {
                return Ok(false);
            }

            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    }

    /// Select an option in a `<select>` element by value or visible text.
    pub async fn select_option(
        &self,
        selector: &str,
        value: Option<&str>,
        text: Option<&str>,
        tab_index: usize,
    ) -> Result<String> {
        let ws_url = self.tab_ws_url(tab_index).await?;

        let match_logic = if let Some(val) = value {
            format!(
                "opt.value === {}",
                serde_json::to_string(val).unwrap_or_default()
            )
        } else if let Some(txt) = text {
            format!(
                "opt.textContent.trim() === {}",
                serde_json::to_string(txt).unwrap_or_default()
            )
        } else {
            return Err(AivyxError::Validation(
                "Either value or text is required for select_option".into(),
            ));
        };

        let js = format!(
            r#"(() => {{
                const sel = document.querySelector({selector});
                if (!sel || sel.tagName !== 'SELECT') return 'not_a_select';
                for (const opt of sel.options) {{
                    if ({match_logic}) {{
                        sel.value = opt.value;
                        sel.dispatchEvent(new Event('change', {{bubbles: true}}));
                        return 'Selected: ' + opt.textContent.trim();
                    }}
                }}
                return 'option_not_found';
            }})()"#,
            selector = serde_json::to_string(selector).unwrap_or_default(),
            match_logic = match_logic,
        );

        let result = cdp_send(
            &ws_url,
            "Runtime.evaluate",
            &serde_json::json!({"expression": js, "returnByValue": true}),
        )
        .await?;

        let val = result["result"]["value"].as_str().unwrap_or("unknown");

        if val == "not_a_select" {
            return Err(AivyxError::Other(format!(
                "Element '{selector}' is not a <select>"
            )));
        }
        if val == "option_not_found" {
            return Err(AivyxError::Other("Option not found in dropdown".into()));
        }

        Ok(val.to_string())
    }

    /// Clear a form field (select all + delete via JS).
    pub async fn clear_field(&self, selector: &str, tab_index: usize) -> Result<String> {
        let ws_url = self.tab_ws_url(tab_index).await?;

        let js = format!(
            r#"(() => {{
                const el = document.querySelector({sel});
                if (!el) return 'not_found';
                el.focus();
                if ('value' in el) {{
                    el.value = '';
                    el.dispatchEvent(new Event('input', {{bubbles: true}}));
                    el.dispatchEvent(new Event('change', {{bubbles: true}}));
                    return 'cleared';
                }}
                if (el.isContentEditable) {{
                    el.textContent = '';
                    el.dispatchEvent(new Event('input', {{bubbles: true}}));
                    return 'cleared';
                }}
                return 'unsupported';
            }})()"#,
            sel = serde_json::to_string(selector).unwrap_or_default(),
        );

        let result = cdp_send(
            &ws_url,
            "Runtime.evaluate",
            &serde_json::json!({"expression": js, "returnByValue": true}),
        )
        .await?;

        let val = result["result"]["value"].as_str().unwrap_or("unknown");

        if val == "not_found" {
            return Err(AivyxError::Other(format!("Element not found: {selector}")));
        }

        Ok(val.to_string())
    }

    /// Save the current page as PDF. Returns base64-encoded PDF data.
    pub async fn print_to_pdf(&self, tab_index: usize, landscape: bool) -> Result<String> {
        let ws_url = self.tab_ws_url(tab_index).await?;

        let result = cdp_send(
            &ws_url,
            "Page.printToPDF",
            &serde_json::json!({
                "landscape": landscape,
                "printBackground": true,
                "preferCSSPageSize": true,
            }),
        )
        .await?;

        let data = result["data"]
            .as_str()
            .ok_or_else(|| AivyxError::Other("printToPDF: no data returned".into()))?;

        if data.len() > MAX_SCREENSHOT_BYTES * 4 {
            return Err(AivyxError::Other(format!(
                "PDF too large ({} bytes). Try a shorter page.",
                data.len()
            )));
        }

        Ok(data.to_string())
    }

    /// Find text on the page. Returns match count and highlights.
    pub async fn find_text_on_page(
        &self,
        query: &str,
        tab_index: usize,
    ) -> Result<serde_json::Value> {
        let ws_url = self.tab_ws_url(tab_index).await?;

        let js = format!(
            r#"(() => {{
                const text = {query};
                const body = document.body.innerText;
                const lower = body.toLowerCase();
                const target = text.toLowerCase();
                let count = 0;
                let pos = 0;
                const positions = [];
                while ((pos = lower.indexOf(target, pos)) !== -1) {{
                    count++;
                    // Get surrounding context (40 chars each side).
                    const start = Math.max(0, pos - 40);
                    const end = Math.min(body.length, pos + target.length + 40);
                    if (positions.length < 10) {{
                        positions.push({{
                            index: pos,
                            context: body.substring(start, end),
                        }});
                    }}
                    pos += target.length;
                }}
                return JSON.stringify({{ count, matches: positions }});
            }})()"#,
            query = serde_json::to_string(query).unwrap_or_default(),
        );

        let result = cdp_send(
            &ws_url,
            "Runtime.evaluate",
            &serde_json::json!({"expression": js, "returnByValue": true}),
        )
        .await?;

        let val_str = result["result"]["value"]
            .as_str()
            .unwrap_or(r#"{"count":0,"matches":[]}"#);

        let parsed: serde_json::Value =
            serde_json::from_str(val_str).unwrap_or(serde_json::json!({"count": 0, "matches": []}));

        Ok(parsed)
    }
}

#[async_trait::async_trait]
impl UiBackend for CdpBackend {
    fn name(&self) -> &str {
        "cdp"
    }

    async fn inspect(&self, _window: &WindowRef, _max_depth: u32) -> Result<Vec<UiTreeNode>> {
        Err(AivyxError::Other(
            "CDP inspect: use browser_query with a CSS selector instead".into(),
        ))
    }

    async fn find_element(
        &self,
        _window: &WindowRef,
        query: &ElementQuery,
    ) -> Result<Vec<UiElement>> {
        // Map ElementQuery to a CSS selector.
        let selector = if let Some(ref name) = query.name {
            format!("[aria-label*='{name}'], [title*='{name}'], [placeholder*='{name}']")
        } else if let Some(ref role) = query.role {
            format!("[role='{role}']")
        } else {
            return Err(AivyxError::Other(
                "CDP find_element requires name or role".into(),
            ));
        };

        let result = self.query_selector(&selector, 0).await?;
        let elements = result["elements"].as_array().cloned().unwrap_or_default();

        Ok(elements
            .iter()
            .enumerate()
            .map(|(i, el)| UiElement {
                path: format!("cdp/{i}"),
                role: query.role.clone().unwrap_or_default(),
                name: query.name.clone().unwrap_or_default(),
                text: el["outerHTML"].as_str().map(|s| s.to_string()),
                bounds: None,
                states: Vec::new(),
            })
            .collect())
    }

    async fn click(&self, element: &UiElement) -> Result<()> {
        // Use the element name as a selector hint.
        let selector = if !element.name.is_empty() {
            format!(
                "[aria-label*='{}'], [title*='{}']",
                element.name, element.name
            )
        } else {
            return Err(AivyxError::Other(
                "CDP click: element has no name to use as selector".into(),
            ));
        };
        self.click_selector(&selector, 0).await?;
        Ok(())
    }

    async fn type_text(&self, element: Option<&UiElement>, text: &str) -> Result<()> {
        let selector = if let Some(el) = element {
            if !el.name.is_empty() {
                format!("[aria-label*='{}'], [name*='{}']", el.name, el.name)
            } else {
                ":focus".to_string()
            }
        } else {
            ":focus".to_string()
        };
        self.type_text(&selector, text, 0).await?;
        Ok(())
    }

    async fn read_text(&self, element: &UiElement) -> Result<String> {
        if let Some(ref text) = element.text {
            Ok(text.clone())
        } else {
            self.read_page(None, 0).await
        }
    }

    async fn scroll(
        &self,
        _window: &WindowRef,
        direction: ScrollDirection,
        amount: u32,
    ) -> Result<()> {
        self.scroll_page(direction, amount, None, 0).await?;
        Ok(())
    }
}

// ── CDP WebSocket transport ──────────────────────────────────────

/// Tab info from Chrome's /json endpoint.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct CdpTab {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(rename = "type")]
    tab_type: String,
    #[serde(default)]
    web_socket_debugger_url: Option<String>,
    /// Internal Chrome tab ID (from /json endpoint "id" field).
    #[serde(default)]
    id: String,
}

/// Send a CDP command over WebSocket and return the result.
///
/// Uses a simple one-shot WebSocket connection per command. This is not
/// optimal for throughput but keeps the implementation stateless and avoids
/// connection management complexity.
#[cfg(feature = "browser-automation")]
async fn cdp_send(
    ws_url: &str,
    method: &str,
    params: &serde_json::Value,
) -> Result<serde_json::Value> {
    use tokio_tungstenite::{connect_async, tungstenite::Message};

    let (mut ws, _) =
        tokio::time::timeout(std::time::Duration::from_secs(5), connect_async(ws_url))
            .await
            .map_err(|_| AivyxError::Other("CDP WebSocket connect timed out".into()))?
            .map_err(|e| AivyxError::Other(format!("CDP WebSocket connect failed: {e}")))?;

    use futures_util::{SinkExt, StreamExt};

    let msg = serde_json::json!({
        "id": 1,
        "method": method,
        "params": params,
    });

    ws.send(Message::Text(msg.to_string().into()))
        .await
        .map_err(|e| AivyxError::Other(format!("CDP send failed: {e}")))?;

    // Read responses until we get our result (id=1).
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let msg = tokio::time::timeout_at(deadline, ws.next())
            .await
            .map_err(|_| AivyxError::Other(format!("CDP {method} timed out")))?
            .ok_or_else(|| AivyxError::Other("CDP WebSocket closed unexpectedly".into()))?
            .map_err(|e| AivyxError::Other(format!("CDP read error: {e}")))?;

        if let Message::Text(text) = msg {
            let resp: serde_json::Value = serde_json::from_str(&text)
                .map_err(|e| AivyxError::Other(format!("CDP parse error: {e}")))?;

            if resp.get("id").and_then(|v| v.as_i64()) == Some(1) {
                if let Some(error) = resp.get("error") {
                    return Err(AivyxError::Other(format!(
                        "CDP {method} error: {}",
                        error["message"].as_str().unwrap_or("unknown")
                    )));
                }
                return Ok(resp
                    .get("result")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null));
            }
            // Otherwise it's an event — skip it.
        }
    }
}

/// Stub when browser-automation feature is not enabled.
#[cfg(not(feature = "browser-automation"))]
async fn cdp_send(
    _ws_url: &str,
    _method: &str,
    _params: &serde_json::Value,
) -> Result<serde_json::Value> {
    Err(AivyxError::Other(
        "Browser automation requires the 'browser-automation' Cargo feature. \
         Build with: cargo build --features browser-automation"
            .into(),
    ))
}

// ── Helpers ──────────────────────────────────────────────────────

/// Validate a URL for browser navigation.
fn validate_url(url: &str) -> Result<()> {
    if super::ALLOWED_URL_SCHEMES
        .iter()
        .any(|s| url.starts_with(s))
    {
        Ok(())
    } else {
        Err(AivyxError::Other(format!(
            "URL scheme not allowed: {url}. Only http://, https://, and file:// are permitted."
        )))
    }
}

/// Truncate a string to max_len, appending "..." if truncated.
fn truncate_str(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        s
    } else {
        // Find a valid UTF-8 boundary.
        let mut end = max_len;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        &s[..end]
    }
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_url_allowed() {
        assert!(validate_url("https://example.com").is_ok());
        assert!(validate_url("http://localhost:3000").is_ok());
        assert!(validate_url("file:///home/user/doc.html").is_ok());
    }

    #[test]
    fn validate_url_blocked() {
        assert!(validate_url("javascript:alert(1)").is_err());
        assert!(validate_url("ftp://evil.com").is_err());
        assert!(validate_url("data:text/html,<script>").is_err());
    }

    #[test]
    fn truncate_str_short() {
        assert_eq!(truncate_str("hello", 10), "hello");
    }

    #[test]
    fn truncate_str_long() {
        let s = "a".repeat(1000);
        assert_eq!(truncate_str(&s, 100).len(), 100);
    }

    #[test]
    fn backend_name() {
        let backend = CdpBackend::new(9222);
        assert_eq!(UiBackend::name(&backend), "cdp");
    }

    #[tokio::test]
    async fn execute_js_rejects_oversized() {
        let backend = CdpBackend::new(9222);
        let big = "x".repeat(super::MAX_JS_INPUT_BYTES + 1);
        let result = backend.execute_js(&big, 0).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("too large"), "error: {err}");
    }
}
