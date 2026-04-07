//! Windows UI Automation backend — semantic UI automation for native Win32/WPF/UWP apps.
//!
//! Uses the Win32 UI Automation COM API via the `windows` crate to traverse the
//! accessibility tree, find elements by role/name/text, and perform actions
//! (click, type, read). This is the Windows equivalent of `atspi.rs` on Linux.
//!
//! Requires the `windows-automation` Cargo feature.
//!
//! Architecture: UIA provides element location and semantic actions (Invoke,
//! SetValue, etc.). For operations without native UIA support (right-click,
//! hover, drag), we resolve element bounds via UIA then delegate to
//! `WinInputBackend` for input injection — the same pattern AT-SPI2 uses
//! with ydotool on Linux.

use aivyx_core::{AivyxError, Result};

use super::{ElementQuery, ScrollDirection, UiBackend, UiElement, UiTreeNode, WindowRef};
use super::win_input::WinInputBackend;

#[cfg(target_os = "windows")]
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationCondition, IUIAutomationElement,
    IUIAutomationElementArray, IUIAutomationInvokePattern, IUIAutomationValuePattern,
    IUIAutomationScrollPattern,
    TreeScope_Children, TreeScope_Subtree, TreeScope_Element,
    UIA_InvokePatternId, UIA_ValuePatternId, UIA_ScrollPatternId,
    UIA_ButtonControlTypeId, UIA_CheckBoxControlTypeId, UIA_ComboBoxControlTypeId,
    UIA_EditControlTypeId, UIA_HyperlinkControlTypeId, UIA_ListItemControlTypeId,
    UIA_MenuItemControlTypeId, UIA_RadioButtonControlTypeId, UIA_TabItemControlTypeId,
    UIA_TextControlTypeId, UIA_TreeItemControlTypeId, UIA_WindowControlTypeId,
    UIA_DocumentControlTypeId, UIA_GroupControlTypeId, UIA_HeaderControlTypeId,
    UIA_ImageControlTypeId, UIA_ListControlTypeId, UIA_MenuBarControlTypeId,
    UIA_MenuControlTypeId, UIA_PaneControlTypeId, UIA_ProgressBarControlTypeId,
    UIA_ScrollBarControlTypeId, UIA_SliderControlTypeId, UIA_SpinnerControlTypeId,
    UIA_SplitButtonControlTypeId, UIA_StatusBarControlTypeId, UIA_TabControlTypeId,
    UIA_TableControlTypeId, UIA_ToolBarControlTypeId, UIA_ToolTipControlTypeId,
    UIA_TreeControlTypeId,
};
#[cfg(target_os = "windows")]
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
#[cfg(target_os = "windows")]
use windows::core::Interface;

/// Windows UI Automation backend.
///
/// Stores a `WinInputBackend` for operations that UIA can locate (via element
/// bounds) but can't perform natively — right-click, hover, drag, mouse movement.
pub struct UiaBackend {
    #[cfg(target_os = "windows")]
    automation: IUIAutomation,
    /// Input backend for fallback operations (right-click, hover, drag).
    win_input: WinInputBackend,
}

impl UiaBackend {
    /// Initialize the UI Automation COM interface.
    pub fn new() -> Result<Self> {
        #[cfg(target_os = "windows")]
        {
            let automation: IUIAutomation = unsafe {
                CoCreateInstance(&CUIAutomation, None, CLSCTX_ALL)
                    .map_err(|e| AivyxError::Other(format!("UIA CoCreateInstance failed: {e}")))?
            };
            Ok(Self {
                automation,
                win_input: WinInputBackend::new(),
            })
        }
        #[cfg(not(target_os = "windows"))]
        {
            Err(AivyxError::Other("UIA is only available on Windows".into()))
        }
    }

    /// Get the root element (desktop).
    #[cfg(target_os = "windows")]
    fn root_element(&self) -> Result<IUIAutomationElement> {
        unsafe {
            self.automation
                .GetRootElement()
                .map_err(|e| AivyxError::Other(format!("UIA GetRootElement: {e}")))
        }
    }

