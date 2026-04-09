// ── BackendRouter (macOS fallback) ──────────────────────────────

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
/// Routes UI operations to the best available backend for a given window.
pub struct BackendRouter {
    pub(crate) cdp: Option<cdp::CdpBackend>,
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
impl BackendRouter {
    /// Create a router from the interaction config.
    pub fn new(config: &InteractionConfig) -> Self {
        Self {
            cdp: if config.browser.enabled {
                Some(cdp::CdpBackend::new(config.browser.debug_port))
            } else {
                None
            },
        }
    }

    /// Pick the best backend for a window.
    pub(crate) async fn route(&self, window: &WindowRef) -> &dyn UiBackend {
        if let Some(ref cdp_backend) = self.cdp {
            if let Ok(Some(class)) = get_window_class(window).await {
                let lower = class.to_lowercase();
                if is_browser_class(&lower) {
                    return cdp_backend;
                }
            }
        }
        // macOS UI Automation not natively supported yet. Fallback to generic panic.
        unimplemented!("macOS / generic fallback interaction backend not implemented yet")
    }

    pub(crate) fn cdp(&self) -> Option<&cdp::CdpBackend> {
        self.cdp.as_ref()
    }

    pub(crate) fn input_backend(&self) -> &dyn InputBackend {
        unimplemented!("macOS / generic fallback input backend not implemented yet")
    }
}

// ── Window class detection (macOS fallback) ─────────────────────

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
async fn get_window_class(_window: &WindowRef) -> Result<Option<String>> {
    Ok(None)
}

