//! Repro harness: Playnite import while scraper-like writers and readers hit the same database.
//! Usage: cargo run --release -p gamut-core --example import_stress -- <data dir copy> [playnite dir]
use gamut_core::{playnite, Library, Paths};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let paths = Paths::at(args[0].clone().into())?;
    let pn = args.get(1).map(Into::into).or_else(playnite::default_dir).expect("playnite dir");
    let items = playnite::load(&pn)?;
    {
        let (lib, notice) = Library::open_checked(&paths.db)?;
        println!("start: {} rows, check {}, notice {notice:?}", lib.all()?.len(), lib.integrity_check()?);
    }
    let stop = Arc::new(AtomicBool::new(false));
    let mut hs = Vec::new();
    for w in 0..3 {
        let (p, stop) = (paths.clone(), stop.clone());
        hs.push(std::thread::spawn(move || {
            let lib = Library::open(&p.db).unwrap();
            let mut n = 0u64;
            while !stop.load(Ordering::Relaxed) {
                // What the scrapers and UI do: read lists, update single rows.
                let list = if w == 0 { lib.needing_cover() } else { lib.needing_meta() }.unwrap_or_default();
                for g in list.iter().take(20) {
                    let _ = lib.set_cover_state(g.id, g.has_cover, None, g.cover_state);
                    let _ = lib.set_meta_state(g.id, g.meta_state);
                    n += 1;
                }
                if w == 2 {
                    let _ = lib.all();
                }
            }
            n
        }));
    }
    let mut lib = Library::open(&paths.db)?;
    for round in 0..3 {
        let st = playnite::import(&mut lib, &paths, &items, &|_, _| {})?;
        println!("round {round}: {} games, check {}", st.games, lib.integrity_check()?);
    }
    stop.store(true, Ordering::Relaxed);
    let writes: u64 = hs.into_iter().map(|h| h.join().unwrap()).sum();
    println!("concurrent writes: {writes}, final check: {}", lib.integrity_check()?);
    Ok(())
}
