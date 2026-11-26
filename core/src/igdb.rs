//! IGDB (api.igdb.com v4): metadata and art for games not on Steam. Needs a Twitch developer app's
//! Client ID + secret; the access token is fetched with client credentials and kept in memory.
use crate::model::Metadata;
use crate::Config;
use anyhow::Result;
use serde_json::Value;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const API: &str = "https://api.igdb.com/v4/";
const IMG: &str = "https://images.igdb.com/igdb/image/upload";
const FIELDS: &str = "fields name,summary,storyline,first_release_date,genres.name,platforms.name,\
platforms.abbreviation,platforms.alternative_name,cover.image_id,artworks.image_id,screenshots.image_id,\
involved_companies.company.name,involved_companies.developer,involved_companies.publisher,collections.name,\
franchise.name,franchises.name,aggregated_rating,rating,alternative_names.name,game_type.type;";

#[derive(Debug, Clone, Default)]
pub struct IgdbGame {
    pub id: i64,
    pub name: String,
    pub year: Option<i32>,
    pub platforms: Vec<String>,
    /// Platform names, abbreviations and alternative names, for matching.
    platform_keys: Vec<String>,
    pub alt_names: Vec<String>,
    /// False for DLC, bundles, mods, episodes and the like.
    pub main: bool,
    pub meta: Metadata,
    pub cover: Option<String>,
    pub artworks: Vec<String>,
    pub screenshots: Vec<String>,
}

pub fn configured(cfg: &Config) -> bool {
    !cfg.igdb_client_id.trim().is_empty() && !cfg.igdb_client_secret.trim().is_empty()
}

/// True when the error means the credentials are wrong (not a network hiccup).
pub fn is_auth_error(e: &anyhow::Error) -> bool {
    e.to_string().starts_with(AUTH_ERR)
}
const AUTH_ERR: &str = "IGDB rejected the Client ID or secret";

struct Token {
    client: String,
    token: String,
    until: Instant,
}
static TOKEN: Mutex<Option<Token>> = Mutex::new(None);
/// A recent auth failure (client id, when), so a bad key doesn't hit Twitch once per game.
static FAILED: Mutex<Option<(String, String, Instant)>> = Mutex::new(None);

fn status(e: &ureq::Error) -> Option<u16> {
    match e {
        ureq::Error::StatusCode(c) => Some(*c),
        _ => None,
    }
}

fn token(cfg: &Config, fresh: bool) -> Result<String> {
    let (id, secret) = (cfg.igdb_client_id.trim(), cfg.igdb_client_secret.trim());
    if let Some((c, s, at)) = FAILED.lock().unwrap().as_ref() {
        if c == id && s == secret && at.elapsed() < Duration::from_secs(300) {
            anyhow::bail!(AUTH_ERR);
        }
    }
    let mut guard = TOKEN.lock().unwrap();
    if let Some(t) = guard.as_ref().filter(|t| !fresh && t.client == id && t.until > Instant::now()) {
        return Ok(t.token.clone());
    }
    let url = "https://id.twitch.tv/oauth2/token";
    crate::http::throttle(url);
    // Credentials go in the form body, never the URL.
    let r = crate::http::agent()
        .post(url)
        .send_form([("client_id", id), ("client_secret", secret), ("grant_type", "client_credentials")]);
    let v: Value = match r {
        Ok(mut r) => r.body_mut().read_json()?,
        Err(e) if matches!(status(&e), Some(400 | 401 | 403)) => {
            *FAILED.lock().unwrap() = Some((id.to_string(), secret.to_string(), Instant::now()));
            anyhow::bail!(AUTH_ERR)
        }
        Err(e) => anyhow::bail!("Couldn't reach Twitch to sign in to IGDB ({})", status(&e).map_or("network error".into(), |c| format!("HTTP {c}"))),
    };
    let token = v.get("access_token").and_then(Value::as_str).ok_or_else(|| anyhow::anyhow!("Twitch sent no IGDB token"))?.to_string();
    let secs = v.get("expires_in").and_then(Value::as_u64).unwrap_or(3600);
    *guard = Some(Token { client: id.to_string(), token: token.clone(), until: Instant::now() + Duration::from_secs(secs.saturating_sub(300)) });
    Ok(token)
}

/// POSTs an Apicalypse query; a stale token is refreshed once.
fn query(cfg: &Config, endpoint: &str, body: &str) -> Result<Value> {
    let url = format!("{API}{endpoint}");
    for fresh in [false, true] {
        let tok = token(cfg, fresh)?;
        crate::http::throttle(&url);
        let r = crate::http::agent()
            .post(&url)
            .header("Client-ID", cfg.igdb_client_id.trim())
            .header("Authorization", format!("Bearer {tok}"))
            .header("Accept", "application/json")
            .content_type("text/plain")
            .send(body);
        match r {
            Ok(mut r) => return Ok(r.body_mut().read_json()?),
            Err(e) if status(&e) == Some(401) && !fresh => continue,
            Err(e) if matches!(status(&e), Some(401 | 403)) => anyhow::bail!(AUTH_ERR),
            Err(e) => match status(&e) {
                Some(c) => anyhow::bail!("IGDB answered HTTP {c}"),
                None => anyhow::bail!("Couldn't reach IGDB"),
            },
        }
    }
    unreachable!()
}

