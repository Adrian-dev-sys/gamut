use crate::db::SteamEntry;
use crate::vdf;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Steam client install folder, from the registry with a default fallback.
pub fn steam_path() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        use winreg::enums::HKEY_CURRENT_USER;
        if let Ok(key) = winreg::RegKey::predef(HKEY_CURRENT_USER).open_subkey(r"Software\Valve\Steam") {
            if let Ok(p) = key.get_value::<String, _>("SteamPath") {
                let p = PathBuf::from(p.replace('/', "\\"));
                if p.exists() {
                    return Some(p);
                }
            }
        }
    }
    let p = PathBuf::from(r"C:\Program Files (x86)\Steam");
    p.exists().then_some(p)
}

/// All Steam library folders (each holds a `steamapps` dir).
pub fn libraries(steam: &Path) -> Vec<PathBuf> {
    let mut out = vec![steam.to_path_buf()];
    if let Ok(src) = std::fs::read_to_string(steam.join(r"steamapps\libraryfolders.vdf")) {
        let doc = vdf::parse(&src);
        if let Some(lf) = doc.get("libraryfolders") {
            for (_, lib) in lf.entries() {
                if let Some(p) = lib.str("path") {
                    let p = PathBuf::from(p);
                    if !out.iter().any(|o| o.as_os_str().eq_ignore_ascii_case(p.as_os_str())) {
                        out.push(p);
                    }
                }
            }
        }
    }
    out
}

/// Non-game apps that show up in manifests.
fn is_tool(appid: u32, name: &str) -> bool {
    const IDS: &[u32] = &[228980, 1070560, 1391110, 1628350, 1493710, 2180100];
    IDS.contains(&appid)
        || name.starts_with("Proton")
        || name.starts_with("Steam Linux Runtime")
        || name.starts_with("Steamworks Common")
}

/// Installed games from `appmanifest_*.acf` files across all libraries.
pub fn installed(steam: &Path) -> Vec<SteamEntry> {
    let mut out = Vec::new();
    for lib in libraries(steam) {
        let apps = lib.join("steamapps");
        let Ok(rd) = std::fs::read_dir(&apps) else { continue };
        for f in rd.flatten() {
            let fname = f.file_name().to_string_lossy().to_string();
            if !(fname.starts_with("appmanifest_") && fname.ends_with(".acf")) {
                continue;
            }
            let Ok(src) = std::fs::read_to_string(f.path()) else { continue };
            let doc = vdf::parse(&src);
            let Some(app) = doc.get("AppState") else { continue };
            let Some(appid) = app.str("appid").and_then(|s| s.parse::<u32>().ok()) else { continue };
            let name = app.str("name").unwrap_or_default().to_string();
            if name.is_empty() || is_tool(appid, &name) {
                continue;
            }
            let flags: u32 = app.str("StateFlags").and_then(|s| s.parse().ok()).unwrap_or(0);
            let dir = app.str("installdir").map(|d| apps.join("common").join(d).to_string_lossy().to_string());
            out.push(SteamEntry {
                appid,
                name,
                installed: flags & 4 != 0,
                install_dir: dir,
                playtime_min: None,
                last_played: app.str("LastPlayed").and_then(|s| s.parse().ok()).unwrap_or(0),
            });
        }
    }
    out
}

/// SteamID64 of the most recently logged-in account.
pub fn detect_steam_id(steam: &Path) -> Option<String> {
    let src = std::fs::read_to_string(steam.join(r"config\loginusers.vdf")).ok()?;
    let doc = vdf::parse(&src);
    let users = doc.get("users")?.entries();
    users
        .iter()
        .find(|(_, u)| u.str("MostRecent") == Some("1"))
        .or_else(|| users.iter().max_by_key(|(_, u)| u.str("Timestamp").and_then(|t| t.parse::<i64>().ok())))
        .map(|(id, _)| id.clone())
}

#[derive(Deserialize)]
struct OwnedResp {
    response: OwnedInner,
}
#[derive(Deserialize)]
struct OwnedInner {
    #[serde(default)]
    games: Vec<OwnedGame>,
}
#[derive(Deserialize)]
struct OwnedGame {
    appid: u32,
    #[serde(default)]
    name: String,
    #[serde(default)]
    playtime_forever: i64,
    #[serde(default)]
    rtime_last_played: i64,
}

/// Owned games via the Steam Web API (needs a key; profile game details must be visible to the key owner).
pub fn owned(api_key: &str, steam_id: &str) -> Result<Vec<SteamEntry>> {
    let url = format!(
        "https://api.steampowered.com/IPlayerService/GetOwnedGames/v1/?key={}&steamid={}&include_appinfo=1&include_played_free_games=1&format=json",
        crate::http::encode(api_key),
        crate::http::encode(steam_id)
    );
    let r: OwnedResp = crate::http::get_json(&url).context("GetOwnedGames failed (check API key / SteamID)")?;
    Ok(r.response
        .games
        .into_iter()
        .filter(|g| !g.name.is_empty() && !is_tool(g.appid, &g.name))
        .map(|g| SteamEntry {
            appid: g.appid,
            name: g.name,
            installed: false,
            install_dir: None,
            playtime_min: Some(g.playtime_forever),
            last_played: g.rtime_last_played,
        })
        .collect())
}

/// Full scan: owned library (when configured) merged with installed manifests.
pub fn scan(cfg: &crate::Config) -> Result<Vec<SteamEntry>> {
    let steam = steam_path();
    let local = steam.as_deref().map(installed).unwrap_or_default();
    let steam_id = if cfg.steam_id.trim().is_empty() {
        steam.as_deref().and_then(detect_steam_id).unwrap_or_default()
    } else {
        cfg.steam_id.trim().to_string()
    };

    let mut map: HashMap<u32, SteamEntry> = HashMap::new();
    if !cfg.steam_api_key.trim().is_empty() && !steam_id.is_empty() {
        match owned(cfg.steam_api_key.trim(), &steam_id) {
            Ok(list) => map.extend(list.into_iter().map(|e| (e.appid, e))),
            Err(e) => log::warn!("owned games: {e:#}"),
        }
    }
    for l in local {
        match map.get_mut(&l.appid) {
            Some(o) => {
                o.installed = l.installed;
                o.install_dir = l.install_dir;
                o.last_played = o.last_played.max(l.last_played);
            }
            None => {
                map.insert(l.appid, l);
            }
        }
    }
    Ok(map.into_values().collect())
}

/// Locally cached library art (Steam client cache), if present.
pub fn local_art(appid: u32, file: &str) -> Option<PathBuf> {
    let cache = steam_path()?.join(r"appcache\librarycache");
    [cache.join(appid.to_string()).join(file), cache.join(format!("{appid}_{file}"))]
        .into_iter()
        .find(|p| p.exists())
}
