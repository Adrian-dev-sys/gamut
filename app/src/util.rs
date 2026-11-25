pub fn playtime(min: i64) -> String {
    match min {
        0 => "Never played".into(),
        m if m < 60 => format!("{m}m"),
        m => format!("{}h {}m", m / 60, m % 60),
    }
}

/// Unix seconds → "YYYY-MM-DD" (UTC).
pub fn date(unix: i64) -> String {
    if unix <= 0 {
        return "Never".into();
    }
    // Howard Hinnant's civil-from-days
    let z = unix.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

pub fn opt(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn dates() {
        assert_eq!(super::date(0), "Never");
        assert_eq!(super::date(1_700_000_000), "2023-11-14");
    }
}
