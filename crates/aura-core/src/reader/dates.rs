use chrono::{Duration, Local, NaiveDate};

// ── Calendar helpers ──────────────────────────────────────────────────────────

pub fn today() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

/// Returns the date that is `n` days before today (n=0 → today).
pub fn n_days_ago(n: u32) -> String {
    (Local::now() - Duration::days(n as i64))
        .format("%Y-%m-%d")
        .to_string()
}

/// Returns the day after `date` ("YYYY-MM-DD").
pub fn add_one_day(date: &str) -> Option<String> {
    let d = NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    Some((d + Duration::days(1)).format("%Y-%m-%d").to_string())
}

/// Extracts the "YYYY-MM-DD" portion from an ISO 8601 timestamp.
/// Works for both "2026-05-16" and "2026-05-16T19:53:11.152Z".
pub fn date_from_timestamp(ts: &str) -> Option<String> {
    let date_part = ts.get(..10)?;
    // Quick sanity: must look like YYYY-MM-DD
    if date_part.len() == 10 && date_part.chars().nth(4) == Some('-') {
        Some(date_part.to_string())
    } else {
        None
    }
}

/// Local hour (0–23) from an ISO 8601 UTC timestamp string.
pub fn hour_from_timestamp(ts: &str) -> Option<u8> {
    use chrono::{DateTime, NaiveDateTime, Utc};
    let dt: DateTime<Utc> = DateTime::parse_from_rfc3339(ts)
        .map(|d| d.with_timezone(&Utc))
        .or_else(|_| {
            NaiveDateTime::parse_from_str(ts, "%Y-%m-%dT%H:%M:%S%.fZ").map(|d| d.and_utc())
        })
        .ok()?;
    let local = dt.with_timezone(&Local);
    Some(local.hour() as u8)
}

use chrono::Timelike;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_from_timestamp_handles_both_forms() {
        assert_eq!(
            date_from_timestamp("2026-05-16T19:53:11.152Z"),
            Some("2026-05-16".to_string())
        );
        assert_eq!(
            date_from_timestamp("2026-05-16"),
            Some("2026-05-16".to_string())
        );
        assert_eq!(date_from_timestamp("bad"), None);
    }

    #[test]
    fn add_one_day_basic() {
        assert_eq!(add_one_day("2026-01-31"), Some("2026-02-01".to_string()));
        assert_eq!(add_one_day("2026-12-31"), Some("2027-01-01".to_string()));
    }

    #[test]
    fn today_is_valid_date() {
        let t = today();
        assert_eq!(t.len(), 10);
        assert!(t.chars().nth(4) == Some('-'));
    }

    #[test]
    fn n_days_ago_ordering() {
        let d0 = n_days_ago(0);
        let d7 = n_days_ago(7);
        assert!(d7 < d0);
    }
}
