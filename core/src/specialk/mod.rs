//! Special K global injection helper: starts SK's injection service before a game launches and
//! stops it again afterwards (see docs/specialk.md).
use crate::model::Game;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

pub mod profile;
pub mod update;

/// User settings, stored as `[specialk]` in config.toml.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// "off", "global" (every game unless turned off for it) or "selective" (only games turned on).
    pub mode: String,
    /// When to stop the service: "injection" (once SK is in the game), "exit" (game closed), "never".
    pub stop: String,
    /// Skip games that ship EasyAntiCheat or BattlEye.
    pub skip_anticheat: bool,
    /// Special K folder; empty = read from the registry.
    pub path: String,
    /// Defaults written to Global\default_SpecialK.ini for new profiles.
    pub steam_overlay: bool,
    pub reshade: bool,
    pub flip_model: bool,
    /// Frame rate limit for new profiles; 0 = none.
    pub fps_limit: f32,
    pub disable_nvidia_bloat: bool,
    /// Let non-Steam games use the Big Picture overlay while Steam runs in Big Picture.
    pub bpm_overlay: bool,
    pub check_updates: bool,
    /// Update channel: "Website", "Discord" or "Ancient".
    pub channel: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            mode: "off".into(),
            stop: "injection".into(),
            skip_anticheat: true,
            path: String::new(),
            steam_overlay: true,
            reshade: true,
            flip_model: true,
            fps_limit: 0.0,
            disable_nvidia_bloat: false,
            bpm_overlay: false,
            check_updates: true,
            channel: "Website".into(),
        }
    }
}

/// Whether SK should be injected into this game; Err holds why not (for the log / a toast).
pub fn decide(g: &Game, s: &Settings) -> std::result::Result<(), String> {
    match (s.mode.as_str(), g.specialk) {
        ("off", _) => return Err("Special K is off".into()),
        (_, Some(false)) => return Err("Special K is turned off for this game".into()),
        ("selective", None) => return Err("not turned on for this game (selective mode)".into()),
        _ => {}
    }
    if !g.installed {
        return Err("not installed".into());
    }
    if s.skip_anticheat {
        if let Some(f) = game_dirs(g).iter().find_map(|d| find_anticheat(d, 4)) {
            return Err(format!("anti-cheat found ({})", f.display()));
        }
    }
    // Hand-modded games break when SK hooks on top; "always" for the game still injects.
    if g.specialk != Some(true) {
        if let Some(f) = own_setup(g) {
            return Err(format!("already has its own mods ({f}); turn Special K on for this game to inject anyway"));
        }
    }
    Ok(())
}

/// Files that mean a game was modded by hand: ReShade (and RenoDX add-ons), REFramework, ASI loaders, a local SK.
const OWN_SETUP: [&str; 14] = [
    "reshade.ini", "reshadepreset.ini", "reshade-shaders", "reshade.log", "reshade.asi", "dxgi.dll", "d3d9.dll", "d3d11.dll",
    "d3d12.dll", "opengl32.dll", "dinput8.dll", "specialk64.dll", "specialk32.dll", "dsound.dll",
];

/// First sign of a hand-made mod setup next to the exe or in the install folder. Gamut's own ReShade doesn't count.
pub fn own_setup(g: &Game) -> Option<String> {
    let mut dirs = crate::procs::game_dirs(g);
    if let Some(d) = crate::procs::game_exe(g).as_deref().and_then(Path::parent) {
        if !dirs.iter().any(|x| x == d) {
            dirs.push(d.to_path_buf());
        }
    }
    dirs.iter().find_map(|d| {
        if d.join(crate::reshade::MARKER).exists() {
            return None;
        }
        std::fs::read_dir(d).ok()?.flatten().find_map(|e| {
            let n = e.file_name().to_string_lossy().to_lowercase();
            (OWN_SETUP.contains(&n.as_str()) || n.ends_with(".addon64") || n.ends_with(".addon32") || n.ends_with(".addon")).then_some(n)
        })
    })
}