    /// Find the window element matching a WindowRef.
    #[cfg(target_os = "windows")]
    fn find_window(&self, window: &WindowRef) -> Result<IUIAutomationElement> {
        let root = self.root_element()?;
        match window {
            WindowRef::Active => unsafe {
                self.automation
                    .GetFocusedElement()
                    .map_err(|e| AivyxError::Other(format!("UIA GetFocusedElement: {e}")))
            },
            WindowRef::Title(title) => {
                // Search for a window with matching name.
                let condition = self.name_condition(title)?;
                unsafe {
                    root.FindFirst(TreeScope_Children, &condition)
                        .map_err(|e| AivyxError::Other(format!("UIA FindFirst: {e}")))
                }
            }
            WindowRef::Id(_) => {
                Err(AivyxError::Other("UIA: window lookup by ID not supported".into()))
            }
        }
    }

    /// Create a property condition for Name matching.
    #[cfg(target_os = "windows")]
    fn name_condition(&self, name: &str) -> Result<IUIAutomationCondition> {
        use windows::Win32::UI::Accessibility::UIA_NamePropertyId;
        use windows::core::BSTR;
        use windows::Win32::System::Variant::VARIANT;

        let bstr = BSTR::from(name);
        let variant = VARIANT::from(bstr);
        unsafe {
            self.automation
                .CreatePropertyCondition(UIA_NamePropertyId, &variant)
                .map_err(|e| AivyxError::Other(format!("UIA CreatePropertyCondition: {e}")))
        }
    }

    /// Create a "true" condition that matches all elements.
    #[cfg(target_os = "windows")]
    fn true_condition(&self) -> Result<IUIAutomationCondition> {
        unsafe {
            self.automation
                .CreateTrueCondition()
                .map_err(|e| AivyxError::Other(format!("UIA CreateTrueCondition: {e}")))
        }
    }

    /// Convert a UIA element to our UiElement representation.
    #[cfg(target_os = "windows")]
    fn element_to_ui(element: &IUIAutomationElement, path: &str) -> UiElement {
        let name = unsafe { element.CurrentName() }
            .map(|s| s.to_string())
            .unwrap_or_default();

        let control_type = unsafe { element.CurrentControlType() }.unwrap_or(0);
        let role = control_type_to_role(control_type);

        let bounds = unsafe { element.CurrentBoundingRectangle() }.ok().map(|r| {
            [r.left, r.top, r.right - r.left, r.bottom - r.top]
        });

        let is_enabled = unsafe { element.CurrentIsEnabled() }.unwrap_or(true.into());
        let has_focus = unsafe { element.CurrentHasKeyboardFocus() }.unwrap_or(false.into());

        let mut states = Vec::new();
        if has_focus.as_bool() {
            states.push("focused".to_string());
        }
        if !is_enabled.as_bool() {
            states.push("disabled".to_string());
        }

        // Try to get text content via Value pattern.
        let text = unsafe {
            element
                .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
                .ok()
                .and_then(|vp| vp.CurrentValue().ok())
                .map(|s| s.to_string())
        };

        UiElement {
            path: path.to_string(),
            role,
            name,
            text,
            bounds,
            states,
        }
    }

    /// Build the UI tree recursively.
    #[cfg(target_os = "windows")]
    fn build_tree(
        &self,
        element: &IUIAutomationElement,
        path_prefix: &str,
        depth: u32,
        max_depth: u32,
    ) -> Result<Vec<UiTreeNode>> {
        if depth >= max_depth {
            return Ok(Vec::new());
        }

        let condition = self.true_condition()?;
        let children: IUIAutomationElementArray = unsafe {
            element
                .FindAll(TreeScope_Children, &condition)
                .map_err(|e| AivyxError::Other(format!("UIA FindAll: {e}")))?
        };

        let count = unsafe { children.Length() }.unwrap_or(0);
        let mut nodes = Vec::new();

        for i in 0..count {
            let child = match unsafe { children.GetElement(i) } {
                Ok(c) => c,
                Err(_) => continue,
            };

            let path = if path_prefix.is_empty() {
                format!("{i}")
            } else {
                format!("{path_prefix}/{i}")
            };

            let el = Self::element_to_ui(&child, &path);
            let child_nodes = self.build_tree(&child, &path, depth + 1, max_depth)?;

            nodes.push(UiTreeNode {
                element: el,
                children: child_nodes,
            });
        }

        Ok(nodes)
    }

