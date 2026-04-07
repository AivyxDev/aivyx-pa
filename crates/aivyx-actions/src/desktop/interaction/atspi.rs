//! AT-SPI2 accessibility backend — semantic UI automation for GTK/Qt/Electron.
//!
//! Uses the `atspi` crate (pure Rust, D-Bus based) to traverse the accessibility
//! tree, find elements by role/name/text, and perform actions (click, type, read).
//!
//! Requires the `accessibility` Cargo feature.

use aivyx_core::{AivyxError, Result};
use atspi::proxy::accessible::AccessibleProxy;
use atspi::proxy::action::ActionProxy;
use atspi::proxy::component::ComponentProxy;
use atspi::proxy::editable_text::EditableTextProxy;
use atspi::proxy::text::TextProxy;
use atspi::{CoordType, Interface, InterfaceSet, State};

use super::{ElementQuery, ScrollDirection, UiBackend, UiElement, UiTreeNode, WindowRef};
use super::ydotool::YdotoolBackend;

/// AT-SPI2 backend for semantic UI automation.
///
/// Stores a `YdotoolBackend` for operations that AT-SPI2 can locate
/// (via element bounds) but can't perform natively — right-click, hover,
/// drag, and mouse movement.
pub struct AtSpiBackend {
    /// Cached D-Bus connection (created on first use).
    connection: tokio::sync::OnceCell<zbus::Connection>,
    /// ydotool for input injection fallback (right-click, hover, drag).
    ydotool: YdotoolBackend,
}

impl AtSpiBackend {
    pub fn new() -> Self {
        Self {
            connection: tokio::sync::OnceCell::new(),
            ydotool: YdotoolBackend::new(),
        }
    }

    /// Get or create the D-Bus session connection.
    async fn conn(&self) -> Result<&zbus::Connection> {
        self.connection
            .get_or_try_init(|| async {
                zbus::Connection::session()
                    .await
                    .map_err(|e| AivyxError::Other(format!("D-Bus session connect failed: {e}")))
            })
            .await
    }

