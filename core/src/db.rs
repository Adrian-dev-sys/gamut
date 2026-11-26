use crate::model::{state, Game, Metadata, Source};
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension, Row};
use std::path::Path;

/// SQLite-backed game library. Each thread opens its own handle (rollback journal).
pub struct Library {
    conn: Connection,
}

const COLS: &str = "id, source, source_id, name, installed, install_dir, exe, args, work_dir, \
    playtime_min, last_played, description, developer, publisher, release_date, genres, \
    has_cover, has_hero, cover_state, meta_state, favorite, hidden, added, \
    playnite_id, platform, store, completion, play_count, critic_score, community_score, tags, series, specialk, reshade, auto_hdr";

/// Columns added after the first release: (name, SQL type).
const ADDED_COLS: [(&str, &str); 12] = [
    ("playnite_id", "TEXT"),
    ("platform", "TEXT"),
    ("store", "TEXT"),
    ("completion", "TEXT"),
    ("play_count", "INTEGER NOT NULL DEFAULT 0"),
    ("critic_score", "INTEGER"),
    ("community_score", "INTEGER"),
    ("tags", "TEXT"),
    ("series", "TEXT"),
    ("specialk", "INTEGER"),
    ("reshade", "INTEGER"),
    ("auto_hdr", "INTEGER NOT NULL DEFAULT 0"),
];

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
        conn.busy_timeout(std::time::Duration::from_secs(30))?;
        // Rollback journal, not WAL: WAL's shared-memory index is what two processes corrupted twice.
        // Leaving WAL needs exclusive access, so the first handle (app start, before workers) converts.
        let mode: String = conn.query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
        if !mode.eq_ignore_ascii_case("delete") {
            let _ = conn.query_row("PRAGMA journal_mode=DELETE", [], |_| Ok(()));
        }
        conn.execute_batch(
            "PRAGMA synchronous=FULL;
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
        let mut lib = Library { conn };
        lib.migrate()?;
        Ok(lib)
    }

    /// Adds missing columns; safe to race with another handle (checked under a write lock).
    fn migrate(&mut self) -> Result<()> {
        let missing = |c: &Connection| -> rusqlite::Result<Vec<(&str, &str)>> {
            let mut st = c.prepare("SELECT name FROM pragma_table_info('games')")?;
            let have: Vec<String> = st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
            Ok(ADDED_COLS.iter().copied().filter(|(n, _)| !have.iter().any(|h| h == n)).collect())
        };
        let version = |c: &Connection| c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0));
        if missing(&self.conn)?.is_empty() && version(&self.conn)? >= 2 {
            return Ok(());
        }
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        for (name, ty) in missing(&tx)? {
            tx.execute(&format!("ALTER TABLE games ADD COLUMN {name} {ty}"), [])?;
        }
        tx.execute(
            "CREATE UNIQUE INDEX IF NOT EXISTS games_playnite ON games(playnite_id) WHERE playnite_id IS NOT NULL",
            [],
        )?;
        if version(&tx)? < 2 {
            // v2: games with a cover but no banner go back to the art queue once, for a banner only.
            tx.execute("UPDATE games SET cover_state = 0 WHERE has_cover = 1 AND has_hero = 0 AND cover_state = 1", [])?;
            tx.execute("PRAGMA user_version = 2", [])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Opens the library; if SQLite finds damage, rebuilds it from every readable row.
    /// Returns a user-facing notice when a repair happened.
    pub fn open_checked(path: &Path) -> Result<(Self, Option<String>)> {
        // Only real damage triggers a rebuild; a busy or missing file is an error, not corruption.
        let corrupt = |e: &rusqlite::Error| {
            matches!(e.sqlite_error_code(), Some(rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase))
        };
        match Self::open(path) {
            Ok(lib) => match lib.conn.query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0)) {
                Ok(r) if r == "ok" => return Ok((lib, None)),
                Ok(_) => {}
                Err(e) if corrupt(&e) => {}
                Err(e) => return Err(e.into()),
            },
            Err(e) if e.downcast_ref::<rusqlite::Error>().is_some_and(corrupt) => {}
            Err(e) => return Err(e),
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
            let cols = Self::readable_cols(&old);
            // Sequential scan until the first unreadable row...
            if let Ok(mut st) = old.prepare(&format!("SELECT {cols} FROM games")) {
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
            let sql = format!("SELECT {cols} FROM games WHERE id = ?1");
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

    /// COLS, with defaults in place of columns an older (unmigrated) file lacks.
    fn readable_cols(c: &Connection) -> String {
        let have: Vec<String> = c
            .prepare("SELECT name FROM pragma_table_info('games')")
            .and_then(|mut st| st.query_map([], |r| r.get(0))?.collect())
            .unwrap_or_default();
        COLS.split(',')
            .map(str::trim)
            .map(|col| match ADDED_COLS.iter().find(|(n, _)| *n == col) {
                Some((n, ty)) if !have.iter().any(|h| h == n) => {
                    if ty.contains("DEFAULT 0") { "0" } else { "NULL" }.to_string()
                }
                _ => col.to_string(),
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Inserts a complete row, keeping its id (used when rebuilding).
    fn insert_full(&self, g: &Game) -> Result<()> {
        self.conn.execute(
            &format!("INSERT OR REPLACE INTO games ({COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31, ?32, ?33, ?34, ?35)"),
            params![
                g.id, g.source.as_str(), g.source_id, g.name, g.installed, g.install_dir, g.exe, g.args, g.work_dir,
                g.playtime_min, g.last_played, g.description, g.developer, g.publisher, g.release_date, g.genres,
                g.has_cover, g.has_hero, g.cover_state, g.meta_state, g.favorite, g.hidden, g.added,
                g.playnite_id, g.platform, g.store, g.completion, g.play_count, g.critic_score, g.community_score,
                g.tags, g.series, g.specialk, g.reshade, g.auto_hdr
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
            playnite_id: r.get(23)?,
            platform: r.get(24)?,
            store: r.get(25)?,
            completion: r.get(26)?,
            play_count: r.get(27)?,
            critic_score: r.get(28)?,
            community_score: r.get(29)?,
            tags: r.get(30)?,
            series: r.get(31)?,
            specialk: r.get(32)?,
            reshade: r.get(33)?,
            auto_hdr: r.get(34)?,
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
        self.query_where("cover_state IN (0, 3) ORDER BY installed DESC, last_played DESC")
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
             description = ?8, developer = ?9, publisher = ?10, release_date = ?11, genres = ?12,
             platform = ?13, store = ?14, completion = ?15, critic_score = ?16, community_score = ?17,
             tags = ?18, series = ?19, specialk = ?20, reshade = ?21, auto_hdr = ?22
             WHERE id = ?1",
            params![
                g.id, g.name, g.exe, g.args, g.work_dir, g.favorite, g.hidden,
                g.description, g.developer, g.publisher, g.release_date, g.genres,
                g.platform, g.store, g.completion, g.critic_score, g.community_score, g.tags, g.series,
                g.specialk, g.reshade, g.auto_hdr
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

    /// Replaces descriptions of Playnite-imported games (by Playnite id) with `new` ones, only where
    /// the current text is empty or the same apart from images, so edits made in Gamut are kept.
    /// Returns (updated, kept because edited).
    pub fn refresh_descriptions(&mut self, new: &[(String, String)]) -> Result<(usize, usize)> {
        let tx = self.conn.transaction()?;
        let (mut updated, mut kept) = (0, 0);
        {
            let mut get = tx.prepare("SELECT id, description FROM games WHERE playnite_id = ?1")?;
            let mut set = tx.prepare("UPDATE games SET description = ?2 WHERE id = ?1")?;
            for (pid, desc) in new {
                let Some((id, cur)) = get.query_row([pid], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?))).optional()? else {
                    continue;
                };
                match cur {
                    Some(c) if c == *desc => {}
                    Some(c) if crate::scrape::text_only(&c) != crate::scrape::text_only(desc) => kept += 1,
                    _ => {
                        set.execute(params![id, desc])?;
                        updated += 1;
                    }
                }
            }
        }
        tx.commit()?;
        Ok((updated, kept))
    }

    /// Upserts Playnite games in one transaction and returns their row ids (same order).
    /// Merge: playtime/last played/play count take the max, added the min, favorite/hidden are OR'd,
    /// Playnite's metadata wins where it has a value.
    pub fn import_playnite(&mut self, games: &[Game]) -> Result<Vec<i64>> {
        const SHARED: &str = "
            playtime_min = MAX(playtime_min, excluded.playtime_min),
            last_played = MAX(last_played, excluded.last_played),
            added = CASE WHEN excluded.added > 0 AND (added = 0 OR excluded.added < added) THEN excluded.added ELSE added END,
            play_count = MAX(play_count, excluded.play_count),
            favorite = favorite OR excluded.favorite,
            hidden = hidden OR excluded.hidden,
            playnite_id = excluded.playnite_id,
            platform = COALESCE(excluded.platform, platform),
            store = COALESCE(excluded.store, store),
            completion = COALESCE(excluded.completion, completion),
            critic_score = COALESCE(excluded.critic_score, critic_score),
            community_score = COALESCE(excluded.community_score, community_score),
            tags = COALESCE(excluded.tags, tags),
            series = COALESCE(excluded.series, series),
            specialk = COALESCE(excluded.specialk, specialk),
            description = COALESCE(excluded.description, description),
            developer = COALESCE(excluded.developer, developer),
            publisher = COALESCE(excluded.publisher, publisher),
            release_date = COALESCE(excluded.release_date, release_date),
            genres = COALESCE(excluded.genres, genres)";
        let insert = "INSERT INTO games (source, source_id, name, installed, install_dir, exe, args, work_dir,
                playtime_min, last_played, added, play_count, favorite, hidden, playnite_id, platform, store,
                completion, critic_score, community_score, tags, series, description, developer, publisher,
                release_date, genres, meta_state, cover_state, specialk)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20,
                ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30)";
        let steam_sql = format!(
            "{insert} ON CONFLICT(source_id) WHERE source = 'steam' DO UPDATE SET {SHARED},
                meta_state = CASE WHEN excluded.description IS NOT NULL THEN 1 ELSE meta_state END
             RETURNING id"
        );
        let custom_sql = format!(
            "{insert} ON CONFLICT(playnite_id) WHERE playnite_id IS NOT NULL DO UPDATE SET {SHARED},
                name = excluded.name, installed = excluded.installed, install_dir = excluded.install_dir,
                exe = excluded.exe, args = excluded.args, work_dir = excluded.work_dir
             RETURNING id"
        );
        let tx = self.conn.transaction()?;
        let mut ids = Vec::with_capacity(games.len());
        {
            let mut steam = tx.prepare(&steam_sql)?;
            let mut custom = tx.prepare(&custom_sql)?;
            for g in games {
                let st = if g.source == Source::Steam { &mut steam } else { &mut custom };
                // Imported custom games skip the scrapers: art comes from Playnite (cover_state 2 until copied).
                let cover_state = if g.source == Source::Steam { state::PENDING } else { state::NOT_FOUND };
                let id = st.query_row(
                    params![
                        g.source.as_str(), g.source_id, g.name, g.installed, g.install_dir, g.exe, g.args, g.work_dir,
                        g.playtime_min, g.last_played, g.added, g.play_count, g.favorite, g.hidden, g.playnite_id,
                        g.platform, g.store, g.completion, g.critic_score, g.community_score, g.tags, g.series,
                        g.description, g.developer, g.publisher, g.release_date, g.genres, g.meta_state, cover_state,
                        g.specialk
                    ],
                    |r| r.get(0),
                )?;
                ids.push(id);
            }
        }
        tx.commit()?;
        Ok(ids)
    }

    /// Records imported art: (id, cover copied, background copied, cover has the wrong shape).
    /// Anything still missing a cover or banner goes to the art queue.
    pub fn apply_imported_art(&mut self, results: &[(i64, bool, bool, bool)]) -> Result<()> {
        let tx = self.conn.transaction()?;
        for &(id, cover, hero, weak) in results {
            tx.execute(
                "UPDATE games SET has_cover = has_cover OR ?2, has_hero = has_hero OR ?3,
                    cover_state = CASE
                        WHEN ?2 AND ?4 THEN 3
                        WHEN ?2 AND NOT (has_hero OR ?3) THEN 0
                        WHEN ?2 THEN 1
                        WHEN has_cover = 0 THEN 0
                        ELSE cover_state END
                 WHERE id = ?1",
                params![id, cover, hero, weak],
            )?;
        }
        // Imported games Playnite had no art for at all.
        tx.execute(
            "UPDATE games SET cover_state = 0 WHERE playnite_id IS NOT NULL AND has_cover = 0 AND cover_state = 2",
            [],
        )?;
        tx.commit()?;
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
    fn migrates_old_schema_and_merges_playnite() {
        let dir = std::env::temp_dir().join(format!("gamut-mig-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.db");
        {
            // Schema as shipped before the Playnite columns existed.
            let c = Connection::open(&path).unwrap();
            c.execute_batch(
                "CREATE TABLE games (id INTEGER PRIMARY KEY, source TEXT NOT NULL, source_id TEXT NOT NULL DEFAULT '',
                 name TEXT NOT NULL, installed INTEGER NOT NULL DEFAULT 0, install_dir TEXT, exe TEXT, args TEXT,
                 work_dir TEXT, playtime_min INTEGER NOT NULL DEFAULT 0, last_played INTEGER NOT NULL DEFAULT 0,
                 description TEXT, developer TEXT, publisher TEXT, release_date TEXT, genres TEXT,
                 has_cover INTEGER NOT NULL DEFAULT 0, has_hero INTEGER NOT NULL DEFAULT 0,
                 cover_state INTEGER NOT NULL DEFAULT 0, meta_state INTEGER NOT NULL DEFAULT 0,
                 favorite INTEGER NOT NULL DEFAULT 0, hidden INTEGER NOT NULL DEFAULT 0, added INTEGER NOT NULL DEFAULT 0);
                 INSERT INTO games (source, source_id, name, playtime_min, added) VALUES ('steam', '10', 'Old', 100, 500);",
            )
            .unwrap();
            assert!(Library::readable_cols(&c).contains("NULL"));
        }
        let mut lib = Library::open(&path).unwrap();
        let mut steam = Game::new_custom("Old".into(), None);
        steam.source = Source::Steam;
        steam.source_id = "10".into();
        steam.playnite_id = Some("a".into());
        steam.playtime_min = 50;
        steam.added = 200;
        steam.favorite = true;
        steam.completion = Some("Beaten".into());
        let mut custom = Game::new_custom("Rom".into(), Some("emu.exe".into()));
        custom.playnite_id = Some("b".into());
        let ids = lib.import_playnite(&[steam.clone(), custom.clone()]).unwrap();
        custom.name = "Rom (renamed)".into();
        let again = lib.import_playnite(&[steam, custom]).unwrap();
        assert_eq!(ids, again, "re-import updates the same rows");
        let all = lib.all().unwrap();
        assert_eq!(all.len(), 2);
        let s = all.iter().find(|g| g.source_id == "10").unwrap();
        assert_eq!((s.playtime_min, s.added, s.favorite), (100, 200, true));
        assert_eq!(s.completion.as_deref(), Some("Beaten"));
        assert_eq!(all.iter().find(|g| g.playnite_id.as_deref() == Some("b")).unwrap().name, "Rom (renamed)");
        drop(lib);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refreshes_only_unedited_descriptions() {
        let dir = std::env::temp_dir().join(format!("gamut-desc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut lib = Library::open(&dir.join("t.db")).unwrap();
        let g = |pid: &str, d: Option<&str>| {
            let mut g = Game::new_custom(pid.into(), None);
            g.playnite_id = Some(pid.into());
            g.description = d.map(Into::into);
            g
        };
        let games = [g("a", Some("Hello world")), g("b", Some("My own text")), g("c", None)];
        lib.import_playnite(&games).unwrap();
        let new = |pid: &str, d: &str| (pid.to_string(), d.to_string());
        let r = lib
            .refresh_descriptions(&[new("a", "Hello
[img]https://x/1.png
world"), new("b", "Store text"), new("c", "C"), new("zz", "X")])
            .unwrap();
        assert_eq!(r, (2, 1));
        let all = lib.all().unwrap();
        let d = |pid: &str| all.iter().find(|g| g.playnite_id.as_deref() == Some(pid)).unwrap().description.clone();
        assert_eq!(d("a").as_deref(), Some("Hello
[img]https://x/1.png
world"));
        assert_eq!(d("b").as_deref(), Some("My own text"));
        assert_eq!(d("c").as_deref(), Some("C"));
        let _ = std::fs::remove_dir_all(&dir);
    }

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