fn quote(s: &str) -> String {
    s.replace(['"', '\\'], " ")
}

/// Games matching a name, IGDB's best match first (versions/editions left out).
pub fn search(cfg: &Config, name: &str) -> Result<Vec<IgdbGame>> {
    let v = query(cfg, "games", &format!("{FIELDS} search \"{}\"; where version_parent = null; limit 20;", quote(name)))?;
    Ok(parse(&v))
}

pub fn game(cfg: &Config, id: i64) -> Result<Option<IgdbGame>> {
    Ok(parse(&query(cfg, "games", &format!("{FIELDS} where id = {id};"))?).into_iter().next())
}

/// Status line for Settings.
pub fn check(cfg: &Config) -> Result<String> {
    query(cfg, "games", "fields name; limit 1;")?;
    Ok("Connected".into())
}

pub fn cover_url(image_id: &str) -> String {
    format!("{IMG}/t_cover_big_2x/{image_id}.jpg")
}
pub fn hero_url(image_id: &str) -> String {
    format!("{IMG}/t_1080p/{image_id}.jpg")
}
pub fn thumb_url(image_id: &str, cover: bool) -> String {
    format!("{IMG}/{}/{image_id}.jpg", if cover { "t_cover_big" } else { "t_screenshot_med" })
}

fn names(v: &Value, k: &str) -> Vec<String> {
    v.get(k)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.get("name").and_then(Value::as_str)).map(str::to_string).collect())
        .unwrap_or_default()
}

fn images(v: &Value, k: &str) -> Vec<String> {
    v.get(k)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.get("image_id").and_then(Value::as_str)).map(str::to_string).collect())
        .unwrap_or_default()
}

/// Unix seconds to "YYYY-MM-DD" (UTC).
fn ymd(t: i64) -> String {
    let z = t.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{m:02}-{d:02}", yoe + era * 400 + i64::from(m <= 2))
}

/// Parses a /games response.
pub fn parse(v: &Value) -> Vec<IgdbGame> {
    let Some(list) = v.as_array() else { return Vec::new() };
    list.iter()
        .filter_map(|g| {
            let id = g.get("id")?.as_i64()?;
            let name = g.get("name")?.as_str()?.to_string();
            let date = g.get("first_release_date").and_then(Value::as_i64);
            let companies = |role: &str| {
                let v: Vec<String> = g
                    .get("involved_companies")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter(|c| c.get(role).and_then(Value::as_bool) == Some(true))
                    .filter_map(|c| c.pointer("/company/name").and_then(Value::as_str).map(str::to_string))
                    .collect();
                (!v.is_empty()).then(|| v.join(", "))
            };
            let join = |v: Vec<String>| (!v.is_empty()).then(|| v.join(", "));
            let text = |k: &str| g.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
            let score = |k: &str| g.get(k).and_then(Value::as_f64).map(|x| x.round().clamp(0.0, 100.0) as i64);
            let series = names(g, "collections")
                .into_iter()
                .next()
                .or_else(|| g.pointer("/franchise/name").and_then(Value::as_str).map(str::to_string))
                .or_else(|| names(g, "franchises").into_iter().next());
            let kind = g.pointer("/game_type/type").and_then(Value::as_str).unwrap_or("").to_lowercase();
            let main = !["dlc", "bundle", "mod", "episode", "season", "update", "pack"].iter().any(|k| kind.contains(k))
                && kind != "expansion";
            let mut platform_keys = Vec::new();
            for p in g.get("platforms").and_then(Value::as_array).into_iter().flatten() {
                for k in ["name", "abbreviation", "alternative_name"] {
                    for part in p.get(k).and_then(Value::as_str).unwrap_or("").split(['/', ',']) {
                        platform_keys.extend(platform_variants(part));
                    }
                }
            }
            Some(IgdbGame {
                id,
                name,
                year: date.map(|t| ymd(t)[..4].parse().unwrap_or(0)),
                platforms: names(g, "platforms"),
                platform_keys,
                alt_names: names(g, "alternative_names"),
                main,
                meta: Metadata {
                    description: text("summary").or_else(|| text("storyline")),
                    developer: companies("developer"),
                    publisher: companies("publisher"),
                    release_date: date.map(ymd),
                    genres: join(names(g, "genres")),
                    series,
                    critic_score: score("aggregated_rating"),
                    community_score: score("rating"),
                },
                cover: g.pointer("/cover/image_id").and_then(Value::as_str).map(str::to_string),
                artworks: images(g, "artworks"),
                screenshots: images(g, "screenshots"),
            })
        })
        .collect()
}

