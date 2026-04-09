//! Webhook receiver — localhost HTTP server for inbound event triggers.
//!
//! Spawns an axum server on a configured port that accepts
//! `POST /webhooks/{name}` requests. Each webhook:
//!
//! 1. Verifies HMAC-SHA256 signature (if `secret_ref` is configured)
//! 2. Sanitizes the payload to prevent prompt injection
//! 3. Dispatches a notification to the TUI/agent via the notification channel
//!
//! The webhook name corresponds to a `WorkflowTrigger::Webhook` on a
//! workflow template. When a webhook fires, the trigger engine instantiates
//! the associated template.

use std::sync::Arc;

use aivyx_crypto::{EncryptedStore, MasterKey};
use aivyx_loop::{Notification, NotificationKind};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use zeroize::Zeroizing;

// ── Configuration ───────────────────────────────────────────────

/// Webhook server configuration, parsed from `[webhook]` in config.toml.
///
/// ```toml
/// [webhook]
/// enabled = true
/// port = 8975
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookConfig {
    /// Whether the webhook server is enabled. Default: false.
    #[serde(default)]
    pub enabled: bool,

    /// Port to listen on. Default: 8975.
    #[serde(default = "default_port")]
    pub port: u16,
}

impl Default for WebhookConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            port: default_port(),
        }
    }
}

fn default_port() -> u16 {
    8975
}

// ── Shared State ────────────────────────────────────────────────

/// State shared between webhook routes.
#[derive(Clone)]
struct WebhookState {
    store: Arc<EncryptedStore>,
    key_bytes: Arc<Zeroizing<[u8; 32]>>,
    notify_tx: mpsc::Sender<Notification>,
}

impl WebhookState {
    fn key(&self) -> MasterKey {
        MasterKey::from_bytes(**self.key_bytes)
    }
}

// ── Server ──────────────────────────────────────────────────────

/// Spawn the webhook HTTP server as a background task.
///
/// Returns a `JoinHandle` that runs until the server is shut down.
/// The server binds to `127.0.0.1:{port}` (localhost only for security).
pub async fn spawn_webhook_server(
    config: &WebhookConfig,
    store: Arc<EncryptedStore>,
    master_key: &MasterKey,
    notify_tx: mpsc::Sender<Notification>,
) -> anyhow::Result<tokio::task::JoinHandle<()>> {
    let key_bytes: [u8; 32] = master_key
        .expose_secret()
        .try_into()
        .expect("master key must be 32 bytes");

    let state = WebhookState {
        store,
        key_bytes: Arc::new(Zeroizing::new(key_bytes)),
        notify_tx,
    };

    let app = axum::Router::new()
        .route("/webhooks/{name}", axum::routing::post(receive_webhook))
        .route("/health", axum::routing::get(health_check))
        .layer(axum::extract::DefaultBodyLimit::max(1024 * 1024)) // 1 MB
        .with_state(state);

    let addr = format!("127.0.0.1:{}", config.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    tracing::info!("Webhook server listening on {addr}");

    let handle = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!("Webhook server error: {e}");
        }
    });

    Ok(handle)
}

// ── Routes ──────────────────────────────────────────────────────

/// `GET /health` — simple liveness check.
async fn health_check() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

/// `POST /webhooks/{name}` — receive a webhook payload.
///
/// Security model: HMAC-SHA256 via `X-Hub-Signature-256` header.
/// When the named workflow trigger has a `secret_ref`, the signature
/// is verified against the secret stored in the encrypted store.
/// Without `secret_ref`, the endpoint is open (for local-only use).
async fn receive_webhook(
    State(state): State<WebhookState>,
    Path(name): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    tracing::info!("Webhook received: {name}");

    // Look up the workflow template with a matching Webhook trigger to get secret_ref.
    let key = state.key();
    let secret_ref = find_webhook_secret(&state.store, &key, &name);

    // HMAC verification if secret_ref is set.
    if let Some(ref secret_name) = secret_ref {
        verify_hmac_signature(&state.store, &key, secret_name, &headers, &body)?;
    }

    // Sanitize payload to prevent prompt injection.
    let payload_str = String::from_utf8_lossy(&body);
    let sanitized = aivyx_agent::sanitize::sanitize_webhook_payload(&payload_str);

    // Send notification to the TUI/agent.
    let notification = Notification {
        id: uuid::Uuid::new_v4().to_string(),
        kind: NotificationKind::Info,
        title: format!("Webhook: {name}"),
        body: sanitized,
        source: "webhook".into(),
        timestamp: chrono::Utc::now(),
        requires_approval: false,
        goal_id: None,
    };

    if let Err(e) = state.notify_tx.try_send(notification) {
        tracing::warn!("Failed to dispatch webhook notification: {e}");
    }

    Ok((
        StatusCode::ACCEPTED,
        axum::Json(serde_json::json!({
            "status": "accepted",
            "webhook": name,
        })),
    ))
}

// ── HMAC Verification ───────────────────────────────────────────

