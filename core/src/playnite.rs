//! Import from a Playnite library folder (the one holding `library\games.db`).
use crate::litedb::{self, Doc, Value};
use crate::model::{state, Game, Source};
use crate::{images, scrape, Library, Paths};
use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const STEAM_PLUGIN: &str = "cb91dfc9-b977-43bf-8e70-55f46e410fab";
const GOG_PLUGIN: &str = "aebe8b7c-6dc3-4a66-af31-e7375c6b5e9e";
const UBISOFT_PLUGIN: &str = "c2f038e5-8b92-4877-91f1-da9094155fc5";
const NO_ID: &str = "00000000-0000-0000-0000-000000000000";

/// How an imported game will be started from Gamut.
#[derive(Debug, Clone, PartialEq)]
pub enum Launch {
    Steam,
    Exe,
    Emulator(String),
    Url,
    /// Playnite could start it, but we can't (reason).
    Unresolved(String),
    /// Nothing to start on this PC (console library entries etc.).
    None,
}

pub struct Item {
    pub game: Game,
    pub cover: Option<PathBuf>,
    pub background: Option<PathBuf>,
    pub launch: Launch,
}

/// Pulls only descriptions (with their images) from Playnite; nothing else in the library changes.
pub fn refresh_descriptions(lib: &mut Library, items: &[Item]) -> Result<(usize, usize)> {
    let new: Vec<(String, String)> = items
        .iter()
        .filter_map(|i| Some((i.game.playnite_id.clone()?, i.game.description.clone()?)))
        .collect();
    lib.refresh_descriptions(&new)
}

/// Default Playnite data folder for installed (non-portable) Playnite.
pub fn default_dir() -> Option<PathBuf> {
    let d = PathBuf::from(std::env::var_os("APPDATA")?).join("Playnite");
    d.join("library").join("games.db").is_file().then_some(d)
}

/// Reads the whole Playnite library into Gamut games (nothing is written).
pub fn load(dir: &Path) -> Result<Vec<Item>> {
    let lib = dir.join("library");
    if !lib.join("games.db").is_file() {
        bail!("no Playnite library in {}", dir.display());
    }
    let read = |name: &str| -> Result<Vec<Doc>> {
        let p = lib.join(name);
        if !p.exists() {
            return Ok(Vec::new());
        }
        match std::fs::File::open(&p) {
            Err(e) if e.raw_os_error() == Some(32) => bail!("Playnite is running; close it and try again"),
            _ => litedb::read(&p),
        }
    };
    let names = |name: &str| -> Result<HashMap<String, String>> {
        Ok(read(name)?
            .into_iter()
            .filter_map(|d| Some((d.get("_id")?.str()?.to_string(), d.get("Name")?.str()?.to_string())))
            .collect())
    };
    let t = Tables {
        companies: names("companies.db")?,
        genres: names("genres.db")?,
        platforms: names("platforms.db")?,
        sources: names("sources.db")?,
        tags: names("tags.db")?,
        series: names("series.db")?,
        features: names("features.db").unwrap_or_default(),
        completion: names("completionstatuses.db")?,
        emulators: read("emulators.db")?
            .into_iter()
            .filter_map(|d| Some((d.get("_id")?.str()?.to_string(), d)))
            .collect(),
        defs: EmuDefs::load(dir),
        files: lib.join("files"),
    };
    let games = read("games.db")?;
    Ok(games.iter().filter_map(|d| t.item(d)).collect())
}

struct Tables {
    companies: HashMap<String, String>,
    genres: HashMap<String, String>,
    platforms: HashMap<String, String>,
    sources: HashMap<String, String>,
    tags: HashMap<String, String>,
    series: HashMap<String, String>,
    features: HashMap<String, String>,
    completion: HashMap<String, String>,
    emulators: HashMap<String, Doc>,
    defs: EmuDefs,
    files: PathBuf,
}

fn s<'a>(d: &'a Doc, k: &str) -> Option<&'a str> {
    d.get(k).and_then(Value::str).map(str::trim).filter(|v| !v.is_empty())
}

