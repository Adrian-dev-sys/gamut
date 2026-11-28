//! What Gamut's own data folder uses on disk, and clearing what can go.
use crate::Paths;
use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Art,
    Thumbs,
    Quarantine,
    OldCopies,
    Backups,
    Caches,
    Logs,
}

#[derive(Debug, Clone)]
pub struct Usage {
    pub kind: Kind,
    pub label: &'static str,
    pub hint: &'static str,
    pub bytes: u64,
    pub files: usize,
    /// It can be deleted from Settings.
    pub clearable: bool,
}

/// Sizes of each part, biggest first.
pub fn measure(paths: &Paths, backups: &Path) -> Vec<Usage> {
    let rows = [
        (Kind::Art, "Covers, banners and icons", "Needed; custom ones can't be fetched again.", false),
        (Kind::Thumbs, "Thumbnails", "Made again from the covers as you scroll.", true),
        (Kind::Quarantine, "Quarantined art", "Art set aside by an earlier repair; not used.", true),
        (Kind::OldCopies, "Old library copies", "Safety copies left by repairs, imports and restores.", true),
        (Kind::Backups, "Backups", "Settings → Backups.", false),
        (Kind::Caches, "Downloads and caches", "ReShade, RetroAchievements; fetched again when needed.", false),
        (Kind::Logs, "Logs", "", false),
    ];
    let mut v: Vec<Usage> = rows
        .into_iter()
        .map(|(kind, label, hint, clearable)| {
            let (bytes, files) = files_of(paths, backups, kind).iter().fold((0, 0), |(b, n), p| {
                let (b2, n2) = size(p);
                (b + b2, n + n2)
            });
            Usage { kind, label, hint, bytes, files, clearable }
        })
        .collect();
    v.sort_by_key(|u| std::cmp::Reverse(u.bytes));
    v
}

/// Deletes a clearable part. Returns bytes freed.
pub fn clear(paths: &Paths, kind: Kind) -> Result<u64> {
    let mut freed = 0;
    for p in files_of(paths, Path::new(""), kind) {
        let (b, _) = size(&p);
        let r = if p.is_dir() { fs::remove_dir_all(&p) } else { fs::remove_file(&p) };
        match r {
            Ok(()) => freed += b,
            Err(e) => log::warn!("deleting {}: {e}", p.display()),
        }
    }
    // Thumbnails are written into this folder again.
    if kind == Kind::Thumbs {
        fs::create_dir_all(&paths.thumbs)?;
    }
    Ok(freed)
}

fn files_of(paths: &Paths, backups: &Path, kind: Kind) -> Vec<PathBuf> {
    let root = &paths.root;
    match kind {
        Kind::Art => vec![paths.covers.clone(), paths.heroes.clone(), paths.icons.clone()],
        Kind::Thumbs => vec![paths.thumbs.clone()],
        Kind::Quarantine => vec![root.join("art-quarantine")],
        Kind::OldCopies => fs::read_dir(root)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file() && is_old_copy(&p.file_name().unwrap_or_default().to_string_lossy()))
            .collect(),
        Kind::Backups => vec![backups.to_path_buf()],
        Kind::Caches => vec![root.join("reshade"), root.join("ra")],
        Kind::Logs => vec![root.join("gamut.log"), root.join("gamut.old.log")],
    }
}

/// `library.db.<anything>` (not the live database or its journal) and settings set aside by a restore.
fn is_old_copy(name: &str) -> bool {
    let n = name.to_lowercase();
    (n.starts_with("library.db.") && n != "library.db.restoring") || n.contains(".pre-restore-")
}

/// Bytes and files under a path (0 if missing).
fn size(p: &Path) -> (u64, usize) {
    let Ok(m) = fs::symlink_metadata(p) else { return (0, 0) };
    if !m.is_dir() {
        return (m.len(), 1);
    }
    fs::read_dir(p).into_iter().flatten().flatten().fold((0, 0), |(b, n), e| {
        let (b2, n2) = size(&e.path());
        (b + b2, n + n2)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_copies_and_clearing() {
        assert!(is_old_copy("library.db.pre-artfix"));
        assert!(is_old_copy("library.db.damaged-1795644464-wal"));
        assert!(is_old_copy("config.toml.pre-restore-2026-11-28_101010"));
        assert!(!is_old_copy("library.db"));
        assert!(!is_old_copy("library.db-journal"));
        assert!(!is_old_copy("config.toml"));

        let root = std::env::temp_dir().join(format!("gamut-storage-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let paths = Paths::at(root.clone()).unwrap();
        fs::write(root.join("library.db"), b"live").unwrap();
        fs::write(root.join("library.db.pre-x"), b"12345").unwrap();
        fs::write(paths.thumb(1), b"123").unwrap();
        let u = measure(&paths, &root.join("backups"));
        let get = |k| u.iter().find(|x| x.kind == k).unwrap().bytes;
        assert_eq!((get(Kind::OldCopies), get(Kind::Thumbs)), (5, 3));
        assert_eq!(clear(&paths, Kind::OldCopies).unwrap(), 5);
        assert_eq!(clear(&paths, Kind::Thumbs).unwrap(), 3);
        assert!(root.join("library.db").exists() && paths.thumbs.is_dir() && !paths.thumb(1).exists());
        let _ = fs::remove_dir_all(&root);
    }
}
