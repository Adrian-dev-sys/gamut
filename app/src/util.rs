pub fn playtime(min: i64) -> String {
    match min {
        0 => "Never played".into(),
        m if m < 60 => format!("{m}m"),
        m => format!("{}h {}m", m / 60, m % 60),
    }
}

/// Unix seconds → "DD/MM/YYYY" (UTC).
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
    format!("{d:02}/{m:02}/{y:04}")
}

/// Release dates from Steam ("9 Jun, 2009", "Jun 9, 2009") or Playnite ("2009-06-09") → "DD/MM/YYYY".
/// Partial dates (just a year, "Q3 2024", "Coming soon") are shown as they are.
pub fn release(s: &str) -> String {
    const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
    let month = |t: &str| MONTHS.iter().position(|m| t.to_ascii_lowercase().starts_with(m)).map(|i| i + 1);
    let num = |t: &str| t.trim_matches(|c: char| !c.is_ascii_digit()).parse::<u32>().ok();
    let parts: Vec<&str> = s.split(|c: char| c == '-' || c == ' ' || c == ',' || c == '.').filter(|p| !p.is_empty()).collect();
    let dmy = match parts.as_slice() {
        [y, m, d] if y.len() == 4 => num(y).zip(num(m)).zip(num(d)).map(|((y, m), d)| (d, m, y)),
        [d, m, y] if month(m).is_some() => num(d).zip(num(y)).map(|(d, y)| (d, month(m).unwrap() as u32, y)),
        [m, d, y] if month(m).is_some() => num(d).zip(num(y)).map(|(d, y)| (d, month(m).unwrap() as u32, y)),
        _ => None,
    };
    match dmy {
        Some((d, m, y)) if (1..=31).contains(&d) && (1..=12).contains(&m) => format!("{d:02}/{m:02}/{y:04}"),
        _ => s.to_string(),
    }
}

/// Typed release date back to storage form: "DD/MM/YYYY" -> "YYYY-MM-DD"; anything else is kept.
pub fn release_input(s: &str) -> String {
    let p: Vec<&str> = s.trim().split('/').collect();
    match p.as_slice() {
        [d, m, y] if y.len() == 4 => match (d.parse::<u32>(), m.parse::<u32>(), y.parse::<u32>()) {
            (Ok(d), Ok(m), Ok(y)) if (1..=31).contains(&d) && (1..=12).contains(&m) => format!("{y:04}-{m:02}-{d:02}"),
            _ => s.trim().to_string(),
        },
        _ => s.trim().to_string(),
    }
}

pub fn opt(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// Part of a natural sort key: digit runs compare as numbers, so "#56" sorts before "#556".
#[derive(PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum NatPart {
    Num(u128),
    Text(String),
}

/// Case-insensitive natural sort key for a name.
pub fn nat_key(s: &str) -> Vec<NatPart> {
    let mut out = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() {
            let mut n: u128 = 0;
            while let Some(d) = chars.peek().and_then(|c| c.to_digit(10)) {
                n = n.saturating_mul(10).saturating_add(d as u128);
                chars.next();
            }
            out.push(NatPart::Num(n));
        } else {
            let mut t = String::new();
            while let Some(&c) = chars.peek().filter(|c| !c.is_ascii_digit()) {
                t.extend(c.to_lowercase());
                chars.next();
            }
            out.push(NatPart::Text(t));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn natural_order() {
        let mut v = vec!["Game #556", "game #56", "Game #9", "Game", "Game 2: Rise", "Game 10"];
        v.sort_by_key(|s| super::nat_key(s));
        assert_eq!(v, ["Game", "Game 2: Rise", "Game 10", "Game #9", "game #56", "Game #556"]);
    }

    #[test]
    fn release_round_trip() {
        assert_eq!(super::release_input("27/09/2022"), "2022-09-27");
        assert_eq!(super::release(&super::release_input("05/10/2016")), "05/10/2016");
        assert_eq!(super::release_input("2016"), "2016");
    }

    #[test]
    fn dates() {
        assert_eq!(super::date(0), "Never");
        assert_eq!(super::date(1_700_000_000), "14/11/2023");
        assert_eq!(super::release("2009-6-9"), "09/06/2009");
        assert_eq!(super::release("9 Jun, 2009"), "09/06/2009");
        assert_eq!(super::release("Jun 9, 2009"), "09/06/2009");
        assert_eq!(super::release("2009"), "2009");
        assert_eq!(super::release("Coming soon"), "Coming soon");
    }
}
