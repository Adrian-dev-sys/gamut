//! Achievements: shared types and the Steam Web API source. Cached in the library DB.
use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Achievement {
    /// Provider's stable id (Steam apiname).
    pub key: String,
    pub name: String,
    pub description: String,
    pub icon: String,
    pub icon_locked: String,
    pub hidden: bool,
    /// Unlock time (unix seconds); 0 = locked.
    pub unlocked: i64,
    /// Share of players who have it, 0–100.
    pub percent: Option<f32>,
    pub points: i64,
}

/// Cached fetch state for one game.
#[derive(Debug, Clone, Default)]
pub struct State {
    pub fetched: i64,
    /// When names, icons and rarity were last refreshed.
    pub schema_at: i64,
    pub total: i64,
    pub unlocked: i64,
    /// Why the last fetch got nothing (e.g. private profile); None when it worked.
    pub error: Option<String>,
}

/// What a source returned for one game.
pub enum Fetched {
    /// The game has no achievements.
    None,
    /// Full list (names, icons, rarity) or, when `full` is false, only `key` + `unlocked`.
    List { items: Vec<Achievement>, full: bool },
}

pub const PRIVATE: &str = "private";

#[derive(Deserialize)]
struct PlayerResp {
    playerstats: PlayerStats,
}
#[derive(Deserialize)]
struct PlayerStats {
    #[serde(default)]
    achievements: Vec<PlayerAch>,
}
#[derive(Deserialize)]
struct PlayerAch {
    apiname: String,
    #[serde(default)]
    achieved: i64,
    #[serde(default)]
    unlocktime: i64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
}

#[derive(Deserialize)]
struct SchemaResp {
    game: SchemaGame,
}
#[derive(Deserialize)]
struct SchemaGame {
    #[serde(default, rename = "availableGameStats")]
    stats: Option<SchemaStats>,
}
#[derive(Deserialize)]
struct SchemaStats {
    #[serde(default)]
    achievements: Vec<SchemaAch>,
}
#[derive(Deserialize)]
struct SchemaAch {
    name: String,
    #[serde(default, rename = "displayName")]
    display: String,
    #[serde(default)]
    hidden: i64,
    #[serde(default)]
    description: String,
    #[serde(default)]
    icon: String,
    #[serde(default)]
    icongray: String,
}

fn status(e: &anyhow::Error) -> Option<u16> {
    match e.downcast_ref::<ureq::Error>() {
        Some(ureq::Error::StatusCode(c)) => Some(*c),
        _ => None,
    }
}

/// The player's unlocks for `appid`; with `full`, also names, icons and global rarity.
/// A private profile is an error whose text is [`PRIVATE`]. Errors never contain the key.
pub fn steam(key: &str, steam_id: &str, appid: u32, full: bool) -> Result<Fetched> {
    let (k, sid) = (crate::http::encode(key), crate::http::encode(steam_id));
    let url = format!(
        "https://api.steampowered.com/ISteamUserStats/GetPlayerAchievements/v1/?key={k}&steamid={sid}&appid={appid}&l=english"
    );
    let player: PlayerResp = match crate::http::get_json(&url) {
        Ok(p) => p,
        // 400 = "Requested app has no stats".
        Err(e) if status(&e) == Some(400) => return Ok(Fetched::None),
        Err(e) if matches!(status(&e), Some(401 | 403)) => anyhow::bail!(PRIVATE),
        Err(e) => anyhow::bail!("Steam achievements request failed ({})", status(&e).map_or("network".into(), |c| c.to_string())),
    };
    if player.playerstats.achievements.is_empty() {
        return Ok(Fetched::None);
    }
    let mut items: Vec<Achievement> = player
        .playerstats
        .achievements
        .into_iter()
        .map(|a| Achievement {
            key: a.apiname,
            name: a.name,
            description: a.description,
            unlocked: if a.achieved != 0 { a.unlocktime.max(1) } else { 0 },
            ..Default::default()
        })
        .collect();
    if !full {
        return Ok(Fetched::List { items, full: false });
    }
    let (schema, pct) = schema(&k, appid)?;
    merge_schema(&mut items, schema, &pct);
    Ok(Fetched::List { items, full: true })
}

/// Names, icons and rarity; `k` is the URL-encoded key.
fn schema(k: &str, appid: u32) -> Result<(Vec<SchemaAch>, HashMap<String, f32>)> {
    let url = format!("https://api.steampowered.com/ISteamUserStats/GetSchemaForGame/v2/?key={k}&appid={appid}&l=english");
    let schema: SchemaResp = crate::http::get_json(&url).ok().context("Steam achievement names request failed")?;
    let url = format!("https://api.steampowered.com/ISteamUserStats/GetGlobalAchievementPercentagesForApp/v2/?gameid={appid}");
    let pct = crate::http::get_json::<Value>(&url).map(|v| percentages(&v)).unwrap_or_default();
    Ok((schema.game.stats.map(|s| s.achievements).unwrap_or_default(), pct))
}

