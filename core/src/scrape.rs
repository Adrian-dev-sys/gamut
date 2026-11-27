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
    metacritic: Option<Metacritic>,
}
#[derive(Deserialize)]
struct Metacritic {
    score: i64,
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
            critic_score: d.metacritic.map(|m| m.score).filter(|s| (1..=100).contains(s)),
            ..Default::default()
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

/// Finds a Steam appid whose name matches `name` or one of its `name_variants`.
pub fn steam_search(name: &str) -> Result<Option<u32>> {
    for v in name_variants(name) {
        if let Some((id, _)) = steam_find(&v)?.into_iter().find(|(_, n)| same_game(&v, n)) {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

/// Words that mark a fan port, not part of the game's title.
const PORT_WORDS: &[&str] = &["recomp", "recompiled", "recompilation", "decomp", "decompiled", "decompilation", "native"];

/// Names to search for, best first: the name, then without port words ("Fire Emblem Engage Recomp"), then each
/// part around " - " ("Majora's Mask 64 PC port - Zelda64 Recompiled" -> "Majora's Mask", "Zelda64").
pub fn name_variants(name: &str) -> Vec<String> {
    let mut out = vec![name.trim().to_string()];
    let words: Vec<&str> = name.split_whitespace().collect();
    let mut kept = Vec::new();
    for (i, w) in words.iter().enumerate() {
        let l = w.to_lowercase();
        let next = words.get(i + 1).map(|n| n.to_lowercase());
        if PORT_WORDS.contains(&l.as_str()) || l == "port" || l == "pc" && next.as_deref() == Some("port") {
            continue;
        }
        kept.push(*w);
    }
    let stripped = kept.join(" ");
    let mut push = |s: &str| {
        let s = s.trim().trim_matches(|c: char| c == '-' || c == ':').trim();
        // "Ocarina of Time 64" is "Ocarina of Time" elsewhere.
        let s = s.strip_suffix(" 64").unwrap_or(s).trim();
        if normalize(s).len() >= 3 && !out.iter().any(|o| normalize(o) == normalize(s)) {
            out.push(s.to_string());
        }
    };
    push(&stripped);
    for part in stripped.split(" - ") {
        push(part);
    }
    out
}

/// A search hit is the game: same title, or the title without a series prefix
/// ("The Legend of Zelda: Tears of the Kingdom" for "Tears of the Kingdom").
pub fn same_game(want: &str, title: &str) -> bool {
    let k = |s: &str| normalize(&crate::retro::title_key(s));
    let w = k(want);
    if w.is_empty() {
        return false;
    }
    if k(title) == w {
        return true;
    }
    w.len() >= 8 && title.split_once(": ").or_else(|| title.split_once(" - ")).is_some_and(|(_, sub)| k(sub) == w)
}

/// Store search results (appid, name), Steam's order.
pub fn steam_find(name: &str) -> Result<Vec<(u32, String)>> {
    let url = format!("https://store.steampowered.com/api/storesearch/?term={}&l=english&cc=US", encode(name));
    let r: SearchResp = get_json(&url)?;
    Ok(r.items.into_iter().map(|i| (i.id, i.name)).collect())
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

/// Errors when SteamGridDB rejects the key or can't be reached.
pub fn sgdb_check(key: &str) -> Result<()> {
    sgdb_get::<serde_json::Value>(key, "/search/autocomplete/portal").map(|_| ()).map_err(|e| {
        match e.downcast_ref::<ureq::Error>() {
            Some(ureq::Error::StatusCode(c)) => anyhow::anyhow!("SteamGridDB answered HTTP {c}"),
            None if e.to_string().contains("401") => e,
            _ => anyhow::anyhow!("Couldn't reach SteamGridDB"),
        }
    })
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
            // Only an exact name match (or the name without a series prefix): the top fuzzy hit is often a different game.
            for v in name_variants(name) {
                let l: SgdbList<SgdbGame> = sgdb_get(key, &format!("/search/autocomplete/{}", encode(&v)))?;
                if let Some(g) = l.data.iter().find(|g| same_game(&v, &g.name)) {
                    return Ok(Some(format!("game/{}", g.id)));
                }
            }
            None
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

/// Every static icon for a game.
pub fn sgdb_icons(key: &str, game: &str, nsfw: bool) -> Result<Vec<SgdbArt>> {
    let nsfw = if nsfw { "&nsfw=any" } else { "" };
    match sgdb_get::<SgdbList<SgdbArt>>(key, &format!("/icons/{game}?types=static{nsfw}")) {
        Ok(l) => Ok(l.data),
        Err(e) if is_missing(&e) => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// Steam's client icon from the local cache.
pub fn steam_official_icon(appid: u32) -> Vec<SgdbArt> {
    let Some(p) = crate::steam::local_icon(appid) else { return Vec::new() };
    let Ok(b) = std::fs::read(&p) else { return Vec::new() };
    let (width, height) = crate::images::dimensions(&b).unwrap_or((0, 0));
    let url = p.display().to_string();
    vec![SgdbArt { thumb: url.clone(), url, width, height, source: Some("Your Steam library") }]
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

/// How well a search result's title fits the query: 100 same, 60 one starts the other, else shared words (0–50).
pub fn title_score(query: &str, title: &str) -> u32 {
    let (q, t) = (normalize(query), normalize(title));
    if q.is_empty() || t.is_empty() {
        return 0;
    }
    if q == t {
        return 100;
    }
    if q.starts_with(&t) || t.starts_with(&q) {
        return 60;
    }
    let words = |s: &str| -> Vec<String> {
        s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(|w| w.to_lowercase()).collect()
    };
    let (qw, tw) = (words(query), words(title));
    let shared = qw.iter().filter(|w| tw.contains(w)).count();
    (shared * 50 / qw.len().max(tw.len()).max(1)) as u32
}

/// The best `n` results by `title_score` against any of their titles; weak ones (under half the best) are dropped.
pub fn best_matches<T>(query: &str, items: Vec<T>, titles: impl Fn(&T) -> Vec<String>, n: usize) -> Vec<T> {
    let mut scored: Vec<(u32, T)> =
        items.into_iter().map(|i| (titles(&i).iter().map(|t| title_score(query, t)).max().unwrap_or(0), i)).collect();
    // Stable: equal scores keep the source's own order.
    scored.sort_by_key(|(s, _)| std::cmp::Reverse(*s));
    let best = scored.first().map(|(s, _)| *s).unwrap_or(0);
    scored.into_iter().take_while(|(s, _)| *s * 2 >= best).take(n).map(|(_, i)| i).collect()
}

/// Crude HTML → text for Steam descriptions.
/// Line prefix that keeps an image's URL in otherwise plain descriptions.
pub const IMG_MARKER: &str = "[img]";

/// A description's text without image lines or whitespace differences, for comparing versions.
pub fn text_only(d: &str) -> String {
    d.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with(IMG_MARKER))
        .map(|l| l.trim_start_matches(HEADING).trim_start_matches(BULLET).replace(BOLD, ""))
        .flat_map(|l| l.split_whitespace().map(str::to_string).collect::<Vec<_>>())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Light markup kept from store HTML: `## ` heading lines, `• ` list items, `**bold**`;
/// a blank line separates paragraphs, a single newline is a `<br>`.
pub const HEADING: &str = "## ";
pub const BULLET: &str = "• ";
pub const BOLD: &str = "**";

pub fn strip_html(html: &str) -> String {
    let src_re = regex::Regex::new(r#"(?i)\bsrc\s*=\s*["']([^"']+)["']"#).unwrap();
    let poster_re = regex::Regex::new(r#"(?i)\bposter\s*=\s*["']([^"']+)["']"#).unwrap();
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
                let close = tag.starts_with('/');
                let name = tag.trim_start_matches('/').split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or("").to_ascii_lowercase();
                match name.as_str() {
                    // Steam clips are <video poster=...>; the poster URL leads the player to the webm/mp4.
                    "img" | "video" if !close => {
                        let re = if name == "img" { &src_re } else { &poster_re };
                        if let Some(c) = re.captures(&tag).filter(|c| c[1].starts_with("http")) {
                            out.push_str(&format!("\n\n{IMG_MARKER}{}\n\n", &c[1]));
                        }
                    }
                    "br" => out.push('\n'),
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" if !close => out.push_str(&format!("\n\n{HEADING}")),
                    "p" | "div" | "ul" | "ol" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => out.push_str("\n\n"),
                    "li" if !close => out.push_str(&format!("\n{BULLET}")),
                    "li" => out.push('\n'),
                    "strong" | "b" => out.push_str(BOLD),
                    _ => {}
                }
            }
            _ if in_tag => tag.push(c),
            // Newlines in HTML source are just whitespace.
            '\r' | '\n' | '\t' => out.push(' '),
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
    // Tidy each line, drop empty markup, and keep at most one blank line between blocks.
    let mut res = String::new();
    let (mut blank, mut prev_bullet, mut pending_bullet) = (0, false, false);
    for line in out.split('\n') {
        let mut l = line.split_whitespace().collect::<Vec<_>>().join(" ").replace("****", "");
        // <li><p>text</p></li> leaves the marker alone on its line; it belongs to the next text.
        if l == BULLET.trim() {
            pending_bullet = true;
            continue;
        }
        if l == BOLD || l == HEADING.trim() {
            l.clear();
        }
        if l.is_empty() {
            blank += 1;
            continue;
        }
        if std::mem::take(&mut pending_bullet) && !l.starts_with(BULLET) && !l.starts_with(IMG_MARKER) {
            l.insert_str(0, BULLET);
        }
        // List items stay on adjacent lines even when a <br> ends the previous one.
        let bullet = l.starts_with(BULLET);
        if !res.is_empty() {
            res.push_str(if blank > 0 && !(bullet && prev_bullet) { "\n\n" } else { "\n" });
        }
        blank = 0;
        prev_bullet = bullet;
        res.push_str(&l);
    }
    res
}

#[cfg(test)]
mod tests {
    #[test]
    fn port_name_variants() {
        use super::{name_variants, same_game};
        assert_eq!(name_variants("Fire Emblem Engage Recomp"), ["Fire Emblem Engage Recomp", "Fire Emblem Engage"]);
        assert_eq!(
            name_variants("Majora's Mask 64 PC port - Zelda64 Recompiled"),
            ["Majora's Mask 64 PC port - Zelda64 Recompiled", "Majora's Mask 64 - Zelda64", "Majora's Mask", "Zelda64"]
        );
        assert_eq!(name_variants("Celeste"), ["Celeste"]);
        assert!(same_game("Tears of the Kingdom", "The Legend of Zelda: Tears of the Kingdom"));
        assert!(same_game("Pokemon Legends Z-A", "Pokémon Legends: Z-A"));
        assert!(same_game("Hyrule Warriors Age of Imprisonment", "Hyrule Warriors: Age of Imprisonment"));
        assert!(!same_game("Zelda64", "Zelda 64: Dawn & Dusk"));
        assert!(!same_game("Doom", "Final: Doom"));
    }

    #[test]
    fn ranks_search_results() {
        use super::{best_matches, title_score};
        assert_eq!(title_score("Senren Banka", "Senren * Banka"), 100);
        assert_eq!(title_score("Aokana", "Aokana EXTRA1"), 60);
        assert_eq!(title_score("Black Souls II", "Dark Souls II"), 33);
        let hits = vec!["Dark Souls II", "Black Souls", "Souls", "Black Souls II", "Demon's Souls", "Lords"];
        assert_eq!(best_matches("Black Souls II", hits, |h| vec![h.to_string()], 3), ["Black Souls II", "Black Souls"]);
    }

    use super::*;
    #[test]
    fn html_and_normalize() {
        assert_eq!(strip_html("<h2>Hi</h2>A &amp; B<br>C"), "## Hi\n\nA & B\nC");
        assert_eq!(strip_html("A<img src=\"https://x/y.gif?t=1\" width=5>B"), "A\n\n[img]https://x/y.gif?t=1\n\nB");
        // Playnite: source newlines are whitespace, <br><br> is a paragraph.
        assert_eq!(
            strip_html(r#"<video class="bb_img" poster="https://x/a.poster.avif?t=1"><source src="https://x/a.webm?t=1"></video>Hi"#),
            "[img]https://x/a.poster.avif?t=1\n\nHi"
        );
        assert_eq!(
            strip_html("Feat:<ul><li><p>One</p></li></ul><ul><li><p>Two</p></li><li><p>Three</p></li></ul>End"),
            "Feat:\n\n• One\n• Two\n• Three\n\nEnd"
        );
        assert_eq!(strip_html("One.<br />\r\nTwo.<br><br>Three."), "One.\nTwo.\n\nThree.");
        assert_eq!(
            strip_html("Intro.<h2 class=\"bb_tag\">Key Features</h2> <ul class=\"bb_ul\"><li><strong>Horror</strong> \u{2013} text <br></li><li><b>Co-op</b> - more</li></ul>End"),
            "Intro.\n\n## Key Features\n\n\u{2022} **Horror** \u{2013} text\n\u{2022} **Co-op** - more\n\nEnd"
        );
        assert_eq!(text_only("## Key\n\n\u{2022} **Horror** x"), text_only("Key\nHorror x"));
        assert_eq!(normalize("DOOM Eternal™"), "doometernal");
    }
}

#[cfg(test)]
mod preview {
    /// `GAMUT_HTML=<file> cargo test -p gamut-core -- --ignored preview --nocapture`
    #[test]
    #[ignore]
    fn preview() {
        println!("{}", super::strip_html(&String::from_utf8_lossy(&std::fs::read(std::env::var("GAMUT_HTML").unwrap()).unwrap())));
    }
}
