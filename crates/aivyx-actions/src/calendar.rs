//! Calendar integration — CalDAV client for reading/writing events.
//!
//! Supports any CalDAV-compliant server (Google Calendar, Nextcloud,
//! iCloud, Radicale, etc.) via standard PROPFIND/REPORT requests.

use aivyx_core::Result;
use chrono::{DateTime, NaiveDate, Utc};
use icalendar::EventLike;
use serde::{Deserialize, Serialize};

use crate::Action;

// ── Configuration ─────────────────────────────────────────────────

/// CalDAV connection configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarConfig {
    /// CalDAV server URL (e.g., "https://cal.example.com/dav/calendars/user/")
    pub url: String,
    /// Username for Basic auth.
    pub username: String,
    /// Password for Basic auth — loaded from encrypted keystore at runtime.
    #[serde(skip)]
    pub password: String,
    /// Optional specific calendar path (if not set, discovers first calendar).
    pub calendar_path: Option<String>,
}

// ── Data types ────────────────────────────────────────────────────

/// A calendar event parsed from iCalendar VEVENT data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarEvent {
    pub uid: String,
    pub summary: String,
    pub start: DateTime<Utc>,
    pub end: Option<DateTime<Utc>>,
    pub location: Option<String>,
    pub description: Option<String>,
    pub all_day: bool,
}

// ── CalDAV client ─────────────────────────────────────────────────

/// Fetch events from a CalDAV server for a given date range.
pub async fn fetch_events(
    config: &CalendarConfig,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Result<Vec<CalendarEvent>> {
    let client = crate::http_client();

    // Determine calendar URL: use explicit path or discover.
    let calendar_url = if let Some(ref path) = config.calendar_path {
        if path.starts_with("http") {
            path.clone()
        } else {
            format!("{}{}", config.url.trim_end_matches('/'), path)
        }
    } else {
        discover_calendar(client, config).await?
    };

    // REPORT: calendar-query with time-range filter.
    let report_body = build_calendar_query_xml(&from, &to);

    let response = crate::retry::retry(
        &crate::retry::RetryConfig::network(),
        || async {
            client
                .request(
                    reqwest::Method::from_bytes(b"REPORT").unwrap(),
                    &calendar_url,
                )
                .basic_auth(&config.username, Some(&config.password))
                .header("Depth", "1")
                .header("Content-Type", "application/xml; charset=utf-8")
                .body(report_body.clone())
                .send()
                .await
                .map_err(|e| aivyx_core::AivyxError::Http(e.to_string()))
        },
        crate::retry::is_transient,
    )
    .await?;

    let status = response.status();
    if !status.is_success() && status.as_u16() != 207 {
        return Err(aivyx_core::AivyxError::Http(format!(
            "CalDAV REPORT failed: HTTP {status}"
        )));
    }

    let body = response
        .text()
        .await
        .map_err(|e| aivyx_core::AivyxError::Http(e.to_string()))?;

    parse_multistatus_events(&body)
}

/// Discover the first calendar URL via PROPFIND.
async fn discover_calendar(
    client: &reqwest::Client,
    config: &CalendarConfig,
) -> Result<String> {
    let propfind_body = r#"<?xml version="1.0" encoding="UTF-8"?>
<d:propfind xmlns:d="DAV:" xmlns:cs="urn:ietf:params:xml:ns:caldav">
  <d:prop>
    <d:resourcetype/>
    <d:displayname/>
  </d:prop>
</d:propfind>"#;

    let response = client
        .request(
            reqwest::Method::from_bytes(b"PROPFIND").unwrap(),
            &config.url,
        )
        .basic_auth(&config.username, Some(&config.password))
        .header("Depth", "1")
        .header("Content-Type", "application/xml; charset=utf-8")
        .body(propfind_body)
        .send()
        .await
        .map_err(|e| aivyx_core::AivyxError::Http(format!("CalDAV PROPFIND failed: {e}")))?;

    let body = response
        .text()
        .await
        .map_err(|e| aivyx_core::AivyxError::Http(e.to_string()))?;

    // Parse the multistatus XML to find a calendar resource.
    let doc = roxmltree::Document::parse(&body).map_err(|e| {
        aivyx_core::AivyxError::Http(format!("CalDAV XML parse error: {e}"))
    })?;

    // Look for <response> elements that have <resourcetype><calendar/> inside.
    for response_node in doc.descendants().filter(|n| n.has_tag_name("response")) {
        let is_calendar = response_node.descendants().any(|n| n.has_tag_name("calendar"));
        if is_calendar
            && let Some(href_node) = response_node.descendants().find(|n| n.has_tag_name("href"))
                && let Some(href) = href_node.text() {
                    // Convert relative href to absolute URL.
                    return Ok(resolve_url(&config.url, href));
                }
    }

    Err(aivyx_core::AivyxError::Http(
        "No calendar found via PROPFIND discovery".into(),
    ))
}

/// Build the XML body for a CalDAV calendar-query REPORT with time-range filter.
fn build_calendar_query_xml(from: &DateTime<Utc>, to: &DateTime<Utc>) -> String {
    let from_str = from.format("%Y%m%dT%H%M%SZ").to_string();
    let to_str = to.format("%Y%m%dT%H%M%SZ").to_string();

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop>
    <d:getetag/>
    <c:calendar-data/>
  </d:prop>
  <c:filter>
    <c:comp-filter name="VCALENDAR">
      <c:comp-filter name="VEVENT">
        <c:time-range start="{from_str}" end="{to_str}"/>
      </c:comp-filter>
    </c:comp-filter>
  </c:filter>
</c:calendar-query>"#
    )
}

