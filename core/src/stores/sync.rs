//! Matching a store's games to library rows, and applying the user's choices.
//! Rows are only ever linked by id (an earlier link, or the store link in a Playnite import);
//! same name or same folder is a doubt for the user, never merged on its own.
use super::{Store, StoreGame};
use crate::{Game, Library, Source};
use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub enum Match {
    /// Linked before.
    Linked(i64),
    /// Not linked yet, but the row starts this store game (e.g. imported from Playnite).
    Exact(i64),
    /// Same name or same folder as these rows: the user decides.
    Doubt(Vec<i64>),
    New,
}

#[derive(Debug, Clone)]
pub struct Planned {
    pub game: StoreGame,
    pub m: Match,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Decision {
    /// It's this library row.
    Link(i64),
    Import,
    Skip,
}

#[derive(Debug, Default, PartialEq)]
pub struct Applied {
    pub added: usize,
    pub updated: usize,
    pub skipped: usize,
    /// Doubts and new games left for the user.
    pub pending: usize,
}

/// Letters and digits only, lowercase: "DREDGE™" == "Dredge".
pub fn norm(name: &str) -> String {
    name.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

fn same_dir(a: &str, b: &str) -> bool {
    let n = |p: &str| p.trim().trim_end_matches(['\\', '/']).replace('/', "\\").to_lowercase();
    !a.trim().is_empty() && n(a) == n(b)
}

/// What each store game is in the library. Games the user skipped are left out.
pub fn plan(lib: &Library, store: Store, games: &[StoreGame]) -> Result<Vec<Planned>> {
    let links = lib.store_links(store.name())?;
    let skipped = lib.store_skipped(store.name())?;
    let all = lib.all()?;
    let linked: std::collections::HashSet<i64> = links.values().copied().collect();
    let free: Vec<&Game> = all.iter().filter(|g| g.source == Source::Custom && !linked.contains(&g.id)).collect();
    let exists = |id: &i64| all.iter().any(|g| g.id == *id);
    let mut out = Vec::new();
    for sg in games {
        if skipped.contains(&sg.id) {
            continue;
        }
        let m = if let Some(id) = links.get(&sg.id).filter(|id| exists(id)) {
            Match::Linked(*id)
        } else if let Some(g) = free.iter().find(|g| {
            let launch = format!("{} {}", g.exe.as_deref().unwrap_or(""), g.args.as_deref().unwrap_or(""));
            store.launches(&sg.id, &launch)
        }) {
            Match::Exact(g.id)
        } else {
            let n = norm(&sg.name);
            let dir = sg.install_dir.as_deref().unwrap_or("");
            let hits: Vec<i64> = free
                .iter()
                .filter(|g| (!n.is_empty() && norm(&g.name) == n) || g.folder().is_some_and(|f| same_dir(&f.to_string_lossy(), dir)))
                .map(|g| g.id)
                .collect();
            if hits.is_empty() { Match::New } else { Match::Doubt(hits) }
        };
        out.push(Planned { game: sg.clone(), m });
    }
    Ok(out)
}

/// Applies the plan: linked and exact rows follow the store; `decide` answers doubts and new games
/// (None leaves them pending).
pub fn apply(lib: &mut Library, store: Store, plan: &[Planned], decide: impl Fn(&Planned) -> Option<Decision>) -> Result<Applied> {
    let mut out = Applied::default();
    for p in plan {
        let d = match &p.m {
            Match::Linked(id) | Match::Exact(id) => Some(Decision::Link(*id)),
            _ => decide(p),
        };
        match d {
            Some(Decision::Link(id)) => {
                let Some(g) = lib.get(id)? else { continue };
                lib.link_store(store.name(), &p.game.id, id)?;
                out.updated += follow(lib, store, &g, &p.game)? as usize;
            }
            Some(Decision::Import) => {
                let mut g = Game::new_custom(p.game.name.clone(), None);
                g.installed = false;
                let id = lib.add_custom(&g)?;
                lib.link_store(store.name(), &p.game.id, id)?;
                let g = lib.get(id)?.unwrap_or(g);
                follow(lib, store, &Game { exe: None, ..g }, &p.game)?;
                out.added += 1;
            }
            Some(Decision::Skip) => {
                lib.skip_store_game(store.name(), &p.game.id)?;
                out.skipped += 1;
            }
            None => out.pending += 1,
        }
    }
    Ok(out)
}

/// Sets the row's installed state and folder from the store. Its launch only changes while it isn't a
/// working exe of the user's (empty, a link, or a missing file), so custom launch setups survive.
fn follow(lib: &Library, store: Store, g: &Game, sg: &StoreGame) -> Result<bool> {
    let cur = g.exe.as_deref().unwrap_or("").trim();
    let replaceable = cur.is_empty() || cur.contains("://") || !Path::new(cur).is_file();
    let uri = store.open_uri(&sg.id);
    let launch = match (&sg.exe, replaceable) {
        (_, false) => None,
        (Some(exe), true) if sg.installed => Some((exe.as_str(), sg.args.as_deref(), sg.work_dir.as_deref())),
        (_, true) => Some((uri.as_str(), None, None)),
    };
    lib.set_store_state(g.id, store.name(), sg.installed, sg.install_dir.as_deref().filter(|_| sg.installed), launch)
}

/// Decisions given in the review dialog, by store game id.
pub fn decided(map: &HashMap<String, Decision>) -> impl Fn(&Planned) -> Option<Decision> + '_ {
    move |p| map.get(&p.game.id).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_lib(name: &str) -> (Library, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("gamut-stores-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        (Library::open(&dir.join("t.db")).unwrap(), dir)
    }

    fn sg(id: &str, name: &str) -> StoreGame {
        StoreGame { id: id.into(), name: name.into(), ..Default::default() }
    }

    #[test]
    fn plans_links_doubts_and_new() {
        let (mut lib, dir) = tmp_lib("plan");
        // A Playnite-imported GOG row (link in its launch), a same-name row, an unrelated one.
        let pn = lib.add_custom(&Game::new_custom("Dredge".into(), Some("goggalaxy://openGameView/17".into()))).unwrap();
        let same = lib.add_custom(&Game::new_custom("SOMA™".into(), Some(r"D:\x\soma.exe".into()))).unwrap();
        lib.add_custom(&Game::new_custom("Other".into(), Some("goggalaxy://openGameView/170".into()))).unwrap();
        let games = [sg("17", "DREDGE"), sg("18", "SOMA"), sg("19", "MDK")];
        let p = plan(&lib, Store::Gog, &games).unwrap();
        let ms: Vec<_> = p.iter().map(|p| p.m.clone()).collect();
        assert_eq!(ms, [Match::Exact(pn), Match::Doubt(vec![same]), Match::New]);

        // Nothing decided: only the exact one is applied (to not installed, with the Galaxy link).
        let r = apply(&mut lib, Store::Gog, &p, |_| None).unwrap();
        assert_eq!(r, Applied { added: 0, updated: 1, skipped: 0, pending: 2 });
        let g = lib.get(pn).unwrap().unwrap();
        assert_eq!((g.installed, g.store.as_deref(), g.exe.as_deref()), (false, Some("GOG"), Some("goggalaxy://openGameView/17")));

        // Decide: SOMA is the existing row (its working exe stays), MDK skipped.
        let dec: HashMap<String, Decision> = [("18".to_string(), Decision::Link(same)), ("19".to_string(), Decision::Skip)].into();
        apply(&mut lib, Store::Gog, &p, decided(&dec)).unwrap();
        let again = plan(&lib, Store::Gog, &games).unwrap();
        assert_eq!(again.iter().map(|p| p.m.clone()).collect::<Vec<_>>(), [Match::Linked(pn), Match::Linked(same)]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn imports_and_follows_install() {
        let (mut lib, dir) = tmp_lib("import");
        let exe = dir.join("game.exe");
        std::fs::write(&exe, b"").unwrap();
        let mut g = sg("5", "MDK");
        let p = plan(&lib, Store::Gog, &[g.clone()]).unwrap();
        apply(&mut lib, Store::Gog, &p, |_| Some(Decision::Import)).unwrap();
        let id = lib.store_links("GOG").unwrap()["5"];
        let row = lib.get(id).unwrap().unwrap();
        assert_eq!((row.installed, row.exe.as_deref()), (false, Some("goggalaxy://openGameView/5")));
        // Installed in Galaxy: the next sync points it at the exe.
        (g.installed, g.exe, g.install_dir) = (true, Some(exe.display().to_string()), Some(dir.display().to_string()));
        let p = plan(&lib, Store::Gog, &[g]).unwrap();
        assert_eq!(apply(&mut lib, Store::Gog, &p, |_| None).unwrap().updated, 1);
        let row = lib.get(id).unwrap().unwrap();
        assert_eq!((row.installed, row.exe), (true, Some(exe.display().to_string())));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Dry run against a copy of a real library: `GAMUT_TEST_DB=C:/t/ach/lib.db cargo test -p gamut-core -- --ignored gog_dry_run --nocapture`
    #[test]
    #[ignore]
    fn gog_dry_run() {
        let lib = Library::open(Path::new(&std::env::var("GAMUT_TEST_DB").unwrap())).unwrap();
        let games = Store::Gog.scan().unwrap();
        let p = plan(&lib, Store::Gog, &games).unwrap();
        let count = |f: fn(&Match) -> bool| p.iter().filter(|p| f(&p.m)).count();
        println!("{} GOG games: linked {}, exact {}, doubts {}, new {}", p.len(), count(|m| matches!(m, Match::Linked(_))),
            count(|m| matches!(m, Match::Exact(_))), count(|m| matches!(m, Match::Doubt(_))), count(|m| *m == Match::New));
        for x in p.iter().filter(|p| !matches!(p.m, Match::Exact(_) | Match::Linked(_))) {
            println!("  {:?} {}", x.m, x.game.name);
        }
        for x in p.iter().filter(|p| p.game.installed) {
            println!("  installed: {} {:?} {:?}", x.game.name, x.game.exe, x.game.args);
        }
    }
}
