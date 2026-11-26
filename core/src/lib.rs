pub mod config;
pub mod db;
pub mod http;
pub mod images;
pub mod launch;
pub mod litedb;
pub mod model;
pub mod playnite;
pub mod scrape;
pub mod specialk;
pub mod steam;
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
            root,
        };
        for d in [&p.root, &p.covers, &p.thumbs, &p.heroes] {
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
}

/// Released when dropped (or when the process exits).
pub struct InstanceLock(#[allow(dead_code)] std::fs::File);

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