    /// Search for elements matching a query.
    #[cfg(target_os = "windows")]
    fn search_tree(
        &self,
        element: &IUIAutomationElement,
        query: &ElementQuery,
        path_prefix: &str,
        max_depth: u32,
        depth: u32,
        results: &mut Vec<UiElement>,
    ) -> Result<()> {
        if depth >= max_depth || results.len() >= 50 {
            return Ok(());
        }

        let condition = self.true_condition()?;
        let children: IUIAutomationElementArray = unsafe {
            element
                .FindAll(TreeScope_Children, &condition)
                .map_err(|e| AivyxError::Other(format!("UIA FindAll: {e}")))?
        };

        let count = unsafe { children.Length() }.unwrap_or(0);

        for i in 0..count {
            if results.len() >= 50 {
                break;
            }
            let child = match unsafe { children.GetElement(i) } {
                Ok(c) => c,
                Err(_) => continue,
            };

            let path = if path_prefix.is_empty() {
                format!("{i}")
            } else {
                format!("{path_prefix}/{i}")
            };

            let el = Self::element_to_ui(&child, &path);

            if matches_query(&el, query) {
                results.push(el);
            }

            self.search_tree(&child, query, &path, max_depth, depth + 1, results)?;
        }

        Ok(())
    }

    /// Navigate to a child element by path (e.g., "0/3/1").
    #[cfg(target_os = "windows")]
    fn resolve_path(
        &self,
        root: &IUIAutomationElement,
        path: &str,
    ) -> Result<IUIAutomationElement> {
        let indices: Vec<i32> = path
            .split('/')
            .filter(|s| !s.is_empty())
            .map(|s| {
                s.parse::<i32>()
                    .map_err(|_| AivyxError::Other(format!("Invalid path segment: '{s}'")))
            })
            .collect::<Result<Vec<_>>>()?;

        let condition = self.true_condition()?;
        let mut current = root.clone();

        for &idx in &indices {
            let children: IUIAutomationElementArray = unsafe {
                current
                    .FindAll(TreeScope_Children, &condition)
                    .map_err(|e| AivyxError::Other(format!("UIA FindAll: {e}")))?
            };
            current = unsafe {
                children
                    .GetElement(idx)
                    .map_err(|e| AivyxError::Other(format!(
                        "UIA: child at index {idx} not found: {e}"
                    )))?
            };
        }

        Ok(current)
    }

    /// Resolve an element's bounding rectangle.
    #[cfg(target_os = "windows")]
    fn resolve_element_bounds(&self, element: &UiElement) -> Result<[i32; 4]> {
        if let Some(bounds) = element.bounds {
            return Ok(bounds);
        }

        if element.path.is_empty() {
            return Err(AivyxError::Other(
                "UIA: element has no bounds and no path to resolve them".into(),
            ));
        }

        let win = self.find_window(&WindowRef::Active)?;
        let target = self.resolve_path(&win, &element.path)?;
        let rect = unsafe {
            target
                .CurrentBoundingRectangle()
                .map_err(|e| AivyxError::Other(format!("UIA CurrentBoundingRectangle: {e}")))?
        };
        Ok([rect.left, rect.top, rect.right - rect.left, rect.bottom - rect.top])
    }

    /// Invoke the Invoke pattern on an element (click).
    #[cfg(target_os = "windows")]
    fn do_invoke(&self, element: &IUIAutomationElement) -> Result<()> {
        let pattern: IUIAutomationInvokePattern = unsafe {
            element
                .GetCurrentPatternAs(UIA_InvokePatternId)
                .map_err(|e| AivyxError::Other(format!("UIA: no InvokePattern: {e}")))?
        };
        unsafe {
            pattern
                .Invoke()
                .map_err(|e| AivyxError::Other(format!("UIA Invoke failed: {e}")))
        }
    }

    /// Set a value on an element via ValuePattern.
    #[cfg(target_os = "windows")]
    fn set_value(&self, element: &IUIAutomationElement, text: &str) -> Result<()> {
        use windows::core::BSTR;

        let pattern: IUIAutomationValuePattern = unsafe {
            element
                .GetCurrentPatternAs(UIA_ValuePatternId)
                .map_err(|e| AivyxError::Other(format!("UIA: no ValuePattern: {e}")))?
        };
        let bstr = BSTR::from(text);
        unsafe {
            pattern
                .SetValue(&bstr)
                .map_err(|e| AivyxError::Other(format!("UIA SetValue failed: {e}")))
        }
    }

    /// Read a value from an element via ValuePattern.
    #[cfg(target_os = "windows")]
    fn get_value(&self, element: &IUIAutomationElement) -> Result<String> {
        let pattern: IUIAutomationValuePattern = unsafe {
            element
                .GetCurrentPatternAs(UIA_ValuePatternId)
                .map_err(|e| AivyxError::Other(format!("UIA: no ValuePattern: {e}")))?
        };
        unsafe {
            pattern
                .CurrentValue()
                .map(|s| s.to_string())
                .map_err(|e| AivyxError::Other(format!("UIA CurrentValue: {e}")))
        }
    }
}

