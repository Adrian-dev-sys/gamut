//! Backups: the database, settings and local achievements in a dated zip (rotated); art mirrored incrementally.
//! Restores are staged and applied at the next start, before anything opens the database.
use crate::Paths;
use anyhow::{bail, Context, Result};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Small files next to the database that can't be fetched again.
const EXTRA: [&str; 2] = ["config.toml", "local-ach.json"];
const PREFIX: &str = "gamut-backup-";
const PENDING: &str = "restore.pending";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// Back up on its own every `days` days.
    pub auto: bool,
    pub days: u32,
    /// Zips kept; older ones are deleted.
    pub keep: usize,
    /// Mirror covers, banners and icons too.
    pub art: bool,
    /// Empty = `<data>\backups`.
    pub dir: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { auto: true, days: 7, keep: 10, art: true, dir: String::new() }
    }
}

pub fn dir(paths: &Paths, s: &Settings) -> PathBuf {
    match s.dir.trim() {
        "" => paths.root.join("backups"),
        d => PathBuf::from(d),
    }
}

#[derive(Debug, Clone)]
pub struct Backup {
    pub path: PathBuf,
    pub time: SystemTime,
    pub size: u64,
}

/// Backups in `dir`, newest first.
pub fn list(dir: &Path) -> Vec<Backup> {
    let mut v: Vec<Backup> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(PREFIX) && e.file_name().to_string_lossy().ends_with(".zip"))
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some(Backup { path: e.path(), time: m.modified().ok()?, size: m.len() })
        })
        .collect();
    // Names carry the time, so they sort even if mtimes were touched.
    v.sort_by(|a, b| b.path.file_name().cmp(&a.path.file_name()));
    v
}

/// True when the newest backup is older than `days` (or there is none).
pub fn due(dir: &Path, days: u32) -> bool {
    let age = Duration::from_secs(u64::from(days.max(1)) * 86_400);
    list(dir).first().is_none_or(|b| b.time.elapsed().unwrap_or_default() >= age)
}

#[derive(Debug, Default)]
pub struct Created {
    pub path: PathBuf,
    /// Art files copied into the mirror this time.
    pub art_copied: usize,
}

/// Makes a backup now: a checked database snapshot plus settings in a zip, then the art mirror and rotation.
pub fn create(paths: &Paths, s: &Settings) -> Result<Created> {
    let dir = dir(paths, s);
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let stamp = stamp();
    let snap = dir.join(format!(".snapshot-{stamp}.db"));
    let _ = fs::remove_file(&snap);
    let r = (|| {
        snapshot(&paths.db, &snap)?;
        check(&snap).context("the database snapshot failed its integrity check; older backups were kept")?;
        let path = dir.join(format!("{PREFIX}{stamp}.zip"));
        let part = path.with_extension("zip.part");
        write_zip(&part, &snap, &paths.root)?;
        fs::rename(&part, &path)?;
        Ok::<_, anyhow::Error>(path)
    })();
    let _ = fs::remove_file(&snap);
    let path = r?;
    let art_copied = if s.art { mirror_art(paths, &dir.join("art"))? } else { 0 };
    prune(&dir, s.keep.max(1));
    Ok(Created { path, art_copied })
}

/// A consistent copy of the live database, read through its own handle.
fn snapshot(db: &Path, to: &Path) -> Result<()> {
    let c = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    c.busy_timeout(Duration::from_secs(20))?;
    c.execute("VACUUM INTO ?1", [to.to_string_lossy()]).context("copying the database")?;
    Ok(())
}

fn check(db: &Path) -> Result<()> {
    let c = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let r: String = c.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    if r != "ok" {
        bail!("integrity check: {r}");
    }
    c.query_row("SELECT count(*) FROM games", [], |r| r.get::<_, i64>(0)).context("no games table")?;
    Ok(())
}

fn write_zip(to: &Path, db: &Path, root: &Path) -> Result<()> {
    let mut z = zip::ZipWriter::new(fs::File::create(to)?);
    let opt = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    z.start_file("library.db", opt)?;
    std::io::copy(&mut fs::File::open(db)?, &mut z)?;
    for name in EXTRA {
        if let Ok(bytes) = fs::read(root.join(name)) {
            z.start_file(name, opt)?;
            z.write_all(&bytes)?;
        }
    }
    z.finish()?.sync_all()?;
    Ok(())
}

