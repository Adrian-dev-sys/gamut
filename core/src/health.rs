//! Library health: games that can't start (no launcher, missing files) and likely duplicates.
use crate::model::{Game, Source};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// Platforms not checked (e.g. consoles played on the real console).
    pub skip_platforms: Vec<String>,
    /// Issue keys the player dismissed (`Issue::key`).
    pub ignored: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Problem {
    /// Installed custom game with nothing to start.
    NoLauncher,
    ExeMissing(String),
    /// A file named in the launch arguments (usually a ROM).
    FileMissing(String),
    FolderMissing(String),
    /// The whole drive isn't there (unplugged disk); the files may be fine.
    DriveMissing(String),
    /// Same name on the same platform.
    Duplicate,
}

#[derive(Debug, Clone)]
pub struct Issue {
    /// Stable id for Ignore.
    pub key: String,
    pub ids: Vec<i64>,
    pub problem: Problem,
}

impl Problem {
    pub fn describe(&self) -> String {
        match self {
            Problem::NoLauncher => "No executable or link to start it".into(),
            Problem::ExeMissing(p) => format!("Executable not found: {p}"),
            Problem::FileMissing(p) => format!("File in the arguments not found: {p}"),
            Problem::FolderMissing(p) => format!("Folder not found: {p}"),
            Problem::DriveMissing(d) => format!("Drive {d} not found (disconnected?)"),
            Problem::Duplicate => "Same name on the same platform".into(),
        }
    }
}

/// The platform a game is checked under (Steam games have none set).
pub fn platform(g: &Game) -> &str {
    g.platform.as_deref().filter(|p| !p.trim().is_empty()).unwrap_or(if g.source == Source::Steam { "PC (Windows)" } else { "" })
}

pub fn check(games: &[Game], s: &Settings) -> Vec<Issue> {
    check_with(games, s, |p| p.exists())
}

/// `check` with the file system abstracted for tests.
pub fn check_with(games: &[Game], s: &Settings, exists: impl Fn(&Path) -> bool) -> Vec<Issue> {
    let skip = |g: &Game| s.skip_platforms.iter().any(|p| p.eq_ignore_ascii_case(platform(g)));
    let mut out = Vec::new();
    let mut drives = BTreeMap::new();
    for g in games.iter().filter(|g| g.source == Source::Custom && g.installed && !skip(g)) {
        if let Some(problem) = missing(g, &exists, &mut drives) {
            out.push(Issue { key: format!("file:{}", g.id), ids: vec![g.id], problem });
        }
    }
    let mut groups: BTreeMap<(String, String), Vec<&Game>> = BTreeMap::new();
    for g in games.iter().filter(|g| !skip(g)) {
        let name = crate::scrape::normalize(&g.name);
        if !name.is_empty() {
            groups.entry((platform(g).to_lowercase(), name)).or_default().push(g);
        }
    }
    for ((plat, name), gs) in groups.into_iter().filter(|(_, gs)| gs.len() > 1) {
        // Bought on several stores (Steam + GOG) is on purpose; a store copy plus a scanned exe isn't.
        let stores: Vec<String> = gs.iter().map(|g| store(g).to_lowercase()).collect();
        if stores.iter().all(|x| !x.is_empty()) && stores.iter().enumerate().all(|(i, x)| !stores[..i].contains(x)) {
            continue;
        }
        out.push(Issue { key: format!("dup:{plat}|{name}"), ids: gs.iter().map(|g| g.id).collect(), problem: Problem::Duplicate });
    }
    out.retain(|i| !s.ignored.contains(&i.key));
    out
}

fn store(g: &Game) -> &str {
    if g.source == Source::Steam { "Steam" } else { g.store.as_deref().unwrap_or("").trim() }
}

/// First missing thing needed to start `g`; drive roots are checked once each.
fn missing(g: &Game, exists: &impl Fn(&Path) -> bool, drives: &mut BTreeMap<String, bool>) -> Option<Problem> {
    let Some(exe) = g.exe.as_deref().map(str::trim).filter(|e| !e.is_empty()) else {
        return Some(Problem::NoLauncher);
    };
    let mut check = |p: &str, file: fn(String) -> Problem| -> Option<Problem> {
        let root = drive(p)?;
        if !*drives.entry(root.clone()).or_insert_with(|| exists(Path::new(&root))) {
            return Some(Problem::DriveMissing(root.trim_end_matches('\\').to_string()));
        }
        let path = Path::new(p);
        if exists(path) {
            return None;
        }
        match path.parent() {
            Some(d) if d.parent().is_some() && !exists(d) => Some(Problem::FolderMissing(d.display().to_string())),
            _ => Some(file(p.to_string())),
        }
    };
    // URLs (goggalaxy://), store names and Playnite {variables} aren't files to check.
    if let Some(p) = check(exe, Problem::ExeMissing) {
        return Some(p);
    }
    for a in quoted(g.args.as_deref().unwrap_or("")) {
        if let Some(p) = check(a, Problem::FileMissing) {
            return Some(p);
        }
    }
    None
}