    /// Build an AccessibleProxy for the AT-SPI2 registry root.
    async fn registry(&self) -> Result<AccessibleProxy<'_>> {
        let conn = self.conn().await?;
        AccessibleProxy::builder(conn)
            .destination("org.a11y.atspi.Registry")
            .map_err(|e| AivyxError::Other(format!("AT-SPI2 registry dest: {e}")))?
            .path("/org/a11y/atspi/accessible/root")
            .map_err(|e| AivyxError::Other(format!("AT-SPI2 registry path: {e}")))?
            .build()
            .await
            .map_err(|e| AivyxError::Other(format!("AT-SPI2 registry proxy: {e}")))
    }

    /// Build an AccessibleProxy from an ObjectRef (bus name + path).
    async fn proxy_from_ref(
        &self,
        obj: &atspi::object_ref::ObjectRefOwned,
    ) -> Result<AccessibleProxy<'_>> {
        let conn = self.conn().await?;
        let name = obj
            .name()
            .ok_or_else(|| AivyxError::Other("AT-SPI2: null object ref".into()))?;
        let path = obj.path();

        AccessibleProxy::builder(conn)
            .destination(name.clone())
            .map_err(|e| AivyxError::Other(format!("AT-SPI2 proxy dest: {e}")))?
            .path(path.clone())
            .map_err(|e| AivyxError::Other(format!("AT-SPI2 proxy path: {e}")))?
            .build()
            .await
            .map_err(|e| AivyxError::Other(format!("AT-SPI2 proxy build: {e}")))
    }

    /// Find the application/window accessible matching the WindowRef.
    async fn find_window(&self, window: &WindowRef) -> Result<AccessibleProxy<'_>> {
        let registry = self.registry().await?;
        let apps = registry
            .get_children()
            .await
            .map_err(|e| AivyxError::Other(format!("AT-SPI2 get apps: {e}")))?;

        match window {
            WindowRef::Active => {
                // Walk all apps' top-level windows, return the first "active" one.
                for app_ref in &apps {
                    if app_ref.is_null() {
                        continue;
                    }
                    let app = match self.proxy_from_ref(app_ref).await {
                        Ok(p) => p,
                        Err(_) => continue,
                    };
                    let windows = match app.get_children().await {
                        Ok(w) => w,
                        Err(_) => continue,
                    };
                    for win_ref in &windows {
                        if win_ref.is_null() {
                            continue;
                        }
                        let win = match self.proxy_from_ref(win_ref).await {
                            Ok(p) => p,
                            Err(_) => continue,
                        };
                        // Check if window has Active state.
                        if let Ok(states) = win.get_state().await {
                            if states.contains(State::Active) {
                                return Ok(win);
                            }
                        }
                    }
                }
                Err(AivyxError::Other(
                    "AT-SPI2: no active window found. Specify a window title.".into(),
                ))
            }
            WindowRef::Title(title) => {
                let lower = title.to_lowercase();
                for app_ref in &apps {
                    if app_ref.is_null() {
                        continue;
                    }
                    let app = match self.proxy_from_ref(app_ref).await {
                        Ok(p) => p,
                        Err(_) => continue,
                    };
                    let windows = match app.get_children().await {
                        Ok(w) => w,
                        Err(_) => continue,
                    };
                    for win_ref in &windows {
                        if win_ref.is_null() {
                            continue;
                        }
                        let win = match self.proxy_from_ref(win_ref).await {
                            Ok(p) => p,
                            Err(_) => continue,
                        };
                        if let Ok(name) = win.name().await {
                            if name.to_lowercase().contains(&lower) {
                                return Ok(win);
                            }
                        }
                    }
                    // Also check app-level name.
                    if let Ok(name) = app.name().await {
                        if name.to_lowercase().contains(&lower) {
                            return Ok(app);
                        }
                    }
                }
                Err(AivyxError::Other(format!(
                    "AT-SPI2: no window matching '{title}' found"
                )))
            }
            WindowRef::Id(_id) => Err(AivyxError::Other(
                "AT-SPI2: window lookup by ID not supported; use title instead".into(),
            )),
        }
    }

    /// Recursively build the accessibility tree from a proxy.
    async fn build_tree(
        &self,
        proxy: &AccessibleProxy<'_>,
        path_prefix: &str,
        depth: u32,
        max_depth: u32,
    ) -> Result<Vec<UiTreeNode>> {
        if depth >= max_depth {
            return Ok(Vec::new());
        }

        let children = proxy
            .get_children()
            .await
            .map_err(|e| AivyxError::Other(format!("AT-SPI2 get_children: {e}")))?;

        let mut nodes = Vec::new();
        for (i, child_ref) in children.iter().enumerate() {
            if child_ref.is_null() {
                continue;
            }
            let child = match self.proxy_from_ref(child_ref).await {
                Ok(p) => p,
                Err(_) => continue,
            };
            let path = if path_prefix.is_empty() {
                format!("{i}")
            } else {
                format!("{path_prefix}/{i}")
            };

            let element = self.element_from_proxy(&child, &path).await;
            let subtree = Box::pin(self.build_tree(&child, &path, depth + 1, max_depth))
                .await
                .unwrap_or_default();

            nodes.push(UiTreeNode {
                element,
                children: subtree,
            });
        }
        Ok(nodes)
    }

    /// Extract a UiElement from an accessible proxy.
    async fn element_from_proxy(
        &self,
        proxy: &AccessibleProxy<'_>,
        path: &str,
    ) -> UiElement {
        let role = proxy
            .get_role_name()
            .await
            .unwrap_or_else(|_| "unknown".into());
        let name = proxy.name().await.unwrap_or_default();

        let interfaces = proxy.get_interfaces().await.unwrap_or_else(|_| InterfaceSet::empty());

        // Try to get text content if Text interface is available.
        let text = if interfaces.contains(Interface::Text) {
            self.read_text_from_proxy(proxy).await.ok()
        } else {
            None
        };

        // Try to get bounds if Component interface is available.
        let bounds = if interfaces.contains(Interface::Component) {
            self.get_bounds(proxy).await.ok()
        } else {
            None
        };

        // Get states.
        let states = proxy
            .get_state()
            .await
            .map(|ss| {
                let mut v = Vec::new();
                for state in ss.iter() {
                    v.push(format!("{state:?}"));
                }
                v
            })
            .unwrap_or_default();

        UiElement {
            path: path.to_string(),
            role,
            name,
            text,
            bounds,
            states,
        }
    }

    /// Read text content from an accessible that implements the Text interface.
    async fn read_text_from_proxy(&self, proxy: &AccessibleProxy<'_>) -> Result<String> {
        let conn = self.conn().await?;
        let text_proxy = TextProxy::builder(conn)
            .destination(proxy.inner().destination().to_owned())
            .map_err(|e| AivyxError::Other(format!("Text proxy dest: {e}")))?
            .path(proxy.inner().path().to_owned())
            .map_err(|e| AivyxError::Other(format!("Text proxy path: {e}")))?
            .build()
            .await
            .map_err(|e| AivyxError::Other(format!("Text proxy build: {e}")))?;

        let count = text_proxy
            .character_count()
            .await
            .map_err(|e| AivyxError::Other(format!("Text character_count: {e}")))?;

        if count == 0 {
            return Ok(String::new());
        }

        let text = text_proxy
            .get_text(0, count)
            .await
            .map_err(|e| AivyxError::Other(format!("Text get_text: {e}")))?;

        Ok(text)
    }

    /// Get the bounding box of an accessible via the Component interface.
    async fn get_bounds(&self, proxy: &AccessibleProxy<'_>) -> Result<[i32; 4]> {
        let conn = self.conn().await?;
        let comp = ComponentProxy::builder(conn)
            .destination(proxy.inner().destination().to_owned())
            .map_err(|e| AivyxError::Other(format!("Component proxy dest: {e}")))?
            .path(proxy.inner().path().to_owned())
            .map_err(|e| AivyxError::Other(format!("Component proxy path: {e}")))?
            .build()
            .await
            .map_err(|e| AivyxError::Other(format!("Component proxy build: {e}")))?;

        let (x, y, w, h) = comp
            .get_extents(CoordType::Screen)
            .await
            .map_err(|e| AivyxError::Other(format!("Component get_extents: {e}")))?;

        Ok([x, y, w, h])
    }

    /// Perform a click action on an accessible.
    async fn do_click(&self, proxy: &AccessibleProxy<'_>) -> Result<()> {
        let conn = self.conn().await?;
        let action = ActionProxy::builder(conn)
            .destination(proxy.inner().destination().to_owned())
            .map_err(|e| AivyxError::Other(format!("Action proxy dest: {e}")))?
            .path(proxy.inner().path().to_owned())
            .map_err(|e| AivyxError::Other(format!("Action proxy path: {e}")))?
            .build()
            .await
            .map_err(|e| AivyxError::Other(format!("Action proxy build: {e}")))?;

        let result = action
            .do_action(0)
            .await
            .map_err(|e| AivyxError::Other(format!("Action do_action: {e}")))?;

        if result {
            Ok(())
        } else {
            Err(AivyxError::Other(
                "AT-SPI2 do_action(0) returned false — action not performed".into(),
            ))
        }
    }

    /// Set text contents on an editable text element.
    async fn set_text(&self, proxy: &AccessibleProxy<'_>, text: &str) -> Result<()> {
        let conn = self.conn().await?;
        let editable = EditableTextProxy::builder(conn)
            .destination(proxy.inner().destination().to_owned())
            .map_err(|e| AivyxError::Other(format!("EditableText proxy dest: {e}")))?
            .path(proxy.inner().path().to_owned())
            .map_err(|e| AivyxError::Other(format!("EditableText proxy path: {e}")))?
            .build()
            .await
            .map_err(|e| AivyxError::Other(format!("EditableText proxy build: {e}")))?;

        let result = editable
            .set_text_contents(text)
            .await
            .map_err(|e| AivyxError::Other(format!("EditableText set_text_contents: {e}")))?;

        if result {
            Ok(())
        } else {
            Err(AivyxError::Other(
                "AT-SPI2 set_text_contents returned false".into(),
            ))
        }
    }

    /// Navigate to a child accessible by path (e.g., "0/3/1").
    async fn resolve_path(&self, root: &AccessibleProxy<'_>, path: &str) -> Result<AccessibleProxy<'_>> {
        let indices: Vec<i32> = path
            .split('/')
            .filter(|s| !s.is_empty())
            .map(|s| {
                s.parse::<i32>()
                    .map_err(|_| AivyxError::Other(format!("Invalid path segment: '{s}'")))
            })
            .collect::<Result<Vec<_>>>()?;

        let mut current_ref = None;
        let mut current = None;

        for (i, &idx) in indices.iter().enumerate() {
            let parent = if i == 0 {
                root
            } else {
                current.as_ref().unwrap()
            };

            let child_ref = parent
                .get_child_at_index(idx)
                .await
                .map_err(|e| AivyxError::Other(format!("AT-SPI2 get_child_at_index({idx}): {e}")))?;

            if child_ref.is_null() {
                return Err(AivyxError::Other(format!(
                    "AT-SPI2: child at index {idx} is null (path: {path})"
                )));
            }

            let proxy = self.proxy_from_ref(&child_ref).await?;
            current_ref = Some(child_ref);
            current = Some(proxy);
        }

        let _ = current_ref; // keep ref alive
        current.ok_or_else(|| AivyxError::Other("AT-SPI2: empty path".into()))
    }

    /// Search the tree for elements matching a query.
    async fn search(
        &self,
        proxy: &AccessibleProxy<'_>,
        query: &ElementQuery,
        path_prefix: &str,
        max_depth: u32,
        depth: u32,
        results: &mut Vec<UiElement>,
    ) -> Result<()> {
        if depth >= max_depth || results.len() >= 50 {
            return Ok(());
        }

        let children = proxy
            .get_children()
            .await
            .map_err(|e| AivyxError::Other(format!("AT-SPI2 get_children: {e}")))?;

        for (i, child_ref) in children.iter().enumerate() {
            if child_ref.is_null() || results.len() >= 50 {
                continue;
            }
            let child = match self.proxy_from_ref(child_ref).await {
                Ok(p) => p,
                Err(_) => continue,
            };
            let path = if path_prefix.is_empty() {
                format!("{i}")
            } else {
                format!("{path_prefix}/{i}")
            };

            let el = self.element_from_proxy(&child, &path).await;

            if matches_query(&el, query) {
                results.push(el);
            }

            // Recurse into children.
            // Box the future to avoid unbounded recursive type.
            Box::pin(self.search(&child, query, &path, max_depth, depth + 1, results)).await?;
        }
        Ok(())
    }

    /// Resolve an element's bounds, using cached bounds or looking them up via AT-SPI2.
    async fn resolve_element_bounds(&self, element: &UiElement) -> Result<[i32; 4]> {
        // If bounds are already cached on the element, use them.
        if let Some(bounds) = element.bounds {
            return Ok(bounds);
        }

        // Otherwise, resolve path and get bounds from the Component interface.
        if element.path.is_empty() {
            return Err(AivyxError::Other(
                "AT-SPI2: element has no bounds and no path to resolve them".into(),
            ));
        }

        let win = self.find_window(&WindowRef::Active).await?;
        let target = self.resolve_path(&win, &element.path).await?;
        self.get_bounds(&target).await
    }
}