impl Tables {
    fn join(map: &HashMap<String, String>, d: &Doc, k: &str) -> Option<String> {
        let v: Vec<&str> = d
            .get(k)
            .map(Value::array)
            .unwrap_or_default()
            .iter()
            .filter_map(|id| map.get(id.str()?).map(String::as_str))
            .collect();
        (!v.is_empty()).then(|| v.join(", "))
    }

    fn media(&self, d: &Doc, k: &str) -> Option<PathBuf> {
        let v = s(d, k)?;
        if v.contains("://") {
            return None;
        }
        let p = Path::new(v);
        let p = if p.is_absolute() { p.to_path_buf() } else { self.files.join(p) };
        p.is_file().then_some(p)
    }

    fn item(&self, d: &Doc) -> Option<Item> {
        let id = s(d, "_id")?.to_string();
        let name = s(d, "Name")?.to_string();
        let plugin = s(d, "PluginId").unwrap_or(NO_ID);
        let steam_appid = (plugin == STEAM_PLUGIN).then(|| s(d, "GameId")?.parse::<u32>().ok()).flatten();
        let secs = |k: &str| d.get(k).and_then(Value::date_ms).map(|ms| ms / 1000).unwrap_or(0);
        let completion = d
            .get("CompletionStatus")
            .and_then(Value::doc)
            .and_then(|c| s(c, "Name"))
            .map(str::to_string)
            .or_else(|| self.completion.get(s(d, "CompletionStatusId")?).cloned());

        let mut g = Game::new_custom(name, None);
        g.source = if steam_appid.is_some() { Source::Steam } else { Source::Custom };
        g.source_id = steam_appid.map(|a| a.to_string()).unwrap_or_default();
        g.playnite_id = Some(id);
        g.install_dir = s(d, "InstallDirectory").map(str::to_string);
        g.playtime_min = (d.get("Playtime").and_then(Value::int).unwrap_or(0) + 30) / 60;
        g.last_played = secs("LastActivity");
        g.added = secs("Added");
        g.play_count = d.get("PlayCount").and_then(Value::int).unwrap_or(0);
        g.favorite = d.get("Favorite").is_some_and(Value::bool);
        g.hidden = d.get("Hidden").is_some_and(Value::bool);
        g.completion = completion;
        g.critic_score = d.get("CriticScore").and_then(Value::int);
        g.community_score = d.get("CommunityScore").and_then(Value::int);
        g.description = s(d, "Description").map(|h| scrape::strip_html(h)).filter(|t| !t.is_empty());
        g.developer = Self::join(&self.companies, d, "DeveloperIds");
        g.publisher = Self::join(&self.companies, d, "PublisherIds");
        g.genres = Self::join(&self.genres, d, "GenreIds");
        g.tags = Self::join(&self.tags, d, "TagIds");
        g.series = Self::join(&self.series, d, "SeriesIds");
        g.specialk = specialk_override(Self::join(&self.features, d, "FeatureIds").as_deref().unwrap_or(""));
        g.platform = Self::join(&self.platforms, d, "PlatformIds");
        g.release_date = s(d, "ReleaseDate").map(normalize_date);
        g.store = s(d, "SourceId").and_then(|i| self.sources.get(i)).cloned();
        g.meta_state = if g.description.is_some() { state::DONE } else { state::NOT_FOUND };

        let launch = if g.source == Source::Steam {
            Launch::Steam
        } else {
            self.launch(d, plugin, &mut g)
        };
        let playnite_installed = d.get("IsInstalled").is_some_and(Value::bool);
        g.installed = match &launch {
            Launch::Steam => false, // Gamut's own Steam scan decides
            Launch::Exe | Launch::Emulator(_) | Launch::Url => playnite_installed || g.exe.is_some(),
            Launch::Unresolved(_) | Launch::None => false,
        };
        Some(Item { cover: self.media(d, "CoverImage"), background: self.media(d, "BackgroundImage"), game: g, launch })
    }

