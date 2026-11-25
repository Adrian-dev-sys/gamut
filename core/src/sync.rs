//! One-game scraping steps, shared by the UI's background workers and the CLI.
use crate::model::{state, Game, Source};
use crate::scrape::{self, SgdbRef};
use crate::{images, steam, Config, Library, Paths};
use anyhow::Result;

/// Fetches cover + hero art. Errors mean "try again later" (network, rate limit).
pub fn fetch_cover(lib: &Library, paths: &Paths, cfg: &Config, g: &Game) -> Result<bool> {
    let appid = g.steam_appid();
    let key = cfg.steamgriddb_key.trim();

    let mut cover: Option<Vec<u8>> = None;
    let mut hero: Option<Vec<u8>> = None;

    if let Some(id) = appid {
        if g.source == Source::Steam {
            cover = steam::local_art(id, "library_600x900.jpg").and_then(|p| std::fs::read(p).ok());
            hero = steam::local_art(id, "library_hero.jpg").and_then(|p| std::fs::read(p).ok());
        }
        if cover.is_none() {
            cover = scrape::steam_cdn_cover(id)?;
        }
        if hero.is_none() {
            // A missing banner shouldn't hold up the cover.
            hero = scrape::steam_cdn_hero(id).unwrap_or_else(|e| {
                log::warn!("hero {}: {e:#}", g.name);
                None
            });
        }
    }
    if (cover.is_none() || hero.is_none()) && !key.is_empty() {
        let r = match appid {
            Some(id) => SgdbRef::Steam(id),
            None => SgdbRef::Name(&g.name),
        };
        match scrape::sgdb_art(key, r, cover.is_none(), hero.is_none()) {
            Ok((c, h)) => {
                cover = cover.or(c);
                hero = hero.or(h);
            }
            // Fatal only if SteamGridDB was our last chance at a cover.
            Err(e) if cover.is_none() => return Err(e),
            Err(e) => log::warn!("sgdb {}: {e:#}", g.name),
        }
    }

    let has_hero = match hero {
        Some(h) => images::store_hero(&h, &paths.hero(g.id)).is_ok(),
        None => g.has_hero,
    };
    let has_cover = match cover {
        Some(c) => match images::store_cover(&c, &paths.cover(g.id), &paths.thumb(g.id)) {
            Ok(()) => true,
            Err(e) => {
                log::warn!("bad cover for {}: {e}", g.name);
                false
            }
        },
        None => false,
    };
    let st = if has_cover { state::DONE } else { state::NOT_FOUND };
    lib.set_cover_state(g.id, has_cover || g.has_cover, Some(has_hero), st)?;
    Ok(has_cover)
}

/// Fetches store metadata. Custom games are matched to Steam by exact (normalized) name.
pub fn fetch_meta(lib: &Library, g: &Game) -> Result<bool> {
    let appid = match (g.source, g.steam_appid()) {
        (_, Some(id)) => Some(id),
        (Source::Custom, None) => scrape::steam_search(&g.name)?,
        (Source::Steam, None) => None,
    };
    let Some(appid) = appid else {
        lib.set_meta_state(g.id, state::NOT_FOUND)?;
        return Ok(false);
    };
    let Some(d) = scrape::steam_details(appid)? else {
        lib.set_meta_state(g.id, state::NOT_FOUND)?;
        return Ok(false);
    };
    let matched = (g.source == Source::Custom && g.source_id.is_empty()).then_some(appid);
    lib.save_metadata(g.id, &d.meta, matched)?;
    if matches!(d.kind.as_str(), "dlc" | "music" | "video" | "advertising" | "series" | "episode") {
        lib.set_hidden(g.id, true)?;
    }
    // A fresh Steam match may unlock Steam CDN art for a custom game.
    if matched.is_some() && !g.has_cover {
        lib.set_cover_state(g.id, false, None, state::PENDING)?;
    }
    Ok(true)
}
