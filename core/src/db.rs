use crate::achievements::{Achievement, Fetched, State as AchState};
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
    playnite_id, platform, store, completion, play_count, critic_score, community_score, tags, series, specialk, reshade, auto_hdr, ra_id, \
    sort_name, user_score, notes, links, actions, script_before, script_after, install_size, save_dir";

const ACH_COLS: &str = "key, name, description, icon, icon_locked, hidden, unlocked, percent, points";

/// Columns added after the first release: (name, SQL type).
const ADDED_COLS: [(&str, &str); 22] = [
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
    ("ra_id", "INTEGER"),
    ("sort_name", "TEXT"),
    ("user_score", "INTEGER"),
    ("notes", "TEXT"),
    ("links", "TEXT"),
    ("actions", "TEXT"),
    ("script_before", "TEXT"),
    ("script_after", "TEXT"),
    ("install_size", "INTEGER"),
    ("save_dir", "TEXT"),
];

/// A Steam app as seen by a scan (local manifest and/or owned-games API).
#[derive(Debug, Clone)]
pub struct SteamEntry {
    pub appid: u32,
    pub name: String,
    pub installed: bool,
    pub install_dir: Option<String>,
    /// Bytes on disk (from the app manifest).
    pub size: Option<i64>,
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
             CREATE UNIQUE INDEX IF NOT EXISTS games_steam ON games(source_id) WHERE source = 'steam';
             CREATE TABLE IF NOT EXISTS achievements (
                game_id INTEGER NOT NULL, key TEXT NOT NULL, ord INTEGER NOT NULL DEFAULT 0,
                name TEXT NOT NULL DEFAULT '', description TEXT NOT NULL DEFAULT '',
                icon TEXT NOT NULL DEFAULT '', icon_locked TEXT NOT NULL DEFAULT '',
                hidden INTEGER NOT NULL DEFAULT 0, unlocked INTEGER NOT NULL DEFAULT 0,
                percent REAL, points INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (game_id, key)
             );
             -- Play sessions for statistics. kind 0 = timed by Gamut, 1 = Steam total grew between scans.
             CREATE TABLE IF NOT EXISTS sessions (
                id INTEGER PRIMARY KEY, game_id INTEGER NOT NULL, start INTEGER NOT NULL,
                minutes INTEGER NOT NULL, kind INTEGER NOT NULL DEFAULT 0
             );
             CREATE INDEX IF NOT EXISTS sessions_start ON sessions(start);
             CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value INTEGER NOT NULL);
             -- IGDB game per game: >0 the IGDB id, 0 = looked up, no confident match.
             CREATE TABLE IF NOT EXISTS igdb_match (game_id INTEGER PRIMARY KEY, igdb_id INTEGER NOT NULL);
             -- Last Steam API total per game and when it was first seen at that value (session baseline).
             CREATE TABLE IF NOT EXISTS hltb (game_id INTEGER PRIMARY KEY, hltb_id INTEGER NOT NULL, main INTEGER NOT NULL,
                 plus INTEGER NOT NULL, full INTEGER NOT NULL, fetched INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS steam_seen (game_id INTEGER PRIMARY KEY, minutes INTEGER NOT NULL, since INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS achievement_state (
                game_id INTEGER PRIMARY KEY, fetched INTEGER NOT NULL DEFAULT 0, schema_at INTEGER NOT NULL DEFAULT 0,
                total INTEGER NOT NULL DEFAULT 0, unlocked INTEGER NOT NULL DEFAULT 0, error TEXT
             );",
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
        if missing(&self.conn)?.is_empty() && version(&self.conn)? >= 4 {
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
        if version(&tx)? < 3 {
            // v3: Steam games from the library's first day were stamped with scan time, not a real
            // added date; mark them unknown so they don't bury actual new games.
            tx.execute(
                "UPDATE games SET added = 0 WHERE source = 'steam' AND playnite_id IS NULL AND added < 86400 +
                 (SELECT MIN(added) FROM games WHERE source = 'steam' AND playnite_id IS NULL AND added > 0)",
                [],
            )?;
            tx.execute("PRAGMA user_version = 3", [])?;
        }
        if version(&tx)? < 4 {
            // v4: title matching learned RA's "Version" suffix; look unmatched games up again.
            tx.execute("UPDATE games SET ra_id = NULL WHERE ra_id = 0", [])?;
            tx.execute("PRAGMA user_version = 4", [])?;
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
            &format!("INSERT OR REPLACE INTO games ({COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31, ?32, ?33, ?34, ?35, ?36, ?37, ?38, ?39, ?40, ?41, ?42, ?43, ?44, ?45)"),
            params![
                g.id, g.source.as_str(), g.source_id, g.name, g.installed, g.install_dir, g.exe, g.args, g.work_dir,
                g.playtime_min, g.last_played, g.description, g.developer, g.publisher, g.release_date, g.genres,
                g.has_cover, g.has_hero, g.cover_state, g.meta_state, g.favorite, g.hidden, g.added,
                g.playnite_id, g.platform, g.store, g.completion, g.play_count, g.critic_score, g.community_score,
                g.tags, g.series, g.specialk, g.reshade, g.auto_hdr, g.ra_id,
                g.sort_name, g.user_score, g.notes, g.links, g.actions, g.script_before, g.script_after, g.install_size, g.save_dir
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
            ra_id: r.get(35)?,
            sort_name: r.get(36)?,
            user_score: r.get(37)?,
            notes: r.get(38)?,
            links: r.get(39)?,
            actions: r.get(40)?,
            script_before: r.get(41)?,
            script_after: r.get(42)?,
            install_size: r.get(43)?,
            save_dir: r.get(44)?,
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
        // The first scan finds the whole existing library: no real added date for those.
        let first = tx.query_row("SELECT COUNT(*) FROM games WHERE source = 'steam'", [], |r| r.get::<_, i64>(0))? == 0;
        tx.execute("UPDATE games SET installed = 0 WHERE source = 'steam'", [])?;
        let now = if first { 0 } else { crate::now() };
        // Steam totals that grew become sessions. Baseline per game: the API total and when it was first
        // seen at that value (`steam_seen`), so a session still running during one scan counts at the next.
        // Growth that can't fit since then (first sightings, imports) isn't play.
        let scan_at = crate::now();
        {
            let mut seen = tx.prepare("SELECT s.game_id, s.minutes, s.since FROM games g JOIN steam_seen s ON s.game_id = g.id WHERE g.source = 'steam' AND g.source_id = ?1")?;
            let mut id_of = tx.prepare("SELECT id FROM games WHERE source = 'steam' AND source_id = ?1")?;
            let mut set = tx.prepare("INSERT OR REPLACE INTO steam_seen (game_id, minutes, since) VALUES (?1, ?2, ?3)")?;
            let mut log = tx.prepare("INSERT INTO sessions (game_id, start, minutes, kind) VALUES (?1, ?2, ?3, 1)")?;
            for e in entries {
                let Some(new) = e.playtime_min else { continue };
                let app = e.appid.to_string();
                match seen.query_row([&app], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))).optional()? {
                    Some((_, was, _)) if was == new => {}
                    Some((id, was, since)) => {
                        let delta = new - was;
                        if delta > 0 && delta <= (scan_at - since) / 60 + 10 {
                            // Dated at Steam's last-played time when it's plausible, else back from now.
                            let start = match e.last_played {
                                t if t >= since && t <= scan_at => t,
                                _ => scan_at - delta * 60,
                            };
                            log.execute(params![id, start, delta])?;
                        }
                        set.execute(params![id, new, scan_at])?;
                    }
                    None => {
                        // Not in the library yet (inserted below): seen on the next scan.
                        if let Some(id) = id_of.query_row([&app], |r| r.get::<_, i64>(0)).optional()? {
                            set.execute(params![id, new, scan_at])?;
                        }
                    }
                }
            }
        }
        if entries.iter().any(|e| e.playtime_min.is_some()) {
            tx.execute("INSERT OR IGNORE INTO meta (key, value) VALUES ('steam_scan_first', ?1)", [scan_at])?;
        }
        {
            let mut st = tx.prepare(
                "INSERT INTO games (source, source_id, name, installed, install_dir, playtime_min, last_played, added)
                 VALUES ('steam', ?1, ?2, ?3, ?4, COALESCE(?5, 0), ?6, ?7)
                 ON CONFLICT(source_id) WHERE source = 'steam' DO UPDATE SET
                    installed = excluded.installed,
                    install_dir = COALESCE(excluded.install_dir, install_dir),
                    playtime_min = COALESCE(?5, playtime_min),
                    last_played = MAX(last_played, excluded.last_played),
                    install_size = COALESCE(?8, install_size)",
            )?;
            for e in entries {
                st.execute(params![
                    e.appid.to_string(),
                    e.name,
                    e.installed,
                    e.install_dir,
                    e.playtime_min,
                    e.last_played,
                    now,
                    e.size
                ])?;
            }
        }
        tx.commit()?;
        Ok(entries.len())
    }

    /// Adds scanned games (name, exe, working dir) in one transaction, with a platform/tags ("" = none).
    /// `games`: name, exe, working dir, tags (comma separated).
    pub fn add_scanned(&mut self, games: &[(String, String, Option<String>, String)], platform: &str) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let opt = |s: &str| (!s.trim().is_empty()).then(|| s.trim().to_string());
        for (name, exe, dir, tags) in games {
            tx.execute(
                "INSERT INTO games (source, source_id, name, installed, exe, work_dir, added, platform, tags)
                 VALUES ('custom', '', ?1, 1, ?2, ?3, ?4, ?5, ?6)",
                params![name, exe, dir, crate::now(), opt(platform), opt(tags)],
            )?;
        }
        tx.commit()?;
        Ok(games.len())
    }

    /// Adds comma-separated tags to games (kept: existing tags, case-insensitive duplicates). Returns how many changed.
    pub fn add_tags(&mut self, add: &[(i64, String)]) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let mut changed = 0;
        for (id, tags) in add {
            let old: Option<String> = tx.query_row("SELECT tags FROM games WHERE id = ?1", [id], |r| r.get(0)).optional()?.flatten();
            let mut list: Vec<String> = old.as_deref().unwrap_or("").split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect();
            let before = list.len();
            for t in tags.split(',').map(str::trim).filter(|t| !t.is_empty()) {
                if !list.iter().any(|x| x.eq_ignore_ascii_case(t)) {
                    list.push(t.to_string());
                }
            }
            if list.len() != before {
                tx.execute("UPDATE games SET tags = ?2 WHERE id = ?1", params![id, list.join(", ")])?;
                changed += 1;
            }
        }
        tx.commit()?;
        Ok(changed)
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
        // A renamed or moved game needs a new IGDB lookup.
        self.conn.execute(
            "DELETE FROM igdb_match WHERE game_id = ?1 AND EXISTS
             (SELECT 1 FROM games WHERE id = ?1 AND (name IS NOT ?2 OR platform IS NOT ?3))",
            params![g.id, g.name, g.platform],
        )?;
        self.conn.execute(
            "UPDATE games SET name = ?2, exe = ?3, args = ?4, work_dir = ?5, favorite = ?6, hidden = ?7,
             description = ?8, developer = ?9, publisher = ?10, release_date = ?11, genres = ?12,
             platform = ?13, store = ?14, completion = ?15, critic_score = ?16, community_score = ?17,
             tags = ?18, series = ?19, specialk = ?20, reshade = ?21, auto_hdr = ?22,
             sort_name = ?23, user_score = ?24, notes = ?25, links = ?26, actions = ?27, script_before = ?28,
             script_after = ?29, save_dir = ?30, install_size = ?31,
             -- Steam knows where its games are.
             install_dir = CASE WHEN source = 'custom' THEN ?32 ELSE install_dir END,
             -- A new platform (or a new name for an unmatched game) means a new RetroAchievements lookup.
             ra_id = CASE WHEN platform IS NOT ?13 OR (ra_id <= 0 AND name IS NOT ?2) THEN NULL ELSE ra_id END
             WHERE id = ?1",
            params![
                g.id, g.name, g.exe, g.args, g.work_dir, g.favorite, g.hidden,
                g.description, g.developer, g.publisher, g.release_date, g.genres,
                g.platform, g.store, g.completion, g.critic_score, g.community_score, g.tags, g.series,
                g.specialk, g.reshade, g.auto_hdr,
                g.sort_name, g.user_score, g.notes, g.links, g.actions, g.script_before, g.script_after, g.save_dir,
                g.install_size, g.install_dir
            ],
        )?;
        Ok(())
    }

    pub fn delete(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM games WHERE id = ?1", [id])?;
        // Ids get reused; a new game must not inherit these.
        self.conn.execute("DELETE FROM achievements WHERE game_id = ?1", [id])?;
        self.conn.execute("DELETE FROM achievement_state WHERE game_id = ?1", [id])?;
        self.conn.execute("DELETE FROM sessions WHERE game_id = ?1", [id])?;
        self.conn.execute("DELETE FROM steam_seen WHERE game_id = ?1", [id])?;
        self.conn.execute("DELETE FROM igdb_match WHERE game_id = ?1", [id])?;
        self.conn.execute("DELETE FROM hltb WHERE game_id = ?1", [id])?;
        Ok(())
    }

    /// Cached achievements for a game, in the source's order.
    pub fn achievements(&self, id: i64) -> Result<Vec<Achievement>> {
        let mut st = self.conn.prepare_cached(&format!("SELECT {ACH_COLS} FROM achievements WHERE game_id = ?1 ORDER BY ord"))?;
        let rows = st.query_map([id], Self::ach_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    fn ach_row(r: &Row) -> rusqlite::Result<Achievement> {
        Ok(Achievement {
            key: r.get(0)?,
            name: r.get(1)?,
            description: r.get(2)?,
            icon: r.get(3)?,
            icon_locked: r.get(4)?,
            hidden: r.get(5)?,
            unlocked: r.get(6)?,
            percent: r.get(7)?,
            points: r.get(8)?,
        })
    }

    /// Unlocked achievements of all games as (game id, achievement): newest first, or rarest first.
    pub fn unlocked_achievements(&self, rarest: bool, limit: usize) -> Result<Vec<(i64, Achievement)>> {
        let order = if rarest { "percent IS NULL, percent ASC, unlocked DESC" } else { "unlocked DESC" };
        let mut st = self.conn.prepare(&format!(
            "SELECT {ACH_COLS}, game_id FROM achievements WHERE unlocked > 0 ORDER BY {order} LIMIT ?1"
        ))?;
        let rows = st.query_map([limit as i64], |r| Ok((r.get(9)?, Self::ach_row(r)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Games that have achievements: (game id, total, unlocked, newest unlock time, points earned).
    pub fn achievement_progress(&self) -> Result<Vec<(i64, i64, i64, i64, i64)>> {
        let mut st = self.conn.prepare(
            "SELECT game_id, COUNT(*), SUM(unlocked > 0), MAX(unlocked), SUM(CASE WHEN unlocked > 0 THEN points ELSE 0 END)
             FROM achievements GROUP BY game_id",
        )?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn achievement_state(&self, id: i64) -> Result<Option<AchState>> {
        Ok(self
            .conn
            .query_row(
                "SELECT fetched, schema_at, total, unlocked, error FROM achievement_state WHERE game_id = ?1",
                [id],
                |r| Ok(AchState { fetched: r.get(0)?, schema_at: r.get(1)?, total: r.get(2)?, unlocked: r.get(3)?, error: r.get(4)? }),
            )
            .optional()?)
    }

    /// Stores a fetch. Returns false when a partial (unlocks-only) result doesn't match the cache,
    /// i.e. a full fetch is needed.
    pub fn save_achievements(&mut self, id: i64, f: &Fetched) -> Result<bool> {
        let now = crate::now();
        // Immediate: the unlock-only path reads then writes, which a deferred lock can't upgrade under contention.
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let schema_at = match f {
            Fetched::None => {
                tx.execute("DELETE FROM achievements WHERE game_id = ?1", [id])?;
                now
            }
            Fetched::List { items, full: true } => {
                tx.execute("DELETE FROM achievements WHERE game_id = ?1", [id])?;
                for (i, a) in items.iter().enumerate() {
                    tx.execute(
                        "INSERT OR REPLACE INTO achievements (game_id, key, ord, name, description, icon, icon_locked, hidden, unlocked, percent, points)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                        params![id, a.key, i as i64, a.name, a.description, a.icon, a.icon_locked, a.hidden, a.unlocked, a.percent, a.points],
                    )?;
                }
                now
            }
            Fetched::List { items, full: false } => {
                let cached: i64 = tx.query_row("SELECT COUNT(*) FROM achievements WHERE game_id = ?1", [id], |r| r.get(0))?;
                let mut hit = 0;
                for a in items {
                    hit += tx.execute(
                        "UPDATE achievements SET unlocked = ?3 WHERE game_id = ?1 AND key = ?2",
                        params![id, a.key, a.unlocked],
                    )?;
                }
                if hit != items.len() || cached != items.len() as i64 {
                    return Ok(false);
                }
                tx.query_row("SELECT schema_at FROM achievement_state WHERE game_id = ?1", [id], |r| r.get(0))
                    .optional()?
                    .unwrap_or(0)
            }
        };
        tx.execute(
            "INSERT OR REPLACE INTO achievement_state (game_id, fetched, schema_at, total, unlocked, error)
             SELECT ?1, ?2, ?3, COUNT(*), COALESCE(SUM(unlocked > 0), 0), NULL FROM achievements WHERE game_id = ?1",
            params![id, now, schema_at],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// Records a fetch that got nothing usable (e.g. private profile), keeping any cached list.
    pub fn save_achievement_error(&self, id: i64, error: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO achievement_state (game_id, fetched, error) VALUES (?1, ?2, ?3)
             ON CONFLICT(game_id) DO UPDATE SET fetched = excluded.fetched, error = excluded.error",
            params![id, crate::now(), error],
        )?;
        Ok(())
    }

    /// Next game needing achievement work: played and never fetched or played since, or (RA) not looked up yet.
    pub fn achievements_due(&self, steam: bool, ra: bool) -> Result<Option<Game>> {
        let sql = format!(
            "SELECT {COLS} FROM games g LEFT JOIN achievement_state s ON s.game_id = g.id
             WHERE (?1 AND g.source = 'steam' AND (g.playtime_min > 0 OR g.last_played > 0)
                    AND (s.game_id IS NULL OR g.last_played > s.fetched))
                OR (?2 AND g.source = 'custom' AND g.platform IS NOT NULL AND (g.ra_id IS NULL
                    OR (g.ra_id > 0 AND (g.playtime_min > 0 OR g.last_played > 0)
                        AND (s.game_id IS NULL OR g.last_played > s.fetched))))
             ORDER BY g.ra_id IS NOT NULL, g.last_played DESC LIMIT 1"
        );
        Ok(self.conn.query_row(&sql, params![steam, ra], Self::row).optional()?)
    }

    /// Sets the RetroAchievements game; a different game's cached achievements are dropped.
    pub fn set_ra_id(&self, id: i64, ra: i64) -> Result<()> {
        let old: Option<i64> = self.conn.query_row("SELECT ra_id FROM games WHERE id = ?1", [id], |r| r.get(0))?;
        if old != Some(ra) {
            self.conn.execute("UPDATE games SET ra_id = ?2 WHERE id = ?1", params![id, ra])?;
            self.conn.execute("DELETE FROM achievements WHERE game_id = ?1", [id])?;
            self.conn.execute("DELETE FROM achievement_state WHERE game_id = ?1", [id])?;
        }
        Ok(())
    }

    /// Fills metadata fields only where they are still empty, so user edits win.
    pub fn save_metadata(&self, id: i64, m: &Metadata, matched_appid: Option<u32>) -> Result<()> {
        self.conn.execute(
            "UPDATE games SET
                description = COALESCE(description, ?2), developer = COALESCE(developer, ?3),
                publisher = COALESCE(publisher, ?4), release_date = COALESCE(release_date, ?5),
                genres = COALESCE(genres, ?6), series = COALESCE(series, ?8),
                critic_score = COALESCE(critic_score, ?9), community_score = COALESCE(community_score, ?10), meta_state = 1,
                source_id = CASE WHEN source = 'custom' AND ?7 IS NOT NULL THEN ?7 ELSE source_id END
             WHERE id = ?1",
            params![
                id, m.description, m.developer, m.publisher, m.release_date, m.genres,
                matched_appid.map(|a| a.to_string()), m.series, m.critic_score, m.community_score
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

    /// One more launch from Gamut.
    pub fn count_launch(&self, id: i64) -> Result<()> {
        self.conn.execute("UPDATE games SET play_count = play_count + 1 WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Hand-edited totals (no session rows: these aren't plays).
    pub fn set_play_stats(&self, id: i64, playtime_min: i64, play_count: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE games SET playtime_min = ?2, play_count = ?3 WHERE id = ?1",
            params![id, playtime_min.max(0), play_count.max(0)],
        )?;
        Ok(())
    }

    /// Per-game Special K / ReShade overrides (None = follow Settings).
    pub fn set_hooks(&self, id: i64, specialk: Option<bool>, reshade: Option<bool>) -> Result<()> {
        self.conn.execute("UPDATE games SET specialk = ?2, reshade = ?3 WHERE id = ?1", params![id, specialk, reshade])?;
        Ok(())
    }

    pub fn set_install_size(&self, id: i64, bytes: Option<i64>) -> Result<()> {
        self.conn.execute("UPDATE games SET install_size = ?2 WHERE id = ?1", params![id, bytes])?;
        Ok(())
    }

    pub fn set_save_dir(&self, id: i64, dir: &str) -> Result<()> {
        self.conn.execute("UPDATE games SET save_dir = ?2 WHERE id = ?1", params![id, dir])?;
        Ok(())
    }

    /// Cached HowLongToBeat entry: (hltb id, main, main+extra, completionist seconds, fetched); id 0 = not found.
    pub fn hltb(&self, id: i64) -> Result<Option<crate::hltb::Times>> {
        Ok(self
            .conn
            .query_row("SELECT hltb_id, main, plus, full, fetched FROM hltb WHERE game_id = ?1", [id], |r| {
                Ok(crate::hltb::Times { id: r.get(0)?, main: r.get(1)?, plus: r.get(2)?, full: r.get(3)?, fetched: r.get(4)? })
            })
            .optional()?)
    }

    pub fn set_hltb(&self, id: i64, t: &crate::hltb::Times) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO hltb (game_id, hltb_id, main, plus, full, fetched) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, t.id, t.main, t.plus, t.full, t.fetched],
        )?;
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
        if minutes > 0 {
            self.conn.execute(
                "INSERT INTO sessions (game_id, start, minutes, kind) VALUES (?1, ?2, ?3, 0)",
                params![id, when, minutes],
            )?;
        }
        Ok(())
    }

    /// Play sessions starting at or after `since`: (game id, start, minutes).
    pub fn sessions_since(&self, since: i64) -> Result<Vec<(i64, i64, i64)>> {
        let mut st = self.conn.prepare("SELECT game_id, start, minutes FROM sessions WHERE start >= ?1 ORDER BY start")?;
        let rows = st.query_map([since], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// One game's play sessions, newest first: (start, minutes, kind).
    pub fn game_sessions(&self, id: i64) -> Result<Vec<(i64, i64, i64)>> {
        let mut st = self.conn.prepare("SELECT start, minutes, kind FROM sessions WHERE game_id = ?1 ORDER BY start DESC, id DESC")?;
        let rows = st.query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The stored IGDB match: None = not looked up, Some(0) = no confident match.
    pub fn igdb_id(&self, id: i64) -> Result<Option<i64>> {
        Ok(self.conn.query_row("SELECT igdb_id FROM igdb_match WHERE game_id = ?1", [id], |r| r.get(0)).optional()?)
    }

    pub fn set_igdb_id(&self, id: i64, igdb: i64) -> Result<()> {
        self.conn.execute("INSERT OR REPLACE INTO igdb_match (game_id, igdb_id) VALUES (?1, ?2)", params![id, igdb])?;
        Ok(())
    }

    /// True when `igdb_requeue` already ran for this Client ID.
    pub fn igdb_requeued(&self, creds: &str) -> Result<bool> {
        let seen: Option<i64> =
            self.conn.query_row("SELECT value FROM meta WHERE key = 'igdb_client'", [], |r| r.get(0)).optional()?;
        Ok(seen == Some(client_hash(creds)))
    }

    /// Once per new IGDB credentials (`creds` = id + secret; only a hash is kept): requeues custom games with gaps (metadata or cover) so IGDB can
    /// fill them, and forgets earlier "no match" results. Returns how many games were requeued.
    pub fn igdb_requeue(&mut self, creds: &str) -> Result<usize> {
        let h = client_hash(creds);
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let seen: Option<i64> = tx.query_row("SELECT value FROM meta WHERE key = 'igdb_client'", [], |r| r.get(0)).optional()?;
        if seen == Some(h) {
            return Ok(0);
        }
        tx.execute("DELETE FROM igdb_match WHERE igdb_id = 0", [])?;
        let n = tx.execute(
            "UPDATE games SET meta_state = 0 WHERE source = 'custom' AND meta_state != 0 AND (meta_state = 2
             OR description IS NULL OR developer IS NULL OR genres IS NULL OR release_date IS NULL)",
            [],
        )? + tx.execute("UPDATE games SET cover_state = 0 WHERE source = 'custom' AND has_cover = 0 AND cover_state = 2", [])?;
        tx.execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('igdb_client', ?1)", [h])?;
        tx.commit()?;
        Ok(n)
    }

    /// When session logging began (first session or first API scan), if it has.
    pub fn stats_since(&self) -> Result<Option<i64>> {
        Ok(self.conn.query_row(
            "SELECT MIN(t) FROM (SELECT MIN(start) AS t FROM sessions UNION ALL SELECT value FROM meta WHERE key = 'steam_scan_first')",
            [],
            |r| r.get(0),
        )?)
    }

    /// Every unlock time: (game id, unix time).
    pub fn unlock_times(&self) -> Result<Vec<(i64, i64)>> {
        let mut st = self.conn.prepare("SELECT game_id, unlocked FROM achievements WHERE unlocked > 1")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Re-queues a game for scraping. `force` also clears scraped fields.
    pub fn requeue(&self, id: i64, force: bool) -> Result<()> {
        if force {
            self.conn.execute(
                "UPDATE games SET cover_state = 0, meta_state = 0, description = NULL, developer = NULL,
                 publisher = NULL, release_date = NULL, genres = NULL WHERE id = ?1",
                [id],
            )?;
            // A forced refresh retries IGDB too when it found nothing before.
            self.conn.execute("DELETE FROM igdb_match WHERE game_id = ?1 AND igdb_id = 0", [id])?;
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

    /// Fills Playnite's extras (links, notes, user score…) where the game has none; ids in `games` order
    /// (None = not in the library) and whether anything changed.
    pub fn import_extras(&mut self, games: &[&Game]) -> Result<Vec<(Option<i64>, bool)>> {
        let tx = self.conn.transaction()?;
        let mut out = Vec::new();
        {
            let mut get = tx.prepare("SELECT id FROM games WHERE playnite_id = ?1")?;
            let mut set = tx.prepare(
                "UPDATE games SET sort_name = COALESCE(sort_name, ?2), user_score = COALESCE(user_score, ?3),
                 notes = COALESCE(notes, ?4), links = COALESCE(links, ?5), actions = COALESCE(actions, ?6),
                 script_before = COALESCE(script_before, ?7), script_after = COALESCE(script_after, ?8),
                 install_size = COALESCE(install_size, ?9)
                 WHERE id = ?1 AND (sort_name IS NOT COALESCE(sort_name, ?2) OR user_score IS NOT COALESCE(user_score, ?3)
                   OR notes IS NOT COALESCE(notes, ?4) OR links IS NOT COALESCE(links, ?5) OR actions IS NOT COALESCE(actions, ?6)
                   OR script_before IS NOT COALESCE(script_before, ?7) OR script_after IS NOT COALESCE(script_after, ?8)
                   OR install_size IS NOT COALESCE(install_size, ?9))",
            )?;
            for g in games {
                let Some(pid) = &g.playnite_id else {
                    out.push((None, false));
                    continue;
                };
                let id: Option<i64> = get.query_row([pid], |r| r.get(0)).optional()?;
                let changed = match id {
                    Some(id) => {
                        set.execute(params![
                            id, g.sort_name, g.user_score, g.notes, g.links, g.actions, g.script_before, g.script_after, g.install_size
                        ])? > 0
                    }
                    None => false,
                };
                out.push((id, changed));
            }
        }
        tx.commit()?;
        Ok(out)
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

/// FNV-1a of the trimmed credentials (the meta table holds integers).
fn client_hash(id: &str) -> i64 {
    id.trim().bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3)) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_scanned_games_with_platform_and_tags() {
        let dir = std::env::temp_dir().join(format!("gamut-scanadd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.db");
        let _ = std::fs::remove_file(&path);
        let mut lib = Library::open(&path).unwrap();
        let rows = vec![
            ("Celeste".to_string(), r"D:\G\Celeste\Celeste.exe".to_string(), Some(r"D:\G\Celeste".to_string()), " ".to_string()),
            ("Hades".to_string(), r"D:\G\Hades\Hades.exe".to_string(), None, "Roguelike, Fan port".to_string()),
        ];
        assert_eq!(lib.add_scanned(&rows, "PC (Windows)").unwrap(), 2);
        let all = lib.all().unwrap();
        let c = all.iter().find(|g| g.name == "Celeste").unwrap();
        assert_eq!((c.source, c.exe.as_deref(), c.work_dir.as_deref()), (Source::Custom, Some(r"D:\G\Celeste\Celeste.exe"), Some(r"D:\G\Celeste")));
        assert_eq!((c.platform.as_deref(), c.tags.as_deref()), (Some("PC (Windows)"), None));
        assert_eq!(all.iter().find(|g| g.name == "Hades").unwrap().tags.as_deref(), Some("Roguelike, Fan port"));
        let (c, h) = (c.id, all.iter().find(|g| g.name == "Hades").unwrap().id);
        assert_eq!(lib.add_tags(&[(c, "Indie".into()), (h, "fan PORT, Recomp".into())]).unwrap(), 2);
        assert_eq!(lib.add_tags(&[(c, "indie".into())]).unwrap(), 0);
        let all = lib.all().unwrap();
        let tags = |id| all.iter().find(|g| g.id == id).unwrap().tags.clone();
        assert_eq!((tags(c).as_deref(), tags(h).as_deref()), (Some("Indie"), Some("Roguelike, Fan port, Recomp")));
        drop(lib);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn first_steam_scan_has_no_added_date() {
        let dir = std::env::temp_dir().join(format!("gamut-added-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.db");
        let _ = std::fs::remove_file(&path);
        let mut lib = Library::open(&path).unwrap();
        let e = |appid, name: &str| SteamEntry { appid, name: name.into(), installed: true, install_dir: None, size: None, playtime_min: None, last_played: 0 };
        lib.sync_steam(&[e(1, "Old")]).unwrap();
        lib.sync_steam(&[e(1, "Old"), e(2, "New")]).unwrap();
        let added = |n: &str| lib.conn.query_row("SELECT added FROM games WHERE name = ?1", [n], |r| r.get::<_, i64>(0)).unwrap();
        assert_eq!(added("Old"), 0);
        assert!(added("New") > 0);
        drop(lib);
        let _ = std::fs::remove_dir_all(&dir);
    }

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
    fn saves_details_fields() {
        let dir = std::env::temp_dir().join(format!("gamut-test-extra-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut lib = Library::open(&dir.join("t.db")).unwrap();
        let id = lib.add_custom(&Game::new_custom("Hades".into(), Some(r"D:\G\Hades.exe".into()))).unwrap();
        let mut g = lib.get(id).unwrap().unwrap();
        (g.sort_name, g.user_score, g.notes, g.install_dir) = (Some("Hades 1".into()), Some(90), Some("100% run".into()), Some(r"D:\G".into()));
        g.links = crate::model::to_json(&[crate::model::Link { name: "Wiki".into(), url: "https://x".into() }]);
        lib.update_user_fields(&g).unwrap();
        let back = lib.get(id).unwrap().unwrap();
        assert_eq!((&back.sort_name, back.user_score, &back.notes, &back.install_dir), (&g.sort_name, g.user_score, &g.notes, &g.install_dir));
        assert_eq!(back.links()[0].url, "https://x");
        // Playnite extras fill only what's empty.
        let mut pn = Game { playnite_id: Some("p".into()), notes: Some("from Playnite".into()), install_size: Some(5), ..back.clone() };
        lib.conn.execute("UPDATE games SET playnite_id = 'p' WHERE id = ?1", [id]).unwrap();
        assert_eq!(lib.import_extras(&[&pn]).unwrap(), [(Some(id), true)]);
        let back = lib.get(id).unwrap().unwrap();
        assert_eq!((back.notes.as_deref(), back.install_size), (Some("100% run"), Some(5)));
        pn.install_size = Some(9);
        assert_eq!(lib.import_extras(&[&pn]).unwrap(), [(Some(id), false)]);
        let t = crate::hltb::Times { id: 7, main: 3600, plus: 0, full: 0, fetched: 1 };
        lib.set_hltb(id, &t).unwrap();
        assert_eq!(lib.hltb(id).unwrap(), Some(t));
        std::fs::remove_dir_all(&dir).ok();
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
            size: None,
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
    #[test]
    fn achievements_cache_round_trip() {
        use crate::achievements::{Achievement, Fetched};
        let dir = std::env::temp_dir().join(format!("gamut-ach-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut lib = Library::open(&dir.join("t.db")).unwrap();
        let entry = SteamEntry { appid: 9, name: "G".into(), installed: true, install_dir: None, size: None, playtime_min: Some(5), last_played: 100 };
        lib.sync_steam(&[entry]).unwrap();
        let id = lib.all().unwrap()[0].id;
        assert_eq!(lib.achievements_due(true, true).unwrap().map(|g| g.id), Some(id));
        assert!(lib.achievements_due(false, true).unwrap().is_none());

        let a = |k: &str, u| Achievement { key: k.into(), name: k.into(), unlocked: u, percent: Some(1.5), ..Default::default() };
        assert!(lib.save_achievements(id, &Fetched::List { items: vec![a("x", 0), a("y", 50)], full: true }).unwrap());
        let st = lib.achievement_state(id).unwrap().unwrap();
        assert_eq!((st.total, st.unlocked), (2, 1));
        assert!(lib.achievements_due(true, true).unwrap().is_none(), "fetched after last play");

        // Unlock-only refresh keeps names/rarity; an unknown key asks for a full fetch.
        assert!(lib.save_achievements(id, &Fetched::List { items: vec![a("x", 70), a("y", 50)], full: false }).unwrap());
        let got = lib.achievements(id).unwrap();
        assert_eq!((got[0].unlocked, got[0].percent), (70, Some(1.5)));
        assert_eq!(lib.achievement_state(id).unwrap().unwrap().unlocked, 2);
        assert!(!lib.save_achievements(id, &Fetched::List { items: vec![a("z", 1)], full: false }).unwrap());
        assert_eq!(lib.achievements(id).unwrap()[0].unlocked, 70, "rejected refresh rolled back");
        assert_eq!(lib.achievement_progress().unwrap(), vec![(id, 2, 2, 70, 0)]);
        let recent = lib.unlocked_achievements(false, 5).unwrap();
        assert_eq!(recent.iter().map(|(g, a)| (*g, a.key.as_str())).collect::<Vec<_>>(), vec![(id, "x"), (id, "y")]);

        lib.save_achievement_error(id, "private").unwrap();
        assert_eq!(lib.achievements(id).unwrap().len(), 2);
        lib.delete(id).unwrap();
        assert!(lib.achievements(id).unwrap().is_empty() && lib.achievement_state(id).unwrap().is_none());
        drop(lib);
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn igdb_match_and_requeue() {
        let dir = std::env::temp_dir().join(format!("gamut-igdb-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut lib = Library::open(&dir.join("t.db")).unwrap();
        let g = Game::new_custom("Rom".into(), None);
        let id = lib.add_custom(&g).unwrap();
        lib.set_igdb_id(id, 42).unwrap();
        lib.update_user_fields(&Game { id, ..g.clone() }).unwrap();
        assert_eq!(lib.igdb_id(id).unwrap(), Some(42), "unchanged name keeps the match");
        lib.update_user_fields(&Game { id, name: "Rom 2".into(), ..g.clone() }).unwrap();
        assert_eq!(lib.igdb_id(id).unwrap(), None, "rename drops it");
        lib.set_igdb_id(id, 0).unwrap();
        lib.set_meta_state(id, state::NOT_FOUND).unwrap();
        assert_eq!(lib.igdb_requeue("abc").unwrap(), 1);
        assert_eq!(lib.igdb_id(id).unwrap(), None, "no-match forgotten");
        assert_eq!(lib.needing_meta().unwrap().len(), 1);
        lib.set_meta_state(id, state::NOT_FOUND).unwrap();
        assert_eq!(lib.igdb_requeue(" abc ").unwrap(), 0, "same client id: once");
        let m = Metadata { series: Some("S".into()), critic_score: Some(80), ..Default::default() };
        lib.save_metadata(id, &m, None).unwrap();
        let got = lib.get(id).unwrap().unwrap();
        assert_eq!((got.series.as_deref(), got.critic_score), (Some("S"), Some(80)));
        lib.delete(id).unwrap();
        assert_eq!(lib.igdb_id(id).unwrap(), None);
        drop(lib);
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn retro_matching_state() {
        use crate::achievements::{Achievement, Fetched};
        let dir = std::env::temp_dir().join(format!("gamut-ra-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut lib = Library::open(&dir.join("t.db")).unwrap();
        let mut g = Game::new_custom("Rom".into(), None);
        g.platform = Some("Nintendo SNES".into());
        let id = lib.add_custom(&g).unwrap();
        assert!(lib.achievements_due(false, true).unwrap().is_none(), "no platform yet");
        lib.update_user_fields(&Game { id, ..g.clone() }).unwrap();
        assert_eq!(lib.achievements_due(false, true).unwrap().map(|g| g.id), Some(id), "needs lookup");
        lib.set_ra_id(id, 55).unwrap();
        assert!(lib.achievements_due(false, true).unwrap().is_none(), "matched but never played");
        lib.add_playtime(id, 10, 100).unwrap();
        assert_eq!(lib.achievements_due(false, true).unwrap().map(|g| g.id), Some(id));
        let a = Achievement { key: "1".into(), unlocked: 5, ..Default::default() };
        lib.save_achievements(id, &Fetched::List { items: vec![a], full: true }).unwrap();
        assert!(lib.achievements_due(false, true).unwrap().is_none());
        lib.set_ra_id(id, 55).unwrap();
        assert_eq!(lib.achievements(id).unwrap().len(), 1, "same game keeps its cache");
        lib.set_ra_id(id, 77).unwrap();
        assert!(lib.achievements(id).unwrap().is_empty(), "another game drops it");
        assert_eq!(lib.get(id).unwrap().unwrap().ra_id, Some(77));
        lib.update_user_fields(&Game { id, favorite: true, ..g.clone() }).unwrap();
        assert_eq!(lib.get(id).unwrap().unwrap().ra_id, Some(77), "other edits keep the match");
        lib.update_user_fields(&Game { id, platform: Some("Nintendo 64".into()), ..g }).unwrap();
        assert_eq!(lib.get(id).unwrap().unwrap().ra_id, None);
        drop(lib);
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn steam_playtime_growth_becomes_sessions() {
        let dir = std::env::temp_dir().join(format!("gamut-sess-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut lib = Library::open(&dir.join("t.db")).unwrap();
        let e = |mins, last| SteamEntry { appid: 5, name: "G".into(), installed: true, install_dir: None, size: None, playtime_min: Some(mins), last_played: last };
        lib.sync_steam(&[e(10, 0)]).unwrap();
        lib.sync_steam(&[e(10, 0)]).unwrap();
        assert!(lib.sessions_since(0).unwrap().is_empty(), "first sightings aren't play");
        let id = lib.all().unwrap()[0].id;
        let ago = |s: i64| crate::now() - s;
        // Total unchanged since 2 h ago; a scan 2 min ago (mid-session) saw nothing new.
        lib.conn.execute("UPDATE steam_seen SET since = ?1", [ago(7200)]).unwrap();
        lib.sync_steam(&[e(10, 0)]).unwrap();
        lib.sync_steam(&[e(100, ago(5600))]).unwrap();
        assert_eq!(lib.sessions_since(0).unwrap(), vec![(id, ago(5600), 90)], "a session spanning scans counts");
        lib.sync_steam(&[e(100_000, 0)]).unwrap();
        assert_eq!(lib.sessions_since(0).unwrap().len(), 1, "jumps bigger than the time since the last change aren't play");
        assert!(lib.stats_since().unwrap().is_some());

        let c = lib.add_custom(&Game::new_custom("Emu".into(), None)).unwrap();
        lib.add_playtime(c, 25, 1000).unwrap();
        assert_eq!(lib.sessions_since(900).unwrap()[0], (c, 1000, 25));
        lib.add_playtime(c, 5, 2000).unwrap();
        assert_eq!(lib.game_sessions(c).unwrap(), vec![(2000, 5, 0), (1000, 25, 0)]);
        lib.delete(c).unwrap();
        assert!(lib.sessions_since(900).unwrap().iter().all(|s| s.0 != c));
        drop(lib);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