    fn launch(&self, d: &Doc, plugin: &str, g: &mut Game) -> Launch {
        let actions = d.get("GameActions").map(Value::array).unwrap_or_default();
        let play = actions.iter().filter_map(Value::doc).find(|a| a.get("IsPlayAction").is_some_and(Value::bool));
        if let Some(found) = g.install_dir.as_deref().and_then(relocate) {
            g.install_dir = Some(found);
        }
        let install_dir = g.install_dir.clone().unwrap_or_default().trim_end_matches(['\\', '/']).to_string();
        let vars = |v: &str| v.replace("{InstallDir}", &install_dir).replace("{InstallDirName}", dir_name(&install_dir));
        let Some(a) = play else {
            let game_id = s(d, "GameId").unwrap_or_default();
            let uri = match plugin {
                GOG_PLUGIN => format!("goggalaxy://openGameView/{game_id}"),
                UBISOFT_PLUGIN => format!("uplay://launch/{game_id}/0"),
                _ => return Launch::None,
            };
            g.exe = Some(uri);
            return Launch::Url;
        };
        match s(a, "Type").unwrap_or("File") {
            "File" => {
                let Some(path) = s(a, "Path").map(vars) else { return Launch::Unresolved("action has no file".into()) };
                let path = absolute(&path, &install_dir);
                if !Path::new(&path).is_file() {
                    return Launch::Unresolved(format!("file not found: {path}"));
                }
                g.exe = Some(path);
                g.args = s(a, "Arguments").map(vars);
                g.work_dir = s(a, "WorkingDir").map(vars);
                Launch::Exe
            }
            "URL" => match s(a, "Path") {
                Some(u) => {
                    g.exe = Some(u.to_string());
                    Launch::Url
                }
                None => Launch::Unresolved("URL action has no address".into()),
            },
            "Emulator" => match self.emulator(d, a, &install_dir, g) {
                Ok(name) => Launch::Emulator(name),
                Err(e) => Launch::Unresolved(e.to_string()),
            },
            other => Launch::Unresolved(format!("{other} actions aren't supported")),
        }
    }

    /// Turns an emulator action into exe + args + working dir, like Playnite does at launch.
    fn emulator(&self, d: &Doc, a: &Doc, install_dir: &str, g: &mut Game) -> Result<String> {
        let emu = self.emulators.get(s(a, "EmulatorId").unwrap_or_default()).context("emulator was removed")?;
        let emu_name = s(emu, "Name").unwrap_or("emulator").to_string();
        let emu_dir = s(emu, "InstallDir").unwrap_or_default().to_string();
        let profile_id = s(a, "EmulatorProfileId").context("no emulator profile")?;
        let find = |k: &str| {
            emu.get(k).map(Value::array).unwrap_or_default().iter().filter_map(Value::doc).find(|p| s(p, "_id") == Some(profile_id))
        };

        let rom = d
            .get("Roms")
            .map(Value::array)
            .unwrap_or_default()
            .iter()
            .filter_map(Value::doc)
            .find_map(|r| s(r, "Path"))
            .context("no ROM path")?;
        let rom = absolute(&rom.replace("{InstallDir}", install_dir).replace("{EmulatorDir}", &emu_dir), install_dir);
        if !Path::new(&rom).exists() {
            bail!("ROM not found: {rom}");
        }

        let (exe, mut args, work) = if let Some(p) = find("CustomProfiles") {
            let exe = s(p, "Executable").context("custom profile has no executable")?;
            (exe.replace("{EmulatorDir}", &emu_dir), s(p, "Arguments").unwrap_or_default().to_string(), s(p, "WorkingDirectory").map(str::to_string))
        } else if let Some(p) = find("BuiltinProfiles") {
            let builtin = s(emu, "BuiltInConfigId").context("emulator has no built-in definition")?;
            let pname = s(p, "BuiltInProfileName").context("profile has no name")?;
            let def = self.defs.profile(builtin, pname).with_context(|| format!("no definition for {emu_name} / {pname}"))?;
            let exe = find_exe(&emu_dir, &def.exe_pattern)
                .with_context(|| format!("{emu_name}: no file matching {} in {emu_dir}", def.exe_pattern))?;
            let args = if p.get("OverrideDefaultArgs").is_some_and(Value::bool) {
                s(p, "CustomArguments").unwrap_or_default().to_string()
            } else {
                def.args.clone()
            };
            (exe, args, Some(emu_dir.clone()))
        } else {
            bail!("{emu_name}: profile {profile_id} not found");
        };
        if a.get("OverrideDefaultArgs").is_some_and(Value::bool) {
            args = s(a, "Arguments").unwrap_or_default().to_string();
        } else if let Some(extra) = s(a, "AdditionalArguments") {
            args = format!("{args} {extra}");
        }

        let rom_path = Path::new(&rom);
        let image_dir = rom_path.parent().map(|p| p.display().to_string()).unwrap_or_default();
        let expand = |v: &str| {
            v.replace("{ImagePath}", &rom)
                .replace("{ImageName}", &rom_path.file_name().unwrap_or_default().to_string_lossy())
                .replace("{ImageNameNoExt}", &rom_path.file_stem().unwrap_or_default().to_string_lossy())
                .replace("{ImageDir}", &image_dir)
                .replace("{EmulatorDir}", &emu_dir)
                .replace("{InstallDir}", install_dir)
                .replace("{InstallDirName}", dir_name(install_dir))
                .replace("{GameId}", s(d, "GameId").unwrap_or_default())
        };
        let exe = expand(&exe);
        if !Path::new(&exe).is_file() {
            bail!("{emu_name}: {exe} not found");
        }
        if args.contains('{') && expand(&args).contains('{') {
            bail!("{emu_name}: unsupported variable in arguments: {args}");
        }
        g.exe = Some(exe);
        g.args = Some(expand(&args)).filter(|a| !a.trim().is_empty());
        g.work_dir = work.map(|w| expand(&w));
        Ok(emu_name)
    }
}

