//! RetroAchievements: console lookup, title matching and per-game progress.
use crate::achievements::{Achievement, Fetched};
use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;

const API: &str = "https://retroachievements.org/API/";
/// Console and game lists change rarely; refetched after this long.
const LIST_TTL: i64 = 7 * 86400;

/// `ra_id` values that aren't RA game ids.
pub const NOT_FOUND: i64 = 0;
pub const UNSUPPORTED: i64 = -1;

/// GETs an endpoint; errors name the endpoint and status, never the URL (it holds the key).
fn get(endpoint: &str, key: &str, params: &[(&str, String)]) -> Result<Value> {
    let mut url = format!("{API}{endpoint}.php?y={}", crate::http::encode(key));
    for (k, v) in params {
        url.push_str(&format!("&{k}={}", crate::http::encode(v)));
    }
    crate::http::get_json::<Value>(&url).map_err(|e| {
        let what = match e.downcast_ref::<ureq::Error>() {
            Some(ureq::Error::StatusCode(401 | 403)) => "check the RetroAchievements username and API key".to_string(),
            Some(ureq::Error::StatusCode(c)) => format!("HTTP {c}"),
            _ => "network error".to_string(),
        };
        anyhow::anyhow!("RetroAchievements {endpoint} failed: {what}")
    })
}

/// A JSON list cached in `dir/file`, refetched when older than a week (or kept if the fetch fails).
fn cached(dir: &Path, file: &str, fetch: impl FnOnce() -> Result<Value>) -> Result<Value> {
    let path = dir.join(file);
    let age = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .map_or(i64::MAX, |d| d.as_secs() as i64);
    let old = || std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok());
    if age < LIST_TTL {
        if let Some(v) = old() {
            return Ok(v);
        }
    }
    match fetch() {
        Ok(v) => {
            std::fs::create_dir_all(dir)?;
            std::fs::write(&path, serde_json::to_vec(&v)?)?;
            Ok(v)
        }
        Err(e) => old().ok_or(e),
    }
}

fn int(v: &Value, k: &str) -> i64 {
    v.get(k).and_then(|x| x.as_i64().or_else(|| x.as_str()?.parse().ok())).unwrap_or(0)
}
fn text<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

/// RA systems as (id, name).
pub fn consoles(key: &str, cache: &Path) -> Result<Vec<(i64, String)>> {
    let v = cached(cache, "consoles.json", || get("API_GetConsoleIDs", key, &[("g", "1".into())]))?;
    Ok(v.as_array().into_iter().flatten().map(|c| (int(c, "ID"), text(c, "Name").to_string())).collect())
}

/// Games with achievements on one system, as (id, title).
pub fn games(key: &str, console: i64, cache: &Path) -> Result<Vec<(i64, String)>> {
    let v = cached(cache, &format!("games-{console}.json"), || {
        get("API_GetGameList", key, &[("i", console.to_string()), ("f", "1".into())])
    })?;
    Ok(v.as_array().into_iter().flatten().map(|g| (int(g, "ID"), text(g, "Title").to_string())).collect())
}

/// Lowercase words without punctuation or manufacturer, for comparing system names.
fn plain(s: &str) -> String {
    let s: String = s.to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { ' ' }).collect();
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The RA system for one of a game's platforms (Playnite names like "Nintendo SNES", "Sega Genesis").
pub fn console_for(platform: &str, consoles: &[(i64, String)]) -> Option<i64> {
    const ALIASES: &[(&str, &str)] = &[
        ("nintendo entertainment system", "nes"),
        ("nintendo family computer", "nes"),
        ("nintendo family computer disk system", "nes"),
        ("super nintendo entertainment system", "snes"),
        ("nintendo super famicom", "snes"),
        ("sega mega drive", "mega drive"),
        ("nec turbografx 16", "pc engine"),
        ("nec pc engine", "pc engine"),
        ("nec turbografx cd", "pc engine cd"),
        ("sega 32x", "32x"),
        ("sega cd", "sega cd"),
        ("sony psp", "playstation portable"),
    ];
    const MAKERS: &[&str] = &["nintendo ", "sony ", "sega ", "microsoft ", "nec ", "atari ", "snk ", "bandai ", "coleco ", "mattel "];
    platform.split(',').map(str::trim).filter(|p| !p.is_empty()).find_map(|p| {
        let full = plain(p);
        let alias = ALIASES.iter().find(|(k, _)| *k == full).map(|(_, v)| v.to_string());
        let bare = MAKERS.iter().find_map(|m| full.strip_prefix(m)).map(str::to_string);
        let wanted: Vec<String> = [Some(full.clone()), alias, bare].into_iter().flatten().collect();
        consoles.iter().find_map(|(id, name)| {
            let names: Vec<String> = name.split('/').map(plain).collect();
            names.iter().any(|n| wanted.contains(n)).then_some(*id)
        })
    })
}