/// "D:\" for an absolute local path without placeholders; None otherwise.
fn drive(p: &str) -> Option<String> {
    let b = p.as_bytes();
    let ok = b.len() > 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/') && !p.contains(['{', '}', '%']);
    ok.then(|| format!("{}:\\", (b[0] as char).to_ascii_uppercase()))
}

/// The double-quoted parts of an argument string.
fn quoted(args: &str) -> impl Iterator<Item = &str> {
    args.split('"').skip(1).step_by(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game(id: i64, name: &str, platform: &str, exe: Option<&str>, args: Option<&str>) -> Game {
        let mut g = Game::new_custom(name.into(), exe.map(Into::into));
        (g.id, g.platform, g.args) = (id, Some(platform.into()), args.map(Into::into));
        g
    }

    #[test]
    fn finds_problems() {
        let on_disk = ["C:\\", "D:\\", "D:\\G\\Celeste\\Celeste.exe", "D:\\Emu\\ryujinx.exe", "D:\\Emu", "D:\\G"];
        let exists = |p: &Path| on_disk.iter().any(|d| Path::new(d) == p);
        let mut uninstalled = game(9, "Bloodborne", "Sony PlayStation 4", None, None);
        uninstalled.installed = false;
        let mut steam = game(10, "Celeste", "", None, None);
        steam.source = Source::Steam;
        let games = vec![
            game(1, "Celeste", "PC (Windows)", Some("D:\\G\\Celeste\\Celeste.exe"), None),
            game(2, "Hades", "PC (Windows)", Some("D:\\G\\Hades\\Hades.exe"), None),
            game(3, "Zelda", "Nintendo Switch", Some("D:\\Emu\\ryujinx.exe"), Some("--fullscreen \"D:\\Emu\\roms\\zelda.nsp\"")),
            game(4, "Old", "PC (Windows)", Some("E:\\Old\\old.exe"), None),
            game(5, "Leap", "PC (Windows)", Some("goggalaxy://openGameView/1"), None),
            game(6, "Empty", "PC (Windows)", None, None),
            game(7, "Gone", "PC (Windows)", Some("D:\\G\\Gone.exe"), None),
            game(8, "Zelda", "Nintendo Switch", Some("D:\\Emu\\ryujinx.exe"), Some("\"D:\\Emu\\zelda2.nsp\"")),
            uninstalled,
            steam,
        ];
        let s = Settings::default();
        let got: Vec<(Vec<i64>, Problem)> = check_with(&games, &s, exists).into_iter().map(|i| (i.ids, i.problem)).collect();
        assert_eq!(
            got,
            [
                (vec![2], Problem::FolderMissing("D:\\G\\Hades".into())),
                (vec![3], Problem::FolderMissing("D:\\Emu\\roms".into())),
                (vec![4], Problem::DriveMissing("E:".into())),
                (vec![6], Problem::NoLauncher),
                (vec![7], Problem::ExeMissing("D:\\G\\Gone.exe".into())),
                (vec![8], Problem::FileMissing("D:\\Emu\\zelda2.nsp".into())),
                (vec![3, 8], Problem::Duplicate),
                (vec![1, 10], Problem::Duplicate),
            ]
        );
        let s = Settings { skip_platforms: vec!["nintendo switch".into()], ignored: vec!["file:6".into(), "dup:pc (windows)|celeste".into()] };
        let ids: Vec<Vec<i64>> = check_with(&games, &s, exists).into_iter().map(|i| i.ids).collect();
        assert_eq!(ids, [vec![2], vec![4], vec![7]]);
        // Different stores of one game aren't duplicates.
        let mut gog = game(11, "Celeste", "PC (Windows)", Some("goggalaxy://x"), None);
        gog.store = Some("GOG".into());
        assert!(check_with(&[games[9].clone(), gog], &Settings::default(), exists).is_empty());
    }
}
