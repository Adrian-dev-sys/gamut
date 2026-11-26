use crate::http::{agent, encode, get_bytes, get_bytes_opt, get_json, is_missing};
use crate::model::Metadata;
use anyhow::Result;
use serde::Deserialize;
use std::collections::HashMap;

// ---------- Steam store ----------

#[derive(Deserialize)]
struct AppDetailsEnvelope {
    success: bool,
    data: Option<AppData>,
}

#[derive(Deserialize)]
struct AppData {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    short_description: String,
    #[serde(default)]
    about_the_game: String,
    #[serde(default)]
    developers: Vec<String>,
    #[serde(default)]
    publishers: Vec<String>,
    #[serde(default)]
    genres: Vec<Named>,
    release_date: Option<Release>,
    fullgame: Option<FullGame>,
}
#[derive(Deserialize)]
struct FullGame {
    appid: serde_json::Value,
}
#[derive(Deserialize)]
struct Named {
    description: String,
}
#[derive(Deserialize)]
struct Release {
    #[serde(default)]
    date: String,
}

pub struct SteamDetails {
    pub kind: String,
    /// For demos/playtests: the game they belong to.
    pub fullgame: Option<u32>,
    pub meta: Metadata,
}

/// Store page data for an app. `Ok(None)` when Steam has no store entry.
pub fn steam_details(appid: u32) -> Result<Option<SteamDetails>> {
    let url = format!("https://store.steampowered.com/api/appdetails?appids={appid}&l=english");
    let mut r: HashMap<String, AppDetailsEnvelope> = get_json(&url)?;
    let Some(env) = r.remove(&appid.to_string()) else { return Ok(None) };
    let Some(d) = env.data.filter(|_| env.success) else { return Ok(None) };
    let about = strip_html(&d.about_the_game);
    let desc = if about.len() > d.short_description.len() { about } else { d.short_description.clone() };
    let join = |v: &[String]| (!v.is_empty()).then(|| v.join(", "));
    let fullgame = d.fullgame.and_then(|f| match f.appid {
        serde_json::Value::String(s) => s.parse().ok(),
        serde_json::Value::Number(n) => n.as_u64().map(|n| n as u32),
        _ => None,
    });
    Ok(Some(SteamDetails {
        kind: d.kind,
        fullgame,
        meta: Metadata {
            description: (!desc.trim().is_empty()).then(|| desc.trim().to_string()),
            developer: join(&d.developers),
            publisher: join(&d.publishers),
            release_date: d.release_date.map(|r| r.date).filter(|s| !s.is_empty()),
            genres: join(&d.genres.into_iter().map(|g| g.description).collect::<Vec<_>>()),
        },
    }))
}

#[derive(Deserialize)]
struct SearchResp {
    #[serde(default)]
    items: Vec<SearchItem>,
}
#[derive(Deserialize)]
struct SearchItem {
    id: u32,
    name: String,
}

/// Finds a Steam appid whose name matches `name` (normalized).
pub fn steam_search(name: &str) -> Result<Option<u32>> {
    let url = format!(
        "https://store.steampowered.com/api/storesearch/?term={}&l=english&cc=US",
        encode(name)
    );
    let r: SearchResp = get_json(&url)?;
    let want = normalize(name);
    Ok(r.items.into_iter().find(|i| normalize(&i.name) == want).map(|i| i.id))
}

const CDN: &str = "https://shared.cloudflare.steamstatic.com/store_item_assets/steam/apps";

/// Portrait cover (600x900) from Steam's CDN.
pub fn steam_cdn_cover(appid: u32) -> Result<Option<Vec<u8>>> {
    match get_bytes_opt(&format!("{CDN}/{appid}/library_600x900_2x.jpg"))? {
        Some(b) => Ok(Some(b)),
        None => get_bytes_opt(&format!("{CDN}/{appid}/library_600x900.jpg")),
    }
}

pub fn steam_cdn_hero(appid: u32) -> Result<Option<Vec<u8>>> {
    get_bytes_opt(&format!("{CDN}/{appid}/library_hero.jpg"))
}