use crate::resolve_url;

// ── iCalendar parsing ─────────────────────────────────────────────

/// Parse a CalDAV multistatus XML response and extract VEVENT data.
fn parse_multistatus_events(xml: &str) -> Result<Vec<CalendarEvent>> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| {
        aivyx_core::AivyxError::Http(format!("CalDAV XML parse error: {e}"))
    })?;

    let mut events = Vec::new();

    // Find all <cal:calendar-data> or <C:calendar-data> text nodes.
    for node in doc.descendants() {
        if node.has_tag_name("calendar-data")
            && let Some(ical_text) = node.text()
                && let Ok(parsed) = parse_ical_events(ical_text) {
                    events.extend(parsed);
                }
    }

    // Sort by start time.
    events.sort_by_key(|e| e.start);
    Ok(events)
}

/// Parse iCalendar text into CalendarEvent structs.
fn parse_ical_events(ical_text: &str) -> Result<Vec<CalendarEvent>> {
    use icalendar::{Calendar, CalendarComponent, Component};
    use std::str::FromStr;

    let calendar = Calendar::from_str(ical_text).map_err(|e| {
        aivyx_core::AivyxError::Other(format!("iCal parse error: {e}"))
    })?;

    let mut events = Vec::new();

    for component in calendar.iter() {
        if let CalendarComponent::Event(event) = component {
            let uid = event
                .get_uid()
                .unwrap_or("unknown")
                .to_string();

            let summary = event
                .get_summary()
                .unwrap_or("(No title)")
                .to_string();

            let (start, all_day) = match event.get_start() {
                Some(icalendar::DatePerhapsTime::DateTime(dt)) => {
                    (date_perhaps_to_utc_dt(dt), false)
                }
                Some(icalendar::DatePerhapsTime::Date(d)) => {
                    (naive_date_to_utc(d), true)
                }
                None => continue, // Skip events without a start time.
            };

            let end = match event.get_end() {
                Some(icalendar::DatePerhapsTime::DateTime(dt)) => {
                    Some(date_perhaps_to_utc_dt(dt))
                }
                Some(icalendar::DatePerhapsTime::Date(d)) => Some(naive_date_to_utc(d)),
                None => None,
            };

            let location = event.get_location().map(String::from);
            let description = event.get_description().map(String::from);

            events.push(CalendarEvent {
                uid,
                summary,
                start,
                end,
                location,
                description,
                all_day,
            });
        }
    }

    Ok(events)
}

/// Convert an icalendar CalendarDateTime to UTC.
fn date_perhaps_to_utc_dt(dt: icalendar::CalendarDateTime) -> DateTime<Utc> {
    match dt {
        icalendar::CalendarDateTime::Floating(naive) => {
            // Treat floating as local time, convert to UTC assuming local timezone.
            naive.and_utc()
        }
        icalendar::CalendarDateTime::Utc(utc) => utc,
        icalendar::CalendarDateTime::WithTimezone { date_time, tzid: _ } => {
            // Best-effort: treat as UTC if we can't resolve the timezone.
            date_time.and_utc()
        }
    }
}

/// Convert a NaiveDate to midnight UTC.
fn naive_date_to_utc(d: NaiveDate) -> DateTime<Utc> {
    d.and_hms_opt(0, 0, 0).unwrap().and_utc()
}

// ── Action tools ──────────────────────────────────────────────────

/// Tool: fetch today's agenda from the calendar.
pub struct TodayAgenda {
    pub config: CalendarConfig,
}

#[async_trait::async_trait]
impl Action for TodayAgenda {
    fn name(&self) -> &str {
        "today_agenda"
    }

    fn description(&self) -> &str {
        "Get today's calendar events. Returns a list of events with times, titles, and locations."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn execute(&self, _input: serde_json::Value) -> Result<serde_json::Value> {
        let today = chrono::Local::now().date_naive();
        let from = today.and_hms_opt(0, 0, 0).unwrap().and_utc();
        let to = today.succ_opt().unwrap().and_hms_opt(0, 0, 0).unwrap().and_utc();

        let events = fetch_events(&self.config, from, to).await?;
        Ok(serde_json::to_value(&events)?)
    }
}

/// Tool: fetch calendar events for a date range.
pub struct FetchCalendarEvents {
    pub config: CalendarConfig,
}

#[async_trait::async_trait]
impl Action for FetchCalendarEvents {
    fn name(&self) -> &str {
        "fetch_calendar_events"
    }