/// Title key for matching: no region/revision tags, articles, accents or punctuation.
pub fn title_key(t: &str) -> String {
    let mut s = String::new();
    let mut depth = 0;
    for c in t.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = (depth - 1).max(0),
            _ if depth == 0 => s.push(c),
            _ => {}
        }
    }
    let s = s.to_lowercase().replace('&', " and ");
    let s: String = s
        .chars()
        .map(|c| match c {
            'à' | 'á' | 'â' | 'ä' | 'ã' => 'a',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'ò' | 'ó' | 'ô' | 'ö' | 'õ' => 'o',
            'ù' | 'ú' | 'û' | 'ü' => 'u',
            'ñ' => 'n',
            c if c.is_alphanumeric() => c,
            _ => ' ',
        })
        .collect();
    // RA writes "Legend of Zelda, The: ..."; drop articles wherever they ended up.
    let mut words: Vec<&str> = s.split_whitespace().filter(|w| !matches!(*w, "the" | "a" | "an")).collect();
    // "Pokémon FireRed Version" is "Pokémon FireRed" elsewhere.
    if words.len() > 1 && words.last() == Some(&"version") {
        words.pop();
    }
    words.join(" ")
}

/// RA game whose title matches `name` exactly after normalizing; skips hacks, homebrew and subsets.
pub fn find(name: &str, games: &[(i64, String)]) -> Option<i64> {
    let want = title_key(name);
    if want.is_empty() {
        return None;
    }
    games.iter().find_map(|(id, title)| {
        if title.starts_with('~') || title.contains("[Subset") {
            return None;
        }
        title.split(" | ").any(|t| title_key(t) == want).then_some(*id)
    })
}

/// "2023-01-31 18:04:05" (UTC) to unix seconds.
fn unix(s: &str) -> Option<i64> {
    let n: Vec<i64> = s.split(|c: char| !c.is_ascii_digit()).filter(|p| !p.is_empty()).filter_map(|p| p.parse().ok()).collect();
    let [y, m, d, hh, mm, ss, ..] = n[..] else { return None };
    // Days from civil date (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146097 + doe - 719468) * 86400 + hh * 3600 + mm * 60 + ss)
}

/// The user's progress in one RA game, with names, badges, points and rarity.
pub fn progress(user: &str, key: &str, game: i64) -> Result<Fetched> {
    let v = get("API_GetGameInfoAndUserProgress", key, &[("u", user.to_string()), ("g", game.to_string())])?;
    parse_progress(&v).context("unexpected RetroAchievements reply")
}

