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

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        std::fs::write(path, toml::to_string_pretty(self)?)?;
        Ok(())
    }
}
