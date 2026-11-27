//! Headless tool: gamut-cli [--data DIR] <scan|list|status|igdb NAME [PLATFORM]|vndb NAME|gog NAME|steam NAME|fetch [N]|add NAME EXE|remove ID|launch ID|seed N|find TEXT|achievements APPID|status|import-playnite [DIR] [--dry-run] [--verbose]>
use gamut_core::{model::Game, sync, Config, Library, Paths};

fn main() -> anyhow::Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let paths = match args.iter().position(|a| a == "--data") {
        Some(i) => {
            let dir = args.remove(i + 1);
            args.remove(i);
            Paths::at(dir.into())?
        }
        None => Paths::resolve()?,
    };
    let cfg = Config::load(&paths.config);
    if args.first().map(String::as_str) == Some("import-playnite") {
        let dry = args.iter().any(|a| a == "--dry-run");
        let dir = match args.iter().skip(1).find(|a| !a.starts_with("--")) {
            Some(d) => std::path::PathBuf::from(d),
            None => gamut_core::playnite::default_dir().ok_or_else(|| anyhow::anyhow!("no Playnite library found"))?,
        };
        let t = std::time::Instant::now();
        let items = gamut_core::playnite::load(&dir)?;
        println!("Read {} in {:.1}s", dir.display(), t.elapsed().as_secs_f32());
        print!("{}", gamut_core::playnite::report(&items));
        if args.iter().any(|a| a == "--verbose") {
            for it in &items {
                if let gamut_core::playnite::Launch::Unresolved(r) = &it.launch {
                    println!("  [{}] {}: {r}", it.game.platform.as_deref().unwrap_or("?"), it.game.name);
                }
            }
        }
        if dry {
            return Ok(());
        }
        let _lock = paths.lock().ok_or_else(|| anyhow::anyhow!("Gamut is running; close it first"))?;
        let (mut lib, _) = Library::open_checked(&paths.db)?;
        let t = std::time::Instant::now();
        let st = gamut_core::playnite::import(&mut lib, &paths, &items, &|_, _| {})?;
        println!(
            "Imported {} games, {} covers, {} backgrounds in {:.1}s",
            st.games, st.covers, st.backgrounds, t.elapsed().as_secs_f32()
        );
        return Ok(());
    }
    // Network-only checks: fine while the app runs (the library isn't opened).
    match args.first().map(String::as_str) {
        Some("status") => {
            use gamut_core::status;
            let show = |r: anyhow::Result<String>| r.unwrap_or_else(|e| format!("error: {e:#}"));
            println!("Steam:             {}", show(status::steam(&cfg)));
            println!("SteamGridDB:       {}", show(status::steamgriddb(&cfg.steamgriddb_key)));
            println!("RetroAchievements: {}", show(status::retro(&cfg.ra_user, &cfg.ra_api_key)));
            let ig = if gamut_core::igdb::configured(&cfg) { show(gamut_core::igdb::check(&cfg)) } else { "not set".into() };
            println!("IGDB:              {ig}");
        }
        // igdb NAME [PLATFORM]: search results and which one would be matched automatically.
        Some("igdb") if args.len() >= 2 => {
            anyhow::ensure!(gamut_core::igdb::configured(&cfg), "IGDB isn't set up: add the Client ID and secret in Settings");
            use gamut_core::igdb;
            let list = igdb::search(&cfg, &igdb::search_name(&args[1]))?;
            let pick = igdb::best(&args[1], args.get(2).map(String::as_str), None, &list).map(|g| g.id);
            for g in &list {
                let mark = if Some(g.id) == pick { "*" } else { " " };
                println!("{mark} {:>7}  {}  ({})  main={}  [{}]", g.id, g.name, g.year.unwrap_or(0), g.main, g.platforms.join(", "));
            }
            if let Some(g) = list.iter().find(|g| Some(g.id) == pick) {
                println!("\n{:#?}\ncover: {:?}", g.meta, g.cover.as_deref().map(igdb::cover_url));
            }
        }
        // scanfolder DIR: what a folder scanner would find there (read-only).
        Some("scanfolder") if args.len() >= 2 => {
            let s = gamut_core::scanner::Scanner { dir: args[1].clone(), ..Default::default() };
            for c in gamut_core::scanner::scan(&s, &Default::default())? {
                let alts = c.exes.len() - 1;
                println!("{:<40} {}{}", c.name, c.exes[0].display(), if alts > 0 { format!("  (+{alts} more)") } else { String::new() });
            }
        }
        // vndb NAME: search results, then the first one's details.
        Some("vndb") if args.len() >= 2 => {
            use gamut_core::vndb;
            let list = vndb::search(&args[1])?;
            for v in &list {
                println!("{:>8}  {}  ({})  [{}]", v.id, v.title, v.year().unwrap_or(0), v.platforms.join(", "));
            }
            if let Some(v) = list.first() {
                let d = vndb::details(&v.id)?;
                println!("\n{:#?}\ntags: {}", d.meta, d.tags.join(", "));
            }
        }
        // gog NAME / steam NAME: store search results, then the first one's details.
        Some("gog") if args.len() >= 2 => {
            use gamut_core::gog;
            let list = gog::search(&args[1])?;
            for g in &list {
                println!("{:>11}  {}  ({})  {}", g.id, g.title, g.year.unwrap_or(0), g.kind);
            }
            if let Some(g) = list.first() {
                let d = gog::description(&g.id)?.unwrap_or_default();
                println!("\n{:#?}\ntags: {}\ndescription: {}…", g.meta, g.tags.join(", "), d.chars().take(300).collect::<String>());
            }
        }
        Some("steam") if args.len() >= 2 => {
            use gamut_core::scrape;
            let list = scrape::steam_find(&args[1])?;
            for (id, n) in &list {
                println!("{id:>8}  {n}");
            }
            if let Some(d) = list.first().map(|(id, _)| scrape::steam_details(*id)).transpose()?.flatten() {
                println!("\n{:#?}", gamut_core::model::Metadata { description: d.meta.description.map(|s| s.chars().take(300).collect()), ..d.meta });
            }
        }
        _ => {}
    }
    if matches!(args.first().map(String::as_str), Some("status" | "igdb" | "vndb" | "gog" | "steam" | "scanfolder")) {
        return Ok(());
    }
    // Writing commands must not run alongside the app.
    // Every command, even read-only ones, stays out while the app has the library open.
    let _lock = paths.lock().ok_or_else(|| anyhow::anyhow!("Gamut is running; close it first"))?;
    let (mut lib, notice) = Library::open_checked(&paths.db)?;
    if let Some(n) = notice {
        eprintln!("{n}");
    }

    match args.first().map(String::as_str) {
        Some("scan") => {
            let t = std::time::Instant::now();
            let (entries, warning) = gamut_core::steam::scan(&cfg)?;
            if let Some(w) = warning {
                eprintln!("warning: {w}");
            }
            let installed = entries.iter().filter(|e| e.installed).count();
            lib.sync_steam(&entries)?;
            println!("steam: {} games ({installed} installed) in {:?}", entries.len(), t.elapsed());
        }
        Some("find") if args.len() >= 2 => {
            let q = args[1].to_lowercase();
            for g in lib.all()?.into_iter().filter(|g| g.name.to_lowercase().contains(&q)) {
                println!("{g:#?}");
            }
        }
        Some("list") => {
            let mut games = lib.all()?;
            games.sort_by_key(|g| g.name.to_lowercase());
            for g in games {
                println!(
                    "{:>5} {:6} {:>8} {}{}{} cover={} meta={} {}",
                    g.id,
                    g.source.as_str(),
                    g.source_id,
                    if g.installed { "I" } else { "-" },
                    if g.hidden { "H" } else { " " },
                    if g.favorite { "F" } else { " " },
                    g.cover_state,
                    g.meta_state,
                    g.name
                );
            }
        }
        Some("achievements") if args.len() >= 2 => {
            use gamut_core::achievements::{steam, Fetched};
            let app: u32 = args[1].parse()?;
            let sid = gamut_core::steam::steam_id(&cfg);
            match steam(cfg.steam_api_key.trim(), &sid, app, true)? {
                Fetched::None => println!("no achievements"),
                Fetched::List { items, .. } => {
                    let done = items.iter().filter(|a| a.unlocked > 0).count();
                    println!("{done} / {} unlocked", items.len());
                    for a in items.iter().take(8) {
                        println!("  {:<40} unlocked={:<11} {:?}% hidden={} icon={}", a.name, a.unlocked, a.percent, a.hidden, !a.icon.is_empty());
                    }
                }
            }
        }
        Some("fetch") => {
            let n: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(5);
            for g in lib.needing_cover()?.into_iter().take(n) {
                let r = sync::fetch_cover(&lib, &paths, &cfg, &g);
                println!("cover {:<40} {r:?}", g.name);
            }
            for g in lib.needing_meta()?.into_iter().take(n) {
                let r = sync::fetch_meta(&lib, &cfg, &g);
                println!("meta  {:<40} {r:?}", g.name);
            }
        }
        Some("seed") => {
            // Stress-test data: clones games that have covers.
            let n: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(2000);
            let src: Vec<Game> = lib.all()?.into_iter().filter(|g| g.has_cover).collect();
            anyhow::ensure!(!src.is_empty(), "run scan + fetch first");
            for i in 0..n {
                let s = &src[i % src.len()];
                let id = lib.add_custom(&Game::new_custom(format!("{} #{i}", s.name), None))?;
                std::fs::copy(paths.thumb(s.id), paths.thumb(id))?;
                lib.set_cover_state(id, true, Some(false), gamut_core::model::state::DONE)?;
                lib.set_meta_state(id, gamut_core::model::state::NOT_FOUND)?;
            }
            println!("seeded {n}");
        }
        Some("launch") => {
            let id: i64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
            let g = lib.get(id)?.ok_or_else(|| anyhow::anyhow!("no game #{id}"))?;
            if let gamut_core::launch::Launched::Tracked(h) = gamut_core::launch::launch(&g)? {
                if let Ok(Some((mins, start))) = h.join() {
                    lib.add_playtime(id, mins, start)?;
                    println!("exited: +{mins} min, last_played={start}");
                }
            }
        }
        Some("remove") => {
            let id: i64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
            lib.delete(id)?;
            for p in [paths.cover(id), paths.thumb(id), paths.hero(id)] {
                let _ = std::fs::remove_file(p);
            }
            println!("removed #{id}");
        }
        Some("add") if args.len() >= 3 => {
            let id = lib.add_custom(&Game::new_custom(args[1].clone(), Some(args[2].clone())))?;
            println!("added #{id}");
        }
        _ => eprintln!("usage: gamut-cli [--data DIR] <scan|list|status|igdb NAME [PLATFORM]|vndb NAME|gog NAME|steam NAME|fetch [N]|add NAME EXE|remove ID|launch ID|seed N>"),
    }
    Ok(())
}