// ---------- SteamGridDB ----------

const SGDB: &str = "https://www.steamgriddb.com/api/v2";

#[derive(Deserialize)]
struct SgdbList<T> {
    #[serde(default = "Vec::new")]
    data: Vec<T>,
}
#[derive(Deserialize)]
struct SgdbImage {
    url: String,
}
#[derive(Deserialize)]
struct SgdbGame {
    id: u64,
    #[serde(default)]
    name: String,
}

fn sgdb_get<T: serde::de::DeserializeOwned>(key: &str, path: &str) -> Result<T> {
    let url = format!("{SGDB}{path}");
    crate::http::throttle(&url);
    let resp = agent()
        .get(&url)
        .header("Authorization", &format!("Bearer {key}"))
        .call();
    match resp {
        Ok(mut r) => Ok(r.body_mut().read_json()?),
        Err(ureq::Error::StatusCode(401)) => anyhow::bail!("SteamGridDB rejected the API key (401)"),
        Err(e) => Err(e.into()),
    }
}

/// Which game a SteamGridDB lookup refers to.
pub enum SgdbRef<'a> {
    Steam(u32),
    Name(&'a str),
}

fn sgdb_game_path(key: &str, r: &SgdbRef) -> Result<Option<String>> {
    Ok(match r {
        SgdbRef::Steam(id) => Some(format!("steam/{id}")),
        SgdbRef::Name(name) => {
            let l: SgdbList<SgdbGame> = sgdb_get(key, &format!("/search/autocomplete/{}", encode(name)))?;
            // Only an exact name match: the top fuzzy hit is often a different game.
            let want = normalize(name);
            l.data.iter().find(|g| normalize(&g.name) == want).map(|g| format!("game/{}", g.id))
        }
    })
}

fn sgdb_first(key: &str, kind: &str, game: &str, query: &str) -> Result<Option<Vec<u8>>> {
    // Unknown steam ids come back as 404; treat as "nothing found".
    let l: SgdbList<SgdbImage> = match sgdb_get(key, &format!("/{kind}/{game}?{query}")) {
        Ok(l) => l,
        Err(e) if is_missing(&e) => return Ok(None),
        Err(e) => return Err(e),
    };
    match l.data.first() {
        Some(img) => Ok(Some(get_bytes(&img.url)?)),
        None => Ok(None),
    }
}

/// Portrait cover and/or hero from SteamGridDB (only the ones asked for).
pub fn sgdb_art(key: &str, r: SgdbRef, cover: bool, hero: bool) -> Result<(Option<Vec<u8>>, Option<Vec<u8>>)> {
    let Some(game) = sgdb_game_path(key, &r)? else { return Ok((None, None)) };
    let cover = if cover { sgdb_first(key, "grids", &game, "dimensions=600x900&types=static")? } else { None };
    let hero = if hero { sgdb_first(key, "heroes", &game, "types=static").unwrap_or(None) } else { None };
    Ok((cover, hero))
}

/// A SteamGridDB game the art picker can browse.
#[derive(Debug, Clone)]
pub struct SgdbMatch {
    /// API path segment, e.g. `game/123` or `steam/400`.
    pub path: String,
    pub name: String,
    pub year: Option<i32>,
}

/// One image choice: full-size url plus a small preview.
#[derive(Debug, Clone, Deserialize)]
pub struct SgdbArt {
    pub url: String,
    pub thumb: String,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    /// Where it comes from, when that isn't SteamGridDB.
    #[serde(skip)]
    pub source: Option<&'static str>,
}

#[derive(Deserialize)]
struct SgdbHit {
    id: u64,
    name: String,
    release_date: Option<i64>,
}

/// Games matching a name, best first.
pub fn sgdb_search(key: &str, name: &str) -> Result<Vec<SgdbMatch>> {
    let l: SgdbList<SgdbHit> = sgdb_get(key, &format!("/search/autocomplete/{}", encode(name)))?;
    Ok(l.data
        .into_iter()
        .map(|h| SgdbMatch {
            path: format!("game/{}", h.id),
            name: h.name,
            year: h.release_date.map(|t| 1970 + (t / 31_556_952) as i32),
        })
        .collect())
}