/// A missing install folder that was moved into a sibling folder (e.g. `Games\X` -> `Games\Ports\X`).
fn relocate(dir: &str) -> Option<String> {
    let p = Path::new(dir.trim_end_matches(['\\', '/']));
    if p.exists() {
        return None;
    }
    let (parent, name) = (p.parent()?, p.file_name()?);
    std::fs::read_dir(parent).ok()?.flatten().map(|e| e.path().join(name)).find(|c| c.is_dir()).map(|c| c.display().to_string())
}

fn dir_name(p: &str) -> &str {
    p.trim_end_matches(['\\', '/']).rsplit(['\\', '/']).next().unwrap_or_default()
}

fn absolute(p: &str, base: &str) -> String {
    if Path::new(p).is_absolute() || base.is_empty() {
        p.to_string()
    } else {
        Path::new(base).join(p).display().to_string()
    }
}

/// First file in `dir` (or one level down) whose name matches Playnite's regex, case-insensitive.
fn find_exe(dir: &str, pattern: &str) -> Option<String> {
    let re = regex::RegexBuilder::new(pattern).case_insensitive(true).build().ok()?;
    let mut dirs = vec![PathBuf::from(dir)];
    for depth in 0..2 {
        let mut next = Vec::new();
        for d in &dirs {
            let Ok(rd) = std::fs::read_dir(d) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    next.push(p);
                } else if re.is_match(&e.file_name().to_string_lossy()) {
                    return Some(p.display().to_string());
                }
            }
        }
        if depth == 0 {
            dirs = next;
        }
    }
    None
}

/// "2009-6-9" -> "2009-06-09"; anything else is kept as is.
fn normalize_date(d: &str) -> String {
    let parts: Vec<&str> = d.split('-').collect();
    match parts.as_slice() {
        [y, m, day] if parts.iter().all(|p| p.chars().all(|c| c.is_ascii_digit())) => {
            format!("{y}-{:0>2}-{:0>2}", m, day)
        }
        _ => d.to_string(),
    }
}

struct EmuProfile {
    args: String,
    exe_pattern: String,
}

/// Playnite's built-in emulator definitions (`Emulation\Emulators\*\emulator.yaml`).
struct EmuDefs(HashMap<(String, String), EmuProfile>);

impl EmuDefs {
    fn load(data_dir: &Path) -> Self {
        let mut map = HashMap::new();
        let mut roots = vec![data_dir.join("Emulation").join("Emulators")];
        if let Some(l) = std::env::var_os("LOCALAPPDATA") {
            roots.push(PathBuf::from(l).join("Playnite").join("Emulation").join("Emulators"));
        }
        for root in roots {
            let Ok(rd) = std::fs::read_dir(root) else { continue };
            for e in rd.flatten() {
                if let Ok(text) = std::fs::read_to_string(e.path().join("emulator.yaml")) {
                    for (id, prof, p) in parse_emulator_yaml(&text) {
                        map.entry((id, prof)).or_insert(p);
                    }
                }
            }
        }
        EmuDefs(map)
    }

