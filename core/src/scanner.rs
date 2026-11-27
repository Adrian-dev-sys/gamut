//! Folder scanners: find game executables under a folder, one candidate per game folder, with a best-guess
//! main exe and a cleaned-up name. The UI reviews candidates before anything is added.
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// A saved scanner (Settings → Scanners).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Scanner {
    pub name: String,
    pub dir: String,
    /// Comma-separated, without dots ("exe", "exe, bat").
    pub extensions: String,
    /// One pattern per line, `*`/`?` wildcards, matched against the path inside the scanned folder.
    pub exclude: String,
    /// Platform and tags given to added games ("" = none).
    pub platform: String,
    pub tags: String,
    /// Folders left unticked when adding (`key` form): not offered again unless asked.
    pub ignored: Vec<String>,
    /// Extra tags by folder name, one rule per line: `recomp = Recompilation` (`*`/`?` allowed, else "contains").
    pub tag_rules: String,
    /// Other scanners' folders inside this one (`key` form), skipped; filled in before a scan.
    #[serde(skip)]
    pub nested: Vec<String>,
}

/// Safety stop for deep trees (folder junctions, huge mod folders); games sit far shallower.
const MAX_DEPTH: u32 = 12;

impl Default for Scanner {
    fn default() -> Self {
        Scanner {
            name: "PC games".into(),
            dir: String::new(),
            extensions: "exe".into(),
            exclude: String::new(),
            platform: String::new(),
            tags: String::new(),
            ignored: Vec::new(),
            tag_rules: String::new(),
            nested: Vec::new(),
        }
    }
}

/// One game folder (or a loose file in the scanned folder).
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub folder: PathBuf,
    pub name: String,
    /// Matching files, best guess for the main one first.
    pub exes: Vec<PathBuf>,
    /// Already in the library (one of its files is a game's exe).
    pub existing: bool,
    /// Left unticked in an earlier scan.
    pub ignored: bool,
    /// The scanner's tags plus those its rules give this folder name, comma separated.
    pub tags: String,
}

impl Scanner {
    /// Tags for a game folder: the scanner's own, then every matching rule's (no duplicates).
    pub fn tags_for(&self, folder_name: &str) -> String {
        let split = |s: &str| s.split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect::<Vec<_>>();
        let name = folder_name.to_lowercase();
        let mut tags = split(&self.tags);
        for (pat, add) in self.tag_rules.lines().filter_map(|l| l.split_once('=')) {
            let pat = pat.trim().to_lowercase();
            let pat = if pat.contains(['*', '?']) { pat } else { format!("*{pat}*") };
            if pat != "**" && wildcard(&pat, &name) {
                for t in split(add) {
                    if !tags.iter().any(|x| x.eq_ignore_ascii_case(&t)) {
                        tags.push(t);
                    }
                }
            }
        }
        tags.join(", ")
    }
}

/// Helpers, installers, crash reporters and anti-cheat that are never the game.
const JUNK_FILES: &[&str] = &[
    "unins*", "uninstall*", "*setup*", "*redist*", "vcredist*", "dxwebsetup*", "dotnet*", "*crashhandler*",
    "*crashreport*", "crashpad_handler*", "*prereq*", "easyanticheat*", "battleye*", "be_service*", "*_be.*",
    "*launcherhelper*", "notification_helper*", "*updater*", "cefprocess*", "*cefsubprocess*", "zfgamebrowser*",
    "quicksfv*", "unitycrashhandler*", "dxsetup*", "physx*", "oalinst*", "touchup*", "createdump*",
];
const JUNK_DIRS: &[&str] = &[
    "_commonredist", "commonredist", "redist", "redistributables", "directx", "__installer", "_installer",
    "installers", "prerequisites", "prereqs", "support", "easyanticheat", "battleye", "vcredist", "dotnet",
    "__support", "_redist", "crashreporter", "engine", "steamapps", "llvm", "compiler", "node_modules", "python", "mono", "jre",
    "venv", "site-packages", "__pycache__", "toolchain", "obj", "test", "tests",
];

