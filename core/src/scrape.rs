use crate::http::{agent, encode, get_bytes, get_json};
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
    Ok(Some(SteamDetails {
        kind: d.kind,
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
pub fn steam_cdn_cover(appid: u32) -> Result<Vec<u8>> {
    get_bytes(&format!("{CDN}/{appid}/library_600x900_2x.jpg"))
        .or_else(|_| get_bytes(&format!("{CDN}/{appid}/library_600x900.jpg")))
}

pub fn steam_cdn_hero(appid: u32) -> Result<Vec<u8>> {
    get_bytes(&format!("{CDN}/{appid}/library_hero.jpg"))
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
}

fn sgdb_get<T: serde::de::DeserializeOwned>(key: &str, path: &str) -> Result<T> {
    Ok(agent()
        .get(&format!("{SGDB}{path}"))
        .header("Authorization", &format!("Bearer {key}"))
        .call()?
        .body_mut()
        .read_json()?)
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
            l.data.first().map(|g| format!("game/{}", g.id))
        }
    })
}

fn sgdb_first(key: &str, kind: &str, game: &str, query: &str) -> Result<Option<Vec<u8>>> {
    // Unknown steam ids come back as 404; treat as "nothing found".
    let l: SgdbList<SgdbImage> = match sgdb_get(key, &format!("/{kind}/{game}?{query}")) {
        Ok(l) => l,
        Err(e) if e.to_string().contains("404") => return Ok(None),
        Err(e) => return Err(e),
    };
    match l.data.first() {
        Some(img) => Ok(Some(get_bytes(&img.url)?)),
        None => Ok(None),
    }
}

/// Portrait cover + hero from SteamGridDB.
pub fn sgdb_art(key: &str, r: SgdbRef) -> Result<(Option<Vec<u8>>, Option<Vec<u8>>)> {
    let Some(game) = sgdb_game_path(key, &r)? else { return Ok((None, None)) };
    let cover = sgdb_first(key, "grids", &game, "dimensions=600x900&types=static")?;
    let hero = sgdb_first(key, "heroes", &game, "types=static").unwrap_or(None);
    Ok((cover, hero))
}

// ---------- helpers ----------

/// Lowercase alphanumerics only, so "DOOM Eternal™" == "doom eternal".
pub fn normalize(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// Crude HTML → text for Steam descriptions.
pub fn strip_html(html: &str) -> String {
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
                if t.starts_with("br") || t.starts_with("p") || t.starts_with("li") || t.starts_with("h") {
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
        assert_eq!(normalize("DOOM Eternal™"), "doometernal");
    }
}