    fn profile(&self, id: &str, name: &str) -> Option<&EmuProfile> {
        self.0.get(&(id.to_ascii_lowercase(), name.to_string()))
    }
}

/// Minimal reader for Playnite's emulator.yaml: top-level Id and each profile's name/args/exe.
fn parse_emulator_yaml(text: &str) -> Vec<(String, String, EmuProfile)> {
    let text = text.trim_start_matches('\u{feff}');
    let mut id = String::new();
    let mut out = Vec::new();
    let mut cur: Option<(String, EmuProfile)> = None;
    let flush = |cur: &mut Option<(String, EmuProfile)>, id: &str, out: &mut Vec<_>| {
        if let Some((n, p)) = cur.take() {
            if !p.exe_pattern.is_empty() {
                out.push((id.to_ascii_lowercase(), n, p));
            }
        }
    };
    for line in text.lines() {
        let t = line.trim();
        if let Some(v) = line.strip_prefix("Id:") {
            id = yaml_scalar(v);
        } else if let Some(v) = t.strip_prefix("- Name:") {
            flush(&mut cur, &id, &mut out);
            cur = Some((yaml_scalar(v), EmuProfile { args: String::new(), exe_pattern: String::new() }));
        } else if let Some((_, p)) = cur.as_mut() {
            if let Some(v) = t.strip_prefix("StartupArguments:") {
                p.args = yaml_scalar(v);
            } else if let Some(v) = t.strip_prefix("StartupExecutable:") {
                p.exe_pattern = yaml_scalar(v);
            }
        }
    }
    flush(&mut cur, &id, &mut out);
    out
}

fn yaml_scalar(v: &str) -> String {
    let v = v.trim();
    if let Some(inner) = v.strip_prefix('\'').and_then(|x| x.strip_suffix('\'')) {
        inner.replace("''", "'")
    } else if let Some(inner) = v.strip_prefix('"').and_then(|x| x.strip_suffix('"')) {
        inner.replace("\\\"", "\"").replace("\\\\", "\\")
    } else {
        v.to_string()
    }
}

/// What an import would do, grouped for display.
#[derive(Debug, Clone, Default)]
pub struct Summary {
    pub games: usize,
    pub covers: usize,
    pub backgrounds: usize,
    /// (group, games) that Gamut can start.
    pub playable: Vec<(String, usize)>,
    /// (group, games) imported without a way to start them.
    pub unplayable: Vec<(String, usize)>,
    /// (reason, games) for launches that couldn't be resolved.
    pub reasons: Vec<(String, usize)>,
}

pub fn summarize(items: &[Item]) -> Summary {
    use std::collections::BTreeMap;
    let (mut ok, mut no, mut why): (BTreeMap<String, usize>, BTreeMap<String, usize>, BTreeMap<String, usize>) =
        Default::default();
    for it in items {
        let store = it.game.store.as_deref();
        match &it.launch {
            Launch::Steam => *ok.entry("Steam".into()).or_default() += 1,
            Launch::Exe => *ok.entry("PC games (exe)".into()).or_default() += 1,
            Launch::Emulator(e) => *ok.entry(e.clone()).or_default() += 1,
            Launch::Url => *ok.entry(format!("{} (opens launcher)", store.unwrap_or("Link"))).or_default() += 1,
            Launch::None => *no.entry(format!("{} library", store.unwrap_or("Other"))).or_default() += 1,
            Launch::Unresolved(r) => {
                *no.entry(it.game.platform.clone().unwrap_or_else(|| "Unknown platform".into())).or_default() += 1;
                let key = match r.split(": ").next().unwrap_or(r) {
                    "ROM not found" => "ROM file is missing",
                    "file not found" => "Game files are missing",
                    k => k,
                };
                *why.entry(key.to_string()).or_default() += 1;
            }
        }
    }
    let sorted = |m: BTreeMap<String, usize>| {
        let mut v: Vec<_> = m.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        v
    };
    Summary {
        games: items.len(),
        covers: items.iter().filter(|i| i.cover.is_some()).count(),
        backgrounds: items.iter().filter(|i| i.background.is_some()).count(),
        playable: sorted(ok),
        unplayable: sorted(no),
        reasons: sorted(why),
    }
}

