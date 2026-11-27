//! Heavy write workload on a DB path; run two at once to test multi-process access.
use gamut_core::db::SteamEntry;
use gamut_core::model::{state, Metadata};
use gamut_core::Library;

fn main() {
    let db = std::path::PathBuf::from(std::env::args().nth(1).expect("db path"));
    let entries: Vec<SteamEntry> = (1..1500)
        .map(|i| SteamEntry { appid: i, name: format!("Game {i}"), installed: i % 3 == 0, install_dir: None, size: None, playtime_min: Some(i as i64), last_played: 0 })
        .collect();
    let desc = "x".repeat(6000);
    let hs: Vec<_> = (0..4)
        .map(|t| {
            let (db, entries, desc) = (db.clone(), entries.clone(), desc.clone());
            std::thread::spawn(move || {
                let mut lib = Library::open(&db).unwrap();
                for round in 0..8 {
                    if t == 0 { lib.sync_steam(&entries).unwrap(); }
                    for g in lib.all().unwrap() {
                        match t {
                            1 => lib.save_metadata(g.id, &Metadata { description: Some(format!("{desc}{round}")), ..Default::default() }, None).unwrap(),
                            2 => lib.set_cover_state(g.id, true, Some(true), state::DONE).unwrap(),
                            _ => lib.requeue(g.id, round % 2 == 0).unwrap(),
                        }
                    }
                }
            })
        })
        .collect();
    for h in hs { h.join().unwrap(); }
    println!("{}", Library::open(&db).unwrap().integrity_check().unwrap());
}