fn plain(s: &str) -> String {
    let s: String = s.to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { ' ' }).collect();
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A platform name, without its maker and with common aliases ("Sony PSP" -> "psp", "playstation portable").
fn platform_variants(p: &str) -> Vec<String> {
    const MAKERS: &[&str] = &["nintendo ", "sony ", "sega ", "microsoft ", "nec ", "atari ", "snk ", "bandai "];
    const ALIASES: &[(&str, &str)] = &[
        ("pc windows", "pc"),
        ("windows", "pc"),
        ("pc microsoft windows", "pc"),
        ("snes", "super nintendo entertainment system"),
        ("super famicom", "super nintendo entertainment system"),
        ("famicom", "nintendo entertainment system"),
        ("nes", "nintendo entertainment system"),
        ("psp", "playstation portable"),
        ("genesis", "mega drive"),
    ];
    let full = plain(p);
    if full.is_empty() {
        return Vec::new();
    }
    let mut v = vec![full.clone()];
    if let Some(b) = MAKERS.iter().find_map(|m| full.strip_prefix(m)) {
        v.push(b.to_string());
    }
    for x in v.clone() {
        if let Some((_, a)) = ALIASES.iter().find(|(k, _)| *k == x) {
            v.push(a.to_string());
        }
    }
    v
}

/// True when a game's platform (Playnite-style, may list several) is one of the IGDB entry's.
pub fn on_platform(g: &IgdbGame, platform: &str) -> bool {
    platform.split(',').flat_map(platform_variants).any(|k| g.platform_keys.contains(&k))
}

/// The one confident match: a main game whose title (or an alternative name) equals `name` after
/// normalizing, on the game's platform when it has one. Anything less is left to a manual pick.
pub fn best<'a>(name: &str, platform: Option<&str>, games: &'a [IgdbGame]) -> Option<&'a IgdbGame> {
    let want = crate::retro::title_key(name);
    if want.is_empty() {
        return None;
    }
    let platform = platform.map(str::trim).filter(|p| !p.is_empty());
    let ok = |g: &&IgdbGame| g.main && platform.is_none_or(|p| on_platform(g, p));
    let exact = games.iter().filter(ok).find(|g| crate::retro::title_key(&g.name) == want);
    exact.or_else(|| games.iter().filter(ok).find(|g| g.alt_names.iter().any(|a| crate::retro::title_key(a) == want)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Value {
        serde_json::json!([
            {"id": 7, "name": "Ben 10: Protector of Earth", "first_release_date": 1193702400,
             "platforms": [{"name": "PlayStation 2", "abbreviation": "PS2"}, {"name": "Wii", "abbreviation": "Wii"}],
             "genres": [{"name": "Adventure"}, {"name": "Platform"}],
             "involved_companies": [
                {"company": {"name": "High Voltage Software"}, "developer": true, "publisher": false},
                {"company": {"name": "D3 Publisher"}, "developer": false, "publisher": true}],
             "collections": [{"name": "Ben 10"}], "aggregated_rating": 61.4, "rating": 70.6,
             "summary": "Ben races across America.", "cover": {"image_id": "co1abc"},
             "artworks": [{"image_id": "ar1"}], "screenshots": [{"image_id": "sc1"}],
             "game_type": {"type": "Main Game"}},
            {"id": 8, "name": "Ben 10: Protector of Earth - Bonus Pack", "game_type": {"type": "DLC"},
             "platforms": [{"name": "PlayStation 2"}]},
            {"id": 9, "name": "Pokémon FireRed Version", "alternative_names": [{"name": "Pocket Monsters FireRed"}],
             "platforms": [{"name": "Game Boy Advance", "abbreviation": "GBA"}]}
        ])
    }

    #[test]
    fn parses_metadata() {
        let g = &parse(&sample())[0];
        assert_eq!(g.meta.release_date.as_deref(), Some("2007-10-30"));
        assert_eq!(g.year, Some(2007));
        assert_eq!(g.meta.developer.as_deref(), Some("High Voltage Software"));
        assert_eq!(g.meta.publisher.as_deref(), Some("D3 Publisher"));
        assert_eq!(g.meta.genres.as_deref(), Some("Adventure, Platform"));
        assert_eq!(g.meta.series.as_deref(), Some("Ben 10"));
        assert_eq!((g.meta.critic_score, g.meta.community_score), (Some(61), Some(71)));
        assert_eq!(g.cover.as_deref(), Some("co1abc"));
        assert!(g.main && !parse(&sample())[1].main);
    }

    #[test]
    fn matches_strictly() {
        let list = parse(&sample());
        let id = |n: &str, p: Option<&str>| best(n, p, &list).map(|g| g.id);
        assert_eq!(id("Ben 10: Protector of Earth", Some("Sony PlayStation 2")), Some(7));
        assert_eq!(id("Ben 10 - Protector of Earth", None), Some(7));
        assert_eq!(id("Ben 10: Protector of Earth", Some("Nintendo Switch")), None);
        assert_eq!(id("Ben 10", Some("Sony PlayStation 2")), None);
        assert_eq!(id("Pokémon FireRed", Some("Nintendo Game Boy Advance")), Some(9));
        assert_eq!(id("Pocket Monsters FireRed", Some("Nintendo Game Boy Advance")), Some(9));
        assert!(platform_variants("PC (Windows)").contains(&"pc".to_string()));
        assert!(platform_variants("Sony PSP").contains(&"playstation portable".to_string()));
    }
}