    fn description(&self) -> &str {
        "Fetch calendar events for a given date range. Use 'today_agenda' for today's events."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "from": {
                    "type": "string",
                    "description": "Start date (YYYY-MM-DD)"
                },
                "to": {
                    "type": "string",
                    "description": "End date (YYYY-MM-DD), inclusive"
                }
            },
            "required": ["from", "to"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let from_str = input["from"]
            .as_str()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("'from' is required".into()))?;
        let to_str = input["to"]
            .as_str()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("'to' is required".into()))?;

        let from = NaiveDate::parse_from_str(from_str, "%Y-%m-%d")
            .map_err(|e| aivyx_core::AivyxError::Validation(format!("Invalid 'from' date: {e}")))?
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc();
        let to = NaiveDate::parse_from_str(to_str, "%Y-%m-%d")
            .map_err(|e| aivyx_core::AivyxError::Validation(format!("Invalid 'to' date: {e}")))?
            .succ_opt()
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc();

        let events = fetch_events(&self.config, from, to).await?;
        Ok(serde_json::to_value(&events)?)
    }
}

// ── Conflict detection ────────────────────────────────────────────

/// A pair of overlapping calendar events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conflict {
    pub event_a: String,
    pub event_b: String,
    pub overlap_start: DateTime<Utc>,
    pub overlap_end: DateTime<Utc>,
}

/// Detect scheduling conflicts (time overlaps) in a list of events.
///
/// Two events conflict if their time ranges overlap. All-day events are
/// excluded from conflict checks since they represent background events
/// (holidays, birthdays) that don't block time slots.
pub fn detect_conflicts(events: &[CalendarEvent]) -> Vec<Conflict> {
    let mut conflicts = Vec::new();

    // Only consider timed (non-all-day) events with an end time.
    let timed: Vec<&CalendarEvent> = events
        .iter()
        .filter(|e| !e.all_day && e.end.is_some())
        .collect();

    for i in 0..timed.len() {
        for j in (i + 1)..timed.len() {
            let a = timed[i];
            let b = timed[j];
            let a_end = a.end.unwrap();
            let b_end = b.end.unwrap();

            // Two intervals [a.start, a_end) and [b.start, b_end) overlap
            // iff a.start < b_end AND b.start < a_end.
            if a.start < b_end && b.start < a_end {
                let overlap_start = a.start.max(b.start);
                let overlap_end = a_end.min(b_end);
                conflicts.push(Conflict {
                    event_a: a.summary.clone(),
                    event_b: b.summary.clone(),
                    overlap_start,
                    overlap_end,
                });
            }
        }
    }

    conflicts
}

/// Tool: check today's calendar for scheduling conflicts.
pub struct CheckConflicts {
    pub config: CalendarConfig,
}

#[async_trait::async_trait]
impl Action for CheckConflicts {
    fn name(&self) -> &str {
        "check_calendar_conflicts"
    }

    fn description(&self) -> &str {
        "Check for scheduling conflicts in calendar events. \
         Optionally specify a date range; defaults to today."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "from": {
                    "type": "string",
                    "description": "Start date (YYYY-MM-DD), defaults to today"
                },
                "to": {
                    "type": "string",
                    "description": "End date (YYYY-MM-DD, inclusive), defaults to today"
                }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let today = chrono::Local::now().date_naive();

        let from_date = input["from"]
            .as_str()
            .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
            .unwrap_or(today);
        let to_date = input["to"]
            .as_str()
            .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
            .unwrap_or(today);

        let from = from_date.and_hms_opt(0, 0, 0).unwrap().and_utc();
        let to = to_date.succ_opt().unwrap().and_hms_opt(0, 0, 0).unwrap().and_utc();

        let events = fetch_events(&self.config, from, to).await?;
        let conflicts = detect_conflicts(&events);

        Ok(serde_json::json!({
            "total_events": events.len(),
            "conflicts": conflicts,
        }))
    }
}

// ── CalDAV write helpers ─────────────────────────────────────────

/// Resolve the calendar URL from config (explicit path or auto-discover).
async fn resolve_calendar_url(config: &CalendarConfig) -> Result<String> {
    if let Some(ref path) = config.calendar_path {
        if path.starts_with("http") {
            Ok(path.clone())
        } else {
            Ok(format!("{}{}", config.url.trim_end_matches('/'), path))
        }
    } else {
        let client = crate::http_client();
        discover_calendar(client, config).await
    }
}

/// Build an iCalendar VCALENDAR/VEVENT string from event fields.
fn build_vevent_ical(
    uid: &str,
    summary: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    location: Option<&str>,
    description: Option<&str>,
    all_day: bool,
) -> String {
    use icalendar::{Calendar, Component, Event, EventLike};

    let mut event = Event::new();
    event.uid(uid);
    event.summary(summary);

    if all_day {
        event.all_day(start.date_naive());
        // Set DTEND as the next day for all-day events
        if let Some(next) = end.date_naive().succ_opt() {
            event.add_property("DTEND;VALUE=DATE", &next.format("%Y%m%d").to_string());
        }
    } else {
        event.starts(start);
        event.ends(end);
    }

    if let Some(loc) = location {
        event.location(loc);
    }
    if let Some(desc) = description {
        event.description(desc);
    }

    let mut cal = Calendar::new();
    cal.push(event.done());
    cal.done().to_string()
}

