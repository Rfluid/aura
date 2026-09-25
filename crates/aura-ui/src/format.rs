use std::sync::OnceLock;

use chrono::{
    format::{DelayedFormat, Locale, StrftimeItems},
    NaiveTime,
};

/// Format a `NaiveTime` with a chrono strftime string under the given locale.
/// `NaiveTime` itself doesn't expose `format_localized` in chrono 0.4, so we
/// build a `DelayedFormat` directly.
fn format_time_localized(t: NaiveTime, fmt: &str, locale: Locale) -> String {
    let items = StrftimeItems::new(fmt);
    DelayedFormat::new_with_locale(None, Some(t), items, locale).to_string()
}

/// Detect the active locale from `LC_ALL` / `LC_TIME` / `LANG`, falling back
/// to `POSIX` if none of them name a locale chrono recognises.
pub fn system_locale() -> Locale {
    static LOCALE: OnceLock<Locale> = OnceLock::new();
    *LOCALE.get_or_init(|| {
        for var in ["LC_ALL", "LC_TIME", "LANG"] {
            if let Ok(val) = std::env::var(var) {
                if let Some(loc) = parse_locale_str(&val) {
                    return loc;
                }
            }
        }
        Locale::POSIX
    })
}

fn parse_locale_str(s: &str) -> Option<Locale> {
    if s.is_empty() || s == "C" || s == "POSIX" {
        return None;
    }
    // Strip the codeset (".UTF-8") before handing the tag to chrono.
    let no_codeset = s.split('.').next().unwrap_or(s);
    if let Ok(loc) = Locale::try_from(no_codeset) {
        return Some(loc);
    }
    // As a last resort drop any `@modifier` (e.g. "ca_ES@valencia").
    let base = no_codeset.split('@').next().unwrap_or(no_codeset);
    Locale::try_from(base).ok()
}

/// Whether the active locale formats wall-clock times with an am/pm marker.
pub fn locale_uses_12h() -> bool {
    static USES_12H: OnceLock<bool> = OnceLock::new();
    *USES_12H.get_or_init(|| {
        let probe = NaiveTime::from_hms_opt(13, 0, 0).expect("13:00 is a valid time");
        !format_time_localized(probe, "%p", system_locale())
            .trim()
            .is_empty()
    })
}

pub fn thousands(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, &b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(b as char);
    }
    out
}

pub fn duration(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h}h {m}m")
    } else if m > 0 {
        format!("{m}m {s}s")
    } else {
        format!("{s}s")
    }
}

pub fn hour_of_day(hour: u8) -> String {
    hour_of_day_with_locale(hour, system_locale(), locale_uses_12h())
}

fn hour_of_day_with_locale(hour: u8, locale: Locale, uses_12h: bool) -> String {
    let t = NaiveTime::from_hms_opt(hour as u32 % 24, 0, 0).expect("hour < 24 is valid");
    if uses_12h {
        let suffix = format_time_localized(t, "%P", locale).to_lowercase();
        let h12 = format_time_localized(t, "%-I", locale);
        format!("{h12}{suffix}")
    } else {
        format_time_localized(t, "%-H", locale)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thousands_basic() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn duration_formats() {
        assert_eq!(duration(0), "0s");
        assert_eq!(duration(45), "45s");
        assert_eq!(duration(75), "1m 15s");
        assert_eq!(duration(3725), "1h 2m");
    }

    #[test]
    fn hour_of_day_12h() {
        let l = Locale::en_US;
        assert_eq!(hour_of_day_with_locale(0, l, true), "12am");
        assert_eq!(hour_of_day_with_locale(9, l, true), "9am");
        assert_eq!(hour_of_day_with_locale(12, l, true), "12pm");
        assert_eq!(hour_of_day_with_locale(17, l, true), "5pm");
        assert_eq!(hour_of_day_with_locale(23, l, true), "11pm");
    }

    #[test]
    fn hour_of_day_24h() {
        let l = Locale::POSIX;
        assert_eq!(hour_of_day_with_locale(0, l, false), "0");
        assert_eq!(hour_of_day_with_locale(9, l, false), "9");
        assert_eq!(hour_of_day_with_locale(17, l, false), "17");
        assert_eq!(hour_of_day_with_locale(23, l, false), "23");
    }
}
