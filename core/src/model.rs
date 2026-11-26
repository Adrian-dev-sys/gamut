#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    Steam,
    Custom,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Steam => "steam",
            Source::Custom => "custom",
        }
    }
    pub fn parse(s: &str) -> Source {
        match s {
            "steam" => Source::Steam,
            _ => Source::Custom,
        }
    }
}

/// Fetch state for covers / metadata.
pub mod state {
    pub const PENDING: i32 = 0;
    pub const DONE: i32 = 1;
    pub const NOT_FOUND: i32 = 2;
    /// Has a cover, but a better one should be looked for (e.g. imported art with the wrong shape).
    pub const REPLACE: i32 = 3;
}

#[derive(Debug, Clone)]
pub struct Game {
    pub id: i64,
    pub source: Source,
    /// Steam appid for Steam games; for custom games an optional matched Steam appid.
    pub source_id: String,
    pub name: String,
    pub installed: bool,
    pub install_dir: Option<String>,
    pub exe: Option<String>,
    pub args: Option<String>,
    pub work_dir: Option<String>,
    pub playtime_min: i64,
    pub last_played: i64,
    pub description: Option<String>,
    pub developer: Option<String>,
    pub publisher: Option<String>,
    pub release_date: Option<String>,
    pub genres: Option<String>,
    pub has_cover: bool,
    pub has_hero: bool,
    pub cover_state: i32,
    pub meta_state: i32,
    pub favorite: bool,
    pub hidden: bool,
    pub added: i64,
    /// Playnite's game id when imported from Playnite (re-imports update the same row).
    pub playnite_id: Option<String>,
    pub platform: Option<String>,
    /// Where the game comes from when it isn't Steam, e.g. "GOG", "PlayStation".
    pub store: Option<String>,
    pub completion: Option<String>,
    pub play_count: i64,
    pub critic_score: Option<i64>,
    pub community_score: Option<i64>,
    pub tags: Option<String>,
    pub series: Option<String>,
    /// Special K per game: None = follow the global mode, Some(true) = always, Some(false) = never.
    pub specialk: Option<bool>,
    /// ReShade per game: None = follow the global setting.
    pub reshade: Option<bool>,
    /// Turn Windows HDR on while this game runs.
    pub auto_hdr: bool,
}

impl Game {
    pub fn new_custom(name: String, exe: Option<String>) -> Self {
        Game {
            id: 0,
            source: Source::Custom,
            source_id: String::new(),
            name,
            installed: true,
            install_dir: None,
            exe,
            args: None,
            work_dir: None,
            playtime_min: 0,
            last_played: 0,
            description: None,
            developer: None,
            publisher: None,
            release_date: None,
            genres: None,
            has_cover: false,
            has_hero: false,
            cover_state: state::PENDING,
            meta_state: state::PENDING,
            favorite: false,
            hidden: false,
            added: crate::now(),
            playnite_id: None,
            platform: None,
            store: None,
            completion: None,
            play_count: 0,
            critic_score: None,
            community_score: None,
            tags: None,
            series: None,
            specialk: None,
            reshade: None,
            auto_hdr: false,
        }
    }

    pub fn steam_appid(&self) -> Option<u32> {
        self.source_id.parse().ok()
    }
}

/// Scraped metadata to merge into a game.
#[derive(Debug, Clone, Default)]
pub struct Metadata {
    pub description: Option<String>,
    pub developer: Option<String>,
    pub publisher: Option<String>,
    pub release_date: Option<String>,
    pub genres: Option<String>,
}
