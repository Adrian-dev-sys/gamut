use crate::model::{state, Game, Metadata, Source};
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension, Row};
use std::path::Path;

/// SQLite-backed game library. Each thread opens its own handle (WAL mode).
pub struct Library {
    conn: Connection,
}

const COLS: &str = "id, source, source_id, name, installed, install_dir, exe, args, work_dir, \
    playtime_min, last_played, description, developer, publisher, release_date, genres, \
    has_cover, has_hero, cover_state, meta_state, favorite, hidden, added";

/// A Steam app as seen by a scan (local manifest and/or owned-games API).
#[derive(Debug, Clone)]
pub struct SteamEntry {
    pub appid: u32,
    pub name: String,
    pub installed: bool,
    pub install_dir: Option<String>,
    pub playtime_min: Option<i64>,
    pub last_played: i64,
}

impl Library {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             CREATE TABLE IF NOT EXISTS games (
                id INTEGER PRIMARY KEY,
                source TEXT NOT NULL,
                source_id TEXT NOT NULL DEFAULT '',
                name TEXT NOT NULL,
                installed INTEGER NOT NULL DEFAULT 0,
                install_dir TEXT, exe TEXT, args TEXT, work_dir TEXT,
                playtime_min INTEGER NOT NULL DEFAULT 0,
                last_played INTEGER NOT NULL DEFAULT 0,
                description TEXT, developer TEXT, publisher TEXT, release_date TEXT, genres TEXT,
                has_cover INTEGER NOT NULL DEFAULT 0,
                has_hero INTEGER NOT NULL DEFAULT 0,
                cover_state INTEGER NOT NULL DEFAULT 0,
                meta_state INTEGER NOT NULL DEFAULT 0,
                favorite INTEGER NOT NULL DEFAULT 0,
                hidden INTEGER NOT NULL DEFAULT 0,
                added INTEGER NOT NULL DEFAULT 0
             );
             CREATE UNIQUE INDEX IF NOT EXISTS games_steam ON games(source_id) WHERE source = 'steam';",
        )?;
        Ok(Library { conn })
    }

    /// Opens the library; if SQLite finds damage, rebuilds it from every readable row.
    /// Returns a user-facing notice when a repair happened.
    pub fn open_checked(path: &Path) -> Result<(Self, Option<String>)> {
        if let Ok(lib) = Self::open(path) {
            let ok = lib.conn.query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0));
            if matches!(ok.as_deref(), Ok("ok")) {
                return Ok((lib, None));
            }
        }
        let notice = Self::salvage(path)?;
        Ok((Self::open(path)?, Some(notice)))
    }

    fn salvage(path: &Path) -> Result<String> {
        use std::collections::HashSet;
        let file = |suffix: &str| std::path::PathBuf::from(format!("{}{suffix}", path.display()));
        let tmp = file(".rebuild");
        for sfx in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(file(&format!(".rebuild{sfx}")));
        }
        let saved;
        let mut lost = 0usize;
        {
            let new = Self::open(&tmp)?;
            let old = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            let mut seen = HashSet::new();
            // Sequential scan until the first unreadable row...
            if let Ok(mut st) = old.prepare(&format!("SELECT {COLS} FROM games")) {
                if let Ok(mut rows) = st.query([]) {
                    while let Ok(Some(r)) = rows.next() {
                        if let Ok(g) = Self::row(r) {
                            if seen.insert(g.id) {
                                new.insert_full(&g)?;
                            }
                        }
                    }
                }
            }
            // ...then look up every other id individually.
            let max_seen = seen.iter().copied().max().unwrap_or(0);
            let max = old
                .query_row("SELECT max(id) FROM games", [], |r| r.get::<_, Option<i64>>(0))
                .ok()
                .flatten()
                .unwrap_or(0)
                .max(max_seen + 2000);
            let sql = format!("SELECT {COLS} FROM games WHERE id = ?1");
            for id in 1..=max {
                if seen.contains(&id) {
                    continue;
                }
                match old.query_row(&sql, [id], Self::row) {
                    Ok(g) => {
                        new.insert_full(&g)?;
                        seen.insert(id);
                    }
                    Err(rusqlite::Error::QueryReturnedNoRows) => {}
                    Err(_) => lost += 1,
                }
            }
            saved = seen.len();
        }
        let backup = format!(".damaged-{}", crate::now());
        for sfx in ["", "-wal", "-shm"] {
            let _ = std::fs::rename(file(sfx), file(&format!("{backup}{sfx}")));
        }
        std::fs::rename(&tmp, path)?;
        Ok(format!(
            "The library database was damaged and has been rebuilt: {saved} games recovered, {lost} unreadable (Steam games come back on the next scan). The damaged copy was kept as {}{backup}.",
            path.file_name().unwrap_or_default().to_string_lossy()
        ))
    }

    /// Inserts a complete row, keeping its id (used when rebuilding).
    fn insert_full(&self, g: &Game) -> Result<()> {
        self.conn.execute(
            &format!("INSERT OR REPLACE INTO games ({COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)"),
            params![
                g.id, g.source.as_str(), g.source_id, g.name, g.installed, g.install_dir, g.exe, g.args, g.work_dir,
                g.playtime_min, g.last_played, g.description, g.developer, g.publisher, g.release_date, g.genres,
                g.has_cover, g.has_hero, g.cover_state, g.meta_state, g.favorite, g.hidden, g.added
            ],
        )?;
        Ok(())
    }

    /// SQLite's integrity check; "ok" when healthy.
    pub fn integrity_check(&self) -> Result<String> {
        Ok(self.conn.query_row("PRAGMA integrity_check", [], |r| r.get(0))?)
    }

    fn row(r: &Row) -> rusqlite::Result<Game> {
        Ok(Game {
            id: r.get(0)?,
            source: Source::parse(&r.get::<_, String>(1)?),
            source_id: r.get(2)?,
            name: r.get(3)?,
            installed: r.get(4)?,
            install_dir: r.get(5)?,
            exe: r.get(6)?,
            args: r.get(7)?,
            work_dir: r.get(8)?,
            playtime_min: r.get(9)?,
            last_played: r.get(10)?,
            description: r.get(11)?,
            developer: r.get(12)?,
            publisher: r.get(13)?,
            release_date: r.get(14)?,
            genres: r.get(15)?,
            has_cover: r.get(16)?,
            has_hero: r.get(17)?,
            cover_state: r.get(18)?,
            meta_state: r.get(19)?,
            favorite: r.get(20)?,
            hidden: r.get(21)?,
            added: r.get(22)?,
        })
    }

    pub fn all(&self) -> Result<Vec<Game>> {
        let mut st = self.conn.prepare(&format!("SELECT {COLS} FROM games"))?;
        let rows = st.query_map([], Self::row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn get(&self, id: i64) -> Result<Option<Game>> {
        Ok(self
            .conn
            .query_row(&format!("SELECT {COLS} FROM games WHERE id = ?1"), [id], Self::row)
            .optional()?)
    }

    pub fn needing_cover(&self) -> Result<Vec<Game>> {
        self.query_where("cover_state = 0 ORDER BY installed DESC, last_played DESC")
    }

    pub fn needing_meta(&self) -> Result<Vec<Game>> {
        self.query_where("meta_state = 0 ORDER BY installed DESC, last_played DESC")
    }

    fn query_where(&self, clause: &str) -> Result<Vec<Game>> {
        let mut st = self.conn.prepare(&format!("SELECT {COLS} FROM games WHERE {clause}"))?;
        let rows = st.query_map([], Self::row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Merges a Steam scan. Games missing from `entries` keep their row but are marked uninstalled.
    pub fn sync_steam(&mut self, entries: &[SteamEntry]) -> Result<usize> {
        let tx = self.conn.transaction()?;
        tx.execute("UPDATE games SET installed = 0 WHERE source = 'steam'", [])?;
        let now = crate::now();
        {
            let mut st = tx.prepare(
                "INSERT INTO games (source, source_id, name, installed, install_dir, playtime_min, last_played, added)
                 VALUES ('steam', ?1, ?2, ?3, ?4, COALESCE(?5, 0), ?6, ?7)
                 ON CONFLICT(source_id) WHERE source = 'steam' DO UPDATE SET
                    installed = excluded.installed,
                    install_dir = COALESCE(excluded.install_dir, install_dir),
                    playtime_min = COALESCE(?5, playtime_min),
                    last_played = MAX(last_played, excluded.last_played)",
            )?;
            for e in entries {
                st.execute(params![
                    e.appid.to_string(),
                    e.name,
                    e.installed,
                    e.install_dir,
                    e.playtime_min,
                    e.last_played,
                    now
                ])?;
            }
        }
        tx.commit()?;
        Ok(entries.len())
    }

    pub fn add_custom(&self, g: &Game) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO games (source, source_id, name, installed, exe, args, work_dir, added)
             VALUES ('custom', '', ?1, 1, ?2, ?3, ?4, ?5)",
            params![g.name, g.exe, g.args, g.work_dir, crate::now()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Saves user-editable fields.
    pub fn update_user_fields(&self, g: &Game) -> Result<()> {
        self.conn.execute(
            "UPDATE games SET name = ?2, exe = ?3, args = ?4, work_dir = ?5, favorite = ?6, hidden = ?7,
             description = ?8, developer = ?9, publisher = ?10, release_date = ?11, genres = ?12
             WHERE id = ?1",
            params![
                g.id, g.name, g.exe, g.args, g.work_dir, g.favorite, g.hidden,
                g.description, g.developer, g.publisher, g.release_date, g.genres
            ],
        )?;
        Ok(())
    }

    pub fn delete(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM games WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Fills metadata fields only where they are still empty, so user edits win.
    pub fn save_metadata(&self, id: i64, m: &Metadata, matched_appid: Option<u32>) -> Result<()> {
        self.conn.execute(
            "UPDATE games SET
                description = COALESCE(description, ?2), developer = COALESCE(developer, ?3),
                publisher = COALESCE(publisher, ?4), release_date = COALESCE(release_date, ?5),
                genres = COALESCE(genres, ?6), meta_state = 1,
                source_id = CASE WHEN source = 'custom' AND ?7 IS NOT NULL THEN ?7 ELSE source_id END
             WHERE id = ?1",
            params![
                id, m.description, m.developer, m.publisher, m.release_date, m.genres,
                matched_appid.map(|a| a.to_string())
            ],
        )?;
        Ok(())
    }

    pub fn set_meta_state(&self, id: i64, st: i32) -> Result<()> {
        self.conn.execute("UPDATE games SET meta_state = ?2 WHERE id = ?1", params![id, st])?;
        Ok(())
    }

    pub fn set_cover_state(&self, id: i64, has_cover: bool, has_hero: Option<bool>, st: i32) -> Result<()> {
        self.conn.execute(
            "UPDATE games SET has_cover = ?2, has_hero = COALESCE(?3, has_hero), cover_state = ?4 WHERE id = ?1",
            params![id, has_cover, has_hero, st],
        )?;
        Ok(())
    }

    pub fn set_favorite(&self, id: i64, fav: bool) -> Result<()> {
        self.conn.execute("UPDATE games SET favorite = ?2 WHERE id = ?1", params![id, fav])?;
        Ok(())
    }

    pub fn set_hidden(&self, id: i64, hidden: bool) -> Result<()> {
        self.conn.execute("UPDATE games SET hidden = ?2 WHERE id = ?1", params![id, hidden])?;
        Ok(())
    }

    pub fn add_playtime(&self, id: i64, minutes: i64, when: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE games SET playtime_min = playtime_min + ?2, last_played = MAX(last_played, ?3) WHERE id = ?1",
            params![id, minutes, when],
        )?;
        Ok(())
    }

    /// Re-queues a game for scraping. `force` also clears scraped fields.
    pub fn requeue(&self, id: i64, force: bool) -> Result<()> {
        if force {
            self.conn.execute(
                "UPDATE games SET cover_state = 0, meta_state = 0, description = NULL, developer = NULL,
                 publisher = NULL, release_date = NULL, genres = NULL WHERE id = ?1",
                [id],
            )?;
        } else {
            self.conn
                .execute("UPDATE games SET cover_state = 0, meta_state = 0 WHERE id = ?1", [id])?;
        }
        Ok(())
    }

    /// Retries every game whose cover wasn't found (e.g. after adding a SteamGridDB key).
    pub fn retry_missing_covers(&self) -> Result<usize> {
        Ok(self.conn.execute(
            "UPDATE games SET cover_state = 0 WHERE cover_state = ?1",
            [state::NOT_FOUND],
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steam_sync_and_custom() {
        let dir = std::env::temp_dir().join(format!("gamut-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut lib = Library::open(&dir.join("t.db")).unwrap();
        let e = |id, inst| SteamEntry {
            appid: id,
            name: format!("G{id}"),
            installed: inst,
            install_dir: None,
            playtime_min: None,
            last_played: 0,
        };
        lib.sync_steam(&[e(1, true), e(2, false)]).unwrap();
        lib.sync_steam(&[e(2, true)]).unwrap();
        let all = lib.all().unwrap();
        assert_eq!(all.len(), 2);
        assert!(!all.iter().find(|g| g.source_id == "1").unwrap().installed);
        assert!(all.iter().find(|g| g.source_id == "2").unwrap().installed);

        let id = lib.add_custom(&Game::new_custom("Mine".into(), None)).unwrap();
        lib.add_playtime(id, 30, 100).unwrap();
        assert_eq!(lib.get(id).unwrap().unwrap().playtime_min, 30);
        drop(lib);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