/// PUT an event to the CalDAV server (create or update).
async fn caldav_put_event(
    config: &CalendarConfig,
    calendar_url: &str,
    uid: &str,
    ical_body: &str,
) -> Result<()> {
    let client = crate::http_client();
    let event_url = format!("{}/{}.ics", calendar_url.trim_end_matches('/'), uid);

    let response = crate::retry::retry(
        &crate::retry::RetryConfig::network(),
        || async {
            client
                .put(&event_url)
                .basic_auth(&config.username, Some(&config.password))
                .header("Content-Type", "text/calendar; charset=utf-8")
                .body(ical_body.to_string())
                .send()
                .await
                .map_err(|e| aivyx_core::AivyxError::Http(e.to_string()))
        },
        crate::retry::is_transient,
    )
    .await?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(aivyx_core::AivyxError::Http(format!(
            "CalDAV PUT failed: HTTP {status} — {body}"
        )));
    }

    Ok(())
}

/// DELETE an event from the CalDAV server.
async fn caldav_delete_event(
    config: &CalendarConfig,
    calendar_url: &str,
    uid: &str,
) -> Result<()> {
    let client = crate::http_client();
    let event_url = format!("{}/{}.ics", calendar_url.trim_end_matches('/'), uid);

    let response = crate::retry::retry(
        &crate::retry::RetryConfig::network(),
        || async {
            client
                .delete(&event_url)
                .basic_auth(&config.username, Some(&config.password))
                .send()
                .await
                .map_err(|e| aivyx_core::AivyxError::Http(e.to_string()))
        },
        crate::retry::is_transient,
    )
    .await?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(aivyx_core::AivyxError::Http(format!(
            "CalDAV DELETE failed: HTTP {status} — {body}"
        )));
    }

    Ok(())
}

// ── Calendar CUD tools ──────────────────────────────────────────

/// Tool: create a new calendar event.
pub struct CreateCalendarEvent {
    pub config: CalendarConfig,
}

#[async_trait::async_trait]
impl Action for CreateCalendarEvent {
    fn name(&self) -> &str { "create_calendar_event" }

    fn description(&self) -> &str {
        "Create a new calendar event. Specify summary, start, and end times. \
         Use check_calendar_conflicts first to avoid double-booking."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["summary", "start", "end"],
            "properties": {
                "summary": { "type": "string", "description": "Event title" },
                "start": { "type": "string", "description": "Start time (ISO 8601, e.g. 2026-04-10T14:00:00Z) or date (YYYY-MM-DD) for all-day" },
                "end": { "type": "string", "description": "End time (ISO 8601) or date (YYYY-MM-DD) for all-day" },
                "location": { "type": "string" },
                "description": { "type": "string" },
                "all_day": { "type": "boolean", "description": "If true, start/end are dates (YYYY-MM-DD)" }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let summary = input["summary"].as_str()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("'summary' is required".into()))?;
        let start_str = input["start"].as_str()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("'start' is required".into()))?;
        let end_str = input["end"].as_str()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("'end' is required".into()))?;
        let all_day = input["all_day"].as_bool().unwrap_or(false);

        let (start, end) = parse_event_times(start_str, end_str, all_day)?;

        let uid = uuid::Uuid::new_v4().to_string();
        let ical = build_vevent_ical(
            &uid, summary, start, end,
            input["location"].as_str(),
            input["description"].as_str(),
            all_day,
        );

        let calendar_url = resolve_calendar_url(&self.config).await?;
        caldav_put_event(&self.config, &calendar_url, &uid, &ical).await?;

        Ok(serde_json::json!({
            "status": "created",
            "uid": uid,
            "summary": summary,
            "start": start.to_rfc3339(),
            "end": end.to_rfc3339(),
        }))
    }
}

/// Tool: update an existing calendar event.
pub struct UpdateCalendarEvent {
    pub config: CalendarConfig,
}

#[async_trait::async_trait]
impl Action for UpdateCalendarEvent {
    fn name(&self) -> &str { "update_calendar_event" }

