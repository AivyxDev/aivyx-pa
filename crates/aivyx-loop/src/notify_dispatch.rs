//! Proactive notification dispatch — routes agent loop notifications to
//! real-world output channels so the user is alerted even when not looking
//! at the TUI.
//!
//! Dispatch channels (in priority order):
//! 1. Desktop: `notify-send` (libnotify) for immediate desktop popups
//! 2. Telegram: bot message if configured and user is away
//! 3. Signal: message via signal-cli if configured
//!
//! The dispatcher respects quiet hours and notification pacing set in config.

use crate::{Notification, NotificationKind};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

// ── Config ──────────────────────────────────────────────────────

/// Configuration for proactive notification delivery.
///
/// ```toml
/// [notifications]
/// desktop = true
/// urgency_level = "normal"   # low | normal | critical
/// telegram = true            # forward to Telegram if configured
/// signal = false
/// quiet_hours_start = 22     # 10 PM (local time, 0-23)
/// quiet_hours_end = 8        # 8 AM (local time, 0-23)
/// min_kind = "Info"          # minimum kind to dispatch: Info | ActionTaken | ApprovalNeeded | Urgent
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationDispatchConfig {
    /// Send desktop notifications via `notify-send`. Default: true.
    #[serde(default = "default_true")]
    pub desktop: bool,

    /// Urgency level for desktop notifications: low | normal | critical.
    #[serde(default = "default_urgency")]
    pub urgency_level: String,

    /// Also forward notifications to Telegram (if Telegram is configured).
    /// Default: false (user must opt-in to avoid spam).
    #[serde(default)]
    pub telegram: bool,

    /// Also forward notifications to Signal (if Signal is configured).
    /// Default: false.
    #[serde(default)]
    pub signal: bool,

    /// Quiet hours start (0-23, local time). No dispatch during quiet hours
    /// unless notification is `Urgent`. None = no quiet hours.
    #[serde(default)]
    pub quiet_hours_start: Option<u8>,

    /// Quiet hours end (0-23, local time).
    #[serde(default)]
    pub quiet_hours_end: Option<u8>,

    /// Minimum notification kind to dispatch externally.
    /// "Info" = all, "ActionTaken" = actions + approvals + urgent,
    /// "ApprovalNeeded" = approvals + urgent, "Urgent" = only urgent.
    #[serde(default = "default_min_kind")]
    pub min_kind: MinNotificationKind,
}

fn default_true() -> bool { true }
fn default_urgency() -> String { "normal".into() }
fn default_min_kind() -> MinNotificationKind { MinNotificationKind::Info }

impl Default for NotificationDispatchConfig {
    fn default() -> Self {
        Self {
            desktop: true,
            urgency_level: default_urgency(),
            telegram: false,
            signal: false,
            quiet_hours_start: None,
            quiet_hours_end: None,
            min_kind: MinNotificationKind::Info,
        }
    }
}

/// Minimum notification kind threshold for external dispatch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum MinNotificationKind {
    Info,
    ActionTaken,
    ApprovalNeeded,
    Urgent,
}

impl MinNotificationKind {
    fn from_notification_kind(kind: &NotificationKind) -> Self {
        match kind {
            NotificationKind::Info        => Self::Info,
            NotificationKind::ActionTaken => Self::ActionTaken,
            NotificationKind::ApprovalNeeded => Self::ApprovalNeeded,
            NotificationKind::Urgent      => Self::Urgent,
        }
    }
}

// ── Dispatch Context ────────────────────────────────────────────

/// Everything needed to dispatch notifications to external channels.
pub struct DispatchContext {
    pub config: NotificationDispatchConfig,
    pub telegram: Option<crate::MessagingCtx>,
}

// ── Main Dispatcher ─────────────────────────────────────────────

/// Spawn a background task that drains the notification channel and
/// dispatches each notification to all configured output channels.
///
/// Returns a `JoinHandle` that runs for the lifetime of the process.
pub fn spawn_dispatcher(
    mut rx: mpsc::Receiver<Notification>,
    ctx: DispatchContext,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(notif) = rx.recv().await {
            dispatch_one(&notif, &ctx).await;
        }
        tracing::debug!("Notification dispatcher shut down (channel closed)");
    })
}

/// Dispatch a single notification to all enabled channels.
async fn dispatch_one(notif: &Notification, ctx: &DispatchContext) {
    // Check minimum kind threshold
    let notif_kind_level = MinNotificationKind::from_notification_kind(&notif.kind);
    if notif_kind_level < ctx.config.min_kind {
        return;
    }

    // Check quiet hours (only bypass for Urgent)
    if !matches!(notif.kind, NotificationKind::Urgent)
        && is_quiet_hours(&ctx.config) {
        tracing::debug!(
            title = %notif.title,
            "Suppressing notification during quiet hours"
        );
        return;
    }

    // 1. Desktop notification
    if ctx.config.desktop {
        send_desktop_notification(notif, &ctx.config.urgency_level);
    }

    // 2. Telegram
    if ctx.config.telegram {
        if let Some(ref msg_ctx) = ctx.telegram {
            if let Some(ref tg) = msg_ctx.telegram {
                send_telegram_notification(notif, tg).await;
            }
        }
    }

    // 3. Signal
    if ctx.config.signal {
        if let Some(ref msg_ctx) = ctx.telegram {
            if let Some(ref sig) = msg_ctx.signal {
                send_signal_notification(notif, sig).await;
            }
        }
    }
}