/// Subfolder names that belong to one game (or a dev build of it), never separate games of a collection.
const PART_DIRS: &[&str] = &[
    "bin", "bin32", "bin64", "build", "builds", "extracted", "lib", "libs", "scripts", "tools", "game", "temp", "tmp",
    "content", "data", "x64", "x86", "win32", "win64", "binaries", "release", "debug", "src", "dist", "out",
    "plugins", "mods", "system", "runtime", "assets", "resources", "_backup", "backup", "backup-old", "_backup-old",
];

/// Other scanners' folders strictly inside `dir`, as `key`s.
pub fn nested_dirs(dir: &str, all: &[Scanner]) -> Vec<String> {
    let base = format!("{}\\", key(Path::new(dir.trim())).trim_end_matches('\\'));
    all.iter().map(|o| key(Path::new(o.dir.trim())).trim_end_matches('\\').to_string()).filter(|k| k.starts_with(&base)).collect()
}

/// Lists candidates under `s.dir`. `known` holds lowercased exe paths already in the library.
pub fn scan(s: &Scanner, known: &HashSet<String>) -> anyhow::Result<Vec<Candidate>> {
    let root = PathBuf::from(s.dir.trim());
    anyhow::ensure!(root.is_dir(), "Folder not found: {}", root.display());
    let exts: Vec<String> = s.extensions.split([',', ' ', ';']).map(|e| e.trim().trim_start_matches('.').to_lowercase()).filter(|e| !e.is_empty()).collect();
    let exclude: Vec<String> = s.exclude.lines().map(|l| l.trim().replace('/', "\\").to_lowercase()).filter(|l| !l.is_empty()).collect();
    let mut out = Vec::new();
    let mut entries: Vec<_> = std::fs::read_dir(&root)?.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        let rel = rel_lower(&root, &path);
        if excluded(&rel, &exclude) || s.nested.contains(&key(&path)) {
            continue;
        }
        if path.is_dir() {
            let mut files = Vec::new();
            walk(&root, &path, MAX_DEPTH, &exts, &exclude, &s.nested, &mut files);
            let mut groups = Vec::new();
            split(&path, files, &mut groups);
            groups.sort();
            for (dir, mut files) in groups {
                if files.is_empty() {
                    continue;
                }
                let folder_name = dir.file_name().unwrap_or_default().to_string_lossy().to_string();
                rank(&dir, &folder_name, &mut files);
                out.push(Candidate { name: clean_name(&folder_name), existing: false, ignored: false, tags: String::new(), exes: files, folder: dir });
            }
        } else if matches_ext(&path, &exts) && !is_junk_file(&path) {
            let stem = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
            out.push(Candidate { name: clean_name(&stem), existing: false, ignored: false, tags: String::new(), exes: vec![path.clone()], folder: path });
        }
    }
    // Known: exe paths and install folders of games already in the library. A known exe anywhere inside
    // the folder counts too (it may be filtered out here).
    for c in &mut out {
        let folder = key(&c.folder);
        c.ignored = s.ignored.contains(&folder);
        let name = if c.folder.is_dir() { c.folder.file_name() } else { c.folder.file_stem() };
        c.tags = s.tags_for(&name.unwrap_or_default().to_string_lossy());
        let inside = format!("{folder}\\");
        c.existing = known.contains(&folder)
            || c.exes.iter().any(|e| known.contains(&key(e)))
            || c.folder.is_dir() && known.iter().any(|k| k.starts_with(&inside));
    }
    Ok(out)
}

