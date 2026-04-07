//! Priority scoring — ranks items from multiple sources by urgency.
//!
//! The priority scorer takes signals from different context sources
//! (calendar, email, reminders, finance, goals) and produces a single
//! urgency score. This allows the heartbeat to focus LLM attention on
//! what matters most and drives the priority field in proactive suggestions.
//!
//! Scores range from 0.0 (no urgency) to 1.0 (critical). The scoring
//! is heuristic, not ML — fast, deterministic, and transparent.

use chrono::{DateTime, Utc};

/// A scored context item ready for ranking.
#[derive(Debug, Clone)]
pub struct ScoredItem {
    /// Human-readable summary of the item.
    pub summary: String,
    /// Which source this came from (e.g., "email", "calendar", "finance").
    pub source: String,
    /// Computed urgency score (0.0 – 1.0).
    pub score: f32,
    /// Priority label derived from score.
    pub priority: Priority,
}

/// Priority levels derived from urgency scores.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum Priority {
    Low,
    Normal,
    High,
    Urgent,
}

impl Priority {
    pub fn as_str(&self) -> &'static str {
        match self {
            Priority::Low => "low",
            Priority::Normal => "normal",
            Priority::High => "high",
            Priority::Urgent => "urgent",
        }
    }

    pub fn from_score(score: f32) -> Self {
        match score {
            s if s >= 0.8 => Priority::Urgent,
            s if s >= 0.6 => Priority::High,
            s if s >= 0.3 => Priority::Normal,
            _ => Priority::Low,
        }
    }
}

impl std::fmt::Display for Priority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ── Scoring functions ────────────────────────────────────────

/// Score a calendar event by proximity to now.
///
/// Events happening within 30 minutes score highest.
/// Events today but later score moderate.
/// Past events (already happened today) score low.
pub fn score_calendar_event(event_time: DateTime<Utc>, now: DateTime<Utc>) -> f32 {
    let minutes_until = event_time.signed_duration_since(now).num_minutes();

    match minutes_until {
        m if m < 0 => 0.1,          // already past
        0..=15 => 0.95,             // imminent
        16..=30 => 0.8,             // very soon
        31..=60 => 0.6,             // within the hour
        61..=120 => 0.4,            // next couple hours
        _ => 0.2,                   // later today
    }
}

/// Score a calendar conflict (always high urgency).
pub fn score_calendar_conflict() -> f32 {
    0.85
}

/// Score an email by age (older unanswered = more urgent).
///
/// `age_hours` is how old the email is.
pub fn score_email_age(age_hours: i64) -> f32 {
    match age_hours {
        0..=2 => 0.3,       // just arrived
        3..=12 => 0.4,      // same day
        13..=24 => 0.5,     // yesterday
        25..=72 => 0.65,    // 1-3 days old
        73..=168 => 0.75,   // 3-7 days old
        _ => 0.85,          // over a week — needs attention
    }
}

/// Score a reminder by overdue-ness.
pub fn score_reminder(due_at: DateTime<Utc>, now: DateTime<Utc>) -> f32 {
    let minutes_overdue = now.signed_duration_since(due_at).num_minutes();

    match minutes_overdue {
        m if m < -60 => 0.2,    // not due for over an hour
        -60..=0 => 0.6,         // due within the hour
        1..=30 => 0.8,          // just overdue
        _ => 0.95,              // significantly overdue
    }
}

/// Score a budget alert (over budget is always high).
pub fn score_over_budget() -> f32 {
    0.7
}

/// Score an upcoming bill by days until due.
pub fn score_upcoming_bill(days_until_due: i64) -> f32 {
    match days_until_due {
        d if d < 0 => 0.9,      // overdue!
        0 => 0.85,               // due today
        1 => 0.7,                // due tomorrow
        2..=3 => 0.5,            // due soon
        _ => 0.3,                // due later this week
    }
}

/// Score a stale goal (no progress update recently).
///
/// `days_since_update` is how long since the goal was last updated.
pub fn score_stale_goal(days_since_update: i64, progress: f32) -> f32 {
    // Nearly-complete goals that stalled are more urgent
    let base: f32 = match days_since_update {
        0..=2 => 0.1,
        3..=7 => 0.3,
        8..=14 => 0.5,
        _ => 0.65,
    };

    // Boost if the goal is close to completion but stalled
    if progress >= 0.7 && days_since_update > 3 {
        (base + 0.15).min(1.0)
    } else {
        base
    }
}