    fn description(&self) -> &str {
        "Update an existing calendar event by UID. Only the fields you provide will be changed. \
         Get the UID from fetch_calendar_events or today_agenda."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["uid"],
            "properties": {
                "uid": { "type": "string", "description": "Event UID to update" },
                "summary": { "type": "string" },
                "start": { "type": "string", "description": "New start time (ISO 8601)" },
                "end": { "type": "string", "description": "New end time (ISO 8601)" },
                "location": { "type": "string" },
                "description": { "type": "string" }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let uid = input["uid"].as_str()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("'uid' is required".into()))?;

        // Fetch the existing event to merge fields.
        // Search a wide range to find the event by UID.
        let from = Utc::now() - chrono::Duration::days(365);
        let to = Utc::now() + chrono::Duration::days(365);
        let events = fetch_events(&self.config, from, to).await?;
        let existing = events.iter().find(|e| e.uid == uid)
            .ok_or_else(|| aivyx_core::AivyxError::Validation(format!("Event '{uid}' not found")))?;

        let summary = input["summary"].as_str().unwrap_or(&existing.summary);
        let all_day = existing.all_day;

        let start = if let Some(s) = input["start"].as_str() {
            parse_datetime_or_date(s)?
        } else {
            existing.start
        };
        let end = if let Some(e) = input["end"].as_str() {
            parse_datetime_or_date(e)?
        } else {
            existing.end.unwrap_or(start + chrono::Duration::hours(1))
        };

        let location = if !input["location"].is_null() {
            input["location"].as_str()
        } else {
            existing.location.as_deref()
        };
        let description = if !input["description"].is_null() {
            input["description"].as_str()
        } else {
            existing.description.as_deref()
        };

        let ical = build_vevent_ical(uid, summary, start, end, location, description, all_day);
        let calendar_url = resolve_calendar_url(&self.config).await?;
        caldav_put_event(&self.config, &calendar_url, uid, &ical).await?;

        Ok(serde_json::json!({
            "status": "updated",
            "uid": uid,
            "summary": summary,
        }))
    }
}

/// Tool: delete a calendar event by UID.
pub struct DeleteCalendarEvent {
    pub config: CalendarConfig,
}

#[async_trait::async_trait]
impl Action for DeleteCalendarEvent {
    fn name(&self) -> &str { "delete_calendar_event" }

    fn description(&self) -> &str {
        "Permanently delete a calendar event by UID."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["uid"],
            "properties": {
                "uid": { "type": "string", "description": "Event UID to delete" }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let uid = input["uid"].as_str()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("'uid' is required".into()))?;

        let calendar_url = resolve_calendar_url(&self.config).await?;
        caldav_delete_event(&self.config, &calendar_url, uid).await?;

        Ok(serde_json::json!({
            "status": "deleted",
            "uid": uid,
        }))
    }
}

/// Parse start/end strings into DateTime<Utc>.
fn parse_event_times(start: &str, end: &str, all_day: bool) -> Result<(DateTime<Utc>, DateTime<Utc>)> {
    if all_day {
        let s = NaiveDate::parse_from_str(start, "%Y-%m-%d")
            .map_err(|e| aivyx_core::AivyxError::Validation(format!("Invalid start date: {e}")))?;
        let e = NaiveDate::parse_from_str(end, "%Y-%m-%d")
            .map_err(|e| aivyx_core::AivyxError::Validation(format!("Invalid end date: {e}")))?;
        Ok((naive_date_to_utc(s), naive_date_to_utc(e)))
    } else {
        let s = parse_datetime_or_date(start)?;
        let e = parse_datetime_or_date(end)?;
        Ok((s, e))
    }
}

/// Parse an ISO 8601 datetime string, falling back to date-only.
fn parse_datetime_or_date(s: &str) -> Result<DateTime<Utc>> {
    // Try full ISO 8601 first
    if let Ok(dt) = s.parse::<DateTime<Utc>>() {
        return Ok(dt);
    }
    // Try date-only
    let date = NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|e| aivyx_core::AivyxError::Validation(format!("Invalid datetime '{s}': {e}")))?;
    Ok(naive_date_to_utc(date))
}

// ── Auto-reminder helpers ────────────────────────────────────────

/// Check upcoming calendar events and return any that need a reminder.
///
/// An event needs a reminder if:
/// 1. It starts within `lead_minutes` from `now`
/// 2. It hasn't started yet
/// 3. It's not an all-day event (those don't need "starting soon" alerts)
///
/// The caller is responsible for checking whether a reminder already exists
/// for each returned event (keyed by `calendar-reminder:{uid}`).
pub fn events_needing_reminder(
    events: &[CalendarEvent],
    now: DateTime<Utc>,
    lead_minutes: i64,
) -> Vec<&CalendarEvent> {
    let horizon = now + chrono::Duration::minutes(lead_minutes);

    events
        .iter()
        .filter(|e| {
            !e.all_day
                && e.start > now       // hasn't started yet
                && e.start <= horizon   // within lead time
        })
        .collect()
}

