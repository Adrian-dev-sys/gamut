//! Files Special K reads at injection: defaults for new profiles, ReShade presets, steam_appid.txt.
use super::Settings;
use crate::model::{Game, Source};
use anyhow::Result;
use std::path::Path;

/// Minimal INI editor that keeps the rest of the file (comments, order, BOM) untouched.
struct Ini {
    lines: Vec<String>,
    bom: bool,
    changed: bool,
}

impl Ini {
    fn load(path: &Path) -> Ini {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let bom = text.starts_with('\u{feff}');
        let lines = text.trim_start_matches('\u{feff}').lines().map(str::to_string).collect();
        Ini { lines, bom, changed: false }
    }

    #[cfg(test)]
    fn parse(text: &str) -> Ini {
        Ini { lines: text.lines().map(str::to_string).collect(), bom: false, changed: false }
    }

    /// Line range of `[section]`'s body.
    fn section(&self, name: &str) -> Option<(usize, usize)> {
        let start = self.lines.iter().position(|l| l.trim().eq_ignore_ascii_case(&format!("[{name}]")))? + 1;
        let end = self.lines[start..].iter().position(|l| l.trim_start().starts_with('[')).map_or(self.lines.len(), |i| start + i);
        Some((start, end))
    }

    fn key_line(&self, section: &str, key: &str) -> Option<usize> {
        let (a, b) = self.section(section)?;
        (a..b).find(|&i| self.lines[i].split_once('=').is_some_and(|(k, _)| k.trim().eq_ignore_ascii_case(key)))
    }

    fn get(&self, section: &str, key: &str) -> Option<String> {
        self.key_line(section, key).and_then(|i| self.lines[i].split_once('=')).map(|(_, v)| v.trim().to_string())
    }

    fn set(&mut self, section: &str, key: &str, value: &str) {
        let line = format!("{key}={value}");
        if let Some(i) = self.key_line(section, key) {
            if self.lines[i].split_once('=').map(|(_, v)| v.trim()) != Some(value) {
                self.lines[i] = line;
                self.changed = true;
            }
            return;
        }
        match self.section(section) {
            Some((a, b)) => {
                // After the section's last non-blank line.
                let at = (a..b).rev().find(|&i| !self.lines[i].trim().is_empty()).map_or(a, |i| i + 1);
                self.lines.insert(at, line);
            }
            None => {
                if self.lines.last().is_some_and(|l| !l.trim().is_empty()) {
                    self.lines.push(String::new());
                }
                self.lines.push(format!("[{section}]"));
                self.lines.push(line);
            }
        }
        self.changed = true;
    }

    fn remove(&mut self, section: &str, key: &str) {
        if let Some(i) = self.key_line(section, key) {
            self.lines.remove(i);
            self.changed = true;
        }
    }

    fn text(&self) -> String {
        let mut s = if self.bom { "\u{feff}".to_string() } else { String::new() };
        s.push_str(&self.lines.join("\r\n"));
        s.push_str("\r\n");
        s
    }

    /// Writes only if something changed; returns whether it did.
    fn save(&self, path: &Path) -> Result<bool> {
        if !self.changed {
            return Ok(false);
        }
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(path, self.text())?;
        Ok(true)
    }
}

const RESHADE: &str = r"..\..\PlugIns\ThirdParty\ReShade";

/// Applies the "new profile" options to `Global\default_SpecialK.ini` (the template SK copies
/// into each game's profile the first time it injects).
pub fn write_defaults(sk_dir: &Path, s: &Settings) -> Result<bool> {
    let path = sk_dir.join(r"Global\default_SpecialK.ini");
    let mut ini = Ini::load(&path);
    apply_defaults(&mut ini, s);
    ini.save(&path)
}

fn apply_defaults(ini: &mut Ini, s: &Settings) {
    let b = |v: bool| if v { "true" } else { "false" };
    ini.set("Steam.System", "PreLoadSteamOverlay", b(s.steam_overlay));
    // Playnite's helper wrote this literal by mistake; a bogus AppID would apply to every new profile.
    if ini.get("Steam.System", "AppID").as_deref() == Some("steamId") {
        ini.remove("Steam.System", "AppID");
    }
    if s.reshade {
        ini.set("Render.FrameRate", "SleeplessRenderThread", "false");
        ini.set("Render.OSD", "ShowInVideoCapture", "false");
    }
    for (sec, arch, dll) in [("Import.ReShade64", "x64", "ReShade64.dll"), ("Import.ReShade32", "Win32", "ReShade32.dll")] {
        if s.reshade {
            ini.set(sec, "Architecture", arch);
            ini.set(sec, "Role", "ThirdParty");
            ini.set(sec, "When", "PlugIn");
            ini.set(sec, "Filename", &format!(r"{RESHADE}\{dll}"));
        } else if ini.section(sec).is_some() {
            ini.set(sec, "Filename", "");
        }
    }
    let fps = if s.fps_limit > 0.0 { format!("{:.1}", s.fps_limit) } else { "0.0".into() };
    ini.set("Render.FrameRate", "TargetFPS", &fps);
    ini.set("Compatibility.General", "DisableBloatWare_NVIDIA", b(s.disable_nvidia_bloat));
    ini.set("Render.DXGI", "UseFlipDiscard", b(s.flip_model));
}

