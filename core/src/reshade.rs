//! Installs ReShade into a game's folder on first launch, like ReShade's own installer, with the
//! Standard effects + SweetFX shader packages shared from Gamut's data folder (docs/reshade.md).
use crate::model::Game;
use crate::procs;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// Install ReShade into every game on launch (unless turned off for it).
    pub enabled: bool,
    pub skip_anticheat: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { enabled: false, skip_anticheat: true }
    }
}

/// Shader packages ReShade's installer ticks by default.
const PACKAGES: [&str; 2] = [
    "https://github.com/crosire/reshade-shaders/archive/refs/heads/slim.zip",
    "https://github.com/CeeJayDK/SweetFX/archive/refs/heads/master.zip",
];
/// Lists the files Gamut added to a game folder, so they can be removed again.
const MARKER: &str = "gamut-reshade.txt";
/// Files that mean the game already has its own ReShade (or another proxy DLL) set up.
const EXISTING: [&str; 9] =
    ["reshade.ini", "reshadepreset.ini", "reshade-shaders", "reshade.log", "dxgi.dll", "d3d9.dll", "d3d11.dll", "opengl32.dll", "reshade.asi"];

/// What happened for a launch.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    Installed(PathBuf),
    AlreadyInstalled,
    Skipped(String),
}

/// Whether ReShade should be installed into this game; Err holds why not.
pub fn decide(g: &Game, s: &Settings) -> std::result::Result<(), String> {
    match (s.enabled, g.reshade) {
        (_, Some(false)) => return Err("ReShade is turned off for this game".into()),
        (false, None) => return Err("ReShade is off".into()),
        _ => {}
    }
    if !g.installed {
        return Err("not installed".into());
    }
    if s.skip_anticheat {
        if let Some(f) = procs::game_dirs(g).iter().find_map(|d| crate::specialk::find_anticheat(d, 4)) {
            return Err(format!("anti-cheat found ({})", f.display()));
        }
    }
    Ok(())
}

/// Version of the downloaded ReShade, if any.
pub fn version(cache: &Path) -> Option<String> {
    std::fs::read_to_string(cache.join("version.txt")).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn ready(cache: &Path) -> bool {
    cache.join("ReShade64.dll").is_file() && cache.join("ReShade32.dll").is_file() && cache.join(r"reshade-shaders\Shaders").is_dir()
}

/// Downloads ReShade and the shader packages into `cache` (when missing, or always with `force`).
pub fn download(cache: &Path, force: bool) -> Result<String> {
    if !force && ready(cache) {
        return Ok(version(cache).unwrap_or_default());
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(600)))
        .user_agent("gamut/0.1")
        .build()
        .into();
    let get = |url: &str| -> Result<Vec<u8>> {
        Ok(agent.get(url).call()?.body_mut().with_config().limit(256 * 1024 * 1024).read_to_vec()?)
    };
    let page = String::from_utf8_lossy(&get("https://reshade.me/").context("opening reshade.me")?).to_string();
    let re = regex::Regex::new(r#"/downloads/ReShade_Setup_([0-9.]+)\.exe"#).unwrap();
    let Some(c) = re.captures(&page) else { bail!("couldn't find the ReShade download on reshade.me") };
    let ver = c[1].to_string();
    let setup = get(&format!("https://reshade.me{}", &c[0])).context("downloading ReShade")?;
    // The setup is an exe with a zip of the DLLs appended.
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(setup)).context("reading the ReShade setup")?;
    let staging = cache.with_extension("new");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    for dll in ["ReShade64.dll", "ReShade32.dll"] {
        let mut f = zip.by_name(dll).with_context(|| format!("{dll} missing from the setup"))?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)?;
        std::fs::write(staging.join(dll), buf)?;
    }
    for url in PACKAGES {
        let bytes = get(url).with_context(|| format!("downloading {url}"))?;
        extract_package(&bytes, &staging.join("reshade-shaders"))?;
    }
    std::fs::write(staging.join("version.txt"), &ver)?;
    let _ = std::fs::remove_dir_all(cache);
    std::fs::rename(&staging, cache)?;
    log::info!("ReShade {ver} downloaded with Standard effects + SweetFX");
    Ok(ver)
}