fn parse_progress(v: &Value) -> Option<Fetched> {
    let players = int(v, "NumDistinctPlayers").max(0) as f32;
    // An object keyed by id, or `[]` when the game has none.
    let list: Vec<&Value> = match v.get("Achievements")? {
        Value::Object(m) => m.values().collect(),
        Value::Array(a) => a.iter().collect(),
        _ => return None,
    };
    let mut list: Vec<&Value> = list.into_iter().filter(|a| a.is_object()).collect();
    if list.is_empty() {
        return Some(Fetched::None);
    }
    list.sort_by_key(|a| (int(a, "DisplayOrder"), int(a, "ID")));
    let badge = |b: &str, lock: &str| if b.is_empty() { String::new() } else { format!("https://media.retroachievements.org/Badge/{b}{lock}.png") };
    let items = list
        .into_iter()
        .map(|a| {
            let earned = [text(a, "DateEarnedHardcore"), text(a, "DateEarned")].into_iter().find_map(unix).unwrap_or(0);
            let b = text(a, "BadgeName");
            Achievement {
                key: int(a, "ID").to_string(),
                name: text(a, "Title").to_string(),
                description: text(a, "Description").to_string(),
                icon: badge(b, ""),
                icon_locked: badge(b, "_lock"),
                hidden: false,
                unlocked: earned,
                percent: (players > 0.0).then(|| int(a, "NumAwarded") as f32 * 100.0 / players),
                points: int(a, "Points"),
            }
        })
        .collect();
    Some(Fetched::List { items, full: true })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consoles_from_playnite_names() {
        let c: Vec<(i64, String)> = [
            (1, "Genesis/Mega Drive"), (2, "Nintendo 64"), (3, "SNES/Super Famicom"), (4, "Game Boy"),
            (5, "Game Boy Advance"), (6, "Game Boy Color"), (7, "NES/Famicom"), (12, "PlayStation"),
            (16, "GameCube"), (18, "Nintendo DS"), (19, "Wii"), (21, "PlayStation 2"), (41, "PlayStation Portable"),
        ]
        .into_iter()
        .map(|(i, n)| (i, n.to_string()))
        .collect();
        for (p, want) in [
            ("Nintendo SNES", Some(3)), ("Nintendo Entertainment System", Some(7)), ("Sega Genesis", Some(1)),
            ("Nintendo 64", Some(2)), ("Nintendo Game Boy", Some(4)), ("Nintendo Game Boy Color", Some(6)),
            ("Nintendo Game Boy Advance", Some(5)), ("Sony PlayStation", Some(12)), ("Sony PlayStation 2", Some(21)),
            ("Sony PlayStation Portable", Some(41)), ("Nintendo GameCube", Some(16)), ("Nintendo Wii", Some(19)),
            ("Nintendo DS", Some(18)), ("Nintendo Entertainment System, Nintendo Family Computer Disk System", Some(7)),
            ("Nintendo Switch", None), ("Sony PlayStation 4", None), ("PC (Windows)", None), ("Nintendo Wii U", None),
        ] {
            assert_eq!(console_for(p, &c), want, "{p}");
        }
    }

    #[test]
    fn titles_match_ra_style() {
        let g: Vec<(i64, String)> = [
            (1, "~Hack~ Super Mario World: The Lost Levels"), (2, "Legend of Zelda, The: A Link to the Past"),
            (3, "Pokemon Emerald Version"), (4, "Super Mario World [Subset - Bonus]"), (5, "Super Mario World"),
            (6, "Pocket Monsters Ao | Pokemon Blue Version"),
        ]
        .into_iter()
        .map(|(i, n)| (i, n.to_string()))
        .collect();
        assert_eq!(find("The Legend of Zelda: A Link to the Past", &g), Some(2));
        assert_eq!(find("Pokémon Emerald Version (USA, Europe)", &g), Some(3));
        assert_eq!(find("Super Mario World", &g), Some(5));
        assert_eq!(find("Pokemon Blue Version", &g), Some(6));
        assert_eq!(find("Pokémon Emerald", &g), Some(3), "RA's \"Version\" suffix is optional");
        assert_eq!(find("Super Mario World: The Lost Levels", &g), None, "hacks are skipped");
        assert_eq!(find("Chrono Trigger", &g), None);
    }

    #[test]
    fn parses_progress_object_and_empty_array() {
        let v = serde_json::json!({
            "ID": 1, "NumDistinctPlayers": 200,
            "Achievements": {
                "9": {"ID": 9, "Title": "B", "Description": "b", "Points": 10, "BadgeName": "250336",
                      "NumAwarded": 50, "DisplayOrder": 2, "DateEarned": "2023-01-31 18:04:05"},
                "8": {"ID": 8, "Title": "A", "Description": "a", "Points": 5, "BadgeName": "1", "NumAwarded": 150, "DisplayOrder": 1}
            }
        });
        let Some(Fetched::List { items, full: true }) = parse_progress(&v) else { panic!() };
        assert_eq!(items[0].name, "A");
        assert_eq!(items[0].unlocked, 0);
        assert_eq!(items[1].unlocked, 1675188245);
        assert_eq!(items[1].percent, Some(25.0));
        assert_eq!(items[1].icon_locked, "https://media.retroachievements.org/Badge/250336_lock.png");
        assert!(matches!(parse_progress(&serde_json::json!({"Achievements": []})), Some(Fetched::None)));
    }
}
