pub mod achievements;
pub mod backup;
pub mod secret;
pub mod storage;
pub mod stores;
pub mod uninstall;
pub mod config;
pub mod db;
pub mod display;
pub mod gog;
pub mod scanner;
pub mod http;
pub mod igdb;
pub mod hdr;
pub mod health;
pub mod hltb;
pub mod pcgw;
pub mod images;
pub mod launch;
pub mod litedb;
pub mod local_ach;
pub mod model;
pub mod playnite;
pub mod procs;
pub mod reshade;
pub mod retro;
pub mod scrape;
pub mod screenshots;
pub mod scripts;
pub mod specialk;
pub mod steam;
pub mod steam_collections;
pub mod status;
pub mod sync;
pub mod vdf;
pub mod vndb;

pub use config::Config;
pub use db::Library;
pub use model::{Game, Source};

use std::path::PathBuf;

/// All on-disk locations the app uses.
#[derive(Debug, Clone)]
pub struct Paths {
    pub root: PathBuf,
    pub db: PathBuf,
    pub config: PathBuf,
    pub covers: PathBuf,
    pub thumbs: PathBuf,
    pub heroes: PathBuf,
    pub icons: PathBuf,
}

impl Paths {
    /// Portable mode if `portable.txt` sits next to the exe, else %LOCALAPPDATA%\Gamut.
    pub fn resolve() -> anyhow::Result<Self> {
        let exe_dir = std::env::current_exe()?.parent().map(PathBuf::from);
        let root = match exe_dir {
            Some(d) if d.join("portable.txt").exists() => d.join("data"),
            _ => directories::ProjectDirs::from("", "", "Gamut")
                .ok_or_else(|| anyhow::anyhow!("no home directory"))?
                .data_local_dir()
                .to_path_buf(),
        };
        Self::at(root)
    }

    pub fn at(root: PathBuf) -> anyhow::Result<Self> {
        let p = Paths {
            db: root.join("library.db"),
            config: root.join("config.toml"),
            covers: root.join("covers"),
            thumbs: root.join("thumbs"),
            heroes: root.join("heroes"),
            icons: root.join("icons"),
            root,
        };
        for d in [&p.root, &p.covers, &p.thumbs, &p.heroes, &p.icons] {
            std::fs::create_dir_all(d)?;
        }
        Ok(p)
    }

    /// Exclusive lock on the data folder; `None` if another Gamut process holds it.
    pub fn lock(&self) -> Option<InstanceLock> {
        let f = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.root.join("gamut.lock"))
            .ok()?;
        f.try_lock().ok()?;
        Some(InstanceLock(f))
    }

    pub fn thumb(&self, id: i64) -> PathBuf {
        self.thumbs.join(format!("{id}.jpg"))
    }
    pub fn cover(&self, id: i64) -> PathBuf {
        self.covers.join(format!("{id}.img"))
    }
    pub fn hero(&self, id: i64) -> PathBuf {
        self.heroes.join(format!("{id}.img"))
    }
    pub fn icon(&self, id: i64) -> PathBuf {
        self.icons.join(format!("{id}.img"))
    }
}

/// Released when dropped (or when the process exits).
pub struct InstanceLock(#[allow(dead_code)] std::fs::File);

/// Seconds to add to UTC for local time right now (DST included; past dates use today's offset).
pub fn utc_offset() -> i64 {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
        let mut tz: TIME_ZONE_INFORMATION = std::mem::zeroed();
        let id = GetTimeZoneInformation(&mut tz);
        // 2 = TIME_ZONE_ID_DAYLIGHT; Bias is minutes to add to local time to get UTC.
        let bias = tz.Bias + if id == 2 { tz.DaylightBias } else { tz.StandardBias };
        return -(bias as i64) * 60;
    }
    #[allow(unreachable_code)]
    0
}