/// Every achievement of `appid` from the schema, with unlock times from `unlocks` (keys matched case-insensitively).
/// Doesn't need the account to own the game.
pub fn steam_with_unlocks(key: &str, appid: u32, unlocks: &HashMap<String, i64>) -> Result<Fetched> {
    let (schema, pct) = schema(&crate::http::encode(key), appid)?;
    if schema.is_empty() {
        return Ok(Fetched::None);
    }
    let lower: HashMap<String, i64> = unlocks.iter().map(|(k, v)| (k.to_lowercase(), *v)).collect();
    let mut items: Vec<Achievement> = schema
        .iter()
        .map(|s| Achievement { key: s.name.clone(), unlocked: lower.get(&s.name.to_lowercase()).copied().unwrap_or(0), ..Default::default() })
        .collect();
    merge_schema(&mut items, schema, &pct);
    Ok(Fetched::List { items, full: true })
}

/// `percent` arrives as a string ("96.8") or a number depending on the day.
fn percentages(v: &Value) -> HashMap<String, f32> {
    let list = v.pointer("/achievementpercentages/achievements").and_then(Value::as_array);
    list.into_iter()
        .flatten()
        .filter_map(|a| {
            let p = a.get("percent")?;
            let p = p.as_f64().or_else(|| p.as_str()?.parse().ok())?;
            Some((a.get("name")?.as_str()?.to_string(), p as f32))
        })
        .collect()
}

/// Fills icons, hidden flags and rarity from the schema, in the schema's order.
fn merge_schema(items: &mut [Achievement], schema: Vec<SchemaAch>, pct: &HashMap<String, f32>) {
    let order: HashMap<&str, usize> = schema.iter().enumerate().map(|(i, s)| (s.name.as_str(), i)).collect();
    for a in items.iter_mut() {
        if let Some(s) = order.get(a.key.as_str()).map(|&i| &schema[i]) {
            if a.name.is_empty() {
                a.name = s.display.clone();
            }
            if a.description.is_empty() {
                a.description = s.description.clone();
            }
            a.hidden = s.hidden != 0;
            a.icon = s.icon.clone();
            a.icon_locked = s.icongray.clone();
        }
        a.percent = pct.get(&a.key).copied();
    }
    items.sort_by_key(|a| order.get(a.key.as_str()).copied().unwrap_or(usize::MAX));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_player_schema_and_string_percentages() {
        let player: PlayerResp = serde_json::from_str(
            r#"{"playerstats":{"steamID":"1","gameName":"G","achievements":[
                {"apiname":"B","achieved":1,"unlocktime":1500,"name":"Bee","description":""},
                {"apiname":"A","achieved":0,"unlocktime":0,"name":"Ay","description":"do a"}]}}"#,
        )
        .unwrap();
        let schema: SchemaResp = serde_json::from_str(
            r#"{"game":{"gameName":"G","availableGameStats":{"achievements":[
                {"name":"A","displayName":"Ay","hidden":0,"description":"do a","icon":"a.jpg","icongray":"a_bw.jpg"},
                {"name":"B","displayName":"Bee","hidden":1,"icon":"b.jpg","icongray":"b_bw.jpg"}]}}}"#,
        )
        .unwrap();
        let pct = percentages(&serde_json::json!({"achievementpercentages":{"achievements":[
            {"name":"A","percent":"12.5"},{"name":"B","percent":3.25}]}}));
        let mut items: Vec<Achievement> = player
            .playerstats
            .achievements
            .into_iter()
            .map(|a| Achievement { key: a.apiname, name: a.name, unlocked: a.unlocktime, ..Default::default() })
            .collect();
        merge_schema(&mut items, schema.game.stats.unwrap().achievements, &pct);
        assert_eq!(items[0].key, "A", "schema order wins");
        assert_eq!(items[0].description, "do a");
        assert_eq!(items[0].percent, Some(12.5));
        assert!(items[1].hidden && items[1].unlocked == 1500 && items[1].icon_locked == "b_bw.jpg");
        assert_eq!(items[1].percent, Some(3.25));
        // A game without stats returns an empty schema object.
        let empty: SchemaResp = serde_json::from_str(r#"{"game":{}}"#).unwrap();
        assert!(empty.game.stats.is_none());
    }
}