/// A collection ("NEKOPARA Collection\Vol. 1", "\Vol. 2"…) has nothing at its top and several subfolders
/// with their own files: each subfolder is a game (recursively). A lone subfolder is looked into.
fn split(dir: &Path, files: Vec<PathBuf>, out: &mut Vec<(PathBuf, Vec<PathBuf>)>) {
    if files.iter().any(|f| f.parent() == Some(dir)) {
        out.push((dir.to_path_buf(), files));
        return;
    }
    let mut groups: Vec<(PathBuf, Vec<PathBuf>)> = Vec::new();
    for f in &files {
        let Some(sub) = f.strip_prefix(dir).ok().and_then(|r| r.components().next()).map(|c| dir.join(c)) else { continue };
        match groups.iter_mut().find(|(g, _)| *g == sub) {
            Some((_, v)) => v.push(f.clone()),
            None => groups.push((sub, vec![f.clone()])),
        }
    }
    let part = |g: &PathBuf| PART_DIRS.contains(&g.file_name().unwrap_or_default().to_string_lossy().to_lowercase().as_str());
    if groups.iter().any(|(g, _)| part(g)) {
        // bin\, build\, extracted\…: parts of one game, not a collection.
        out.push((dir.to_path_buf(), files));
        return;
    }
    match groups.len() {
        0 => {}
        // One subfolder (Game\Binaries\Win64…): still one game, named after the outer folder.
        1 => out.push((dir.to_path_buf(), files)),
        _ => groups.into_iter().for_each(|(g, f)| split(&g, f, out)),
    }
}

/// Lowercased path with backslashes, for comparing exe paths.
pub fn key(p: &Path) -> String {
    p.to_string_lossy().replace('/', "\\").to_lowercase()
}

fn rel_lower(root: &Path, p: &Path) -> String {
    key(p.strip_prefix(root).unwrap_or(p))
}

fn walk(root: &Path, dir: &Path, depth: u32, exts: &[String], exclude: &[String], skip: &[String], out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if excluded(&rel_lower(root, &p), exclude) {
            continue;
        }
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            let name = e.file_name().to_string_lossy().to_lowercase();
            // Hidden/dev folders (.venv, .git) and other scanners' folders are skipped.
            if depth > 0 && !JUNK_DIRS.contains(&name.as_str()) && !name.starts_with('.') && !skip.contains(&key(&p)) {
                walk(root, &p, depth - 1, exts, exclude, skip, out);
            }
        } else if matches_ext(&p, exts) && !is_junk_file(&p) {
            out.push(p);
        }
    }
}

fn matches_ext(p: &Path, exts: &[String]) -> bool {
    p.extension().is_some_and(|e| exts.contains(&e.to_string_lossy().to_lowercase()))
}

fn is_junk_file(p: &Path) -> bool {
    let n = p.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
    JUNK_FILES.iter().any(|pat| wildcard(pat, &n))
}

/// A pattern without a backslash matches any single path part; with one, the whole relative path.
fn excluded(rel: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|pat| if pat.contains('\\') { wildcard(pat, rel) } else { rel.split('\\').any(|part| wildcard(pat, part)) })
}

