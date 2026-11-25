//! One-game scraping steps, shared by the UI's background workers and the CLI.
use crate::model::{state, Game, Source};
use crate::scrape::{self, SgdbRef};
use crate::{images, steam, Config, Library, Paths};
use anyhow::Result;

/// Fetches whatever art is missing (cover and/or banner). Errors mean "try again later".
pub fn fetch_cover(lib: &Library, paths: &Paths, cfg: &Config, g: &Game) -> Result<bool> {
    let appid = g.steam_appid();
    let key = cfg.steamgriddb_key.trim();
    let want_cover = !g.has_cover || g.cover_state == state::REPLACE;
    let want_hero = !g.has_hero;

    let mut cover: Option<Vec<u8>> = None;
    let mut hero: Option<Vec<u8>> = None;
    let missing = |c: &Option<Vec<u8>>, h: &Option<Vec<u8>>| (want_cover && c.is_none()) || (want_hero && h.is_none());

    if let Some(id) = appid {
        if g.source == Source::Steam {
            if want_cover {
                cover = steam::local_art(id, "library_600x900.jpg").and_then(|p| std::fs::read(p).ok());
            }
            if want_hero {
                hero = steam::local_art(id, "library_hero.jpg").and_then(|p| std::fs::read(p).ok());
            }
        }
        cdn_art(id, want_cover, want_hero, &mut cover, &mut hero)?;
        // Demos and playtests rarely have their own art: borrow the full game's.
        if missing(&cover, &hero) && g.source == Source::Steam {
            if let Some(full) = scrape::steam_details(id).ok().flatten().and_then(|d| d.fullgame) {
                cdn_art(full, want_cover, want_hero, &mut cover, &mut hero)?;
            }
        }
    }
    if missing(&cover, &hero) && !key.is_empty() {
        let clean = clean_name(&g.name);
        let mut refs = Vec::new();
        if let Some(id) = appid {
            refs.push(SgdbRef::Steam(id));
        }
        refs.push(SgdbRef::Name(&clean));
        for r in refs {
            if !missing(&cover, &hero) {
                break;
            }
            match scrape::sgdb_art(key, r, want_cover && cover.is_none(), want_hero && hero.is_none()) {
                Ok((c, h)) => {
                    cover = cover.or(c);
                    hero = hero.or(h);
                }
                // Fatal only if SteamGridDB was our last chance at a cover.
                Err(e) if want_cover && cover.is_none() && !g.has_cover => return Err(e),
                Err(e) => log::warn!("sgdb {}: {e:#}", g.name),
            }
        }
    }

    let has_hero = match hero {
        Some(h) => images::store_hero(&h, &paths.hero(g.id)).is_ok() || g.has_hero,
        None => g.has_hero,
    };
    let new_cover = match cover {
        Some(c) => match images::store_cover(&c, &paths.cover(g.id), &paths.thumb(g.id)) {
            Ok(()) => true,
            Err(e) => {
                log::warn!("bad cover for {}: {e}", g.name);
                false
            }
        },
        None => false,
    };
    let has_cover = new_cover || g.has_cover;
    let st = if has_cover { state::DONE } else { state::NOT_FOUND };
    lib.set_cover_state(g.id, has_cover, Some(has_hero), st)?;
    Ok(new_cover || has_hero != g.has_hero)
}

fn cdn_art(id: u32, want_cover: bool, want_hero: bool, cover: &mut Option<Vec<u8>>, hero: &mut Option<Vec<u8>>) -> Result<()> {
    if want_cover && cover.is_none() {
        *cover = scrape::steam_cdn_cover(id)?;
    }
    if want_hero && hero.is_none() {
        // A missing banner shouldn't hold up the cover.
        *hero = scrape::steam_cdn_hero(id).unwrap_or_else(|e| {
            log::warn!("hero {id}: {e:#}");
            None
        });
    }
    Ok(())
}

/// Drops edition noise that SteamGridDB names don't have ("Foo Demo" -> "Foo").
fn clean_name(name: &str) -> String {
    let mut n = name.replace(['™', '®'], "");
    for suffix in [" Demo", " demo", " Playtest", " Prologue", " (Demo)", " - Demo"] {
        if let Some(s) = n.strip_suffix(suffix) {
            n = s.to_string();
        }
    }
    n.trim().to_string()
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