fn game_dirs(g: &Game) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = g.install_dir.iter().filter(|d| !d.trim().is_empty()).map(PathBuf::from).collect();
    if let Some(p) = g.exe.as_deref().map(Path::new).and_then(Path::parent) {
        if !v.iter().any(|d| p.starts_with(d)) && p.is_dir() {
            v.push(p.to_path_buf());
        }
    }
    v
}

/// First EasyAntiCheat / BattlEye file or folder within `depth` levels.
pub fn find_anticheat(dir: &Path, depth: u32) -> Option<PathBuf> {
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let name = e.file_name().to_string_lossy().to_lowercase();
        if name.starts_with("easyanticheat") || name.starts_with("battleye") {
            return Some(e.path());
        }
        if depth > 0 && e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            if let Some(f) = find_anticheat(&e.path(), depth - 1) {
                return Some(f);
            }
        }
    }
    None
}

/// Special K's install folder: the configured one, else HKCU\Software\Kaldaien\Special K\Path.
pub fn install_dir(custom: &str) -> Option<PathBuf> {
    let p = if custom.trim().is_empty() {
        let key = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER).open_subkey(r"Software\Kaldaien\Special K").ok()?;
        PathBuf::from(key.get_value::<String, _>("Path").ok()?)
    } else {
        let p = PathBuf::from(custom.trim());
        // Accept SKIF.exe itself as well as its folder.
        if p.is_file() { p.parent()?.to_path_buf() } else { p }
    };
    p.join("Servlet").is_dir().then_some(p)
}

/// Writes SK's per-launch files for `g`; failures are logged, never fatal.
pub fn prepare(sk_dir: &Path, s: &Settings, g: &Game) {
    match profile::write_defaults(sk_dir, s) {
        Ok(true) => log::info!("Special K: updated default_SpecialK.ini"),
        Ok(false) => {}
        Err(e) => log::warn!("Special K: writing default_SpecialK.ini failed: {e:#}"),
    }
    if s.reshade {
        if let Err(e) = profile::reshade_preset(sk_dir, g) {
            log::warn!("Special K: ReShade preset failed: {e:#}");
        }
    }
    if s.steam_overlay {
        if let Err(e) = profile::steam_appid(g) {
            log::warn!("Special K: writing steam_appid.txt failed: {e:#}");
        }
    }
}

/// Environment for the game process: `SteamTenfoot=1` puts the Big Picture overlay into non-Steam
/// games while Steam runs in Big Picture.
pub fn game_env(s: &Settings, g: &Game) -> Vec<(String, String)> {
    if s.bpm_overlay && g.source != crate::model::Source::Steam && sys::bpm_running() {
        vec![("SteamTenfoot".into(), "1".into())]
    } else {
        Vec::new()
    }
}

/// Stops every running SK service (before an update).
pub fn stop_all(sk_dir: &Path) {
    let servlet = sk_dir.join("Servlet");
    for svc in SERVICES {
        if sys::process_running(svc) {
            let _ = std::process::Command::new(servlet.join(format!("{svc}.exe"))).arg("Stop").current_dir(&servlet).status();
        }
    }
    if let Some(st) = STATE.lock().unwrap().as_mut() {
        st.owned.clear();
    }
}

