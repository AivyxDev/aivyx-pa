//! Communication pacing — notification throttling based on user activity,
//! time of day, and estimated mood.
//!
//! The pacing module operates at the Rust level, BEFORE notifications reach
//! the user. It provides hard delivery constraints (quiet hours, rate limits)
//! while the LLM handles soft tone adaptation via mood context.

use chrono::Timelike;

use crate::{InteractionSignals, MoodSignal, NotificationKind};

/// Decision returned by the pacing engine for each notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PacingDecision {
    /// Deliver the notification immediately.
    Send,
    /// Defer delivery — notification is logged but not sent.
    Defer { reason: &'static str },
}

/// Evaluate whether a notification should be sent right now.
///
/// Rules (in priority order):
/// 1. `Urgent` notifications always send.
/// 2. During quiet hours, only Urgent sends.
/// 3. If hourly rate limit exceeded, defer non-urgent.
/// 4. If user mood is Frustrated, only Urgent + ActionTaken.
/// 5. If user is actively engaged (high message rate), defer Info.
pub fn should_send(
    kind: &NotificationKind,
    signals: &InteractionSignals,
    mood: &MoodSignal,
    quiet_hours: Option<(u8, u8)>,
    max_per_hour: u8,
) -> PacingDecision {
    // Rule 1: Urgent and ApprovalNeeded always send
    if matches!(
        kind,
        NotificationKind::Urgent | NotificationKind::ApprovalNeeded
    ) {
        return PacingDecision::Send;
    }

    // Rule 2: Quiet hours — only Urgent passes (already handled above)
    if let Some((start, end)) = quiet_hours {
        let hour = chrono::Local::now().hour() as u8;
        let in_quiet = if start <= end {
            hour >= start && hour < end
        } else {
            hour >= start || hour < end
        };
        if in_quiet {
            return PacingDecision::Defer {
                reason: "quiet hours",
            };
        }
    }

    // Rule 3: Hourly rate limit
    if signals.notifications_sent_this_hour >= max_per_hour as u32 {
        return PacingDecision::Defer {
            reason: "hourly rate limit exceeded",
        };
    }

    // Rule 4: Frustrated mood — only Urgent (handled) + ActionTaken
    if *mood == MoodSignal::Frustrated && !matches!(kind, NotificationKind::ActionTaken) {
        return PacingDecision::Defer {
            reason: "user appears frustrated",
        };
    }

    // Rule 5: Active engagement — defer Info during active conversation
    if matches!(kind, NotificationKind::Info) {
        if let Some(idle) = signals.idle_minutes() {
            if idle < 2 && signals.message_count_session > 5 {
                return PacingDecision::Defer {
                    reason: "user actively engaged",
                };
            }
        }
    }

    PacingDecision::Send
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signals_default() -> InteractionSignals {
        InteractionSignals::new()
    }

    #[test]
    fn urgent_always_sends() {
        let signals = signals_default();
        let mood = MoodSignal::Frustrated;
        let result = should_send(
            &NotificationKind::Urgent,
            &signals,
            &mood,
            Some((0, 24)), // always quiet
            0,             // zero rate limit
        );
        assert_eq!(result, PacingDecision::Send);
    }

    #[test]
    fn quiet_hours_defers_info() {
        let signals = signals_default();
        let mood = MoodSignal::Neutral;
        let hour = chrono::Local::now().hour() as u8;
        // Set quiet hours to include the current hour
        let result = should_send(
            &NotificationKind::Info,
            &signals,
            &mood,
            Some((hour, hour.wrapping_add(1) % 24)),
            10,
        );
        assert_eq!(
            result,
            PacingDecision::Defer {
                reason: "quiet hours"
            }
        );
    }

    #[test]
    fn rate_limit_defers() {
        let mut signals = signals_default();
        // Simulate sending 5 notifications this hour
        for _ in 0..5 {
            signals.record_notification_sent();
        }
        let mood = MoodSignal::Neutral;
        let result = should_send(
            &NotificationKind::Info,
            &signals,
            &mood,
            None,
            5, // max 5 per hour
        );
        assert_eq!(
            result,
            PacingDecision::Defer {
                reason: "hourly rate limit exceeded"
            }
        );
    }

    #[test]
    fn frustrated_mood_defers_info() {
        let mut signals = signals_default();
        // Need 3+ messages for mood detection but we pass mood directly
        signals.message_count_session = 5;
        let mood = MoodSignal::Frustrated;
        let result = should_send(&NotificationKind::Info, &signals, &mood, None, 10);
        assert_eq!(
            result,
            PacingDecision::Defer {
                reason: "user appears frustrated"
            }
        );
    }

    #[test]
    fn frustrated_allows_action_taken() {
        let signals = signals_default();
        let mood = MoodSignal::Frustrated;
        let result = should_send(&NotificationKind::ActionTaken, &signals, &mood, None, 10);
        assert_eq!(result, PacingDecision::Send);
    }
}
