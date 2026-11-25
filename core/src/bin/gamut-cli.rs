//! Headless tool: gamut-cli [--data DIR] <scan|list|fetch [N]|add NAME EXE|remove ID|launch ID|seed N>
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
    let mut lib = Library::open(&paths.db)?;

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
        Some("fetch") => {
            let n: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(5);
            for g in lib.needing_cover()?.into_iter().take(n) {
                let r = sync::fetch_cover(&lib, &paths, &cfg, &g);
                println!("cover {:<40} {r:?}", g.name);
            }
            for g in lib.needing_meta()?.into_iter().take(n) {
                let r = sync::fetch_meta(&lib, &g);
                println!("meta  {:<40} {r:?}", g.name);
                std::thread::sleep(std::time::Duration::from_millis(1500));
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
