//! Steam library collections (userdata\<account>\config\cloudstorage\cloud-storage-namespace-1.json),
//! mirrored into Gamut collections: Steam stays the master list for Steam games.
use crate::db::Library;
use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SteamCollection {
    /// Steam's id ("favorite", "uc-…").
    pub key: String,
    pub name: String,
    pub added: Vec<u32>,
    pub removed: Vec<u32>,
    /// Dynamic filters Gamut understands: installed, VR supported.
    pub installed: bool,
    pub vr: bool,
    /// Has other dynamic filters (only `added` is used then).
    pub unsupported: bool,
}

/// userdata folder of the account (SteamID64), or the most recently changed one.
fn account_dir(steam: &Path, steam_id64: &str) -> Option<PathBuf> {
    let root = steam.join("userdata");
    if let Ok(id) = steam_id64.trim().parse::<u64>() {
        let d = root.join(id.saturating_sub(76561197960265728).to_string());
        if d.is_dir() {
            return Some(d);
        }
    }
    std::fs::read_dir(&root)
        .ok()?
        .flatten()
        .filter(|e| e.path().join(r"config\cloudstorage").is_dir())
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
        .map(|e| e.path())
}

/// Collections synced to Gamut: Favorites (as "To play") and the user's own; not Hidden or tool-made ones.
pub fn read(steam: &Path, steam_id64: &str) -> Result<Vec<SteamCollection>> {
    let dir = account_dir(steam, steam_id64).context("no Steam user folder")?;
    let src = std::fs::read_to_string(dir.join(r"config\cloudstorage\cloud-storage-namespace-1.json"))?;
    parse(&src)
}

fn parse(src: &str) -> Result<Vec<SteamCollection>> {
    let doc: Vec<(String, serde_json::Value)> = serde_json::from_str(src)?;
    let ids = |v: &serde_json::Value| -> Vec<u32> { v.as_array().map(|a| a.iter().filter_map(|x| x.as_u64()).map(|x| x as u32).collect()).unwrap_or_default() };
    let mut out = Vec::new();
    for (k, entry) in doc {
        let Some(key) = k.strip_prefix("user-collections.") else { continue };
        if entry["is_deleted"].as_bool() == Some(true) || !(key == "favorite" || key.starts_with("uc-")) {
            continue;
        }
        let Some(v) = entry["value"].as_str().and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok()) else { continue };
        let name = match v["name"].as_str() {
            _ if key == "favorite" => "To play".to_string(),
            // Steam shows names in capitals; "finished" reads better as "Finished".
            Some(n) => n.chars().take(1).flat_map(char::to_uppercase).chain(n.chars().skip(1)).collect(),
            None => key.to_string(),
        };
        let mut c = SteamCollection { key: key.to_string(), name, added: ids(&v["added"]), removed: ids(&v["removed"]), ..Default::default() };
        if let Some(spec) = v.get("filterSpec").filter(|s| !s.is_null()) {
            // Group 1 is play state (1 = installed), group 2 features (3 = VR supported).
            for (i, g) in spec["filterGroups"].as_array().into_iter().flatten().enumerate() {
                for o in g["rgOptions"].as_array().into_iter().flatten().filter_map(|o| o.as_u64()) {
                    match (i, o) {
                        (1, 1) => c.installed = true,
                        (2, 3) => c.vr = true,
                        _ => c.unsupported = true,
                    }
                }
            }
            if spec["strSearchText"].as_str().is_some_and(|s| !s.is_empty()) {
                c.unsupported = true;
            }
        }
        out.push(c);
    }
    Ok(out)
}

/// Apps among `want` whose Steam app info says they support VR (appcache\appinfo.vdf, format 28/29).
pub fn vr_apps(steam: &Path, want: &HashSet<u32>) -> Result<HashSet<u32>> {
    let b = std::fs::read(steam.join(r"appcache\appinfo.vdf"))?;
    let u32_at = |o: usize| b.get(o..o + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap()));
    let magic = u32_at(0).context("empty appinfo")?;
    let strings: Option<Vec<&[u8]>> = match magic {
        0x07564429 => {
            let off = i64::from_le_bytes(b.get(8..16).context("short")?.try_into()?) as usize;
            let n = u32_at(off).context("bad string table")? as usize;
            Some(b.get(off + 4..).context("bad string table")?.split(|&c| c == 0).take(n).collect())
        }
        0x07564428 => None,
        m => anyhow::bail!("unknown appinfo format {m:#x}"),
    };
    let mut o = if strings.is_some() { 16 } else { 8 };
    let mut out = HashSet::new();
    while let Some(appid) = u32_at(o).filter(|&a| a != 0) {
        let size = u32_at(o + 4).context("short")? as usize;
        let next = o + 8 + size;
        if want.contains(&appid) {
            let kv = o + 8 + 4 + 4 + 8 + 20 + 4 + 20;
            let mut p = Kv { b: &b, o: kv, strings: strings.as_deref(), path: Vec::new(), vr: false };
            if p.section().is_some() && p.vr {
                out.insert(appid);
            }
        }
        o = next;
    }
    Ok(out)
}

