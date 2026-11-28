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
    /// RetroAchievements game id; None = not looked up, see `retro::NOT_FOUND` / `UNSUPPORTED`.
    pub ra_id: Option<i64>,
    /// Sorts by this instead of the name ("Zelda Tears of the Kingdom").
    pub sort_name: Option<String>,
    /// The player's own score, 0-100.
    pub user_score: Option<i64>,
    pub notes: Option<String>,
    /// JSON list of `Link`.
    pub links: Option<String>,
    /// JSON list of `PlayAction`: extra ways to start the game (mod manager, config tool).
    pub actions: Option<String>,
    /// PowerShell run before launch (waited for) and after the game exits.
    pub script_before: Option<String>,
    pub script_after: Option<String>,
    /// Bytes on disk, from Steam or "Calculate".
    pub install_size: Option<i64>,
    /// Save folder: set by hand or found on PCGamingWiki.
    pub save_dir: Option<String>,
    /// JSON `display::DisplayMode` set while the game runs.
    pub display: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Link {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct PlayAction {
    pub name: String,
    /// Exe, file or URL.
    pub path: String,
    pub args: String,
    pub work_dir: String,
    /// Counts as playing (time tracked, Special K/HDR) instead of just opening it.
    pub track: bool,
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
            ra_id: None,
            sort_name: None,
            user_score: None,
            notes: None,
            links: None,
            actions: None,
            script_before: None,
            script_after: None,
            install_size: None,
            save_dir: None,
            display: None,
        }
    }

    pub fn steam_appid(&self) -> Option<u32> {
        self.source_id.parse().ok()
    }

    pub fn links(&self) -> Vec<Link> {
        self.links.as_deref().and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default()
    }

    pub fn display_mode(&self) -> Option<crate::display::DisplayMode> {
        self.display.as_deref().and_then(|s| serde_json::from_str::<crate::display::DisplayMode>(s).ok()).filter(|m| !m.is_empty())
    }

    pub fn actions(&self) -> Vec<PlayAction> {
        self.actions.as_deref().and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default()
    }

    /// This game started through one of its extra actions instead (same id, so tracking and art still apply).
    pub fn with_action(&self, a: &PlayAction) -> Game {
        let opt = |s: &str| Some(s.trim().to_string()).filter(|s| !s.is_empty());
        Game { source: Source::Custom, exe: opt(&a.path), args: opt(&a.args), work_dir: opt(&a.work_dir), installed: true, ..self.clone() }
    }

    /// The game's folder: install folder, else the ROM's, else the exe's.
    pub fn folder(&self) -> Option<std::path::PathBuf> {
        let d = self.install_dir.as_deref().filter(|d| abs(d)).map(std::path::PathBuf::from);
        d.or_else(|| {
            let f = self.rom().or(self.exe.clone().filter(|e| abs(&e.as_str())))?;
            std::path::Path::new(&f).parent().map(|p| p.to_path_buf())
        })
    }

    /// The file an emulator is given: the first quoted absolute path in the arguments,
    /// else an unquoted one running to the end ("-- D:\Roms\Game (USA).iso") that exists.
    pub fn rom(&self) -> Option<String> {
        let args = self.args.as_deref().unwrap_or("");
        if let Some(q) = args.split('"').skip(1).step_by(2).find(abs) {
            return Some(q.to_string());
        }
        let i = args.find(":\\")?.checked_sub(1)?;
        let rest = args.get(i..)?.trim();
        (abs(&rest) && std::path::Path::new(rest).exists()).then(|| rest.to_string())
    }
}

/// "X:\..." without `{Placeholders}`.
fn abs(p: &&str) -> bool {
    p.len() > 3 && p.as_bytes()[1] == b':' && p.as_bytes()[0].is_ascii_alphabetic() && !p.contains('{')
}

/// JSON for a list column; None when empty.
pub fn to_json<T: serde::Serialize>(v: &[T]) -> Option<String> {
    (!v.is_empty()).then(|| serde_json::to_string(v).ok()).flatten()
}

/// Scraped metadata to merge into a game.
#[derive(Debug, Clone, Default)]
pub struct Metadata {
    pub description: Option<String>,
    pub developer: Option<String>,
    pub publisher: Option<String>,
    pub release_date: Option<String>,
    pub genres: Option<String>,
    pub series: Option<String>,
    pub critic_score: Option<i64>,
    pub community_score: Option<i64>,
}

/// Filters a dynamic collection matches; empty strings mean any. `filter` uses the app's config keys.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Rule {
    pub filter: String,
    pub platform: String,
    /// Single tag from the first version; read as one more entry of `tags`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tag: String,
    pub tags: Vec<String>,
    /// A game needs every tag (AND) instead of any one of them (OR).
    pub all_tags: bool,
    pub completion: String,
}

impl Rule {
    pub fn tag_list(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self.tags.iter().map(String::as_str).collect();
        if !self.tag.is_empty() && !v.contains(&self.tag.as_str()) {
            v.push(&self.tag);
        }
        v
    }
}

/// A user collection: games added by hand, or (with a rule) every game matching it.
#[derive(Debug, Clone, PartialEq)]
pub struct Collection {
    pub id: i64,
    pub name: String,
    pub rule: Option<Rule>,
    pub games: std::collections::HashSet<i64>,
}
