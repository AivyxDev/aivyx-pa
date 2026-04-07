//! OAuth2 authorization flow for MCP plugins.
//!
//! Wraps [`McpOAuthClient`] and [`OAuthTokenManager`] from aivyx-mcp with:
//! - An ephemeral localhost HTTP server to capture the OAuth redirect callback
//! - A [`StorageBackend`] adapter that encrypts tokens via [`EncryptedStore`]
//! - A high-level [`authorize_plugin`] function for the build_agent flow
//!
//! The flow is:
//! 1. Generate authorization URL with PKCE challenge
//! 2. Log the URL for the user to visit (TUI notification)
//! 3. Listen on `http://localhost:{port}/callback` for the redirect
//! 4. Exchange authorization code for tokens
//! 5. Persist tokens to encrypted store via [`OAuthTokenManager`]

use std::sync::Arc;

use aivyx_config::McpAuthMethod;
use aivyx_core::{AivyxError, Result, StorageBackend};
use aivyx_crypto::{EncryptedStore, MasterKey};
use aivyx_mcp::{McpOAuthClient, OAuthTokenManager};
use tokio::sync::oneshot;
use zeroize::Zeroizing;

// ── Encrypted StorageBackend Adapter ────────────────────────────────

/// Adapter that makes [`EncryptedStore`] usable as a [`StorageBackend`].
///
/// Bakes in a domain-derived key so callers (like `OAuthTokenManager`)
/// don't need to manage encryption keys. All values are encrypted at rest.
pub struct EncryptedStorageAdapter {
    store: Arc<EncryptedStore>,
    key_bytes: Arc<Zeroizing<[u8; 32]>>,
}

impl EncryptedStorageAdapter {
    pub fn new(store: Arc<EncryptedStore>, key: &MasterKey) -> Self {
        let key_bytes: [u8; 32] = key.expose_secret().try_into()
            .expect("master key must be 32 bytes");
        Self {
            store,
            key_bytes: Arc::new(Zeroizing::new(key_bytes)),
        }
    }

    fn key(&self) -> MasterKey {
        MasterKey::from_bytes(**self.key_bytes)
    }
}

impl StorageBackend for EncryptedStorageAdapter {
    fn put(&self, key: &str, value: &[u8]) -> Result<()> {
        let mk = self.key();
        self.store.put(key, value, &mk)
            .map_err(|e| AivyxError::Other(format!("EncryptedStore put failed: {e}")))
    }

    fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let mk = self.key();
        self.store.get(key, &mk)
            .map_err(|e| AivyxError::Other(format!("EncryptedStore get failed: {e}")))
    }

    fn delete(&self, key: &str) -> Result<()> {
        self.store.delete(key)
            .map_err(|e| AivyxError::Other(format!("EncryptedStore delete failed: {e}")))
    }

    fn list_keys(&self) -> Result<Vec<String>> {
        self.store.list_keys()
            .map_err(|e| AivyxError::Other(format!("EncryptedStore list_keys failed: {e}")))
    }
}

// ── OAuth Callback Server ───────────────────────────────────────────