/// Binary KeyValues walker that only notes VR support under `common`.
struct Kv<'a> {
    b: &'a [u8],
    o: usize,
    strings: Option<&'a [&'a [u8]]>,
    path: Vec<Vec<u8>>,
    vr: bool,
}

impl Kv<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let s = self.b.get(self.o..self.o + n)?;
        self.o += n;
        Some(s)
    }
    fn cstr(&mut self) -> Option<Vec<u8>> {
        let end = self.b.get(self.o..)?.iter().position(|&c| c == 0)?;
        let s = self.b[self.o..self.o + end].to_vec();
        self.o += end + 1;
        Some(s)
    }
    fn key(&mut self) -> Option<Vec<u8>> {
        match self.strings {
            Some(t) => {
                let i = i32::from_le_bytes(self.take(4)?.try_into().ok()?);
                t.get(i as usize).map(|s| s.to_vec())
            }
            None => self.cstr(),
        }
    }
    fn section(&mut self) -> Option<()> {
        loop {
            let t = *self.take(1)?.first()?;
            if t == 8 {
                return Some(());
            }
            let k = self.key()?;
            let in_common = self.path.last().is_some_and(|p| p == b"common");
            let vr_key = in_common && matches!(&k[..], b"openvrsupport" | b"onlyvrsupport" | b"othervrsupport");
            match t {
                0 => {
                    self.path.push(k);
                    self.section()?;
                    self.path.pop();
                }
                1 => {
                    let v = self.cstr()?;
                    self.vr |= vr_key && v == b"1";
                }
                2 => {
                    let v = i32::from_le_bytes(self.take(4)?.try_into().ok()?);
                    self.vr |= vr_key && v == 1;
                }
                7 => {
                    self.take(8)?;
                }
                _ => return None,
            }
        }
    }
}

/// What a sync did: collections synced, and how many of those were created, changed or removed.
#[derive(Debug, Default, PartialEq)]
pub struct Synced {
    pub synced: usize,
    pub changed: usize,
    /// Games given the Beaten status from Steam's Finished collection.
    pub beaten: usize,
}

/// Steam collections that mean "beaten" (the user's own naming).
fn is_finished(name: &str) -> bool {
    matches!(name.trim().to_lowercase().as_str(), "finished" | "beaten" | "completed" | "done")
}

/// Mirrors Steam's collections into Gamut.
/// `ignored`: Steam collection ids deleted in Gamut (not recreated).
pub fn sync(lib: &mut Library, steam: &Path, steam_id64: &str, ignored: &[String]) -> Result<Synced> {
    let colls = read(steam, steam_id64)?;
    apply(lib, &colls, ignored, |apps| {
        vr_apps(steam, apps).unwrap_or_else(|e| {
            log::warn!("Steam app info (VR flags): {e:#}");
            HashSet::new()
        })
    })
}