/// Copies a GitHub archive's Shaders/ and Textures/ folders into `dest`.
fn extract_package(bytes: &[u8], dest: &Path) -> Result<()> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
    for i in 0..zip.len() {
        let mut f = zip.by_index(i)?;
        let Some(path) = f.enclosed_name() else { continue };
        // "<repo>-<branch>/Shaders/..." -> "Shaders/..."
        let rel: PathBuf = path.components().skip(1).collect();
        let top = rel.components().next().map(|c| c.as_os_str().to_string_lossy().to_string());
        if !matches!(top.as_deref(), Some("Shaders" | "Textures")) || f.is_dir() {
            continue;
        }
        let out = dest.join(&rel);
        std::fs::create_dir_all(out.parent().unwrap())?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)?;
        std::fs::write(out, buf)?;
    }
    Ok(())
}

/// How ReShade hooks a game.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Hook {
    /// Copied next to the exe under this DLL name.
    Proxy(&'static str),
    /// Loaded by the Vulkan loader as an implicit layer; a ReShade.ini next to the exe enables it.
    Vulkan,
}

/// How ReShade must hook this executable (and its bitness), from the graphics APIs it imports.
/// Engines often load the API from a DLL next to the exe (UnityPlayer.dll), so those count too.
pub fn hook_for(exe: &Path) -> Option<(bool, Hook)> {
    let (is64, mut names) = procs::pe_imports(exe)?;
    let graphics = |n: &[String]| n.iter().any(|x| x.starts_with("d3d") || x == "dxgi.dll" || x == "opengl32.dll" || x == "vulkan-1.dll");
    if !graphics(&names) {
        if let Some(dir) = exe.parent() {
            for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let p = e.path();
                if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dll")) {
                    if let Some((_, n)) = procs::pe_imports(&p) {
                        names.extend(n);
                    }
                }
            }
        }
    }
    let has = |n: &str| names.iter().any(|x| x == n);
    let hook = if ["d3d12.dll", "d3d11.dll", "d3d10.dll", "d3d10_1.dll", "dxgi.dll"].iter().any(|n| has(n)) {
        Hook::Proxy("dxgi.dll")
    } else if has("d3d9.dll") {
        Hook::Proxy("d3d9.dll")
    } else if has("opengl32.dll") {
        Hook::Proxy("opengl32.dll")
    } else if has("vulkan-1.dll") {
        Hook::Vulkan
    } else {
        // Unknown: most games today are DirectX 10-12.
        Hook::Proxy("dxgi.dll")
    };
    Some((is64, hook))
}

fn has_existing(dir: &Path) -> Option<String> {
    std::fs::read_dir(dir).ok()?.flatten().find_map(|e| {
        let n = e.file_name().to_string_lossy().to_lowercase();
        EXISTING.contains(&n.as_str()).then_some(n)
    })
}