/// Check if an element matches the query filters.
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

#[async_trait::async_trait]
impl UiBackend for AtSpiBackend {
    fn name(&self) -> &str {
        "at-spi2"
    }

    async fn inspect(
        &self,
        window: &WindowRef,
        max_depth: u32,
    ) -> Result<Vec<UiTreeNode>> {
        let win = self.find_window(window).await?;
        self.build_tree(&win, "", 0, max_depth).await
    }

    async fn find_element(
        &self,
        window: &WindowRef,
        query: &ElementQuery,
    ) -> Result<Vec<UiElement>> {
        let win = self.find_window(window).await?;
        let mut results = Vec::new();
        self.search(&win, query, "", 10, 0, &mut results).await?;
        Ok(results)
    }

    async fn click(&self, element: &UiElement) -> Result<()> {
        if element.path.is_empty() {
            return Err(AivyxError::Other(
                "AT-SPI2 click: element path is empty".into(),
            ));
        }
        // We need a window to resolve the path against.
        // For now, use the active window.
        let win = self.find_window(&WindowRef::Active).await?;
        let target = self.resolve_path(&win, &element.path).await?;

        // Check if Action interface is available.
        let interfaces = target
            .get_interfaces()
            .await
            .unwrap_or_else(|_| InterfaceSet::empty());

        if interfaces.contains(Interface::Action) {
            self.do_click(&target).await
        } else if element.bounds.is_some() {
            // Signal that caller should use ydotool fallback.
            Err(AivyxError::Other(
                "AT-SPI2: element has no Action interface; use coordinate-based click".into(),
            ))
        } else {
            Err(AivyxError::Other(
                "AT-SPI2: element has no Action interface and no bounds for fallback".into(),
            ))
        }
    }