/// Plain-text version of [`summarize`] for the CLI.
pub fn report(items: &[Item]) -> String {
    let s = summarize(items);
    let mut out = format!("{} games, {} covers, {} backgrounds
Can be started:
", s.games, s.covers, s.backgrounds);
    for (k, n) in &s.playable {
        out += &format!("  {n:>5}  {k}
");
    }
    out += "Imported, but can't be started:
";
    for (k, n) in &s.unplayable {
        out += &format!("  {n:>5}  {k}
");
    }
    out += "Why:
";
    for (k, n) in &s.reasons {
        out += &format!("  {n:>5}  {k}
");
    }
    out
}

/// Writes the import: one transaction for rows, then covers/backgrounds on worker threads.
pub fn import(lib: &mut Library, paths: &Paths, items: &[Item], progress: &(dyn Fn(usize, usize) + Sync)) -> Result<ImportStats> {
    let games: Vec<Game> = items.iter().map(|i| i.game.clone()).collect();
    let ids = lib.import_playnite(&games)?;
    let jobs: Vec<(i64, &Item)> = ids.iter().copied().zip(items).filter(|(_, i)| i.cover.is_some() || i.background.is_some()).collect();
    let total = jobs.len();
    let done = std::sync::atomic::AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(8);
    let chunk = jobs.len().div_ceil(threads).max(1);
    let results: Vec<(i64, bool, bool, bool)> = std::thread::scope(|sc| {
        let hs: Vec<_> = jobs
            .chunks(chunk)
            .map(|part| {
                let done = &done;
                sc.spawn(move || {
                    part.iter()
                        .map(|(id, it)| {
                            let mut weak = false;
                            let cover = it.cover.as_ref().is_some_and(|p| {
                                std::fs::read(p).is_ok_and(|b| {
                                    weak = !images::is_cover_shaped(&b);
                                    images::store_cover(&b, &paths.cover(*id), &paths.thumb(*id)).is_ok()
                                })
                            });
                            let hero = it.background.as_ref().is_some_and(|p| {
                                std::fs::read(p).is_ok_and(|b| images::store_hero(&b, &paths.hero(*id)).is_ok())
                            });
                            let n = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                            progress(n, total);
                            (*id, cover, hero, weak)
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        hs.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
    });
    lib.apply_imported_art(&results)?;
    Ok(ImportStats {
        games: ids.len(),
        covers: results.iter().filter(|r| r.1).count(),
        backgrounds: results.iter().filter(|r| r.2).count(),
        ids,
    })
}

pub struct ImportStats {
    pub games: usize,
    pub covers: usize,
    pub backgrounds: usize,
    pub ids: Vec<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_profiles() {
        let y = "\u{feff}Id: retroarch\nName: RetroArch\nProfiles:\n  - Name: mGBA\n    StartupArguments: '-L \".\\cores\\mgba_libretro.dll\" \"{ImagePath}\"'\n    StartupExecutable: ^retroarch\\.exe$\n  - Name: Script\n    ScriptGameImport: true\n";
        let p = parse_emulator_yaml(y);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].0, "retroarch");
        assert_eq!(p[0].1, "mGBA");
        assert_eq!(p[0].2.args, "-L \".\\cores\\mgba_libretro.dll\" \"{ImagePath}\"");
        assert_eq!(p[0].2.exe_pattern, "^retroarch\\.exe$");
    }

    #[test]
    fn dates_and_dirs() {
        assert_eq!(normalize_date("2009-6-9"), "2009-06-09");
        assert_eq!(normalize_date("2009"), "2009");
        assert_eq!(dir_name("D:\\Games\\Foo\\"), "Foo");
    }
}

/// Special K Helper's per-game features; VAC games default to never (like its "stop if VAC" option).
fn specialk_override(features: &str) -> Option<bool> {
    let has = |n: &str| features.split(", ").any(|f| f.eq_ignore_ascii_case(n));
    if has("[SK] Selective Mode Enable") {
        Some(true)
    } else if has("[SK] Global Mode Disable") || has("Valve Anti-Cheat Enabled") {
        Some(false)
    } else {
        None
    }
}
