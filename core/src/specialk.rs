//! Special K global injection helper: starts SK's injection service before a game launches and
//! stops it again afterwards (see docs/specialk.md).
use crate::model::Game;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

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
}

impl Default for Settings {
    fn default() -> Self {
        Settings { mode: "off".into(), stop: "injection".into(), skip_anticheat: true, path: String::new() }
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
    Ok(())
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
    use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

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
            let w: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
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
}