    async fn type_text(
        &self,
        element: Option<&UiElement>,
        text: &str,
    ) -> Result<()> {
        let win = self.find_window(&WindowRef::Active).await?;

        if let Some(el) = element {
            let target = self.resolve_path(&win, &el.path).await?;
            let interfaces = target
                .get_interfaces()
                .await
                .unwrap_or_else(|_| InterfaceSet::empty());

            if interfaces.contains(Interface::EditableText) {
                return self.set_text(&target, text).await;
            }
            // Fall through to error — caller should use ydotool.
        }

        Err(AivyxError::Other(
            "AT-SPI2: no EditableText interface; use ydotool for keyboard input".into(),
        ))
    }

    async fn read_text(&self, element: &UiElement) -> Result<String> {
        let win = self.find_window(&WindowRef::Active).await?;
        let target = self.resolve_path(&win, &element.path).await?;

        let interfaces = target
            .get_interfaces()
            .await
            .unwrap_or_else(|_| InterfaceSet::empty());

        if interfaces.contains(Interface::Text) {
            self.read_text_from_proxy(&target).await
        } else {
            // Try the name as a fallback.
            let name = target.name().await.unwrap_or_default();
            if name.is_empty() {
                Err(AivyxError::Other(
                    "AT-SPI2: element has no Text interface and no name".into(),
                ))
            } else {
                Ok(name)
            }
        }
    }

