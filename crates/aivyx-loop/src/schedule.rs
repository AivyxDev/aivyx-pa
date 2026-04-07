//! Schedule management — defines when the loop wakes up and what it checks.

use chrono::{Local, Timelike};

/// Determine if it's time for the morning briefing.
pub fn is_briefing_time(briefing_hour: u8) -> bool {
    let now = Local::now();
    now.hour() == briefing_hour as u32 && now.minute() < 5
}

/// Calculate seconds until a specific hour today (or tomorrow if past).
pub fn seconds_until_hour(hour: u8) -> u64 {
    let now = Local::now();
    let hour = (hour as u32).min(23);
    let Some(target) = now.date_naive().and_hms_opt(hour, 0, 0) else {
        return 60;
    };
    // Use earliest() to handle DST ambiguity safely, fall back to 60s
    let Some(target) = target.and_local_timezone(now.timezone()).earliest() else {
        return 60;
    };

    if target > now {
        (target - now).num_seconds() as u64
    } else {
        // Tomorrow
        let Some(tomorrow) = now.date_naive().succ_opt()
            .and_then(|d| d.and_hms_opt(hour, 0, 0)) else {
            return 3600;
        };
        match tomorrow.and_local_timezone(now.timezone()).earliest() {
            Some(t) => (t - now).num_seconds() as u64,
            None => 3600, // Fallback: 1 hour
        }
    }
}