/// Installs ReShade next to the game's executable, unless it already has its own.
pub fn install(g: &Game, cache: &Path) -> Result<Outcome> {
    let Some(exe) = procs::game_exe(g) else { return Ok(Outcome::Skipped("couldn't find the game's executable".into())) };
    let dir = exe.parent().context("exe has no folder")?;
    if dir.join(MARKER).exists() {
        return Ok(Outcome::AlreadyInstalled);
    }
    let roots: Vec<PathBuf> = std::iter::once(dir.to_path_buf()).chain(g.install_dir.as_deref().map(PathBuf::from)).collect();
    if let Some(f) = roots.iter().find_map(|d| has_existing(d)) {
        return Ok(Outcome::Skipped(format!("already has its own setup ({f})")));
    }
    let Some((is64, hook)) = hook_for(&exe) else { return Ok(Outcome::Skipped("couldn't read the executable".into())) };
    download(cache, false)?;
    let mut added = Vec::new();
    match hook {
        Hook::Proxy(dll) => {
            std::fs::copy(cache.join(if is64 { "ReShade64.dll" } else { "ReShade32.dll" }), dir.join(dll))
                .with_context(|| format!("copying ReShade into {}", dir.display()))?;
            added.push(dll);
        }
        Hook::Vulkan => vulkan::ensure_layer(cache)?,
    }
    let shaders = cache.join("reshade-shaders");
    std::fs::write(
        dir.join("ReShade.ini"),
        format!(
            "[GENERAL]\r\nEffectSearchPaths={}\\Shaders\\**\r\nTextureSearchPaths={}\\Textures\\**\r\nPresetPath=.\\ReShadePreset.ini\r\n",
            shaders.display(),
            shaders.display()
        ),
    )?;
    // Effects start switched off, like a fresh ReShade install; pick them in the overlay (Home).
    std::fs::write(dir.join("ReShadePreset.ini"), "Techniques=\r\nTechniqueSorting=\r\n")?;
    added.extend(["ReShade.ini", "ReShadePreset.ini", "ReShade.log"]);
    std::fs::write(dir.join(MARKER), added.join("\r\n") + "\r\n")?;
    let how = match hook {
        Hook::Proxy(dll) => dll.to_string(),
        Hook::Vulkan => "Vulkan layer".into(),
    };
    log::info!("ReShade: installed into {} as {how} ({}-bit)", dir.display(), if is64 { 64 } else { 32 });
    Ok(Outcome::Installed(dir.join("ReShade.ini")))
}

/// Folder where Gamut installed ReShade for this game, if it did.
pub fn installed_dir(g: &Game) -> Option<PathBuf> {
    let exe = procs::game_exe(g)?;
    let dir = exe.parent()?;
    dir.join(MARKER).is_file().then(|| dir.to_path_buf())
}

/// Removes the files Gamut added (the preset included).
pub fn uninstall(g: &Game) -> Result<bool> {
    let Some(dir) = installed_dir(g) else { return Ok(false) };
    let list = std::fs::read_to_string(dir.join(MARKER))?;
    for f in list.lines().map(str::trim).filter(|f| !f.is_empty() && !f.contains(['\\', '/'])) {
        let _ = std::fs::remove_file(dir.join(f));
    }
    std::fs::remove_file(dir.join(MARKER))?;
    log::info!("ReShade: removed from {}", dir.display());
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("gamut-rs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn installs_once_and_respects_existing_setups() {
        let cache = temp("cache");
        std::fs::write(cache.join("ReShade64.dll"), b"x64").unwrap();
        std::fs::write(cache.join("ReShade32.dll"), b"x86").unwrap();
        std::fs::create_dir_all(cache.join(r"reshade-shaders\Shaders")).unwrap();
        let game = temp("game");
        // A real PE to read imports from: this test binary.
        let exe = game.join("Game.exe");
        std::fs::copy(std::env::current_exe().unwrap(), &exe).unwrap();
        let mut g = Game::new_custom("T".into(), Some(exe.to_string_lossy().into()));
        g.reshade = Some(true);
        assert!(decide(&g, &Settings::default()).is_ok());
        assert!(matches!(install(&g, &cache).unwrap(), Outcome::Installed(_)));
        assert_eq!(std::fs::read(game.join("dxgi.dll")).unwrap(), b"x64");
        assert!(std::fs::read_to_string(game.join("ReShade.ini")).unwrap().contains("EffectSearchPaths="));
        assert_eq!(install(&g, &cache).unwrap(), Outcome::AlreadyInstalled);
        assert!(uninstall(&g).unwrap());
        assert!(!game.join("dxgi.dll").exists() && !game.join("ReShade.ini").exists());
        // A game with its own ReShade is left alone.
        std::fs::write(game.join("ReShade.ini"), "[GENERAL]").unwrap();
        assert!(matches!(install(&g, &cache).unwrap(), Outcome::Skipped(_)));
        assert_eq!(std::fs::read_to_string(game.join("ReShade.ini")).unwrap(), "[GENERAL]");
        let _ = std::fs::remove_dir_all(&cache);
        let _ = std::fs::remove_dir_all(&game);
    }

    #[test]
    fn extracts_shader_folders() {
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let o = zip::write::SimpleFileOptions::default();
            for (p, d) in [("SweetFX-master/Shaders/SweetFX/A.fx", "a"), ("SweetFX-master/Textures/t.png", "t"), ("SweetFX-master/README.md", "r")] {
                w.start_file(p, o).unwrap();
                std::io::Write::write_all(&mut w, d.as_bytes()).unwrap();
            }
            w.finish().unwrap();
        }
        let dest = temp("pkg");
        extract_package(&buf, &dest).unwrap();
        assert!(dest.join(r"Shaders\SweetFX\A.fx").is_file());
        assert!(dest.join(r"Textures\t.png").is_file());
        assert!(!dest.join("README.md").exists());
        let _ = std::fs::remove_dir_all(&dest);
    }
}