/// Every static cover (portrait grid) or banner (hero) for a game.
pub fn sgdb_list(key: &str, game: &str, hero: bool, nsfw: bool) -> Result<Vec<SgdbArt>> {
    let nsfw = if nsfw { "&nsfw=any" } else { "" };
    let (kind, q) = if hero { ("heroes", "types=static") } else { ("grids", "dimensions=600x900,342x482,660x930&types=static") };
    match sgdb_get::<SgdbList<SgdbArt>>(key, &format!("/{kind}/{game}?{q}{nsfw}")) {
        Ok(l) => Ok(l.data),
        Err(e) if is_missing(&e) => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// Steam's own library art: the local Steam cache copy (includes art customised in Steam), then the store CDN.
pub fn steam_official_art(appid: u32, hero: bool) -> Vec<SgdbArt> {
    let mut out = Vec::new();
    let art = |url: String, bytes: &[u8], source| {
        let (width, height) = crate::images::dimensions(bytes).unwrap_or((0, 0));
        SgdbArt { thumb: url.clone(), url, width, height, source: Some(source) }
    };
    let file = if hero { "library_hero.jpg" } else { "library_600x900.jpg" };
    if let Some(p) = crate::steam::local_art(appid, file) {
        if let Ok(b) = std::fs::read(&p) {
            out.push(art(p.display().to_string(), &b, "Your Steam library"));
        }
    }
    let names = if hero { ["library_hero_2x.jpg", "library_hero.jpg"] } else { ["library_600x900_2x.jpg", "library_600x900.jpg"] };
    for n in names {
        let url = format!("{CDN}/{appid}/{n}");
        if let Ok(Some(b)) = get_bytes_opt(&url) {
            out.push(art(url, &b, "Steam store"));
            break;
        }
    }
    out
}

// ---------- helpers ----------

/// Lowercase alphanumerics only, so "DOOM Eternal™" == "doom eternal".
pub fn normalize(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// Crude HTML → text for Steam descriptions.
/// Line prefix that keeps an image's URL in otherwise plain descriptions.
pub const IMG_MARKER: &str = "[img]";

/// A description's text without image lines or whitespace differences, for comparing versions.
pub fn text_only(d: &str) -> String {
    d.lines()
        .filter(|l| !l.trim().starts_with(IMG_MARKER))
        .flat_map(str::split_whitespace)
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn strip_html(html: &str) -> String {
    let src_re = regex::Regex::new(r#"(?i)\bsrc\s*=\s*["']([^"']+)["']"#).unwrap();
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut tag = String::new();
    for c in html.chars() {
        match c {
            '<' => {
                in_tag = true;
                tag.clear();
            }
            '>' if in_tag => {
                in_tag = false;
                let t = tag.trim_start_matches('/').to_ascii_lowercase();
                if t.starts_with("img") {
                    if let Some(c) = src_re.captures(&tag).filter(|c| c[1].starts_with("http")) {
                        out.push_str(&format!("\n{IMG_MARKER}{}\n", &c[1]));
                    }
                } else if t.starts_with("br") || t.starts_with("p") || t.starts_with("li") || t.starts_with("h") {
                    out.push('\n');
                }
            }
            _ if in_tag => tag.push(c),
            _ => out.push(c),
        }
    }
    let out = out
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ");
    // Collapse 3+ newlines
    let mut res = String::new();
    let mut nl = 0;
    for c in out.chars() {
        if c == '\n' {
            nl += 1;
            if nl <= 2 {
                res.push(c);
            }
        } else if c != '\r' {
            nl = 0;
            res.push(c);
        }
    }
    res.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn html_and_normalize() {
        assert_eq!(strip_html("<h2>Hi</h2>A &amp; B<br>C"), "Hi\nA & B\nC");
        assert_eq!(strip_html("A<img src=\"https://x/y.gif?t=1\" width=5>B"), "A\n[img]https://x/y.gif?t=1\nB");
        assert_eq!(normalize("DOOM Eternal™"), "doometernal");
    }
}
