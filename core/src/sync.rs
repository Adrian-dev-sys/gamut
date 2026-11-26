//! One-game scraping steps, shared by the UI's background workers and the CLI.
use crate::model::{state, Game, Source};
use crate::scrape::{self, SgdbRef};
use crate::{igdb, images, steam, Config, Library, Paths};
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
    let mut sgdb_err = None;
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
                // Fatal only if nothing else finds a cover (IGDB is tried first).
                Err(e) if want_cover && cover.is_none() && !g.has_cover => sgdb_err = Some(e),
                Err(e) => log::warn!("sgdb {}: {e:#}", g.name),
            }
        }
    }

    if missing(&cover, &hero) && g.source == Source::Custom && igdb::configured(cfg) {
        match igdb_entry(lib, cfg, g) {
            Ok(Some(e)) => {
                if want_cover && cover.is_none() {
                    cover = e.cover.as_deref().and_then(|i| download(&igdb::cover_url(i), &g.name));
                }
                if want_hero && hero.is_none() {
                    hero = e.artworks.iter().chain(&e.screenshots).next().and_then(|i| download(&igdb::hero_url(i), &g.name));
                }
            }
            Ok(None) => {}
            Err(e) if igdb::is_auth_error(&e) => log::warn!("igdb {}: {e:#}", g.name),
            Err(e) if want_cover && cover.is_none() && !g.has_cover => return Err(e),
            Err(e) => log::warn!("igdb {}: {e:#}", g.name),
        }
    }
    if let (Some(e), true) = (sgdb_err, want_cover && cover.is_none()) {
        return Err(e);
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

fn download(url: &str, name: &str) -> Option<Vec<u8>> {
    crate::http::get_bytes_opt(url).unwrap_or_else(|e| {
        log::warn!("igdb image for {name}: {e:#}");
        None
    })
}

/// The game's IGDB entry: the stored match, or a strict lookup by name and platform (stored either way).
pub fn igdb_entry(lib: &Library, cfg: &Config, g: &Game) -> Result<Option<igdb::IgdbGame>> {
    match lib.igdb_id(g.id)? {
        Some(0) => Ok(None),
        Some(id) => igdb::game(cfg, id),
        None => {
            let found = igdb::best(&g.name, g.platform.as_deref(), &igdb::search(cfg, &clean_name(&g.name))?).cloned();
            lib.set_igdb_id(g.id, found.as_ref().map_or(0, |f| f.id))?;
            Ok(found)
        }
    }
}

/// True for platforms Steam can't have (consoles, handhelds); PC or unknown is false.
fn console(g: &Game) -> bool {
    g.platform.as_deref().is_some_and(|p| {
        let p = p.to_lowercase();
        !p.trim().is_empty() && !["pc", "windows", "dos", "linux", "mac"].iter().any(|k| p.contains(k))
    })
}

/// Fills metadata from IGDB; false (and not found) without a confident match.
fn igdb_meta(lib: &Library, cfg: &Config, g: &Game) -> Result<bool> {
    let Some(e) = igdb_entry(lib, cfg, g)? else {
        lib.set_meta_state(g.id, state::NOT_FOUND)?;
        return Ok(false);
    };
    lib.save_metadata(g.id, &e.meta, None)?;
    // A cover that wasn't found before may be on IGDB.
    if !g.has_cover && g.cover_state == state::NOT_FOUND {
        lib.set_cover_state(g.id, false, None, state::PENDING)?;
    }
    Ok(true)
}

/// Fetches store metadata. Custom games: console games from IGDB; PC games matched to Steam by exact
/// (normalized) name, then IGDB. Bad IGDB credentials fall back to the Steam-only behavior.
pub fn fetch_meta(lib: &Library, cfg: &Config, g: &Game) -> Result<bool> {
    let use_igdb = g.source == Source::Custom && igdb::configured(cfg);
    if use_igdb && console(g) {
        match igdb_meta(lib, cfg, g) {
            Err(e) if igdb::is_auth_error(&e) => log::warn!("igdb {}: {e:#}", g.name),
            r => return r,
        }
    }
    let appid = match (g.source, g.steam_appid()) {
        (_, Some(id)) => Some(id),
        (Source::Custom, None) => scrape::steam_search(&g.name)?,
        (Source::Steam, None) => None,
    };
    let Some(appid) = appid else {
        if use_igdb && !console(g) {
            match igdb_meta(lib, cfg, g) {
                Err(e) if igdb::is_auth_error(&e) => log::warn!("igdb {}: {e:#}", g.name),
                r => return r,
            }
        }
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
