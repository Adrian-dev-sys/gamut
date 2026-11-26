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
    /// Last grid sort (see `SORTS` in app/src/ui.rs).
    pub sort: String,
    /// Last grid filter ("all", "installed", "favorites", "steam", "custom", "hidden").
    pub filter: String,
    /// Platform / tag the grid was narrowed to ("" = any).
    pub platform: String,
    pub tag: String,
    /// Completion status filter ("" = any).
    pub completion: String,
    /// Last window placement (kept last: TOML tables follow plain keys).
    pub window: Option<WindowState>,
    pub specialk: crate::specialk::Settings,
}

/// Window placement in logical points; size/position are the un-maximized ones.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct WindowState {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub maximized: bool,
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
        on_disk.sort = self.sort.clone();
        on_disk.filter = self.filter.clone();
        on_disk.platform = self.platform.clone();
        on_disk.tag = self.tag.clone();
        on_disk.completion = self.completion.clone();
        on_disk.window = self.window;
        on_disk.save(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_prefs_round_trip_and_keep_keys() {
        let dir = std::env::temp_dir().join(format!("gamut-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let keys = Config { steam_api_key: "KEY".into(), steamgriddb_key: "SG".into(), ..Config::load(&path) };
        keys.save(&path).unwrap();

        // A UI-only save from a copy without keys must not wipe them.
        let mut ui = Config::load(&dir.join("missing.toml"));
        ui.sort = "last_played".into();
        ui.filter = "installed".into();
        ui.window = Some(WindowState { x: -1800.0, y: 40.0, width: 1500.0, height: 900.0, maximized: true });
        ui.save_ui_prefs(&path).unwrap();

        let back = Config::load(&path);
        assert_eq!(back.steam_api_key, "KEY");
        assert_eq!(back.steamgriddb_key, "SG");
        assert_eq!(back.sort, "last_played");
        assert_eq!(back.filter, "installed");
        assert_eq!(back.window, ui.window);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
