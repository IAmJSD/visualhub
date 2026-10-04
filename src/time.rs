//! GitHub's timestamps (`2024-03-05T17:02:11Z`) as "3 days ago".

use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since the epoch for an ISO-8601 UTC timestamp.
pub fn parse(iso: &str) -> Option<i64> {
    let b = iso.as_bytes();
    if b.len() < 19 {
        return None;
    }
    let num = |r: std::ops::Range<usize>| iso.get(r)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, s) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let mut secs = days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + s;
    // An explicit offset ("+02:00") after the seconds.
    let rest = &iso[19..];
    let rest = rest.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    if let Some(sign @ ('+' | '-')) = rest.chars().next() {
        let oh = rest.get(1..3).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
        let om = rest.get(4..6).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
        let off = oh * 3600 + om * 60;
        secs -= if sign == '+' { off } else { -off };
    }
    Some(secs)
}

/// Howard Hinnant's days-from-civil.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// "just now", "5 minutes ago", "3 days ago", or a date past a month.
pub fn ago(iso: &str) -> String {
    let Some(then) = parse(iso) else {
        return String::new();
    };
    let delta = now() - then;
    let plural = |n: i64, unit: &str| {
        if n == 1 {
            format!("1 {unit} ago")
        } else {
            format!("{n} {unit}s ago")
        }
    };
    match delta {
        d if d < 0 => date(iso),
        d if d < 45 => "just now".into(),
        d if d < 3600 => plural((d / 60).max(1), "minute"),
        d if d < 86_400 => plural(d / 3600, "hour"),
        d if d < 86_400 * 30 => plural(d / 86_400, "day"),
        _ => format!("on {}", date(iso)),
    }
}

/// "Mar 5, 2024".
pub fn date(iso: &str) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let num = |r: std::ops::Range<usize>| iso.get(r).and_then(|v| v.parse::<usize>().ok());
    match (num(0..4), num(5..7), num(8..10)) {
        (Some(y), Some(m @ 1..=12), Some(d)) => format!("{} {d}, {y}", MONTHS[m - 1]),
        _ => iso.to_string(),
    }
}

/// A duration between two timestamps as "1m 23s".
pub fn span(from: &str, to: &str) -> String {
    match (parse(from), parse(to)) {
        (Some(a), Some(b)) if b >= a => duration(b - a),
        _ => String::new(),
    }
}

pub fn duration(secs: i64) -> String {
    if secs >= 3600 {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_utc_and_offsets() {
        assert_eq!(parse("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse("1970-01-02T00:00:00Z"), Some(86_400));
        assert_eq!(parse("1970-01-01T02:00:00+02:00"), Some(0));
        assert_eq!(parse("2000-03-01T00:00:00Z"), Some(951_868_800));
        assert_eq!(date("2024-03-05T17:02:11Z"), "Mar 5, 2024");
        assert_eq!(duration(83), "1m 23s");
    }
}