/// `*` and `?` wildcards, case-sensitive (callers lowercase).
pub fn wildcard(pat: &str, s: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pat.chars().collect(), s.chars().collect());
    let (mut pi, mut ti, mut star, mut mark) = (0, 0, None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(sp) = star {
            pi = sp + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

/// Best guess first: named like the folder, shallow (a root launcher beats a deep shipping binary), then big.
fn rank(folder: &Path, folder_name: &str, files: &mut [PathBuf]) {
    let want = squash(&clean_name(folder_name));
    let score = |p: &PathBuf| -> i64 {
        let stem = squash(&p.file_stem().unwrap_or_default().to_string_lossy());
        let depth = p.strip_prefix(folder).map(|r| r.components().count() as i64 - 1).unwrap_or(0);
        let mb = std::fs::metadata(p).map(|m| m.len() as f64 / 1_048_576.0).unwrap_or(0.0);
        let mut s = -15 * depth + ((mb.max(1.0).log2() * 3.0) as i64).min(25);
        if !want.is_empty() && stem == want {
            s += 100;
        } else if !want.is_empty() && !stem.is_empty() && (stem.contains(&want) || want.contains(&stem)) {
            s += 40;
        }
        if stem.contains("launcher") || stem.contains("config") || stem.contains("settings") {
            s -= 20;
        }
        s
    };
    files.sort_by_cached_key(|p| (std::cmp::Reverse(score(p)), p.clone()));
}

/// Letters and digits only, lowercased.
fn squash(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// Store tags, dropped anywhere in a folder name.
const STORE: &[&str] = &["gog", "gog.com", "repack", "fitgirl", "dodi", "elamigos", "drm-free"];
/// Dropped in brackets or glued on ("(Win)", "-pc").
const SOFT: &[&str] = &[
    "steam", "epic", "portable", "multi", "proper", "rip", "x64", "win64", "64bit", "32bit", "fix", "update", "eng",
    "english", "en", "jp", "pc", "win", "windows", "uncensored",
];
/// Of those, the ones safe to drop from the end of a name too ("Minecraft for Windows" keeps "Windows").
const TRAILING: &[&str] = &["eng", "english", "en", "jp", "pc", "x64", "win64", "64bit", "32bit", "uncensored", "proper", "rip"];
/// Scene groups and installer leftovers, only when glued on ("-CODEX") or in brackets.
const GLUED: &[&str] = &[
    "codex", "plaza", "skidrow", "rune", "tenoke", "empress", "darksiders", "flt", "razor1911", "reloaded", "prophet",
    "cpy", "hoodlum", "setup", "installer",
];

fn is_tag(w: &str) -> bool {
    let w = w.to_lowercase();
    [STORE, SOFT, GLUED].iter().any(|l| l.contains(&w.as_str()))
}

/// "Hollow.Knight.v1.5-GOG" → "Hollow Knight"; "Celeste_v1.4 [GOG]" → "Celeste". Keeps real subtitles.
pub fn clean_name(raw: &str) -> String {
    use regex::Regex;
    let version = Regex::new(r"(?i)^(v|ver|version|build|b)?\.?\d+([._]\d+)*[a-z]?$").unwrap();
    let is_version = |w: &str| {
        let l = w.to_lowercase();
        version.is_match(w) && (l.starts_with('v') || l.starts_with('b') || w.contains('.'))
    };
    let mut s = Regex::new(r"\[[^\]]*\]").unwrap().replace_all(raw, " ").to_string();
    // Parentheses only when they hold tags or a version ("(GOG)", "(v1.2)"), not "(The Final Cut)".
    s = Regex::new(r"\(([^)]*)\)")
        .unwrap()
        .replace_all(&s, |c: &regex::Captures| {
            let inner = c[1].trim();
            let tag = !inner.is_empty() && inner.split_whitespace().all(|w| version.is_match(w) || is_tag(w));
            if tag { " ".to_string() } else { c[0].to_string() }
        })
        .to_string();
    // "v1_1" is a version, not two words; "1.13.ENG" ends in a tag.
    s = Regex::new(r"(\d)_(\d)").unwrap().replace_all(&s, "$1.$2").to_string();
    s = Regex::new(r"(\d)\.([A-Za-z])").unwrap().replace_all(&s, "$1 $2").to_string();
    s = s.replace('_', " ");
    // Tags and versions glued on at the end ("Game-1.2-pc", "Game-CODEX"); "Half-Life" keeps its hyphen.
    while let Some((head, _)) = s.trim().rsplit_once('-').filter(|(h, t)| !h.is_empty() && (is_tag(t) || is_version(t))) {
        s = head.to_string();
    }
    // Dots as separators ("Hollow.Knight"), but not inside numbers ("1.5").
    if !s.trim().contains(' ') || Regex::new(r"[A-Za-z]{2}\.[A-Za-z]").unwrap().is_match(&s) {
        s = Regex::new(r"\.(\D)").unwrap().replace_all(&s, " $1").to_string();
        s = Regex::new(r"(\D)\.").unwrap().replace_all(&s, "$1 ").to_string();
    }
    let humps = Regex::new(r"[a-z][A-Z]").unwrap();
    let (camel_a, camel_b) = (Regex::new(r"([a-z])([A-Z])").unwrap(), Regex::new(r"([A-Z])([A-Z][a-z])").unwrap());
    let mut words: Vec<String> = Vec::new();
    let mut after_build = false;
    for w in s.split_whitespace() {
        // A tag glued on with a hyphen: "v1.5-GOG", "Astral Chain-Setup".
        let w = match w.rsplit_once('-') {
            Some((head, tail)) if !head.is_empty() && (is_tag(tail) || is_version(tail)) => head,
            _ => w,
        };
        let lw = w.to_lowercase();
        if lw == "build" {
            after_build = true;
            continue;
        }
        if std::mem::take(&mut after_build) && w.chars().all(|c| c.is_ascii_digit() || c == '.') {
            continue;
        }
        if STORE.contains(&lw.as_str()) || is_version(w) {
            continue;
        }
        // "HappyMemoriesISummonedAWhat" splits; "FlipWitch" and "PlayCabal" stay.
        if humps.find_iter(w).count() >= 2 {
            let w = camel_b.replace_all(&camel_a.replace_all(w, "$1 $2"), "$1 $2").to_string();
            words.extend(w.split_whitespace().map(String::from));
        } else {
            words.push(w.to_string());
        }
    }
    // Trailing soft tags and versions ("Nascar Races PC" keeps PC only mid-name).
    while words.len() > 1 && words.last().is_some_and(|w| TRAILING.contains(&w.to_lowercase().as_str()) || is_version(w)) {
        words.pop();
    }
    // A hyphen left behind by a dropped tag isn't wanted.
    let out = words.join(" ").trim_matches(|c: char| c == '-' || c.is_whitespace()).to_string();
    if out.is_empty() { raw.trim().to_string() } else { out }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_folder_names() {
        for (raw, want) in [
            ("Hollow.Knight.v1.5-GOG", "Hollow Knight"),
            ("Celeste_v1.4 [GOG]", "Celeste"),
            ("Disco Elysium (The Final Cut) [GOG]", "Disco Elysium (The Final Cut)"),
            ("Fallout 4", "Fallout 4"),
            ("Half-Life 2", "Half-Life 2"),
            ("Hades II (v1.0.2)", "Hades II"),
            ("The.Witcher.3.Wild.Hunt.GOTY-GOG", "The Witcher 3 Wild Hunt GOTY"),
            ("Dead Cells Build 12345", "Dead Cells"),
            ("S.T.A.L.K.E.R. Shadow of Chernobyl", "S.T.A.L.K.E.R. Shadow of Chernobyl"),
            ("HappyMemoriesISummonedAWhat-1.2-pc", "Happy Memories I Summoned A What"),
            ("Neet - Angel and naughty Family v1.13.ENG", "Neet - Angel and naughty Family"),
            ("Undertale Yellow v1_1", "Undertale Yellow"),
            ("Naisho no Jikan (ENG)", "Naisho no Jikan"),
            ("Pokemon Zeta 1.4 (Win)", "Pokemon Zeta"),
            ("FlipWitch.Forbidden.Sex.Hex.v1.5", "FlipWitch Forbidden Sex Hex"),
            ("NUKITASHI 2", "NUKITASHI 2"),
            ("Minecraft for Windows", "Minecraft for Windows"),
            ("Gears of War- Reloaded", "Gears of War- Reloaded"),
            ("Nascar Races PC game", "Nascar Races PC game"),
            ("Astral Chain-Setup", "Astral Chain"),
            ("Game-CODEX", "Game"),
            ("Sex Lesson Island-v1.04", "Sex Lesson Island"),
            ("Half-Life", "Half-Life"),
            ("Spider-Man", "Spider-Man"),
            ("Counter-Strike-v1.6", "Counter-Strike"),
            ("PlayCabal", "PlayCabal"),
        ] {
            assert_eq!(clean_name(raw), want, "{raw}");
        }
    }

    #[test]
    fn tag_rules() {
        let s = Scanner { tags: "Emulation".into(), tag_rules: "recomp = Recompilation\ndecomp=Decompilation, Fan port\n*pc port* = Fan port\nbad line".into(), ..Default::default() };
        assert_eq!(s.tags_for("Breath of the Wild recomp - Savage Wind"), "Emulation, Recompilation");
        assert_eq!(s.tags_for("Ocarina of Time 64 PC port"), "Emulation, Fan port");
        assert_eq!(s.tags_for("Mario Decomp PC Port"), "Emulation, Decompilation, Fan port");
        assert_eq!(s.tags_for("Celeste"), "Emulation");
    }

    #[test]
    fn wildcards() {
        assert!(wildcard("unins*", "unins000.exe"));
        assert!(wildcard("*setup*", "dxsetup.exe"));
        assert!(!wildcard("*setup*", "game.exe"));
        assert!(wildcard("a?c", "abc"));
        assert!(excluded("tools\\editor.exe", &["tools".into()]));
        assert!(excluded("x\\tools\\a.exe", &["x\\tools\\*".into()]));
        assert!(!excluded("mytools\\a.exe", &["tools".into()]));
    }

    #[test]
    fn finds_and_ranks_game_folders() {
        let root = std::env::temp_dir().join(format!("gamut-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mk = |p: &str, bytes: usize| {
            let f = root.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, vec![0u8; bytes]).unwrap();
        };
        mk("Hollow Knight/hollow_knight.exe", 10);
        mk("Hollow Knight/UnityCrashHandler64.exe", 10);
        mk("Hollow Knight/unins000.exe", 10);
        mk("Tools Pack/readme.txt", 10);
        mk("Big Game/Launcher.exe", 10);
        mk("Big Game/BigGame.exe", 10);
        mk("Big Game/_CommonRedist/vc.exe", 10);
        mk("Loose Game.exe", 10);
        mk("Saga Collection/Vol. 1/Saga1.exe", 10);
        mk("Saga Collection/Vol. 2/Saga2.exe", 10);
        mk("Nested/Inner/A/a.exe", 10);
        mk("Nested/Inner/B/b.exe", 10);
        mk("Nested/Other/o.exe", 10);
        mk("SteamLibrary/steamapps/common/X/x.exe", 10);
        mk("Unreal Game/UnrealGame.exe", 10);
        mk("Unreal Game/UnrealGame/Binaries/Win64/UnrealGame-Win64-Shipping.exe", 10);
        mk("Old Game/start.exe", 10);
        mk("Deep/a/b/c/d/e/f/deep.exe", 10);
        mk("Recomp-Setup/bin/recomp_xlate.exe", 10);
        mk("Recomp-Setup/extracted/recomp.exe", 10);
        mk("Recomp-Setup/.venv/Scripts/pip.exe", 10);
        mk("VNs/Some VN/vn.exe", 10);
        // Library knows Old Game by an exe this scan doesn't list.
        let known: HashSet<String> = [key(&root.join("Big Game/BigGame.exe")), key(&root.join("Old Game/bin/real.exe"))].into();
        let s = Scanner { dir: root.to_string_lossy().into(), ignored: vec![key(&root.join("Loose Game.exe"))], ..Default::default() };
        let s = Scanner { nested: nested_dirs(&s.dir, &[s.clone(), Scanner { dir: root.join("VNs").to_string_lossy().into(), ..Default::default() }]), ..s };
        let c = scan(&s, &known).unwrap();
        let names: Vec<&str> = c.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Big Game", "Deep", "Hollow Knight", "Loose Game", "A", "B", "Other", "Old Game", "Recomp", "Vol. 1", "Vol. 2", "Unreal Game"]);
        assert_eq!(c[8].exes.len(), 2);
        assert!(c[7].existing && c[3].ignored && !c[2].ignored);
        assert_eq!(c[11].exes[0].file_name().unwrap(), "UnrealGame.exe");
        assert_eq!(c[0].exes[0].file_name().unwrap(), "BigGame.exe");
        assert!(c[0].existing && !c[2].existing);
        assert_eq!(c[2].exes.len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }
}
