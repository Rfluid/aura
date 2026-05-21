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
    let suffix = if hour < 12 { "am" } else { "pm" };
    let h12 = match hour {
        0 => 12,
        13..=23 => hour - 12,
        _ => hour,
    };
    format!("{h12}{suffix}")
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
    fn hour_of_day_formats() {
        assert_eq!(hour_of_day(0), "12am");
        assert_eq!(hour_of_day(9), "9am");
        assert_eq!(hour_of_day(12), "12pm");
        assert_eq!(hour_of_day(17), "5pm");
        assert_eq!(hour_of_day(23), "11pm");
    }
}