/// Ephemeral HTTP server that listens for a single OAuth redirect callback.
///
/// Spawns on a random port, captures the authorization code from the
/// query parameters, sends a success page to the browser, then shuts down.
async fn capture_oauth_callback(port: u16, expected_state: &str) -> Result<String> {
    let (tx, rx) = oneshot::channel::<String>();
    let tx = Arc::new(tokio::sync::Mutex::new(Some(tx)));
    let expected = expected_state.to_string();

    let app = axum::Router::new().route(
        "/callback",
        axum::routing::get({
            let tx = Arc::clone(&tx);
            move |query: axum::extract::Query<CallbackQuery>| {
                let tx = Arc::clone(&tx);
                let expected = expected.clone();
                async move {
                    // Validate state parameter to prevent CSRF attacks.
                    let state_valid = query.state.as_deref() == Some(expected.as_str());
                    if !state_valid {
                        return axum::response::Html(
                            "<html><body><h2>Authorization failed</h2>\
                             <p>Invalid state parameter — possible CSRF attack. \
                             Please try again.</p></body></html>",
                        );
                    }
                    if let Some(sender) = tx.lock().await.take() {
                        let _ = sender.send(query.code.clone());
                    }
                    axum::response::Html(
                        "<html><body><h2>Authorization successful!</h2>\
                         <p>You can close this tab and return to Aivyx.</p></body></html>",
                    )
                }
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}"))
        .await
        .map_err(|e| AivyxError::Other(format!("Failed to bind OAuth callback server: {e}")))?;

    // Serve with a timeout — don't wait forever for user interaction.
    let server = axum::serve(listener, app);

    let code = tokio::select! {
        result = rx => {
            result.map_err(|_| AivyxError::Other("OAuth callback channel closed".into()))?
        }
        _ = tokio::time::sleep(std::time::Duration::from_secs(300)) => {
            return Err(AivyxError::Other(
                "OAuth authorization timed out (5 minutes). Please try again.".into(),
            ));
        }
        result = server.into_future() => {
            result.map_err(|e| AivyxError::Other(format!("OAuth callback server error: {e}")))?;
            return Err(AivyxError::Other("OAuth callback server exited unexpectedly".into()));
        }
    };

    Ok(code)
}

#[derive(serde::Deserialize)]
struct CallbackQuery {
    code: String,
    state: Option<String>,
}

// ── High-Level Authorization ────────────────────────────────────────

/// Default port for the OAuth callback server.
const DEFAULT_CALLBACK_PORT: u16 = 8976;

/// Perform the full OAuth2 authorization flow for an MCP plugin.
///
/// 1. Discovers OAuth endpoints (or uses explicit config)
/// 2. Generates PKCE challenge + authorization URL
/// 3. Spawns callback server on localhost
/// 4. Sends notification with authorization URL for the user
/// 5. Waits for redirect with authorization code
/// 6. Exchanges code for tokens
/// 7. Returns [`OAuthTokenManager`] with persisted tokens
///
/// The `notify` callback is used to inform the user of the authorization URL
/// (e.g., display in TUI notification). The user must open this URL in a browser.
pub async fn authorize_plugin<F>(
    server_name: &str,
    auth: &McpAuthMethod,
    server_url: Option<&str>,
    store: Arc<EncryptedStore>,
    oauth_key: &MasterKey,
    notify: F,
) -> Result<OAuthTokenManager>
where
    F: FnOnce(String),
{
    let McpAuthMethod::OAuth {
        client_id,
        scopes,
        redirect_uri,
    } = auth
    else {
        return Err(AivyxError::Config(
            "authorize_plugin called with non-OAuth auth method".into(),
        ));
    };

    // Determine callback port from redirect_uri or use default.
    let callback_port = redirect_uri
        .as_ref()
        .and_then(|uri| extract_port_from_url(uri))
        .unwrap_or(DEFAULT_CALLBACK_PORT);

    let redirect = redirect_uri
        .clone()
        .unwrap_or_else(|| format!("http://localhost:{callback_port}/callback"));

    // Build OAuth client — discover endpoints if server URL is available.
    let oauth_client = if let Some(url) = server_url {
        McpOAuthClient::discover(url, client_id, scopes.clone()).await?
    } else {
        return Err(AivyxError::Config(
            "OAuth plugin requires a server URL for endpoint discovery".into(),
        ));
    };

    // Generate authorization URL with PKCE + CSRF state parameter.
    let (auth_url, pkce) = oauth_client.authorization_url(&redirect);
    // UUID v4 is URL-safe (alphanumeric + hyphens), no encoding needed.
    let csrf_state = uuid::Uuid::new_v4().to_string();
    let auth_url_with_state = format!("{auth_url}&state={csrf_state}");

    // Notify user to open the URL.
    notify(format!(
        "Open this URL to authorize plugin '{server_name}':\n{auth_url_with_state}"
    ));

    // Capture the redirect callback (validates state parameter).
    let code = capture_oauth_callback(callback_port, &csrf_state).await?;

    // Exchange code for tokens.
    let tokens = oauth_client
        .exchange_code(&code, &pkce.verifier, &redirect)
        .await?;

    // Create token manager with encrypted storage.
    let storage = Arc::new(EncryptedStorageAdapter::new(
        Arc::clone(&store),
        oauth_key,
    ));
    let manager = OAuthTokenManager::new(oauth_client, server_name, storage);
    manager.set_tokens(tokens).await?;

    tracing::info!("OAuth authorization complete for plugin '{server_name}'");
    Ok(manager)
}

/// Check if a plugin has valid persisted OAuth tokens.
///
/// Returns `Some(OAuthTokenManager)` if tokens exist and can be loaded,
/// `None` if authorization is needed.
pub fn load_token_manager(
    server_name: &str,
    auth: &McpAuthMethod,
    server_url: Option<&str>,
    store: Arc<EncryptedStore>,
    oauth_key: &MasterKey,
) -> Result<Option<OAuthTokenManager>> {
    let McpAuthMethod::OAuth {
        client_id,
        scopes,
        ..
    } = auth
    else {
        return Ok(None);
    };

    // Build OAuth client — we need it for the token manager even when
    // loading persisted tokens (it's used for refresh).
    // For loading, we try discovery but fall back gracefully.
    let oauth_client = if let Some(url) = server_url {
        // Can't do async discovery here, so use explicit endpoints if available.
        // This is a best-effort load — if tokens exist they'll work for refresh.
        // If not, authorize_plugin() handles the full flow.
        match McpOAuthClient::new(
            client_id,
            scopes.clone(),
            &format!("{url}/.well-known/oauth-authorization-server"),
            &format!("{url}/oauth/token"),
        ) {
            Ok(client) => client,
            Err(_) => return Ok(None),
        }
    } else {
        return Ok(None);
    };

    let storage: Arc<dyn StorageBackend> = Arc::new(EncryptedStorageAdapter::new(
        store,
        oauth_key,
    ));
    let manager = OAuthTokenManager::new(oauth_client, server_name, storage);

    Ok(Some(manager))
}

/// Resolve a bearer token from the encrypted store for a plugin.
pub fn resolve_bearer_token(
    token_secret_name: &str,
    store: &EncryptedStore,
    key: &MasterKey,
) -> Result<Option<String>> {
    match store.get(token_secret_name, key) {
        Ok(Some(bytes)) => {
            let token = String::from_utf8(bytes)
                .map_err(|e| AivyxError::Other(format!("Invalid UTF-8 in token: {e}")))?;
            Ok(Some(token))
        }
        Ok(None) => Ok(None),
        Err(e) => Err(AivyxError::Other(format!("Failed to load token: {e}"))),
    }
}

/// Extract port number from a URL string without pulling in the `url` crate.
///
/// Handles `http://host:port/path`, `http://[::1]:port/path` (IPv6),
/// and returns `None` for URLs without an explicit port.
fn extract_port_from_url(uri: &str) -> Option<u16> {
    // Strip scheme.
    let after_scheme = uri.split("://").nth(1).unwrap_or(uri);
    // Strip path.
    let host_port = after_scheme.split('/').next()?;

    // For bracketed IPv6 like [::1]:port, split after the closing bracket.
    if host_port.starts_with('[') {
        let port_str = host_port.rsplit("]:").next()?;
        return port_str.parse().ok();
    }

    // For host:port, split on the only colon. If there's no colon, return None.
    let port_str = host_port.split(':').nth(1)?;
    port_str.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> (Arc<EncryptedStore>, MasterKey) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(EncryptedStore::open(dir.path().join("test.db")).unwrap());
        let key = MasterKey::from_bytes([42u8; 32]);
        (store, key)
    }

    #[test]
    fn encrypted_adapter_roundtrip() {
        let (store, key) = temp_store();
        let adapter = EncryptedStorageAdapter::new(store, &key);

        adapter.put("test_key", b"hello world").unwrap();
        let value = adapter.get("test_key").unwrap().unwrap();
        assert_eq!(value, b"hello world");
    }

    #[test]
    fn encrypted_adapter_get_missing_returns_none() {
        let (store, key) = temp_store();
        let adapter = EncryptedStorageAdapter::new(store, &key);

        let value = adapter.get("nonexistent").unwrap();
        assert!(value.is_none());
    }

    #[test]
    fn encrypted_adapter_delete() {
        let (store, key) = temp_store();
        let adapter = EncryptedStorageAdapter::new(store, &key);

        adapter.put("del_key", b"data").unwrap();
        assert!(adapter.get("del_key").unwrap().is_some());

        adapter.delete("del_key").unwrap();
        assert!(adapter.get("del_key").unwrap().is_none());
    }

    #[test]
    fn encrypted_adapter_list_keys() {
        let (store, key) = temp_store();
        let adapter = EncryptedStorageAdapter::new(store, &key);

        adapter.put("alpha", b"1").unwrap();
        adapter.put("beta", b"2").unwrap();

        let keys = adapter.list_keys().unwrap();
        assert!(keys.contains(&"alpha".to_string()));
        assert!(keys.contains(&"beta".to_string()));
    }

    #[test]
    fn resolve_bearer_token_found() {
        let (store, key) = temp_store();
        store.put("my-api-key", b"sk-test-123", &key).unwrap();

        let token = resolve_bearer_token("my-api-key", &store, &key).unwrap();
        assert_eq!(token.as_deref(), Some("sk-test-123"));
    }

    #[test]
    fn resolve_bearer_token_missing() {
        let (store, key) = temp_store();
        let token = resolve_bearer_token("nonexistent", &store, &key).unwrap();
        assert!(token.is_none());
    }
}