/// Shipped ReShade effects, used when no shaders are installed to read techniques from.
const DEFAULT_SORTING: &str = "AdvancedCRT@CRT.fx,ASCII@ASCII.fx,at@UIMask.fx,Before@Splitscreen.fx,Border@Border.fx,CA@ChromaticAberration.fx,Cartoon@Cartoon.fx,ColorMatrix@ColorMatrix.fx,ContrastAdaptiveSharpen@CAS.fx,Curves@Curves.fx,Daltonize@Daltonize.fx,Deband@Deband.fx,DisplayDepth@DisplayDepth.fx,DPX@DPX.fx,FilmGrain@FilmGrain.fx,FXAA@FXAA.fx,HDR@FakeHDR.fx,Layer@Layer.fx,Levels@Levels.fx,LiftGammaGain@LiftGammaGain.fx,LumaSharpen@LumaSharpen.fx,LUT@LUT.fx,Monochrome@Monochrome.fx,Nostalgia@Nostalgia.fx,SMAA@SMAA.fx,Technicolor@Technicolor.fx,Technicolor2@Technicolor2.fx,Tint@Sepia.fx,Tonemap@Tonemap.fx,Vibrance@Vibrance.fx,Vignette@Vignette.fx";

/// Preset file name for a game: Playnite's id when imported (reuses its presets), else Gamut's.
pub fn preset_id(g: &Game) -> String {
    g.playnite_id.clone().unwrap_or_else(|| format!("gamut-{}", g.id))
}

/// Gives the game its own ReShade preset (created once) and points ReShade.ini at it.
pub fn reshade_preset(sk_dir: &Path, g: &Game) -> Result<()> {
    let dir = sk_dir.join(r"PlugIns\ThirdParty\ReShade");
    if !dir.is_dir() {
        return Ok(());
    }
    let id = preset_id(g);
    let preset = dir.join("reshade-presets").join(format!("{id}.ini"));
    if !preset.exists() {
        let mut techniques = techniques(&dir.join(r"reshade-shaders\Shaders"));
        techniques.sort();
        techniques.dedup();
        let sorting = if techniques.is_empty() { DEFAULT_SORTING.to_string() } else { techniques.join(",") };
        std::fs::create_dir_all(preset.parent().unwrap())?;
        std::fs::write(&preset, format!("PreprocessorDefinitions=\r\nTechniques=\r\nTechniqueSorting={sorting}\r\n"))?;
        log::info!("Special K: created ReShade preset {}", preset.display());
    }
    let path = dir.join("ReShade.ini");
    let mut ini = Ini::load(&path);
    ini.set("GENERAL", "PresetPath", &format!(r".\reshade-presets\{id}.ini"));
    for k in ["ForceVSync", "ForceWindowed", "ForceFullscreen", "Force10BitFormat"] {
        ini.set("APP", k, "0");
    }
    ini.set("APP", "ForceResolution", "0,0");
    ini.save(&path)?;
    Ok(())
}

/// "technique@file.fx" for every technique in the shader folder.
fn techniques(dir: &Path) -> Vec<String> {
    let re = regex::Regex::new(r"(?m)^\s*technique\s+([^\s<{(]+)").unwrap();
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(techniques(&p));
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("fx")) {
            let file = e.file_name().to_string_lossy().to_string();
            if let Ok(text) = std::fs::read_to_string(&p) {
                out.extend(re.captures_iter(&text).map(|c| format!("{}@{file}", &c[1])));
            }
        }
    }
    out
}

