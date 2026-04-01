//! Schedule management — defines when the loop wakes up and what it checks.

use chrono::{Local, Timelike};

/// Determine if it's time for the morning briefing.
pub fn is_briefing_time(briefing_hour: u8) -> bool {
    let now = Local::now();
    now.hour() == briefing_hour as u32 && now.minute() < 5
}

/// Calculate seconds until the next check should run.
pub fn seconds_until_next_check(interval_minutes: u32) -> u64 {
    interval_minutes as u64 * 60
}

/// Calculate seconds until a specific hour today (or tomorrow if past).
pub fn seconds_until_hour(hour: u8) -> u64 {
    let now = Local::now();
    let target = now.date_naive().and_hms_opt(hour as u32, 0, 0).unwrap();
    let target = target.and_local_timezone(now.timezone()).unwrap();

    if target > now {
        (target - now).num_seconds() as u64
    } else {
        // Tomorrow
        (target + chrono::Duration::days(1) - now).num_seconds() as u64
    }
}
