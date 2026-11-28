//! Game stores besides Steam (GOG, ...): what they own and install, and syncing that into the library.
//! Docs: docs/stores.md.
pub mod gog;
pub mod sync;

/// A game as its store sees it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StoreGame {
    pub id: String,
    pub name: String,
    pub installed: bool,
    pub install_dir: Option<String>,
    /// How an installed game starts: exe (absolute), arguments, working folder.
    pub exe: Option<String>,
    pub args: Option<String>,
    pub work_dir: Option<String>,
}

/// The stores Gamut can import, by the name it stores in `games.store`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Store {
    Gog,
}

impl Store {
    pub const ALL: [Store; 1] = [Store::Gog];

    /// Also Playnite's source name, so imported rows already carry it.
    pub fn name(self) -> &'static str {
        match self {
            Store::Gog => "GOG",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Store::Gog => "gog",
        }
    }

    pub fn from_key(k: &str) -> Option<Store> {
        Store::ALL.into_iter().find(|s| s.key() == k)
    }

    pub fn scan(self) -> anyhow::Result<Vec<StoreGame>> {
        match self {
            Store::Gog => gog::scan(),
        }
    }

    /// The link that opens the game in its launcher (where Install is).
    pub fn open_uri(self, id: &str) -> String {
        match self {
            Store::Gog => format!("goggalaxy://openGameView/{id}"),
        }
    }

    /// Whether a library row's launch (exe + args) points at this store game (e.g. from a Playnite import).
    pub fn launches(self, id: &str, launch: &str) -> bool {
        let marker = match self {
            Store::Gog => format!("opengameview/{id}"),
        };
        let l = launch.to_lowercase();
        l.match_indices(&marker).any(|(i, m)| !l[i + m.len()..].starts_with(|c: char| c.is_ascii_alphanumeric()))
    }
}
