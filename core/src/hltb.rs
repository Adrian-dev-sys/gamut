//! HowLongToBeat times through the site's own (unofficial, changing) search API.
use crate::http::{agent, throttle};
use crate::scrape;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::sync::Mutex;

const BASE: &str = "https://howlongtobeat.com/";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0 Safari/537.36";
/// Re-check found games after this long (times shift as people submit), missing ones sooner.
pub const REFRESH_SECS: i64 = 60 * 86_400;
pub const RETRY_SECS: i64 = 14 * 86_400;

/// Seconds; 0 = no data. `id` 0 = not found on HowLongToBeat.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Times {
    pub id: i64,
    pub main: i64,
    pub plus: i64,
    pub full: i64,
    pub fetched: i64,
}

impl Times {
    pub fn url(&self) -> Option<String> {
        (self.id > 0).then(|| format!("{BASE}game/{}", self.id))
    }
    pub fn stale(&self, now: i64) -> bool {
        now - self.fetched > if self.id > 0 { REFRESH_SECS } else { RETRY_SECS }
    }
}

#[derive(Debug, Deserialize)]
struct Hit {
    game_id: i64,
    game_name: String,
    #[serde(default)]
    game_alias: Option<String>,
    comp_main: Option<i64>,
    comp_plus: Option<i64>,
    comp_100: Option<i64>,
}

#[derive(Deserialize)]
struct Page {
    data: Vec<Hit>,
}

/// The search endpoint ("api/search/site"), found in the site's scripts; cached for the session.
fn search_path() -> String {
    static PATH: Mutex<Option<String>> = Mutex::new(None);
    if let Some(p) = PATH.lock().unwrap().clone() {
        return p;
    }
    let p = discover().unwrap_or_else(|e| {
        log::warn!("HowLongToBeat search path not found, using the default: {e:#}");
        "api/s".into()
    });
    *PATH.lock().unwrap() = Some(p.clone());
    p
}

fn get_text(url: &str) -> Result<String> {
    throttle(url);
    Ok(agent().get(url).header("User-Agent", UA).header("Referer", BASE).call()?.body_mut().with_config().limit(8 << 20).read_to_string()?)
}

fn discover() -> Result<String> {
    let html = get_text(BASE)?;
    let script = regex::Regex::new(r#"<script[^>]+src="([^"]*/_next/static/chunks/[^"]+)""#)?;
    let call = regex::Regex::new(r#"(?is)fetch\s*\(\s*["']/api/([a-zA-Z0-9_/]+)[^"']*["']\s*,\s*\{[^}]*method:\s*["']POST["'][^}]*\}"#)?;
    for c in script.captures_iter(&html) {
        let src = &c[1];
        let url = if src.starts_with("http") { src.to_string() } else { format!("{BASE}{}", src.trim_start_matches('/')) };
        if let Some(m) = get_text(&url).ok().and_then(|js| call.captures(&js).map(|m| m[1].trim_end_matches('/').to_string())) {
            return Ok(format!("api/{m}"));
        }
    }
    anyhow::bail!("no search call in {} scripts", script.captures_iter(&html).count())
}

fn search(name: &str) -> Result<Vec<Hit>> {
    let path = search_path();
    let init: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&get_text(&format!("{BASE}{path}/init?t={}", crate::now() * 1000))?).context("HowLongToBeat init")?;
    let mut body = serde_json::json!({
        "searchType": "games",
        "searchTerms": name.split_whitespace().collect::<Vec<_>>(),
        "searchPage": 1,
        "size": 20,
        "searchOptions": {
            "games": {
                "userId": 0, "platform": "", "sortCategory": "popular", "rangeCategory": "main",
                "rangeTime": {"min": 0, "max": 0},
                "gameplay": {"perspective": "", "flow": "", "genre": "", "difficulty": ""},
                "rangeYear": {"max": "", "min": ""}, "modifier": ""
            },
            "users": {"sortCategory": "postcount"}, "lists": {"sortCategory": "follows"},
            "filter": "", "sort": 0, "randomizer": 0
        },
        "useCache": true
    });
    let url = format!("{BASE}{path}");
    throttle(&url);
    let mut req = agent()
        .post(&url)
        .header("User-Agent", UA)
        .header("Referer", BASE)
        .header("Origin", BASE)
        .header("Content-Type", "application/json")
        .header("Accept", "*/*");
    if let Some(t) = init.get("token").and_then(|v| v.as_str()) {
        req = req.header("x-auth-token", t);
    }
    // Some versions also hand out a key/value pair that goes in the headers and the body.
    let field = |want: &str| init.iter().find(|(k, _)| k.to_lowercase().contains(want)).and_then(|(_, v)| v.as_str().map(str::to_string));
    if let (Some(k), Some(v)) = (field("key"), field("val")) {
        req = req.header("x-hp-key", &k).header("x-hp-val", &v);
        body[k] = serde_json::Value::String(v);
    }
    let page: Page = req.send(body.to_string())?.body_mut().read_json().context("HowLongToBeat search")?;
    Ok(page.data)
}

/// Times for a game, matched strictly by name (or its fan-port variants); `id` 0 when nothing matches.
pub fn lookup(name: &str) -> Result<Times> {
    for v in scrape::name_variants(name) {
        let hits = search(&v)?;
        let same = |h: &&Hit| scrape::same_game(&v, &h.game_name) || h.game_alias.as_deref().is_some_and(|a| a.split(", ").any(|a| scrape::same_game(&v, a)));
        if let Some(h) = hits.iter().find(same) {
            let s = |x: Option<i64>| x.unwrap_or(0).max(0);
            return Ok(Times { id: h.game_id, main: s(h.comp_main), plus: s(h.comp_plus), full: s(h.comp_100), fetched: crate::now() });
        }
    }
    Ok(Times { fetched: crate::now(), ..Default::default() })
}

/// "18h", "46.5h", "—".
pub fn hours(secs: i64) -> String {
    if secs <= 0 {
        return "—".into();
    }
    let half_hours = (secs + 900) / 1800;
    match (half_hours / 2, half_hours % 2) {
        (0, _) => "<1h".into(),
        (h, 0) => format!("{h}h"),
        (h, _) => format!("{h}.5h"),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "network"]
    fn finds_a_game() {
        let t = super::lookup("Monster Hunter Wilds").unwrap();
        assert_eq!(t.id, 141854);
        assert!(t.main > 3600 && t.full > t.main);
    }

    #[test]
    fn formats_hours() {
        assert_eq!((super::hours(65398), super::hours(165638), super::hours(0)), ("18h".into(), "46h".into(), "—".into()));
        assert_eq!(super::hours(3 * 3600 + 1700), "3.5h");
    }
}