/// Sort scored items by score descending (most urgent first).
pub fn rank(items: &mut [ScoredItem]) {
    items.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
}

/// Format scored items into a prioritized summary for the heartbeat prompt.
///
/// Returns an empty string if no items score above the threshold.
pub fn format_priority_summary(items: &[ScoredItem], max_items: usize, min_score: f32) -> String {
    let relevant: Vec<&ScoredItem> = items
        .iter()
        .filter(|i| i.score >= min_score)
        .take(max_items)
        .collect();

    if relevant.is_empty() {
        return String::new();
    }

    relevant
        .iter()
        .map(|item| {
            format!(
                "- [{}] ({}) {}",
                item.priority.as_str().to_uppercase(),
                item.source,
                item.summary,
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ── Tests ────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn calendar_imminent_scores_high() {
        let now = Utc::now();
        let event = now + Duration::minutes(10);
        assert!(score_calendar_event(event, now) >= 0.9);
    }

    #[test]
    fn calendar_past_scores_low() {
        let now = Utc::now();
        let event = now - Duration::minutes(30);
        assert!(score_calendar_event(event, now) < 0.2);
    }

    #[test]
    fn calendar_later_today_scores_moderate() {
        let now = Utc::now();
        let event = now + Duration::hours(3);
        let score = score_calendar_event(event, now);
        assert!(score >= 0.1 && score <= 0.5);
    }

    #[test]
    fn email_age_increases_urgency() {
        let fresh = score_email_age(1);
        let day_old = score_email_age(24);
        let week_old = score_email_age(168);
        assert!(fresh < day_old);
        assert!(day_old < week_old);
    }

    #[test]
    fn reminder_overdue_scores_high() {
        let now = Utc::now();
        let due = now - Duration::hours(2);
        assert!(score_reminder(due, now) >= 0.9);
    }

    #[test]
    fn reminder_future_scores_low() {
        let now = Utc::now();
        let due = now + Duration::hours(3);
        assert!(score_reminder(due, now) < 0.3);
    }

    #[test]
    fn bill_overdue_scores_highest() {
        assert!(score_upcoming_bill(-1) >= 0.85);
    }

    #[test]
    fn stale_goal_near_completion_boosted() {
        let base = score_stale_goal(7, 0.3);
        let near_done = score_stale_goal(7, 0.8);
        assert!(near_done > base);
    }

    #[test]
    fn priority_from_score_thresholds() {
        assert_eq!(Priority::from_score(0.95), Priority::Urgent);
        assert_eq!(Priority::from_score(0.7), Priority::High);
        assert_eq!(Priority::from_score(0.4), Priority::Normal);
        assert_eq!(Priority::from_score(0.1), Priority::Low);
    }

    #[test]
    fn rank_sorts_descending() {
        let mut items = vec![
            ScoredItem { summary: "low".into(), source: "test".into(), score: 0.2, priority: Priority::Low },
            ScoredItem { summary: "high".into(), source: "test".into(), score: 0.9, priority: Priority::Urgent },
            ScoredItem { summary: "mid".into(), source: "test".into(), score: 0.5, priority: Priority::Normal },
        ];
        rank(&mut items);
        assert_eq!(items[0].summary, "high");
        assert_eq!(items[1].summary, "mid");
        assert_eq!(items[2].summary, "low");
    }

    #[test]
    fn format_priority_summary_filters_by_score() {
        let items = vec![
            ScoredItem { summary: "urgent thing".into(), source: "email".into(), score: 0.9, priority: Priority::Urgent },
            ScoredItem { summary: "low thing".into(), source: "goal".into(), score: 0.1, priority: Priority::Low },
        ];
        let formatted = format_priority_summary(&items, 10, 0.3);
        assert!(formatted.contains("urgent thing"));
        assert!(!formatted.contains("low thing"));
    }

    #[test]
    fn format_empty_when_no_items_above_threshold() {
        let items = vec![
            ScoredItem { summary: "low".into(), source: "test".into(), score: 0.1, priority: Priority::Low },
        ];
        assert!(format_priority_summary(&items, 10, 0.5).is_empty());
    }
}
