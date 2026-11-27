//! Achievements Steam emulators (GSE/Goldberg, CODEX, RUNE, OnlineFix) keep on disk, for PC games outside Steam.
//! Unlocks come from their files; names, icons and rarity from Steam's schema.
use crate::achievements::Fetched;
use crate::model::{Game, Source};
use anyhow::Result;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

/// Folders with one subfolder per appid, and the unlock file inside it.
fn roots() -> Vec<(PathBuf, &'static str)> {
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    let mut v = Vec::new();
    if let Some(a) = env("APPDATA") {
        v.push((a.join("GSE Saves"), "achievements.json"));
        v.push((a.join("Goldberg SteamEmu Saves"), "achievements.json"));
        v.push((a.join("Steam").join("CODEX"), "achievements.ini"));
    }
    if let Some(p) = env("PUBLIC").map(|p| p.join("Documents")) {
        v.push((p.join("Steam").join("CODEX"), "achievements.ini"));
        v.push((p.join("Steam").join("RUNE"), "achievements.ini"));
        v.push((p.join("OnlineFix"), r"Stats\Achievements.ini"));
    }
    v
}

/// Appids that have an unlock file somewhere.
pub fn appids() -> Vec<u32> {
    let mut v: Vec<u32> = roots()
        .into_iter()
        .flat_map(|(root, file)| {
            std::fs::read_dir(&root).into_iter().flatten().flatten().filter_map(move |e| {
                let id: u32 = e.file_name().to_str()?.parse().ok()?;
                e.path().join(file).is_file().then_some(id)
            })
        })
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Unlock times by achievement key, merged over every emulator's file; None when there's no file.
pub fn unlocks(appid: u32) -> Option<HashMap<String, i64>> {
    let mut out: Option<HashMap<String, i64>> = None;
    for (root, file) in roots() {
        let Ok(text) = std::fs::read_to_string(root.join(appid.to_string()).join(file)) else { continue };
        let found = if file.ends_with(".json") { parse_json(&text) } else { parse_ini(&text) };
        let m = out.get_or_insert_with(HashMap::new);
        for (k, t) in found {
            let e = m.entry(k).or_insert(t);
            *e = (*e).min(t);
        }
    }
    out
}

/// Latest change time of `appid`'s unlock files, to notice new unlocks while a game runs.
pub fn modified(appid: u32) -> Option<std::time::SystemTime> {
    roots().into_iter().filter_map(|(r, f)| std::fs::metadata(r.join(appid.to_string()).join(f)).ok()?.modified().ok()).max()
}

/// GSE/Goldberg: `{"KEY": {"earned": true, "earned_time": 1764447268}}`.
fn parse_json(text: &str) -> HashMap<String, i64> {
    let Ok(serde_json::Value::Object(m)) = serde_json::from_str(text) else { return HashMap::new() };
    m.into_iter()
        .filter(|(_, v)| v.get("earned").and_then(|e| e.as_bool()) == Some(true))
        .map(|(k, v)| (k, v.get("earned_time").and_then(|t| t.as_i64()).unwrap_or(0).max(1)))
        .collect()
}

/// CODEX/RUNE `[KEY] Achieved=1 UnlockTime=…`, OnlineFix `[KEY] achieved=true timestamp=…`.
fn parse_ini(text: &str) -> HashMap<String, i64> {
    let mut out = HashMap::new();
    let mut cur: Option<(String, bool, i64)> = None;
    let mut flush = |c: Option<(String, bool, i64)>| {
        if let Some((k, true, t)) = c {
            out.insert(k, t.max(1));
        }
    };
    for line in text.lines().map(str::trim) {
        if let Some(sec) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            flush(cur.take());
            cur = (!sec.eq_ignore_ascii_case("SteamAchievements")).then(|| (sec.to_string(), false, 0));
        } else if let (Some(c), Some((k, v))) = (cur.as_mut(), line.split_once('=')) {
            let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
            match k.as_str() {
                "achieved" => c.1 = v == "1" || v.eq_ignore_ascii_case("true"),
                "unlocktime" | "timestamp" => c.2 = v.parse().unwrap_or(0),
                _ => {}
            }
        }
    }
    flush(cur);
    out
}

/// Store names of the appids, cached in `cache` (a JSON file); "" = no store page.
pub fn names(cache: &Path, ids: &[u32]) -> HashMap<u32, String> {
    let mut m: BTreeMap<u32, String> = std::fs::read_to_string(cache).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let mut changed = false;
    let missing: Vec<u32> = ids.iter().copied().filter(|id| !m.contains_key(id)).collect();
    for id in missing {
        match crate::scrape::steam_details(id) {
            Ok(d) => {
                m.insert(id, d.map(|d| d.name).unwrap_or_default());
                changed = true;
            }
            Err(e) => log::warn!("local achievements: no name for app {id}: {e:#}"),
        }
    }
    if changed {
        if let Ok(s) = serde_json::to_string_pretty(&m) {
            let _ = std::fs::write(cache, s);
        }
    }
    ids.iter().filter_map(|id| Some((*id, m.get(id).filter(|n| !n.is_empty())?.clone()))).collect()
}

/// The appid whose files hold `g`'s achievements: the matched Steam id, a `steam_appid.txt` in the game's folder,
/// else a store name match. Only custom games on PC (or no platform).
pub fn appid_for(g: &Game, names: &HashMap<u32, String>) -> Option<u32> {
    let pc = g.platform.as_deref().is_none_or(|p| p.trim().to_ascii_lowercase().starts_with("pc"));
    if g.source != Source::Custom || !pc {
        return None;
    }
    let txt = g.folder().into_iter().flat_map(|d| [d.join("steam_appid.txt"), d.join("steam_settings").join("steam_appid.txt")]);
    let listed = g.steam_appid().into_iter().chain(txt.filter_map(|f| std::fs::read_to_string(f).ok()?.trim().parse().ok()));
    if let Some(id) = listed.into_iter().find(|id| names.contains_key(id)) {
        return Some(id);
    }
    let variants = crate::scrape::name_variants(&g.name);
    names.iter().filter(|(_, n)| variants.iter().any(|v| crate::scrape::same_game(v, n))).map(|(id, _)| *id).min()
}

/// The game's list from Steam's schema with the local unlocks.
pub fn fetch(key: &str, appid: u32) -> Result<Fetched> {
    if key.trim().is_empty() {
        anyhow::bail!("add a Steam Web API key in Settings for this game's achievement names");
    }
    crate::achievements::steam_with_unlocks(key, appid, &unlocks(appid).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_emulator_files() {
        let j = parse_json(r#"{"0001": {"earned": true, "earned_time": 1764447268}, "0002": {"earned": false, "earned_time": 0}}"#);
        assert_eq!(j, HashMap::from([("0001".to_string(), 1764447268)]));
        let i = parse_ini("[SteamAchievements]\nCount=2\n00000=A\n\n[14]\nAchieved=1\nUnlockTime=1761776080\n\n[15]\nAchieved=0\n[X]\nachieved=true\ntimestamp=5\n");
        assert_eq!(i, HashMap::from([("14".to_string(), 1761776080), ("X".to_string(), 5)]));
    }
}
