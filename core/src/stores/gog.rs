//! GOG: installed games from the registry (+ each game's goggame-<id>.info), owned games from GOG Galaxy's database.
use super::StoreGame;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub fn scan() -> Result<Vec<StoreGame>> {
    let mut games: HashMap<String, StoreGame> = HashMap::new();
    match owned() {
        Ok(v) => games.extend(v.into_iter().map(|(id, name)| (id.clone(), StoreGame { id, name, ..Default::default() }))),
        Err(e) => log::info!("GOG Galaxy library not read: {e:#}"),
    }
    for g in installed() {
        let e = games.entry(g.id.clone()).or_default();
        let name = if e.name.is_empty() { g.name.clone() } else { e.name.clone() };
        *e = StoreGame { name, ..g };
    }
    let mut v: Vec<StoreGame> = games.into_values().collect();
    v.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(v)
}

/// Galaxy's database (C:\ProgramData\GOG.com\Galaxy\storage\galaxy-2.0.db).
fn galaxy_db() -> PathBuf {
    let pd = std::env::var_os("ProgramData").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
    pd.join(r"GOG.com\Galaxy\storage\galaxy-2.0.db")
}

/// (id, title) of owned games: no DLCs, packs or duplicates Galaxy hides from the library.
fn owned() -> Result<Vec<(String, String)>> {
    let db = galaxy_db();
    if !db.exists() {
        anyhow::bail!("GOG Galaxy isn't installed");
    }
    // Galaxy keeps it open (WAL): read a copy, with its -wal and -shm, never the live file.
    let tmp = std::env::temp_dir().join(format!("gamut-gog-{}", std::process::id()));
    std::fs::create_dir_all(&tmp)?;
    let copy = tmp.join("galaxy.db");
    let r = (|| {
        std::fs::copy(&db, &copy).context("copying Galaxy's database")?;
        for ext in ["-wal", "-shm"] {
            let src = PathBuf::from(format!("{}{ext}", db.display()));
            if src.exists() {
                std::fs::copy(&src, format!("{}{ext}", copy.display()))?;
            }
        }
        let c = rusqlite::Connection::open(&copy)?;
        let mut st = c.prepare(
            "SELECT DISTINCT substr(l.releaseKey, 5), json_extract(p.value, '$.title')
             FROM LibraryReleases l
             JOIN ReleaseProperties rp ON rp.releaseKey = l.releaseKey
             JOIN GamePieces p ON p.releaseKey = l.releaseKey
             JOIN GamePieceTypes t ON t.id = p.gamePieceTypeId AND t.type = 'title'
             WHERE l.releaseKey LIKE 'gog\\_%' ESCAPE '\\' AND rp.isDlc = 0 AND rp.isVisibleInLibrary = 1",
        )?;
        let v = st
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))?
            .filter_map(|r| r.ok())
            .filter_map(|(id, t)| Some((id, t.filter(|t| !t.trim().is_empty())?)))
            .collect::<Vec<_>>();
        anyhow::Ok(v)
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    r
}

/// Installed games GOG's installers registered (Galaxy or offline installers); DLCs skipped.
#[cfg(windows)]
fn installed() -> Vec<StoreGame> {
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY};
    let Ok(root) = winreg::RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(r"SOFTWARE\GOG.com\Games", KEY_READ | KEY_WOW64_32KEY) else {
        return Vec::new();
    };
    let mut v = Vec::new();
    for sub in root.enum_keys().flatten() {
        let Ok(k) = root.open_subkey(&sub) else { continue };
        let get = |n: &str| k.get_value::<String, _>(n).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        if get("dependsOn").is_some() {
            continue;
        }
        let (Some(id), Some(dir)) = (get("gameID").or(Some(sub.clone())), get("path")) else { continue };
        let dir = PathBuf::from(dir.trim_matches('"'));
        if !dir.is_dir() {
            continue;
        }
        let (exe, args, wd) = match play_task(&dir, &id) {
            Some(t) => (Some(t.0), t.1, t.2),
            None => (get("exe").filter(|e| Path::new(e).is_file()), get("launchParam"), get("workingDir")),
        };
        let name = get("gameName").unwrap_or_else(|| id.clone());
        v.push(StoreGame { id, name, installed: exe.is_some(), install_dir: Some(dir.display().to_string()), exe, args, work_dir: wd });
    }
    v
}

#[cfg(not(windows))]
fn installed() -> Vec<StoreGame> {
    Vec::new()
}

/// The primary play task from goggame-<id>.info: (exe, arguments, working folder), all absolute.
fn play_task(dir: &Path, id: &str) -> Option<(String, Option<String>, Option<String>)> {
    let text = std::fs::read_to_string(dir.join(format!("goggame-{id}.info"))).ok()?;
    parse_task(dir, &text)
}

fn parse_task(dir: &Path, info: &str) -> Option<(String, Option<String>, Option<String>)> {
    let v: serde_json::Value = serde_json::from_str(info).ok()?;
    let tasks = v["playTasks"].as_array()?;
    let file = |t: &&serde_json::Value| t["type"] == "FileTask" && t["path"].as_str().is_some_and(|p| !p.is_empty());
    let t = tasks.iter().filter(file).find(|t| t["isPrimary"] == true).or_else(|| tasks.iter().find(file))?;
    let exe = dir.join(t["path"].as_str()?);
    let args = t["arguments"].as_str().map(str::trim).filter(|a| !a.is_empty()).map(String::from);
    let wd = t["workingDir"].as_str().filter(|w| !w.is_empty()).map(|w| dir.join(w).display().to_string());
    Some((exe.display().to_string(), args, wd))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_task() {
        let info = r#"{"playTasks":[
            {"category":"game","isHidden":true,"path":"bin\\game.exe","type":"FileTask"},
            {"category":"launcher","isPrimary":true,"path":"launcher.exe","arguments":" -x ","workingDir":"bin","type":"FileTask"},
            {"isPrimary":true,"link":"https://x","type":"URLTask"}]}"#;
        let (exe, args, wd) = parse_task(Path::new(r"D:\G"), info).unwrap();
        assert_eq!((exe.as_str(), args.as_deref(), wd.as_deref()), (r"D:\G\launcher.exe", Some("-x"), Some(r"D:\G\bin")));
        assert!(parse_task(Path::new(r"D:\G"), r#"{"playTasks":[]}"#).is_none());
    }

    /// Prints what this PC's GOG has: `cargo test -p gamut-core -- --ignored gog_local --nocapture`
    #[test]
    #[ignore]
    fn gog_local() {
        for g in scan().unwrap() {
            println!("{g:?}");
        }
    }
}