/// `sync` with the Steam side given: the collections and a VR lookup for owned apps.
fn apply(lib: &mut Library, colls: &[SteamCollection], ignored: &[String], vr_of: impl FnOnce(&HashSet<u32>) -> HashSet<u32>) -> Result<Synced> {
    let games = lib.steam_games()?;
    let by_app: HashMap<u32, (i64, bool)> = games.iter().map(|&(id, app, inst)| (app, (id, inst))).collect();
    let vr = if colls.iter().any(|c| c.vr) { vr_of(&by_app.keys().copied().collect()) } else { HashSet::new() };
    let existing = lib.collections()?;
    let mut out = Synced::default();
    for c in colls {
        let removed: HashSet<u32> = c.removed.iter().copied().collect();
        let mut members: HashSet<u32> = c.added.iter().copied().collect();
        if c.installed || c.vr {
            members.extend(by_app.iter().filter(|(app, (_, inst))| (!c.installed || *inst) && (!c.vr || vr.contains(app))).map(|(app, _)| *app));
        }
        let ids: Vec<i64> = members.iter().filter(|a| !removed.contains(a)).filter_map(|a| by_app.get(a).map(|g| g.0)).collect();
        if is_finished(&c.name) {
            out.beaten += lib.mark_beaten(&ids)?;
        }
        if ignored.contains(&c.key) {
            continue;
        }
        if c.unsupported {
            log::info!("Steam collection {:?} uses filters Gamut can't follow; only its hand-added games are synced", c.name);
        }
        let id = match existing.iter().find(|e| e.steam.as_deref() == Some(&c.key)) {
            Some(e) => e.id,
            // Empty hand-filled ones (e.g. made by tools) aren't worth a tile.
            None if ids.is_empty() && !(c.installed || c.vr) => continue,
            None => {
                let id = lib.create_collection(&c.name, None)?;
                lib.set_collection_steam(id, Some(&c.key))?;
                id
            }
        };
        out.changed += lib.replace_steam_members(id, &ids)? as usize;
        out.synced += 1;
    }
    // Deleted in Steam: gone here too, unless it also holds non-Steam games.
    let steam_ids: HashSet<i64> = games.iter().map(|g| g.0).collect();
    for e in existing.iter().filter(|e| e.steam.as_ref().is_some_and(|k| !colls.iter().any(|c| &c.key == k))) {
        out.changed += 1;
        if e.games.iter().all(|g| steam_ids.contains(g)) {
            lib.delete_collection(e.id)?;
        } else {
            lib.replace_steam_members(e.id, &[])?;
            lib.set_collection_steam(e.id, None)?;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_collections_and_filters() {
        let v = |o: &str| serde_json::Value::String(o.to_string());
        let doc = serde_json::json!([
            ["user-collections.favorite", {"key": "x", "value": v(r#"{"id":"favorite","name":"Favorites","added":[10,20],"removed":[]}"#)}],
            ["user-collections.hidden", {"key": "x", "value": v(r#"{"id":"hidden","name":"Hidden","added":[30],"removed":[]}"#)}],
            ["user-collections.srm-abc", {"key": "x", "value": v(r#"{"id":"srm-abc","name":"NES","added":[],"removed":[]}"#)}],
            ["user-collections.uc-old", {"key": "x", "is_deleted": true}],
            ["user-collections.uc-inst", {"key": "x", "value": v(r#"{"id":"uc-inst","name":"Installed locally","added":[],"removed":[5],
                "filterSpec":{"nFormatVersion":2,"strSearchText":"","filterGroups":[{"rgOptions":[]},{"rgOptions":[1]},{"rgOptions":[]}]}}"#)}],
            ["user-collections.uc-vr", {"key": "x", "value": v(r#"{"id":"uc-vr","name":"VR","added":[7],"removed":[],
                "filterSpec":{"nFormatVersion":2,"strSearchText":"","filterGroups":[{"rgOptions":[]},{"rgOptions":[]},{"rgOptions":[3]}]}}"#)}],
            ["some-other-key", {"key": "x", "value": v("1")}]
        ]);
        let c = parse(&doc.to_string()).unwrap();
        let names: Vec<&str> = c.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["To play", "Installed locally", "VR"]);
        assert_eq!(c[0].added, [10, 20]);
        assert!(c[1].installed && !c[1].vr && c[1].removed == [5] && !c[1].unsupported);
        assert!(c[2].vr && !c[2].installed && c[2].added == [7]);
    }

    #[test]
    fn applies_to_library() {
        let dir = std::env::temp_dir().join(format!("gamut-steamcoll-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.db");
        let _ = std::fs::remove_file(&path);
        let mut lib = Library::open(&path).unwrap();
        let entry = |app: u32, installed| crate::db::SteamEntry { appid: app, name: format!("G{app}"), installed, install_dir: None, size: None, playtime_min: None, last_played: 0 };
        lib.sync_steam(&[entry(1, true), entry(2, false), entry(3, true)]).unwrap();
        lib.add_scanned(&[("Custom".into(), "c.exe".into(), None, String::new())], "").unwrap();
        let id_of = |lib: &Library, n: &str| lib.all().unwrap().into_iter().find(|g| g.name == n).unwrap().id;
        let (g1, g2, g3, cu) = (id_of(&lib, "G1"), id_of(&lib, "G2"), id_of(&lib, "G3"), id_of(&lib, "Custom"));
        let fav = SteamCollection { key: "favorite".into(), name: "To play".into(), added: vec![1, 2, 99], ..Default::default() };
        let inst = SteamCollection { key: "uc-i".into(), name: "Installed".into(), installed: true, removed: vec![3], ..Default::default() };
        let vr = SteamCollection { key: "uc-v".into(), name: "VR".into(), vr: true, ..Default::default() };
        let empty = SteamCollection { key: "uc-e".into(), name: "Empty".into(), ..Default::default() };
        let all = vec![fav.clone(), inst, vr, empty];
        assert_eq!(apply(&mut lib, &all, &[], |_| [2].into()).unwrap(), Synced { synced: 3, changed: 3, beaten: 0 });
        assert_eq!(apply(&mut lib, &all, &[], |_| [2].into()).unwrap(), Synced { synced: 3, changed: 0, beaten: 0 });
        let colls = lib.collections().unwrap();
        let by = |n: &str| colls.iter().find(|c| c.name == n).unwrap().clone();
        assert_eq!(by("To play").games, [g1, g2].into());
        assert_eq!(by("Installed").games, [g1].into());
        assert_eq!(by("VR").games, [g2].into());
        assert!(colls.iter().all(|c| c.name != "Empty"));
        // A non-Steam game added in Gamut stays; Steam's changes win for Steam games; deleted ones go.
        let tp = by("To play").id;
        lib.set_in_collection(tp, &[cu, g3], true).unwrap();
        let fav2 = SteamCollection { added: vec![2], ..fav };
        apply(&mut lib, &[fav2.clone()], &[], |_| HashSet::new()).unwrap();
        let colls = lib.collections().unwrap();
        assert_eq!(colls.len(), 1);
        assert_eq!(colls[0].games, [g2, cu].into());
        assert_eq!(colls[0].steam.as_deref(), Some("favorite"));

        // Finished → Beaten once, never over a further status or a later change; ignored ones aren't made.
        let mut g = lib.get(g2).unwrap().unwrap();
        g.completion = Some("Completed".into());
        lib.update_user_fields(&g).unwrap();
        let fin = SteamCollection { key: "uc-f".into(), name: "finished".into(), added: vec![1, 2], ..Default::default() };
        let r = apply(&mut lib, &[fav2.clone(), fin.clone()], &["uc-f".to_string()], |_| HashSet::new()).unwrap();
        assert_eq!(r.beaten, 1);
        let status = |lib: &Library, id| lib.get(id).unwrap().unwrap().completion;
        assert_eq!((status(&lib, g1).as_deref(), status(&lib, g2).as_deref()), (Some("Beaten"), Some("Completed")));
        assert!(lib.collections().unwrap().iter().all(|c| c.name != "finished"));
        let mut g = lib.get(g1).unwrap().unwrap();
        g.completion = Some("Playing".into());
        lib.update_user_fields(&g).unwrap();
        assert_eq!(apply(&mut lib, &[fav2, fin], &[], |_| HashSet::new()).unwrap().beaten, 0);
        assert_eq!(status(&lib, g1).as_deref(), Some("Playing"));
    }

    #[test]
    #[ignore = "reads the local Steam install"]
    fn reads_local_steam() {
        let steam = crate::steam::steam_path().unwrap();
        let id = crate::steam::detect_steam_id(&steam).unwrap_or_default();
        for c in read(&steam, &id).unwrap() {
            println!("{} added {} removed {} installed {} vr {} other {}", c.name, c.added.len(), c.removed.len(), c.installed, c.vr, c.unsupported);
        }
        let all: HashSet<u32> = (1..3_000_000).collect();
        println!("vr apps in cache: {}", vr_apps(&steam, &all).unwrap().len());
    }
}

#[cfg(test)]
mod local {
    use super::*;

    /// GAMUT_TEST_DB=<copy of library.db> cargo test -p gamut-core syncs_db_copy -- --ignored --nocapture
    #[test]
    #[ignore = "needs a library copy and the local Steam install"]
    fn syncs_db_copy() {
        let mut lib = Library::open(Path::new(&std::env::var("GAMUT_TEST_DB").unwrap())).unwrap();
        let steam = crate::steam::steam_path().unwrap();
        let id = crate::steam::detect_steam_id(&steam).unwrap_or_default();
        println!("{:?}", sync(&mut lib, &steam, &id, &[]).unwrap());
        for c in lib.collections().unwrap() {
            println!("{} ({}) steam={:?}", c.name, c.games.len(), c.steam);
        }
    }
}