// ── UiBackend impl ───────��─────────────────────────────────────

#[async_trait::async_trait]
impl UiBackend for UiaBackend {
    fn name(&self) -> &str {
        "uia"
    }

    async fn inspect(
        &self,
        window: &WindowRef,
        max_depth: u32,
    ) -> Result<Vec<UiTreeNode>> {
        #[cfg(target_os = "windows")]
        {
            let win = self.find_window(window)?;
            self.build_tree(&win, "", 0, max_depth)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (window, max_depth);
            Err(AivyxError::Other("UIA is only available on Windows".into()))
        }
    }

    async fn find_element(
        &self,
        window: &WindowRef,
        query: &ElementQuery,
    ) -> Result<Vec<UiElement>> {
        #[cfg(target_os = "windows")]
        {
            let win = self.find_window(window)?;
            let mut results = Vec::new();
            self.search_tree(&win, query, "", 10, 0, &mut results)?;
            Ok(results)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (window, query);
            Err(AivyxError::Other("UIA is only available on Windows".into()))
        }
    }

    async fn click(&self, element: &UiElement) -> Result<()> {
        #[cfg(target_os = "windows")]
        {
            if element.path.is_empty() {
                return Err(AivyxError::Other("UIA click: element path is empty".into()));
            }
            let win = self.find_window(&WindowRef::Active)?;
            let target = self.resolve_path(&win, &element.path)?;

            // Try InvokePattern first (semantic click).
            match self.do_invoke(&target) {
                Ok(()) => Ok(()),
                Err(_) if element.bounds.is_some() => {
                    // Signal that caller should use input backend fallback.
                    Err(AivyxError::Other(
                        "UIA: element has no InvokePattern; use coordinate-based click".into(),
                    ))
                }
                Err(e) => Err(e),
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = element;
            Err(AivyxError::Other("UIA is only available on Windows".into()))
        }
    }

    async fn type_text(&self, element: Option<&UiElement>, text: &str) -> Result<()> {
        #[cfg(target_os = "windows")]
        {
            if let Some(el) = element {
                let win = self.find_window(&WindowRef::Active)?;
                let target = self.resolve_path(&win, &el.path)?;

                // Try ValuePattern first.
                if self.set_value(&target, text).is_ok() {
                    return Ok(());
                }
            }
            // Fall through — caller should use input backend.
            Err(AivyxError::Other(
                "UIA: no ValuePattern; use keyboard input".into(),
            ))
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (element, text);
            Err(AivyxError::Other("UIA is only available on Windows".into()))
        }
    }

    async fn read_text(&self, element: &UiElement) -> Result<String> {
        #[cfg(target_os = "windows")]
        {
            let win = self.find_window(&WindowRef::Active)?;
            let target = self.resolve_path(&win, &element.path)?;

            // Try ValuePattern first.
            if let Ok(text) = self.get_value(&target) {
                return Ok(text);
            }

            // Fall back to the element name.
            let name = unsafe { target.CurrentName() }
                .map(|s| s.to_string())
                .unwrap_or_default();
            if name.is_empty() {
                Err(AivyxError::Other(
                    "UIA: element has no ValuePattern and no name".into(),
                ))
            } else {
                Ok(name)
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = element;
            Err(AivyxError::Other("UIA is only available on Windows".into()))
        }
    }

    async fn double_click(&self, element: &UiElement) -> Result<()> {
        #[cfg(target_os = "windows")]
        {
            let bounds = self.resolve_element_bounds(element)?;
            let cx = bounds[0] + bounds[2] / 2;
            let cy = bounds[1] + bounds[3] / 2;
            self.win_input.double_click_at(cx, cy).await
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = element;
            Err(AivyxError::Other("UIA is only available on Windows".into()))
        }
    }

    async fn middle_click(&self, element: &UiElement) -> Result<()> {
        #[cfg(target_os = "windows")]
        {
            let bounds = self.resolve_element_bounds(element)?;
            let cx = bounds[0] + bounds[2] / 2;
            let cy = bounds[1] + bounds[3] / 2;
            self.win_input.middle_click_at(cx, cy).await
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = element;
            Err(AivyxError::Other("UIA is only available on Windows".into()))
        }
    }

    async fn scroll(
        &self,
        _window: &WindowRef,
        direction: ScrollDirection,
        amount: u32,
    ) -> Result<()> {
        self.win_input.scroll(direction, amount).await
    }

    async fn right_click(&self, element: &UiElement) -> Result<()> {
        #[cfg(target_os = "windows")]
        {
            let bounds = self.resolve_element_bounds(element)?;
            let cx = bounds[0] + bounds[2] / 2;
            let cy = bounds[1] + bounds[3] / 2;
            self.win_input.right_click_at(cx, cy).await
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = element;
            Err(AivyxError::Other("UIA is only available on Windows".into()))
        }
    }

    async fn hover(&self, element: &UiElement) -> Result<()> {
        #[cfg(target_os = "windows")]
        {
            let bounds = self.resolve_element_bounds(element)?;
            let cx = bounds[0] + bounds[2] / 2;
            let cy = bounds[1] + bounds[3] / 2;
            self.win_input.mouse_move_to(cx, cy).await
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = element;
            Err(AivyxError::Other("UIA is only available on Windows".into()))
        }
    }

    async fn drag(&self, from: &UiElement, to: &UiElement) -> Result<()> {
        #[cfg(target_os = "windows")]
        {
            let from_bounds = self.resolve_element_bounds(from)?;
            let to_bounds = self.resolve_element_bounds(to)?;
            let from_cx = from_bounds[0] + from_bounds[2] / 2;
            let from_cy = from_bounds[1] + from_bounds[3] / 2;
            let to_cx = to_bounds[0] + to_bounds[2] / 2;
            let to_cy = to_bounds[1] + to_bounds[3] / 2;
            WinInputBackend::drag(&self.win_input, from_cx, from_cy, to_cx, to_cy).await
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (from, to);
            Err(AivyxError::Other("UIA is only available on Windows".into()))
        }
    }

    async fn mouse_move(&self, x: i32, y: i32) -> Result<()> {
        self.win_input.mouse_move_to(x, y).await
    }

    async fn screenshot_window(&self, window: &WindowRef) -> Result<String> {
        #[cfg(target_os = "windows")]
        {
            let title = match window {
                WindowRef::Title(t) => Some(t.as_str()),
                _ => None,
            };
            super::win_screenshot::capture_window_by_title(title.unwrap_or("")).await
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = window;
            Err(AivyxError::Other("UIA is only available on Windows".into()))
        }
    }
}

// ── Query matching ──────���──────────────────────────────────────

/// Check if an element matches the query filters (case-insensitive substring).
fn matches_query(el: &UiElement, query: &ElementQuery) -> bool {
    if let Some(ref role) = query.role {
        if !el.role.to_lowercase().contains(&role.to_lowercase()) {
            return false;
        }
    }
    if let Some(ref name) = query.name {
        if !el.name.to_lowercase().contains(&name.to_lowercase()) {
            return false;
        }
    }
    if let Some(ref text) = query.text {
        if let Some(ref el_text) = el.text {
            if !el_text.to_lowercase().contains(&text.to_lowercase()) {
                return false;
            }
        } else {
            return false;
        }
    }
    true
}

// ── Control type mapping ───────────��───────────────────────────

/// Map a UIA control type ID to a human-readable role string.
///
/// Uses the same role vocabulary as AT-SPI2 for consistency so the LLM
/// doesn't need different prompt instructions per platform.
#[cfg(target_os = "windows")]
fn control_type_to_role(ct: i32) -> String {
    // The UIA_*ControlTypeId constants are i32 values.
    match ct {
        x if x == UIA_ButtonControlTypeId => "button".into(),
        x if x == UIA_CheckBoxControlTypeId => "check_box".into(),
        x if x == UIA_ComboBoxControlTypeId => "combo_box".into(),
        x if x == UIA_EditControlTypeId => "text_field".into(),
        x if x == UIA_HyperlinkControlTypeId => "link".into(),
        x if x == UIA_ListItemControlTypeId => "list_item".into(),
        x if x == UIA_MenuItemControlTypeId => "menu_item".into(),
        x if x == UIA_RadioButtonControlTypeId => "radio_button".into(),
        x if x == UIA_TabItemControlTypeId => "tab_item".into(),
        x if x == UIA_TextControlTypeId => "label".into(),
        x if x == UIA_TreeItemControlTypeId => "tree_item".into(),
        x if x == UIA_WindowControlTypeId => "window".into(),
        x if x == UIA_DocumentControlTypeId => "document".into(),
        x if x == UIA_GroupControlTypeId => "group".into(),
        x if x == UIA_HeaderControlTypeId => "header".into(),
        x if x == UIA_ImageControlTypeId => "image".into(),
        x if x == UIA_ListControlTypeId => "list".into(),
        x if x == UIA_MenuBarControlTypeId => "menu_bar".into(),
        x if x == UIA_MenuControlTypeId => "menu".into(),
        x if x == UIA_PaneControlTypeId => "pane".into(),
        x if x == UIA_ProgressBarControlTypeId => "progress_bar".into(),
        x if x == UIA_ScrollBarControlTypeId => "scroll_bar".into(),
        x if x == UIA_SliderControlTypeId => "slider".into(),
        x if x == UIA_SpinnerControlTypeId => "spinner".into(),
        x if x == UIA_SplitButtonControlTypeId => "split_button".into(),
        x if x == UIA_StatusBarControlTypeId => "status_bar".into(),
        x if x == UIA_TabControlTypeId => "tab".into(),
        x if x == UIA_TableControlTypeId => "table".into(),
        x if x == UIA_ToolBarControlTypeId => "toolbar".into(),
        x if x == UIA_ToolTipControlTypeId => "tooltip".into(),
        x if x == UIA_TreeControlTypeId => "tree".into(),
        _ => format!("unknown({ct})"),
    }
}

#[cfg(not(target_os = "windows"))]
fn control_type_to_role(ct: i32) -> String {
    format!("unknown({ct})")
}

// ── Tests ───────────────���──────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_query_role() {
        let el = UiElement {
            path: "0/1".into(),
            role: "button".into(),
            name: "OK".into(),
            text: None,
            bounds: None,
            states: Vec::new(),
        };
        assert!(matches_query(
            &el,
            &ElementQuery {
                role: Some("button".into()),
                name: None,
                text: None,
            }
        ));
        assert!(!matches_query(
            &el,
            &ElementQuery {
                role: Some("menu".into()),
                name: None,
                text: None,
            }
        ));
    }