/// Total size of the files under `dir` (links not followed).
pub fn dir_size(dir: &std::path::Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    rd.flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

/// Folders that more than one game would be sized by (shared ROM folders, "D:\Games").
pub fn shared_folders(games: &[Game]) -> std::collections::HashSet<std::path::PathBuf> {
    let mut n = std::collections::HashMap::new();
    for g in games.iter().filter(|g| g.source == Source::Custom) {
        if let Some(d) = size_dir(g) {
            *n.entry(d).or_insert(0) += 1;
        }
    }
    n.into_iter().filter(|(_, c)| *c > 1).map(|(d, _)| d).collect()
}

fn size_dir(g: &Game) -> Option<std::path::PathBuf> {
    match g.rom() {
        Some(r) => std::path::Path::new(&r).parent().map(|p| p.to_path_buf()),
        None => g.folder(),
    }
}

/// Bytes a game takes: its own folder, or only its ROM (plus same-named tracks) in a shared folder.
pub fn game_size(g: &Game, shared: &std::collections::HashSet<std::path::PathBuf>) -> Option<i64> {
    use std::path::Path;
    if let Some(r) = g.rom() {
        let rom = Path::new(&r);
        if rom.is_dir() {
            return Some(dir_size(rom) as i64);
        }
        let mut dir = rom.parent()?;
        if !rom.is_file() {
            return None;
        }
        if shared.contains(dir) {
            // "Game (USA).cue" + "Game (USA) (Track 1).bin".
            let stem = rom.file_stem()?.to_string_lossy().to_string();
            let tracks = format!("{stem} (");
            let same = |e: &std::fs::DirEntry| {
                let s = e.path().file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                s == stem || s.starts_with(&tracks)
            };
            let files = std::fs::read_dir(dir).ok()?.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_file()) && same(e));
            return Some(files.map(|e| e.metadata().map(|m| m.len()).unwrap_or(0)).sum::<u64>() as i64);
        }
        // PS3 "Game\PS3_GAME\USRDIR\EBOOT.BIN", Wii U "title\code\x.rpx": size the whole game.
        while let Some(p) = dir.parent().filter(|_| {
            let n = dir.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
            ["usrdir", "ps3_game", "code", "content", "meta"].contains(&n.as_str())
        }) {
            dir = p;
        }
        return Some(dir_size(dir) as i64);
    }
    let d = g.folder().filter(|d| d.is_dir() && !shared.contains(d))?;
    let pc = g.platform.as_deref().is_none_or(|p| p.contains("PC") || p.contains("Windows"));
    // An emulator's own folder isn't the game's.
    (pc || g.install_dir.as_deref().is_some_and(|i| !i.contains('{'))).then(|| dir_size(&d) as i64)
}

/// "123.3 GB", "850 MB".
pub fn format_size(bytes: i64) -> String {
    let b = bytes.max(0) as f64;
    match b {
        b if b >= 1e12 => format!("{:.2} TB", b / 1e12),
        b if b >= 1e9 => format!("{:.1} GB", b / 1e9),
        b if b >= 1e6 => format!("{:.0} MB", b / 1e6),
        b => format!("{:.0} KB", b / 1e3),
    }
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_roms_in_shared_folders_alone() {
        let dir = std::env::temp_dir().join(format!("gamut size test {}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (f, n) in [("A (USA).cue", 10), ("A (USA) (Track 1).bin", 100), ("A (USA) 2.bin", 1000), ("B.cue", 5)] {
            std::fs::write(dir.join(f), vec![0u8; n]).unwrap();
        }
        let game = |rom: &str, quoted: bool| {
            let mut g = Game::new_custom(rom.into(), Some("C:\\emu\\emu.exe".into()));
            let p = dir.join(rom).display().to_string();
            g.args = Some(if quoted { format!("-batch \"{p}\"") } else { format!("-fullscreen -- {p}") });
            g.platform = Some("Sony PlayStation".into());
            g
        };
        let (a, b) = (game("A (USA).cue", false), game("B.cue", true));
        assert_eq!(a.rom().as_deref(), Some(dir.join("A (USA).cue").to_str().unwrap()));
        let shared = shared_folders(&[a.clone(), b.clone()]);
        assert!(shared.contains(&dir));
        assert_eq!(game_size(&a, &shared), Some(110));
        assert_eq!(game_size(&b, &shared), Some(5));
        assert_eq!(game_size(&a, &Default::default()), Some(1115));
        std::fs::remove_dir_all(&dir).ok();
    }
}
