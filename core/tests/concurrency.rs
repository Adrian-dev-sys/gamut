//! Several threads with their own handles writing at once, like the app's workers.
use gamut_core::db::SteamEntry;
use gamut_core::model::{state, Metadata};
use gamut_core::Library;

#[test]
fn concurrent_writers_keep_db_intact() {
    let dir = std::env::temp_dir().join(format!("gamut-conc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("t.db");
    let entries: Vec<SteamEntry> = (1..1500)
        .map(|i| SteamEntry { appid: i, name: format!("Game {i}"), installed: i % 3 == 0, install_dir: None, playtime_min: Some(i as i64), last_played: 0 })
        .collect();
    Library::open(&db).unwrap();
    let desc = "x".repeat(6000); // forces overflow pages like long Steam descriptions
    let mut hs = vec![];
    for t in 0..4 {
        let (db, entries, desc) = (db.clone(), entries.clone(), desc.clone());
        hs.push(std::thread::spawn(move || {
            let mut lib = Library::open(&db).unwrap();
            for round in 0..5 {
                if t == 0 { lib.sync_steam(&entries).unwrap(); }
                for g in lib.all().unwrap() {
                    match t {
                        1 => lib.save_metadata(g.id, &Metadata { description: Some(format!("{desc}{round}")), ..Default::default() }, None).unwrap(),
                        2 => lib.set_cover_state(g.id, true, Some(true), state::DONE).unwrap(),
                        _ => lib.requeue(g.id, round % 2 == 0).unwrap(),
                    }
                }
            }
        }));
    }
    for h in hs { h.join().unwrap(); }
    let lib = Library::open(&db).unwrap();
    assert_eq!(lib.integrity_check().unwrap(), "ok");
    assert_eq!(lib.all().unwrap().len(), 1499);
}

#[test]
fn damaged_db_is_rebuilt() {
    let dir = std::env::temp_dir().join(format!("gamut-dmg-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("t.db");
    {
        let mut lib = Library::open(&db).unwrap();
        let entries: Vec<SteamEntry> = (1..400)
            .map(|i| SteamEntry { appid: i, name: format!("Game {i} {}", "y".repeat(300)), installed: true, install_dir: None, playtime_min: None, last_played: 0 })
            .collect();
        lib.sync_steam(&entries).unwrap();
        lib.add_custom(&gamut_core::Game::new_custom("Mine".into(), None)).unwrap();
    }
    // Scribble over a page in the middle of the file.
    let mut bytes = std::fs::read(&db).unwrap();
    let mid = (bytes.len() / 4096 / 2) * 4096;
    for b in &mut bytes[mid + 8..mid + 4096] { *b = 0xAB; }
    std::fs::write(&db, bytes).unwrap();

    let (lib, notice) = Library::open_checked(&db).unwrap();
    let notice = notice.expect("damage should be detected");
    let n = lib.all().unwrap().len();
    println!("{notice} -> {n} rows");
    assert_eq!(lib.integrity_check().unwrap(), "ok");
    assert!(n > 300 && n < 400, "recovered {n}");
}