    async fn double_click(&self, element: &UiElement) -> Result<()> {
        let bounds = self.resolve_element_bounds(element).await?;
        let cx = bounds[0] + bounds[2] / 2;
        let cy = bounds[1] + bounds[3] / 2;
        self.ydotool.double_click_at(cx, cy).await
    }

    async fn middle_click(&self, element: &UiElement) -> Result<()> {
        let bounds = self.resolve_element_bounds(element).await?;
        let cx = bounds[0] + bounds[2] / 2;
        let cy = bounds[1] + bounds[3] / 2;
        self.ydotool.middle_click_at(cx, cy).await
    }

    async fn scroll(
        &self,
        _window: &WindowRef,
        direction: ScrollDirection,
        amount: u32,
    ) -> Result<()> {
        self.ydotool.scroll(direction, amount).await
    }

    async fn right_click(&self, element: &UiElement) -> Result<()> {
        // AT-SPI2 has no native right-click. Resolve bounds → ydotool right-click.
        let bounds = self.resolve_element_bounds(element).await?;
        let cx = bounds[0] + bounds[2] / 2;
        let cy = bounds[1] + bounds[3] / 2;
        self.ydotool.right_click_at(cx, cy).await
    }

    async fn hover(&self, element: &UiElement) -> Result<()> {
        let bounds = self.resolve_element_bounds(element).await?;
        let cx = bounds[0] + bounds[2] / 2;
        let cy = bounds[1] + bounds[3] / 2;
        self.ydotool.mouse_move_to(cx, cy).await
    }

    async fn drag(&self, from: &UiElement, to: &UiElement) -> Result<()> {
        let from_bounds = self.resolve_element_bounds(from).await?;
        let to_bounds = self.resolve_element_bounds(to).await?;
        let from_cx = from_bounds[0] + from_bounds[2] / 2;
        let from_cy = from_bounds[1] + from_bounds[3] / 2;
        let to_cx = to_bounds[0] + to_bounds[2] / 2;
        let to_cy = to_bounds[1] + to_bounds[3] / 2;
        self.ydotool.drag(from_cx, from_cy, to_cx, to_cy).await
    }

    async fn mouse_move(&self, x: i32, y: i32) -> Result<()> {
        self.ydotool.mouse_move_to(x, y).await
    }

    async fn screenshot_window(&self, window: &WindowRef) -> Result<String> {
        // Get window title for geometry lookup.
        let title = match window {
            WindowRef::Title(t) => Some(t.as_str()),
            _ => None,
        };
        let geometry = super::screenshot::get_window_geometry(title).await?;
        super::screenshot::capture_window(geometry.as_deref(), "png").await
    }
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_name() {
        let backend = AtSpiBackend::new();
        assert_eq!(backend.name(), "at-spi2");
    }

    #[test]
    fn matches_query_role() {
        let el = UiElement {
            path: "0/1".into(),
            role: "push button".into(),
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
            role: "text".into(),
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
            role: "push button".into(),
            name: "Submit Form".into(),
            text: None,
            bounds: None,
            states: Vec::new(),
        };
        // Both role and name must match.
        assert!(matches_query(
            &el,
            &ElementQuery {
                role: Some("button".into()),
                name: Some("submit".into()),
                text: None,
            }
        ));
        // Role matches but name doesn't.
        assert!(!matches_query(
            &el,
            &ElementQuery {
                role: Some("button".into()),
                name: Some("cancel".into()),
                text: None,
            }
        ));
    }
}