    #[test]
    fn matches_query_name() {
        let el = UiElement {
            path: "0".into(),
            role: "button".into(),
            name: "Save Document".into(),
            text: None,
            bounds: None,
            states: Vec::new(),
        };
        assert!(matches_query(
            &el,
            &ElementQuery {
                role: None,
                name: Some("save".into()),
                text: None,
            }
        ));
        assert!(!matches_query(
            &el,
            &ElementQuery {
                role: None,
                name: Some("cancel".into()),
                text: None,
            }
        ));
    }

    #[test]
    fn matches_query_text() {
        let el = UiElement {
            path: "0".into(),
            role: "text_field".into(),
            name: "".into(),
            text: Some("Hello World".into()),
            bounds: None,
            states: Vec::new(),
        };
        assert!(matches_query(
            &el,
            &ElementQuery {
                role: None,
                name: None,
                text: Some("hello".into()),
            }
        ));
        assert!(!matches_query(
            &el,
            &ElementQuery {
                role: None,
                name: None,
                text: Some("goodbye".into()),
            }
        ));
    }

    #[test]
    fn matches_query_combined() {
        let el = UiElement {
            path: "0".into(),
            role: "button".into(),
            name: "Submit Form".into(),
            text: None,
            bounds: None,
            states: Vec::new(),
        };
        assert!(matches_query(
            &el,
            &ElementQuery {
                role: Some("button".into()),
                name: Some("submit".into()),
                text: None,
            }
        ));
        assert!(!matches_query(
            &el,
            &ElementQuery {
                role: Some("button".into()),
                name: Some("cancel".into()),
                text: None,
            }
        ));
    }

    #[test]
    fn control_type_mapping() {
        // Non-Windows: everything maps to "unknown(N)"
        #[cfg(not(target_os = "windows"))]
        {
            assert_eq!(control_type_to_role(50000), "unknown(50000)");
            assert_eq!(control_type_to_role(0), "unknown(0)");
        }
    }

    #[test]
    fn empty_query_matches_everything() {
        let el = UiElement {
            path: "0".into(),
            role: "button".into(),
            name: "anything".into(),
            text: Some("some text".into()),
            bounds: None,
            states: Vec::new(),
        };
        assert!(matches_query(&el, &ElementQuery::default()));
    }
}