/// Services Gamut started (and so may stop) and how many launches still need them.
struct State {
    owned: Vec<&'static str>,
    users: usize,
    dir: PathBuf,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
const SERVICES: [&str; 2] = ["SKIFsvc32", "SKIFsvc64"];

/// Keeps the service running while alive; dropping it stops the services Gamut started once no
/// other launch needs them.
pub struct Guard(());

/// Starts any SK service that isn't running. Blocks up to ~2 s; call off the UI thread.
pub fn acquire(dir: &Path) -> Result<Guard> {
    let mut st = STATE.lock().unwrap();
    let state = st.get_or_insert_with(|| State { owned: Vec::new(), users: 0, dir: dir.to_path_buf() });
    for svc in SERVICES {
        if sys::process_running(svc) {
            continue;
        }
        let servlet = dir.join("Servlet");
        let exe = servlet.join(format!("{svc}.exe"));
        if !exe.is_file() {
            bail!("{} not found", exe.display());
        }
        std::process::Command::new(&exe)
            .arg("Start")
            .current_dir(&servlet)
            .spawn()
            .with_context(|| format!("starting {}", exe.display()))?;
        if !(0..20).any(|_| {
            std::thread::sleep(Duration::from_millis(100));
            sys::process_running(svc)
        }) {
            bail!("{svc} did not start");
        }
        log::info!("Special K: started {svc}");
        if !state.owned.contains(&svc) {
            state.owned.push(svc);
        }
    }
    state.users += 1;
    Ok(Guard(()))
}

impl Drop for Guard {
    fn drop(&mut self) {
        let mut st = STATE.lock().unwrap();
        let Some(state) = st.as_mut() else { return };
        state.users = state.users.saturating_sub(1);
        if state.users > 0 {
            return;
        }
        for svc in std::mem::take(&mut state.owned) {
            if !sys::process_running(svc) {
                continue;
            }
            let servlet = state.dir.join("Servlet");
            match std::process::Command::new(servlet.join(format!("{svc}.exe"))).arg("Stop").current_dir(&servlet).spawn() {
                Ok(_) => log::info!("Special K: stopped {svc}"),
                Err(e) => log::warn!("Special K: stopping {svc} failed: {e}"),
            }
        }
    }
}

/// SK signals this named event once it has injected into a game.
pub struct InjectionSignal(sys::Event);

impl InjectionSignal {
    /// Create before launching, so SK finds the event.
    pub fn new() -> Option<Self> {
        sys::Event::open(r"Local\SKIF_InjectAckEx").map(InjectionSignal)
    }
    /// True if SK reported an injection within `timeout`.
    pub fn wait(&self, timeout: Duration) -> bool {
        self.0.wait(timeout)
    }
}

#[cfg(windows)]
mod sys {
    use std::time::Duration;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VS_FIXEDFILEINFO, VerQueryValueW};
    use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
    use windows_sys::Win32::UI::WindowsAndMessaging::FindWindowW;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    /// "a.b.c.d" from a file's version resource.
    pub fn file_version(path: &std::path::Path) -> Option<String> {
        let p = wide(&path.to_string_lossy());
        unsafe {
            let n = GetFileVersionInfoSizeW(p.as_ptr(), std::ptr::null_mut());
            if n == 0 {
                return None;
            }
            let mut buf = vec![0u8; n as usize];
            if GetFileVersionInfoW(p.as_ptr(), 0, n, buf.as_mut_ptr().cast()) == 0 {
                return None;
            }
            let mut info: *mut core::ffi::c_void = std::ptr::null_mut();
            let mut len = 0u32;
            let root = wide("\\");
            if VerQueryValueW(buf.as_ptr().cast(), root.as_ptr(), &mut info, &mut len) == 0 || info.is_null() {
                return None;
            }
            let f = &*(info as *const VS_FIXEDFILEINFO);
            let (ms, ls) = (f.dwFileVersionMS, f.dwFileVersionLS);
            Some(format!("{}.{}.{}.{}", ms >> 16, ms & 0xffff, ls >> 16, ls & 0xffff))
        }
    }

    /// Steam's Big Picture window is open.
    pub fn bpm_running() -> bool {
        let t = wide("Steam Big Picture Mode");
        unsafe { !FindWindowW(std::ptr::null(), t.as_ptr()).is_null() }
    }