/// Copies new or changed covers, banners and icons into `to`; nothing is deleted from it.
fn mirror_art(paths: &Paths, to: &Path) -> Result<usize> {
    let mut n = 0;
    for (name, src) in art_dirs(paths) {
        let dst = to.join(name);
        fs::create_dir_all(&dst)?;
        for e in fs::read_dir(src).into_iter().flatten().flatten() {
            let Ok(m) = e.metadata() else { continue };
            if !m.is_file() {
                continue;
            }
            let d = dst.join(e.file_name());
            let same = fs::metadata(&d).is_ok_and(|dm| dm.len() == m.len() && dm.modified().ok() >= m.modified().ok());
            if !same {
                fs::copy(e.path(), &d).with_context(|| format!("copying {}", e.path().display()))?;
                n += 1;
            }
        }
    }
    Ok(n)
}

fn art_dirs(paths: &Paths) -> [(&'static str, &Path); 3] {
    [("covers", &paths.covers), ("heroes", &paths.heroes), ("icons", &paths.icons)]
}

fn prune(dir: &Path, keep: usize) {
    for b in list(dir).into_iter().skip(keep) {
        if let Err(e) = fs::remove_file(&b.path) {
            log::warn!("removing old backup {}: {e}", b.path.display());
        }
    }
}

/// Checks a backup and marks it to be restored at the next start.
pub fn stage(paths: &Paths, zip_path: &Path) -> Result<()> {
    let mut z = zip::ZipArchive::new(fs::File::open(zip_path)?).context("not a readable zip")?;
    z.by_name("library.db").context("no library.db in this backup")?;
    fs::write(paths.root.join(PENDING), zip_path.to_string_lossy().as_bytes())?;
    Ok(())
}

pub fn pending(paths: &Paths) -> bool {
    paths.root.join(PENDING).exists()
}

/// Applies a staged restore. Call before anything opens the database. Returns what happened.
pub fn apply_pending(paths: &Paths) -> Option<Result<String>> {
    let marker = paths.root.join(PENDING);
    let zip_path = PathBuf::from(fs::read_to_string(&marker).ok()?.trim());
    // Never retried: a restore that fails once would fail on every start.
    let _ = fs::remove_file(&marker);
    Some(restore(paths, &zip_path))
}

fn restore(paths: &Paths, zip_path: &Path) -> Result<String> {
    let mut z = zip::ZipArchive::new(fs::File::open(zip_path).with_context(|| format!("opening {}", zip_path.display()))?)?;
    let incoming = paths.root.join("library.db.restoring");
    {
        let mut f = fs::File::create(&incoming)?;
        std::io::copy(&mut z.by_name("library.db")?, &mut f)?;
        f.sync_all()?;
    }
    if let Err(e) = check(&incoming) {
        let _ = fs::remove_file(&incoming);
        return Err(e.context("the backup's database is damaged; nothing was changed"));
    }
    let stamp = stamp();
    let aside = |p: &Path| PathBuf::from(format!("{}.pre-restore-{stamp}", p.display()));
    // The current database moves aside with its journal; a stale journal would be replayed into the restored one.
    if paths.db.exists() {
        fs::rename(&paths.db, aside(&paths.db))?;
    }
    for ext in ["-journal", "-wal", "-shm"] {
        let j = PathBuf::from(format!("{}{ext}", paths.db.display()));
        if j.exists() {
            fs::rename(&j, PathBuf::from(format!("{}{ext}", aside(&paths.db).display())))?;
        }
    }
    fs::rename(&incoming, &paths.db)?;
    for name in EXTRA {
        let Ok(mut e) = z.by_name(name) else { continue };
        let mut bytes = Vec::new();
        e.read_to_end(&mut bytes)?;
        let p = paths.root.join(name);
        if p.exists() {
            fs::rename(&p, aside(&p))?;
        }
        fs::write(&p, bytes)?;
    }
    let art = zip_path.parent().map(|d| d.join("art")).filter(|d| d.is_dir());
    let filled = match art {
        Some(a) => fill_art(paths, &a)?,
        None => 0,
    };
    let name = zip_path.file_name().unwrap_or_default().to_string_lossy();
    Ok(format!("Restored {name} ({filled} art files put back). The previous library was kept as library.db.pre-restore-{stamp}."))
}

/// Puts back art missing from the data folder, for games in the restored library only.
fn fill_art(paths: &Paths, from: &Path) -> Result<usize> {
    let c = Connection::open_with_flags(&paths.db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let ids: HashSet<i64> = c.prepare("SELECT id FROM games")?.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    drop(c);
    let mut n = 0;
    for (name, dst) in art_dirs(paths) {
        fs::create_dir_all(dst)?;
        for e in fs::read_dir(from.join(name)).into_iter().flatten().flatten() {
            let file = e.file_name();
            let id = file.to_string_lossy().split('.').next().and_then(|s| s.parse::<i64>().ok());
            let d = dst.join(&file);
            if id.is_some_and(|id| ids.contains(&id)) && !d.exists() {
                fs::copy(e.path(), &d)?;
                // Its thumbnail may be of other art.
                if name == "covers" {
                    let _ = fs::remove_file(paths.thumb(id.unwrap()));
                }
                n += 1;
            }
        }
    }
    Ok(n)
}

/// Local time as `2026-11-28_153012`.
fn stamp() -> String {
    #[cfg(windows)]
    {
        let mut t = unsafe { std::mem::zeroed::<windows_sys::Win32::Foundation::SYSTEMTIME>() };
        unsafe { windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut t) };
        format!("{:04}-{:02}-{:02}_{:02}{:02}{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
    }
    #[cfg(not(windows))]
    {
        let s = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs();
        format!("{s}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Library;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("gamut-backup-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn backup_and_restore_round_trip() {
        let root = tmp("rt");
        let paths = Paths::at(root.join("data")).unwrap();
        let s = Settings { dir: root.join("bk").display().to_string(), keep: 2, ..Default::default() };
        let lib = Library::open(&paths.db).unwrap();
        let a = lib.add_custom(&crate::Game::new_custom("Alpha".into(), None)).unwrap();
        fs::write(&paths.config, "steam_id = \"1\"\n").unwrap();
        fs::write(paths.cover(a), b"cover").unwrap();
        drop(lib);

        let made = create(&paths, &s).unwrap();
        assert_eq!(made.art_copied, 1);
        assert_eq!(create(&paths, &s).unwrap().art_copied, 0, "mirror only copies changes");

        // Damage things after the backup: new game, settings, lost cover, stale journal.
        let lib = Library::open(&paths.db).unwrap();
        lib.add_custom(&crate::Game::new_custom("Beta".into(), None)).unwrap();
        drop(lib);
        fs::write(&paths.config, "steam_id = \"2\"\n").unwrap();
        fs::remove_file(paths.cover(a)).unwrap();
        fs::write(format!("{}-journal", paths.db.display()), b"junk").unwrap();

        stage(&paths, &made.path).unwrap();
        assert!(pending(&paths));
        let msg = apply_pending(&paths).unwrap().unwrap();
        assert!(msg.contains("1 art files"), "{msg}");
        assert!(!pending(&paths));
        assert!(!PathBuf::from(format!("{}-journal", paths.db.display())).exists());
        let lib = Library::open(&paths.db).unwrap();
        assert_eq!(lib.all().unwrap().iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), ["Alpha"]);
        assert_eq!(fs::read_to_string(&paths.config).unwrap(), "steam_id = \"1\"\n");
        assert_eq!(fs::read(paths.cover(a)).unwrap(), b"cover");

        // Rotation keeps the newest `keep`.
        std::thread::sleep(Duration::from_millis(1100));
        create(&paths, &s).unwrap();
        assert_eq!(list(&dir(&paths, &s)).len(), 2);
        assert!(!due(&dir(&paths, &s), 1));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn damaged_backup_changes_nothing() {
        let root = tmp("bad");
        let paths = Paths::at(root.join("data")).unwrap();
        Library::open(&paths.db).unwrap().add_custom(&crate::Game::new_custom("Alpha".into(), None)).unwrap();
        let bad = root.join("gamut-backup-x.zip");
        let mut z = zip::ZipWriter::new(fs::File::create(&bad).unwrap());
        z.start_file("library.db", zip::write::SimpleFileOptions::default()).unwrap();
        z.write_all(b"not a database").unwrap();
        z.finish().unwrap();
        stage(&paths, &bad).unwrap();
        assert!(apply_pending(&paths).unwrap().is_err());
        assert_eq!(Library::open(&paths.db).unwrap().all().unwrap().len(), 1);
        let _ = fs::remove_dir_all(&root);
    }
}
