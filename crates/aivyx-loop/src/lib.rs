//! aivyx-loop — Proactive agent loop for the personal assistant.
//!
//! The loop is what makes this an *assistant* instead of a chatbot.
//! It periodically wakes up, checks registered sources (email, calendar,
//! reminders), assesses what's new or urgent, and either takes autonomous
//! action or queues items for user approval.

pub mod briefing;
pub mod schedule;
pub mod sources;

use aivyx_actions::ActionRegistry;
use aivyx_core::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// An item the loop wants to tell the user about or get approval for.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub id: String,
    pub kind: NotificationKind,
    pub title: String,
    pub body: String,
    pub source: String,
    pub timestamp: DateTime<Utc>,
    pub requires_approval: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NotificationKind {
    /// New information (email arrived, reminder due, calendar event soon)
    Info,
    /// The assistant did something autonomously (filed a receipt, drafted a reply)
    ActionTaken,
    /// The assistant wants permission to do something
    ApprovalNeeded,
    /// Something urgent that needs attention
    Urgent,
}

/// Configuration for the agent loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopConfig {
    /// How often to run the full check cycle (minutes).
    pub check_interval_minutes: u32,
    /// Whether to run a morning briefing.
    pub morning_briefing: bool,
    /// Morning briefing hour (0-23, local time).
    pub briefing_hour: u8,
}

impl Default for LoopConfig {
    fn default() -> Self {
        Self {
            check_interval_minutes: 15,
            morning_briefing: true,
            briefing_hour: 8,
        }
    }
}

/// Handle to the running agent loop.
pub struct AgentLoop {
    config: LoopConfig,
    notification_tx: mpsc::UnboundedSender<Notification>,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

impl AgentLoop {
    /// Start the agent loop in a background task.
    /// Returns the loop handle and a receiver for notifications.
    pub fn start(
        config: LoopConfig,
        _actions: ActionRegistry,
    ) -> (Self, mpsc::UnboundedReceiver<Notification>) {
        let (notification_tx, notification_rx) = mpsc::unbounded_channel();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();

        let tx = notification_tx.clone();
        let cfg = config.clone();

        tokio::spawn(async move {
            run_loop(cfg, tx, shutdown_rx).await;
        });

        let handle = Self {
            config,
            notification_tx,
            shutdown_tx: Some(shutdown_tx),
        };

        (handle, notification_rx)
    }

    /// Stop the agent loop.
    pub fn stop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }

    /// Get the loop config.
    pub fn config(&self) -> &LoopConfig {
        &self.config
    }

    /// Manually trigger a check cycle (e.g. on app launch).
    pub fn trigger_check(&self) {
        let _ = self.notification_tx.send(Notification {
            id: uuid::Uuid::new_v4().to_string(),
            kind: NotificationKind::Info,
            title: "Check triggered".into(),
            body: "Manual check cycle requested".into(),
            source: "user".into(),
            timestamp: Utc::now(),
            requires_approval: false,
        });
    }
}

impl Drop for AgentLoop {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Main loop — checks sources on a schedule, sends notifications.
async fn run_loop(
    config: LoopConfig,
    tx: mpsc::UnboundedSender<Notification>,
    mut shutdown: tokio::sync::oneshot::Receiver<()>,
) {
    let interval = std::time::Duration::from_secs(config.check_interval_minutes as u64 * 60);

    loop {
        tokio::select! {
            _ = tokio::time::sleep(interval) => {
                // Run check cycle
                if let Err(e) = check_all_sources(&tx).await {
                    tracing::warn!("loop check cycle failed: {e}");
                }
            }
            _ = &mut shutdown => {
                tracing::info!("agent loop shutting down");
                break;
            }
        }
    }
}

/// Check all registered sources and emit notifications.
async fn check_all_sources(
    _tx: &mpsc::UnboundedSender<Notification>,
) -> Result<()> {
    // TODO: Check email, reminders, calendar, etc.
    // For each new item, send a Notification via tx
    Ok(())
}
