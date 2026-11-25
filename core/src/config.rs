use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    /// Steam Web API key (steamcommunity.com/dev/apikey); enables the full owned library.
    pub steam_api_key: String,
    /// SteamID64; auto-detected from loginusers.vdf when empty.
    pub steam_id: String,
    /// SteamGridDB API key; enables covers for any game.
    pub steamgriddb_key: String,
    /// Grid card width in logical pixels.
    pub card_width: f32,
}

impl Config {
    pub fn load(path: &Path) -> Config {
        let mut c: Config = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default();
        if c.card_width <= 0.0 {
            c.card_width = 170.0;
        }
        c
    }

    /// Writes via a temp file + rename so a crash never leaves a half-written config.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, toml::to_string_pretty(self)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Persists only UI preferences, keeping whatever keys are on disk.
    pub fn save_ui_prefs(&self, path: &Path) -> anyhow::Result<()> {
        let mut on_disk = Config::load(path);
        on_disk.card_width = self.card_width;
        on_disk.save(path)
    }
}