/// Verify the HMAC-SHA256 signature from the `X-Hub-Signature-256` header.
fn verify_hmac_signature(
    store: &EncryptedStore,
    key: &MasterKey,
    secret_name: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<(), (StatusCode, String)> {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    // Extract signature header.
    let signature = headers
        .get("x-hub-signature-256")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                "Missing X-Hub-Signature-256 header".into(),
            )
        })?;

    // Load secret from encrypted store.
    let secret_bytes = store
        .get(secret_name, key)
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to load webhook secret: {e}"),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Webhook secret '{secret_name}' not found in store"),
            )
        })?;

    // Compute HMAC-SHA256.
    let mut mac = Hmac::<Sha256>::new_from_slice(&secret_bytes).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Invalid HMAC key: {e}"),
        )
    })?;
    mac.update(body);

    // Parse expected signature: "sha256=hex_digest"
    let hex_digest = signature.strip_prefix("sha256=").ok_or_else(|| {
        (
            StatusCode::UNAUTHORIZED,
            "Invalid signature format (expected sha256=...)".into(),
        )
    })?;

    let expected_bytes = decode_hex(hex_digest)
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid hex in signature".into()))?;

    mac.verify_slice(&expected_bytes).map_err(|_| {
        (
            StatusCode::UNAUTHORIZED,
            "HMAC signature verification failed".into(),
        )
    })?;

    Ok(())
}

/// Find the `secret_ref` for a webhook trigger by name.
///
/// Searches workflow templates for a `WorkflowTrigger::Webhook` whose
/// template name matches the webhook path. Returns the secret_ref if found.
fn find_webhook_secret(
    store: &EncryptedStore,
    key: &MasterKey,
    webhook_name: &str,
) -> Option<String> {
    use aivyx_actions::workflow::{WorkflowTrigger, list_templates, load_template};

    let names = list_templates(store).ok()?;
    for name in &names {
        let template = match load_template(store, key, name) {
            Ok(Some(t)) => t,
            _ => continue,
        };

        // Check if this template has a webhook trigger.
        // The webhook name matches the template name by convention.
        if name == webhook_name {
            for trigger in &template.triggers {
                if let WorkflowTrigger::Webhook { secret_ref } = trigger {
                    return secret_ref.clone();
                }
            }
        }
    }
    None
}

// ── Hex Decode ──────────────────────────────────────────────────

/// Decode a hex string to bytes. Avoids adding the `hex` crate as a dependency.
fn decode_hex(hex: &str) -> Result<Vec<u8>, ()> {
    if !hex.len().is_multiple_of(2) {
        return Err(());
    }

    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| ()))
        .collect()
}

/// Encode bytes as lowercase hex. Used in tests and for HMAC generation.
#[cfg(test)]
fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ── Tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        let original = b"hello world";
        let hex = encode_hex(original);
        let decoded = decode_hex(&hex).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn hex_decode_invalid_length() {
        assert!(decode_hex("abc").is_err());
    }

    #[test]
    fn hex_decode_invalid_chars() {
        assert!(decode_hex("zzzz").is_err());
    }

    #[test]
    fn hmac_verification_works() {
        use hmac::{Hmac, Mac};
        use sha2::Sha256;

        let secret = b"test-secret";
        let body = b"webhook payload";

        // Generate valid signature.
        let mut mac = Hmac::<Sha256>::new_from_slice(secret).unwrap();
        mac.update(body);
        let result = mac.finalize();
        let hex_sig = encode_hex(&result.into_bytes());

        // Create store with the secret.
        let dir = tempfile::tempdir().unwrap();
        let store = EncryptedStore::open(dir.path().join("test.db")).unwrap();
        let key = MasterKey::from_bytes([99u8; 32]);
        store.put("webhook:my-hook", secret, &key).unwrap();

        // Build headers with valid signature.
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-hub-signature-256",
            format!("sha256={hex_sig}").parse().unwrap(),
        );

        // Should pass verification.
        let result = verify_hmac_signature(&store, &key, "webhook:my-hook", &headers, body);
        assert!(result.is_ok());
    }

    #[test]
    fn hmac_verification_rejects_bad_signature() {
        let dir = tempfile::tempdir().unwrap();
        let store = EncryptedStore::open(dir.path().join("test.db")).unwrap();
        let key = MasterKey::from_bytes([99u8; 32]);
        store.put("webhook:test", b"secret", &key).unwrap();

        let mut headers = HeaderMap::new();
        headers.insert(
            "x-hub-signature-256",
            "sha256=0000000000000000000000000000000000000000000000000000000000000000"
                .parse()
                .unwrap(),
        );

        let result = verify_hmac_signature(&store, &key, "webhook:test", &headers, b"body");
        assert!(result.is_err());
        let (status, _) = result.unwrap_err();
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn hmac_verification_rejects_missing_header() {
        let dir = tempfile::tempdir().unwrap();
        let store = EncryptedStore::open(dir.path().join("test.db")).unwrap();
        let key = MasterKey::from_bytes([99u8; 32]);
        store.put("webhook:test", b"secret", &key).unwrap();

        let headers = HeaderMap::new();
        let result = verify_hmac_signature(&store, &key, "webhook:test", &headers, b"body");
        assert!(result.is_err());
    }

    #[test]
    fn webhook_config_defaults() {
        let config = WebhookConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.port, 8975);
    }

    #[test]
    fn webhook_config_custom_parse() {
        let toml_str = r#"
            enabled = true
            port = 9090
        "#;
        let config: WebhookConfig = toml::from_str(toml_str).unwrap();
        assert!(config.enabled);
        assert_eq!(config.port, 9090);
    }
}
