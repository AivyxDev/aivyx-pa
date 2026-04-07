//! Messaging action tools — Telegram and Matrix.
//!
//! Provides send/read capabilities for instant messaging platforms.
//! Secrets (bot tokens, access tokens) are stored in the encrypted
//! keystore, never in config.toml.

pub mod matrix;
pub mod signal;
pub mod sms;
pub mod telegram;

use serde::{Deserialize, Serialize};
use std::fmt;

// ── Shared Types ───────────────────────────────────────────────

/// A normalized message returned by both platforms.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Platform-specific message ID (stringified).
    pub id: String,
    /// Sender display name or identifier.
    pub from: String,
    /// Message text content.
    pub text: String,
    /// ISO 8601 timestamp.
    pub timestamp: String,
}

// ── Telegram Config ────────────────────────────────────────────

/// Telegram runtime config (with secret included).
///
/// Created by `PaConfig::resolve_telegram_config()` which loads
/// the bot token from the encrypted keystore.
#[derive(Clone)]
pub struct TelegramConfig {
    /// Bot token from keystore (`TELEGRAM_BOT_TOKEN`).
    pub bot_token: String,
    /// Default chat ID for sending notifications (optional).
    pub default_chat_id: Option<String>,
}

impl fmt::Debug for TelegramConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TelegramConfig")
            .field("bot_token", &"[REDACTED]")
            .field("default_chat_id", &self.default_chat_id)
            .finish()
    }
}

// ── Matrix Config ──────────────────────────────────────────────

/// Matrix runtime config (with secret included).
///
/// Created by `PaConfig::resolve_matrix_config()` which loads
/// the access token from the encrypted keystore.
#[derive(Clone)]
pub struct MatrixConfig {
    /// Homeserver base URL (e.g., `https://matrix.example.com`).
    pub homeserver: String,
    /// Access token from keystore (`MATRIX_ACCESS_TOKEN`).
    pub access_token: String,
    /// Default room ID for sending notifications (optional).
    pub default_room_id: Option<String>,
}

impl fmt::Debug for MatrixConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MatrixConfig")
            .field("homeserver", &self.homeserver)
            .field("access_token", &"[REDACTED]")
            .field("default_room_id", &self.default_room_id)
            .finish()
    }
}

// ── Signal Config ─────────────────────────────────────────────

/// Signal runtime config (with account phone number).
///
/// Created by `PaConfig::resolve_signal_config()`. Uses signal-cli's
/// JSON-RPC interface (via unix socket or TCP) for sending/receiving.
#[derive(Clone)]
pub struct SignalConfig {
    /// The phone number registered with signal-cli (e.g., +15551234567).
    pub account: String,
    /// signal-cli JSON-RPC socket path or TCP address.
    /// Default: `$XDG_RUNTIME_DIR/signal-cli/socket` or `/var/run/signal-cli/socket`.
    pub socket_path: String,
    /// Default recipient phone number for notifications (optional).
    pub default_recipient: Option<String>,
}

impl fmt::Debug for SignalConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SignalConfig")
            .field("account", &self.account)
            .field("socket_path", &self.socket_path)
            .field("default_recipient", &self.default_recipient)
            .finish()
    }
}

// ── SMS Config ────────────────────────────────────────────────

/// SMS gateway runtime config.
///
/// Created by `PaConfig::resolve_sms_config()` which loads the auth
/// token from the encrypted keystore.
#[derive(Clone)]
pub struct SmsConfig {
    /// SMS provider: "twilio" or "vonage".
    pub provider: SmsProvider,
    /// API base URL (defaults per provider if not set).
    pub api_url: Option<String>,
    /// Account SID (Twilio) or API key (Vonage).
    pub account_id: String,
    /// Auth token from keystore — never in config.toml.
    pub auth_token: String,
    /// Phone number to send from (e.g., +15551234567).
    pub from_number: String,
    /// Default recipient for notifications (optional).
    pub default_recipient: Option<String>,
}

impl fmt::Debug for SmsConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SmsConfig")
            .field("provider", &self.provider)
            .field("api_url", &self.api_url)
            .field("account_id", &self.account_id)
            .field("auth_token", &"[REDACTED]")
            .field("from_number", &self.from_number)
            .field("default_recipient", &self.default_recipient)
            .finish()
    }
}

/// Supported SMS providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SmsProvider {
    Twilio,
    Vonage,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telegram_debug_redacts_token() {
        let config = TelegramConfig {
            bot_token: "123456:ABC-DEF1234ghIkl-zyx57W2v1u123ew11".into(),
            default_chat_id: Some("42".into()),
        };
        let debug = format!("{config:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("123456:ABC"));
        assert!(debug.contains("42"));
    }

    #[test]
    fn matrix_debug_redacts_token() {
        let config = MatrixConfig {
            homeserver: "https://matrix.example.com".into(),
            access_token: "syt_secret_token_here".into(),
            default_room_id: Some("!room:example.com".into()),
        };
        let debug = format!("{config:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("syt_secret"));
        assert!(debug.contains("matrix.example.com"));
        assert!(debug.contains("!room:example.com"));
    }

    #[test]
    fn signal_debug_shows_account() {
        let config = SignalConfig {
            account: "+15551234567".into(),
            socket_path: "/var/run/signal-cli/socket".into(),
            default_recipient: Some("+15559876543".into()),
        };
        let debug = format!("{config:?}");
        assert!(debug.contains("+15551234567"));
        assert!(debug.contains("signal-cli"));
    }

    #[test]
    fn sms_debug_redacts_token() {
        let config = SmsConfig {
            provider: SmsProvider::Twilio,
            api_url: None,
            account_id: "AC123".into(),
            auth_token: "secret_auth_token".into(),
            from_number: "+15551234567".into(),
            default_recipient: None,
        };
        let debug = format!("{config:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("secret_auth"));
        assert!(debug.contains("AC123"));
        assert!(debug.contains("+15551234567"));
    }

    #[test]
    fn sms_provider_deserializes() {
        let twilio: SmsProvider = serde_json::from_str("\"twilio\"").unwrap();
        assert_eq!(twilio, SmsProvider::Twilio);
        let vonage: SmsProvider = serde_json::from_str("\"vonage\"").unwrap();
        assert_eq!(vonage, SmsProvider::Vonage);
    }
}