    /// Whether a process named `name` (without .exe) is running.
    pub fn process_running(name: &str) -> bool {
        let want = format!("{name}.exe");
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == INVALID_HANDLE_VALUE {
                return false;
            }
            let mut e = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
            let mut found = false;
            let mut ok = Process32FirstW(snap, &mut e) != 0;
            while ok {
                let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                if String::from_utf16_lossy(&e.szExeFile[..len]).eq_ignore_ascii_case(&want) {
                    found = true;
                    break;
                }
                ok = Process32NextW(snap, &mut e) != 0;
            }
            CloseHandle(snap);
            found
        }
    }

    pub struct Event(HANDLE);
    // A kernel handle is usable from any thread.
    unsafe impl Send for Event {}
    unsafe impl Sync for Event {}

    impl Event {
        /// Creates (or opens) a named auto-reset event.
        pub fn open(name: &str) -> Option<Event> {
            let w = wide(name);
            let h = unsafe { CreateEventW(std::ptr::null(), 0, 0, w.as_ptr()) };
            (!h.is_null()).then_some(Event(h))
        }
        pub fn wait(&self, timeout: Duration) -> bool {
            unsafe { WaitForSingleObject(self.0, timeout.as_millis().min(u32::MAX as u128 - 1) as u32) == WAIT_OBJECT_0 }
        }
    }

    impl Drop for Event {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }
}

#[cfg(not(windows))]
mod sys {
    use std::time::Duration;
    pub fn process_running(_: &str) -> bool {
        false
    }
    pub fn file_version(_: &std::path::Path) -> Option<String> {
        None
    }
    pub fn bpm_running() -> bool {
        false
    }
    pub struct Event;
    impl Event {
        pub fn open(_: &str) -> Option<Event> {
            None
        }
        pub fn wait(&self, _: Duration) -> bool {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game() -> Game {
        Game::new_custom("Test".into(), None)
    }

    #[test]
    fn modes_and_overrides() {
        let mut s = Settings::default();
        let mut g = game();
        assert!(decide(&g, &s).is_err());
        s.mode = "global".into();
        assert!(decide(&g, &s).is_ok());
        g.specialk = Some(false);
        assert!(decide(&g, &s).is_err());
        s.mode = "selective".into();
        g.specialk = None;
        assert!(decide(&g, &s).is_err());
        g.specialk = Some(true);
        assert!(decide(&g, &s).is_ok());
    }

    #[test]
    fn finds_anticheat_in_subfolders() {
        let dir = std::env::temp_dir().join(format!("gamut-sk-{}", std::process::id()));
        let deep = dir.join("bin").join("x64");
        std::fs::create_dir_all(&deep).unwrap();
        let mut g = game();
        g.install_dir = Some(dir.to_string_lossy().into());
        let s = Settings { mode: "global".into(), ..Default::default() };
        assert!(decide(&g, &s).is_ok());
        std::fs::create_dir_all(deep.join("EasyAntiCheat")).unwrap();
        assert!(decide(&g, &s).is_err());
        assert!(decide(&g, &Settings { skip_anticheat: false, ..s }).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn skips_hand_modded_games() {
        let dir = std::env::temp_dir().join(format!("gamut-sk-mods-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("Game.exe");
        std::fs::write(&exe, b"MZ").unwrap();
        let mut g = Game::new_custom("T".into(), Some(exe.to_string_lossy().into()));
        g.install_dir = Some(dir.to_string_lossy().into());
        let s = Settings { mode: "global".into(), ..Default::default() };
        assert!(decide(&g, &s).is_ok());
        // A RenoDX add-on means someone set this game up by hand.
        std::fs::write(dir.join("renodx-game.addon64"), b"").unwrap();
        assert!(decide(&g, &s).unwrap_err().contains("renodx-game.addon64"));
        g.specialk = Some(true);
        assert!(decide(&g, &s).is_ok());
        g.specialk = None;
        // Gamut's own ReShade (marker present) doesn't block SK.
        std::fs::remove_file(dir.join("renodx-game.addon64")).unwrap();
        std::fs::write(dir.join("dxgi.dll"), b"").unwrap();
        assert!(decide(&g, &s).is_err());
        std::fs::write(dir.join(crate::reshade::MARKER), "dxgi.dll").unwrap();
        assert!(decide(&g, &s).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