/// Non-Steam games get a steam_appid.txt (when missing and their Steam id is known), so SK can
/// load the Steam overlay into them.
pub fn steam_appid(g: &Game) -> Result<()> {
    let Some(app) = (g.source != Source::Steam).then(|| g.steam_appid()).flatten() else { return Ok(()) };
    let dir = g
        .install_dir
        .as_deref()
        .filter(|d| !d.trim().is_empty())
        .map(Path::new)
        .or_else(|| g.exe.as_deref().map(Path::new).and_then(Path::parent))
        .filter(|d| d.is_dir());
    let Some(dir) = dir else { return Ok(()) };
    let file = dir.join("steam_appid.txt");
    if !file.exists() {
        std::fs::write(&file, app.to_string())?;
        log::info!("Special K: wrote {} ({app})", file.display());
    }
    Ok(())
}

/// SK only hooks non-store games whose path matches a `Global\whitelist.ini` line (a regex);
/// adds the exe's folder when none matches. Returns the added line.
pub fn whitelist(sk_dir: &Path, g: &Game) -> Result<Option<String>> {
    let Some(exe) = (g.source != Source::Steam).then_some(g.exe.as_deref()).flatten().map(Path::new) else { return Ok(None) };
    if !exe.is_absolute() {
        return Ok(None);
    }
    let file = sk_dir.join("Global").join("whitelist.ini");
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    let Some(line) = whitelist_line(&text, exe) else { return Ok(None) };
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let sep = if text.is_empty() || text.ends_with('\n') { "" } else { nl };
    std::fs::write(&file, format!("{text}{sep}{line}"))?;
    Ok(Some(line))
}

/// The line to add for `exe`, or None when an existing line already matches it.
fn whitelist_line(text: &str, exe: &Path) -> Option<String> {
    let path = exe.to_string_lossy();
    let hit = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .any(|l| regex::RegexBuilder::new(l).case_insensitive(true).build().is_ok_and(|re| re.is_match(&path)));
    if hit {
        return None;
    }
    // ECMAScript-escaped folder: SK reads the lines with std::wregex.
    let dir = exe.parent()?.to_string_lossy();
    Some(dir.chars().fold(String::new(), |mut s, c| {
        if r"\^$.|?*+()[]{}".contains(c) {
            s.push('\\');
        }
        s.push(c);
        s
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitelists_unmatched_folders() {
        let text = "Jocuri\\\\SILENT HILL f\r\nXCX-Setup\\\\extracted\r\n";
        let xcx = Path::new(r"D:\Jocuri\Recomps\XCX-Setup\extracted\xcx.exe");
        assert_eq!(whitelist_line(text, xcx), None);
        let xc2 = Path::new(r"D:\Jocuri\Recomps (PC)\XC2-Setup\extracted\xc2.exe");
        let line = whitelist_line(text, xc2).unwrap();
        assert_eq!(line, r"D:\\Jocuri\\Recomps \(PC\)\\XC2-Setup\\extracted");
        assert_eq!(whitelist_line(&format!("{text}{line}"), xc2), None);
    }

    #[test]
    fn ini_edits_keep_other_lines() {
        let mut ini = Ini::parse("; c\n[Steam.System]\nPreLoadSteamOverlay=false\nAppID=steamId\n\n[Other]\nX=1\n");
        let s = Settings { fps_limit: 117.0, ..Default::default() };
        apply_defaults(&mut ini, &s);
        let t = ini.text();
        assert!(t.starts_with("; c\r\n[Steam.System]\r\nPreLoadSteamOverlay=true\r\n"));
        assert!(!t.contains("AppID"));
        assert!(t.contains("[Other]\r\nX=1"));
        assert!(t.contains("TargetFPS=117.0"));
        assert!(t.contains(r"Filename=..\..\PlugIns\ThirdParty\ReShade\ReShade64.dll"));
        // Second pass changes nothing.
        let mut again = Ini::parse(&t);
        apply_defaults(&mut again, &s);
        assert!(!again.changed);
    }

    #[test]
    fn reads_installed_version() {
        // Read-only check against a real install, when there is one.
        if let Some(d) = super::super::install_dir("") {
            let v = super::super::update::installed_version(&d);
            println!("installed Special K: {v:?}");
            assert!(v.is_some());
        }
    }

    #[test]
    fn finds_techniques() {
        let dir = std::env::temp_dir().join(format!("gamut-fx-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("A.fx"), "technique Sharpen <ui_label=\"x\";>\n{\n}\ntechnique Two\n").unwrap();
        std::fs::write(dir.join("sub").join("B.fx"), "  technique Blur\n").unwrap();
        let mut t = techniques(&dir);
        t.sort();
        assert_eq!(t, ["Blur@B.fx", "Sharpen@A.fx", "Two@A.fx"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
