//! Headless tool: gamut-cli [--data DIR] <scan|list|fetch [N]|add NAME EXE|remove ID|launch ID|seed N|find TEXT|achievements APPID|import-playnite [DIR] [--dry-run] [--verbose]>
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
                let r = sync::fetch_meta(&lib, &g);
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
        _ => eprintln!("usage: gamut-cli [--data DIR] <scan|list|fetch [N]|add NAME EXE|remove ID|launch ID|seed N>"),
    }
    Ok(())
}