// ── Desktop (notify-send) ───────────────────────────────────────

/// Send a desktop notification using `notify-send` (libnotify).
///
/// Runs synchronously via `std::process::Command` since desktop
/// notifications are fire-and-forget and don't need async overhead.
/// Falls back gracefully if `notify-send` is not installed.
fn send_desktop_notification(notif: &Notification, urgency: &str) {
    // Map notification kind to urgency if not overridden
    let effective_urgency = match notif.kind {
        NotificationKind::Urgent | NotificationKind::ApprovalNeeded => "critical",
        NotificationKind::ActionTaken => "normal",
        NotificationKind::Info => urgency,
    };

    // Map kind to a hint icon
    let icon = match notif.kind {
        NotificationKind::Urgent          => "dialog-warning",
        NotificationKind::ApprovalNeeded  => "dialog-question",
        NotificationKind::ActionTaken     => "dialog-information",
        NotificationKind::Info            => "dialog-information",
    };

    // Truncate body to avoid giant popups
    let body = truncate_str(&notif.body, 200);

    // Build notify-send command
    // -a: application name, -u: urgency, -i: icon, -t: timeout (ms)
    // timeout: 10s for Info/ActionTaken, 0 (sticky) for Urgent/ApprovalNeeded
    let timeout_ms: i32 = match notif.kind {
        NotificationKind::Urgent | NotificationKind::ApprovalNeeded => 0,
        _ => 10_000,
    };

    let status = std::process::Command::new("notify-send")
        .args([
            "--app-name=Aivyx",
            &format!("--urgency={effective_urgency}"),
            &format!("--icon={icon}"),
            &format!("--expire-time={timeout_ms}"),
            &notif.title,
            &body,
        ])
        .status();

    match status {
        Ok(s) if s.success() => {
            tracing::debug!(title = %notif.title, "Desktop notification sent");
        }
        Ok(s) => {
            tracing::warn!(
                title = %notif.title,
                exit_code = ?s.code(),
                "notify-send exited with non-zero status"
            );
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // notify-send not installed — log once and continue
            tracing::debug!(
                "notify-send not found — desktop notifications require libnotify-bin. \
                 Install with: sudo apt install libnotify-bin"
            );
        }
        Err(e) => {
            tracing::warn!(error = %e, "Failed to run notify-send");
        }
    }
}

// ── Telegram ────────────────────────────────────────────────────

/// Forward a notification to Telegram.
async fn send_telegram_notification(
    notif: &Notification,
    config: &aivyx_actions::messaging::TelegramConfig,
) {
    let kind_tag = match notif.kind {
        NotificationKind::Urgent          => "[URGENT]",
        NotificationKind::ApprovalNeeded  => "[APPROVAL NEEDED]",
        NotificationKind::ActionTaken     => "[ACTION]",
        NotificationKind::Info            => "[INFO]",
    };
    let body = format!("{kind_tag} {}\n\n{}",
        notif.title,
        truncate_str(&notif.body, 400),
    );

    // Use the existing forward_notification helper which reads default_chat_id internally.
    if let Err(e) = aivyx_actions::messaging::telegram::forward_notification(
        config, &notif.title, &body
    ).await {
        tracing::warn!(error = %e, "Failed to send Telegram notification");
    } else {
        tracing::debug!(title = %notif.title, "Telegram notification sent");
    }
}

// ── Signal ──────────────────────────────────────────────────────

/// Forward a notification to Signal.
async fn send_signal_notification(
    notif: &Notification,
    config: &aivyx_actions::messaging::SignalConfig,
) {
    let kind_tag = match notif.kind {
        NotificationKind::Urgent          => "[URGENT]",
        NotificationKind::ApprovalNeeded  => "[APPROVAL NEEDED]",
        NotificationKind::ActionTaken     => "[ACTION]",
        NotificationKind::Info            => "[INFO]",
    };
    let body = format!("{kind_tag} {}\n\n{}",
        notif.title,
        truncate_str(&notif.body, 400),
    );

    // Use the existing forward_notification helper which reads default_recipient internally.
    if let Err(e) = aivyx_actions::messaging::signal::forward_notification(
        config, &notif.title, &body
    ).await {
        tracing::warn!(error = %e, "Failed to send Signal notification");
    } else {
        tracing::debug!(title = %notif.title, "Signal notification sent");
    }
}

// ── Utilities ───────────────────────────────────────────────────

/// Truncate a string to `max_chars` with an ellipsis.
fn truncate_str(s: &str, max_chars: usize) -> String {
    let mut chars = s.chars();
    let mut result: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        result.push_str("…");
    }
    result
}

/// Check whether the current local time falls within configured quiet hours.
fn is_quiet_hours(config: &NotificationDispatchConfig) -> bool {
    let (Some(start), Some(end)) = (config.quiet_hours_start, config.quiet_hours_end) else {
        return false;
    };

    let hour = chrono::Local::now().hour() as u8;

    if start <= end {
        // Simple range, e.g., 09:00-17:00
        hour >= start && hour < end
    } else {
        // Wraps midnight, e.g., 22:00-08:00
        hour >= start || hour < end
    }
}

// Needed for is_quiet_hours
use chrono::Timelike;