/// The store key for a calendar auto-reminder, keyed by event UID + date.
///
/// Including the date ensures recurring events get a fresh reminder each day.
pub fn auto_reminder_key(event_uid: &str, event_date: &NaiveDate) -> String {
    format!("cal-remind:{}:{}", event_uid, event_date)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;

    #[test]
    fn build_query_xml_contains_time_range() {
        let from = NaiveDate::from_ymd_opt(2026, 4, 2)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc();
        let to = NaiveDate::from_ymd_opt(2026, 4, 3)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc();
        let xml = build_calendar_query_xml(&from, &to);
        assert!(xml.contains("20260402T000000Z"));
        assert!(xml.contains("20260403T000000Z"));
        assert!(xml.contains("calendar-query"));
        assert!(xml.contains("time-range"));
    }

    #[test]
    fn resolve_url_absolute() {
        assert_eq!(
            resolve_url("https://cal.example.com/dav/", "https://other.com/cal/"),
            "https://other.com/cal/"
        );
    }

    #[test]
    fn resolve_url_relative() {
        assert_eq!(
            resolve_url(
                "https://cal.example.com/dav/calendars/",
                "/dav/calendars/personal/"
            ),
            "https://cal.example.com/dav/calendars/personal/"
        );
    }

    #[test]
    fn parse_ical_single_event() {
        let ical = r#"BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:test-1@example
SUMMARY:Team standup
DTSTART:20260402T100000Z
DTEND:20260402T103000Z
LOCATION:Room 42
DESCRIPTION:Daily sync
END:VEVENT
END:VCALENDAR"#;

        let events = parse_ical_events(ical).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].summary, "Team standup");
        assert_eq!(events[0].location.as_deref(), Some("Room 42"));
        assert_eq!(events[0].description.as_deref(), Some("Daily sync"));
        assert!(!events[0].all_day);
    }

    #[test]
    fn parse_ical_all_day_event() {
        let ical = r#"BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:allday-1@example
SUMMARY:Public holiday
DTSTART;VALUE=DATE:20260402
DTEND;VALUE=DATE:20260403
END:VEVENT
END:VCALENDAR"#;

        let events = parse_ical_events(ical).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].summary, "Public holiday");
        assert!(events[0].all_day);
    }

    #[test]
    fn parse_ical_multiple_events() {
        let ical = r#"BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:a@example
SUMMARY:Morning meeting
DTSTART:20260402T090000Z
DTEND:20260402T100000Z
END:VEVENT
BEGIN:VEVENT
UID:b@example
SUMMARY:Lunch
DTSTART:20260402T120000Z
DTEND:20260402T130000Z
END:VEVENT
END:VCALENDAR"#;

        let events = parse_ical_events(ical).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].summary, "Morning meeting");
        assert_eq!(events[1].summary, "Lunch");
    }

    #[test]
    fn parse_multistatus_extracts_events() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/cal/event1.ics</d:href>
    <d:propstat>
      <d:prop>
        <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:evt1@example
SUMMARY:Dentist
DTSTART:20260402T140000Z
DTEND:20260402T150000Z
END:VEVENT
END:VCALENDAR</c:calendar-data>
      </d:prop>
    </d:propstat>
  </d:response>