/// ReShade's Vulkan layer. The DLL only activates in games with a ReShade.ini next to their exe,
/// so registering it once is safe; per-game setup is just that ini.
pub mod vulkan {
    use anyhow::{Context, Result};
    use std::path::{Path, PathBuf};

    const KEY: &str = r"SOFTWARE\Khronos\Vulkan\ImplicitLayers";
    const NAME: &str = "VK_LAYER_reshade_gamut";

    /// Manifest path of a ReShade layer already registered for this user or machine.
    pub fn registered() -> Option<PathBuf> {
        use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
        for hive in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
            let Ok(k) = winreg::RegKey::predef(hive).open_subkey(KEY) else { continue };
            for (name, _) in k.enum_values().flatten() {
                let p = PathBuf::from(&name);
                let file = p.file_name().map(|f| f.to_string_lossy().to_lowercase()).unwrap_or_default();
                if file.starts_with("reshade64") && file.ends_with(".json") && p.is_file() {
                    return Some(p);
                }
            }
        }
        None
    }

    fn manifest(bits: u32) -> String {
        format!(
            r#"{{
	"file_format_version": "1.0.0",
	"layer": {{
		"name": "{NAME}",
		"type": "GLOBAL",
		"library_path": ".\\ReShade{bits}.dll",
		"api_version": "1.3.268",
		"implementation_version": "1",
		"description": "ReShade post-processing injector ({bits}-bit, set up by Gamut)",
		"device_extensions": [
			{{ "name": "VK_EXT_tooling_info", "spec_version": "1", "entrypoints": [ "vkGetPhysicalDeviceToolPropertiesEXT" ] }}
		],
		"disable_environment": {{ "DISABLE_VK_LAYER_reshade_gamut_1": "1" }}
	}}
}}
"#
        )
    }

    /// Uses an existing ReShade layer (e.g. from ReShade's installer), else registers Gamut's own
    /// copy for the current user (HKCU, no admin needed).
    pub fn ensure_layer(cache: &Path) -> Result<()> {
        if let Some(p) = registered() {
            log::info!("ReShade: using the Vulkan layer at {}", p.display());
            return Ok(());
        }
        let dir = cache.join("vulkan");
        std::fs::create_dir_all(&dir)?;
        let (k, _) = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER).create_subkey(KEY).context("opening the Vulkan layer key")?;
        for bits in [64, 32] {
            let dll = format!("ReShade{bits}.dll");
            std::fs::copy(cache.join(&dll), dir.join(&dll))?;
            let json = dir.join(format!("ReShade{bits}.json"));
            std::fs::write(&json, manifest(bits))?;
            k.set_value(json.to_string_lossy().as_ref(), &0u32)?;
        }
        log::info!("ReShade: registered Gamut's Vulkan layer for this user ({})", dir.display());
        Ok(())
    }
}

#[cfg(test)]
mod vulkan_tests {
    #[test]
    fn finds_registered_layer() {
        // Read-only: reports an existing ReShade Vulkan layer, if any.
        println!("ReShade Vulkan layer: {:?}", super::vulkan::registered());
    }
}