</d:multistatus>"#;

        let events = parse_multistatus_events(xml).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].summary, "Dentist");
        assert_eq!(events[0].uid, "evt1@example");
    }

    #[test]
    fn parse_multistatus_empty() {
        let xml = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:">
</d:multistatus>"#;

        let events = parse_multistatus_events(xml).unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn today_agenda_schema() {
        let config = CalendarConfig {
            url: "https://example.com".into(),
            username: "user".into(),
            password: "pass".into(),
            calendar_path: None,
        };
        let tool = TodayAgenda { config };
        assert_eq!(tool.name(), "today_agenda");
        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
    }

    #[test]
    fn fetch_events_schema_requires_dates() {
        let config = CalendarConfig {
            url: "https://example.com".into(),
            username: "user".into(),
            password: "pass".into(),
            calendar_path: None,
        };
        let tool = FetchCalendarEvents { config };
        assert_eq!(tool.name(), "fetch_calendar_events");
        let schema = tool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "from"));
        assert!(required.iter().any(|v| v == "to"));
    }

    #[test]
    fn parse_ical_event_without_start_is_skipped() {
        let ical = r#"BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:nostart@example
SUMMARY:Missing start
END:VEVENT
END:VCALENDAR"#;

        let events = parse_ical_events(ical).unwrap();
        assert!(events.is_empty());
    }

    // ── Conflict detection tests ─────────────────────────────────

    fn make_event(summary: &str, start_hour: u32, end_hour: u32) -> CalendarEvent {
        let date = NaiveDate::from_ymd_opt(2026, 4, 2).unwrap();
        CalendarEvent {
            uid: format!("{summary}@test"),
            summary: summary.into(),
            start: date.and_hms_opt(start_hour, 0, 0).unwrap().and_utc(),
            end: Some(date.and_hms_opt(end_hour, 0, 0).unwrap().and_utc()),
            location: None,
            description: None,
            all_day: false,
        }
    }

    #[test]
    fn detect_conflicts_finds_overlap() {
        let events = vec![
            make_event("Meeting A", 10, 11),
            make_event("Meeting B", 10, 12), // overlaps with A: 10-11
        ];
        let conflicts = detect_conflicts(&events);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].event_a, "Meeting A");
        assert_eq!(conflicts[0].event_b, "Meeting B");
    }

    #[test]
    fn detect_conflicts_no_overlap() {
        let events = vec![
            make_event("Meeting A", 9, 10),
            make_event("Meeting B", 10, 11), // starts exactly when A ends — no overlap
        ];
        let conflicts = detect_conflicts(&events);
        assert!(conflicts.is_empty());
    }

    #[test]
    fn detect_conflicts_three_way() {
        let events = vec![
            make_event("A", 9, 11),
            make_event("B", 10, 12),
            make_event("C", 11, 13),
        ];
        // A overlaps B (10-11), B overlaps C (11-12). A does NOT overlap C.
        let conflicts = detect_conflicts(&events);
        assert_eq!(conflicts.len(), 2);
    }

    #[test]
    fn detect_conflicts_ignores_all_day() {
        let mut events = vec![make_event("Meeting", 10, 11)];
        events.push(CalendarEvent {
            uid: "holiday@test".into(),
            summary: "Public Holiday".into(),
            start: NaiveDate::from_ymd_opt(2026, 4, 2).unwrap().and_hms_opt(0, 0, 0).unwrap().and_utc(),
            end: Some(NaiveDate::from_ymd_opt(2026, 4, 3).unwrap().and_hms_opt(0, 0, 0).unwrap().and_utc()),
            location: None,
            description: None,
            all_day: true,
        });
        let conflicts = detect_conflicts(&events);
        assert!(conflicts.is_empty());
    }

    #[test]
    fn detect_conflicts_contained_event() {
        // B is entirely within A
        let events = vec![
            make_event("A", 9, 17),
            make_event("B", 10, 11),
        ];
        let conflicts = detect_conflicts(&events);
        assert_eq!(conflicts.len(), 1);
        // Overlap window should be the inner event's range
        assert_eq!(conflicts[0].overlap_start, events[1].start);
        assert_eq!(conflicts[0].overlap_end, events[1].end.unwrap());
    }

    #[test]
    fn check_conflicts_schema() {
        let config = CalendarConfig {
            url: "https://example.com".into(),
            username: "user".into(),
            password: "pass".into(),
            calendar_path: None,
        };
        let tool = CheckConflicts { config };
        assert_eq!(tool.name(), "check_calendar_conflicts");
        // from/to are optional, so no "required" field
        let schema = tool.input_schema();
        assert!(schema["required"].is_null() || schema["required"].as_array().map_or(true, |a| a.is_empty()));
    }

    // ── Auto-reminder helper tests ───────────────────────────────

    #[test]
    fn events_needing_reminder_within_lead_time() {
        let now = NaiveDate::from_ymd_opt(2026, 4, 2).unwrap()
            .and_hms_opt(9, 45, 0).unwrap().and_utc();

        let make_upcoming = |summary: &str, offset_minutes: i64| -> CalendarEvent {
            let start = now + chrono::Duration::minutes(offset_minutes);
            CalendarEvent {
                uid: format!("{summary}@test"),
                summary: summary.into(),
                start,
                end: Some(start + chrono::Duration::minutes(30)),
                location: None,
                description: None,
                all_day: false,
            }
        };

        let events = vec![
            make_upcoming("In 10 min", 10),   // starts 9:55 — within 15 min
            make_upcoming("In 30 min", 30),   // starts 10:15 — outside 15 min
            make_upcoming("Already started", -15), // started 9:30 — past
        ];
        let need = events_needing_reminder(&events, now, 15);
        assert_eq!(need.len(), 1);
        assert_eq!(need[0].summary, "In 10 min");
    }

    #[test]
    fn events_needing_reminder_ignores_all_day() {
        let now = NaiveDate::from_ymd_opt(2026, 4, 2).unwrap()
            .and_hms_opt(0, 5, 0).unwrap().and_utc();
        let events = vec![CalendarEvent {
            uid: "allday@test".into(),
            summary: "Holiday".into(),
            start: now + chrono::Duration::minutes(10),
            end: Some(now + chrono::Duration::hours(24)),
            location: None,
            description: None,
            all_day: true,
        }];
        let need = events_needing_reminder(&events, now, 15);
        assert!(need.is_empty());
    }

    #[test]
    fn auto_reminder_key_includes_date() {
        let date = NaiveDate::from_ymd_opt(2026, 4, 2).unwrap();
        let key = auto_reminder_key("uid@example", &date);
        assert_eq!(key, "cal-remind:uid@example:2026-04-02");
    }

    // ── Calendar CUD tests ──────────────────────────────────────────

    fn test_config() -> CalendarConfig {
        CalendarConfig {
            url: "https://cal.example.com".into(),
            username: "user".into(),
            password: "pass".into(),
            calendar_path: None,
        }
    }

    #[test]
    fn create_calendar_event_schema() {
        let tool = CreateCalendarEvent { config: test_config() };
        assert_eq!(tool.name(), "create_calendar_event");
        let schema = tool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "summary"));
        assert!(required.iter().any(|v| v == "start"));
        assert!(required.iter().any(|v| v == "end"));
        assert!(schema["properties"]["location"].is_object());
        assert!(schema["properties"]["all_day"].is_object());
    }

    #[test]
    fn update_calendar_event_schema() {
        let tool = UpdateCalendarEvent { config: test_config() };
        assert_eq!(tool.name(), "update_calendar_event");
        let schema = tool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert_eq!(required.len(), 1);
        assert!(required.iter().any(|v| v == "uid"));
        // Optional update fields exist
        assert!(schema["properties"]["summary"].is_object());
        assert!(schema["properties"]["start"].is_object());
        assert!(schema["properties"]["end"].is_object());
    }

    #[test]
    fn delete_calendar_event_schema() {
        let tool = DeleteCalendarEvent { config: test_config() };
        assert_eq!(tool.name(), "delete_calendar_event");
        let schema = tool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert_eq!(required.len(), 1);
        assert!(required.iter().any(|v| v == "uid"));
    }

    #[test]
    fn build_vevent_ical_contains_required_fields() {
        let start = NaiveDate::from_ymd_opt(2026, 4, 10).unwrap()
            .and_hms_opt(14, 0, 0).unwrap().and_utc();
        let end = NaiveDate::from_ymd_opt(2026, 4, 10).unwrap()
            .and_hms_opt(15, 0, 0).unwrap().and_utc();

        let ical = build_vevent_ical(
            "test-uid-123", "Team standup", start, end,
            Some("Room 42"), Some("Daily sync"), false,
        );

        assert!(ical.contains("BEGIN:VCALENDAR"), "missing VCALENDAR");
        assert!(ical.contains("BEGIN:VEVENT"), "missing VEVENT");
        assert!(ical.contains("END:VEVENT"), "missing END:VEVENT");
        assert!(ical.contains("test-uid-123"), "missing UID");
        assert!(ical.contains("Team standup"), "missing SUMMARY");
        assert!(ical.contains("Room 42"), "missing LOCATION");
        assert!(ical.contains("Daily sync"), "missing DESCRIPTION");
        // DTSTART should contain the date
        assert!(ical.contains("20260410"), "missing start date");
    }

    #[test]
    fn build_vevent_ical_all_day_uses_date_format() {
        let start = NaiveDate::from_ymd_opt(2026, 4, 10).unwrap()
            .and_hms_opt(0, 0, 0).unwrap().and_utc();
        let end = NaiveDate::from_ymd_opt(2026, 4, 11).unwrap()
            .and_hms_opt(0, 0, 0).unwrap().and_utc();

        let ical = build_vevent_ical(
            "allday-uid", "Conference", start, end,
            None, None, true,
        );

        // All-day events use VALUE=DATE format, not datetime with T/Z
        assert!(ical.contains("BEGIN:VEVENT"), "missing VEVENT");
        assert!(ical.contains("Conference"), "missing SUMMARY");
        // Should contain DATE value (YYYYMMDD without time component)
        assert!(ical.contains("20260410"), "missing start date");
    }

    #[test]
    fn parse_event_times_iso8601() {
        let (start, end) = parse_event_times(
            "2026-04-10T14:00:00Z", "2026-04-10T15:00:00Z", false,
        ).unwrap();
        assert_eq!(start.hour(), 14);
        assert_eq!(end.hour(), 15);
    }

    #[test]
    fn parse_event_times_all_day() {
        let (start, end) = parse_event_times("2026-04-10", "2026-04-11", true).unwrap();
        assert_eq!(start.date_naive(), NaiveDate::from_ymd_opt(2026, 4, 10).unwrap());
        assert_eq!(end.date_naive(), NaiveDate::from_ymd_opt(2026, 4, 11).unwrap());
    }

    #[test]
    fn parse_datetime_or_date_full() {
        let dt = parse_datetime_or_date("2026-04-10T09:30:00Z").unwrap();
        assert_eq!(dt.hour(), 9);
        assert_eq!(dt.minute(), 30);
    }

    #[test]
    fn parse_datetime_or_date_date_only() {
        let dt = parse_datetime_or_date("2026-04-10").unwrap();
        assert_eq!(dt.date_naive(), NaiveDate::from_ymd_opt(2026, 4, 10).unwrap());
        assert_eq!(dt.hour(), 0);
    }

    #[test]
    fn parse_datetime_or_date_invalid() {
        assert!(parse_datetime_or_date("not-a-date").is_err());
    }

    #[test]
    fn resolve_calendar_url_explicit_absolute() {
        let config = CalendarConfig {
            url: "https://cal.example.com".into(),
            username: "u".into(),
            password: "p".into(),
            calendar_path: Some("https://other.com/dav/personal/".into()),
        };
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let result = rt.block_on(resolve_calendar_url(&config)).unwrap();
        assert_eq!(result, "https://other.com/dav/personal/");
    }

    #[test]
    fn resolve_calendar_url_explicit_relative() {
        let config = CalendarConfig {
            url: "https://cal.example.com/".into(),
            username: "u".into(),
            password: "p".into(),
            calendar_path: Some("/dav/calendars/personal/".into()),
        };
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let result = rt.block_on(resolve_calendar_url(&config)).unwrap();
        assert_eq!(result, "https://cal.example.com/dav/calendars/personal/");
    }
}
