# Gamut

> **Early alpha – personal hobby project.** Built for my own use and shared as-is. Expect missing features, rough
> edges and breaking changes. No support or roadmap promises; issues and PRs may go unanswered.

A small, fast, native game library for Windows — a lightweight alternative to launchers like Playnite.
Written in Rust with [egui](https://github.com/emilk/egui) (OpenGL) and SQLite. No Electron, no web stack:
one ~14 MB executable that opens in under half a second.

## Features
- **Steam** – installed games from local Steam files; your full owned library and playtime via the Steam Web API.
- **Custom games** – any exe, shortcut or URL, with arguments and working folder; playtime is tracked while it runs.
- **Metadata & art** – descriptions, developer, publisher, genres and release dates from the Steam store; covers and
  banners from Steam's local cache, Steam's CDN, or [SteamGridDB](https://www.steamgriddb.com/). Custom games are
  matched to Steam by name, or found on SteamGridDB.
- **Fast grid** – only visible rows are drawn; covers are pre-scaled thumbnails decoded on background threads with a
  bounded GPU cache. Stays smooth with thousands of games.
- **Playnite import** – games from every Playnite library, emulated ROMs with their emulator launch settings,
  playtime, favorites, completion status, covers and backgrounds (Settings → Import from Playnite).
- Search, filters (installed, favorites, hidden, source), sorting, favorites, hiding, per-game metadata refresh.

## Getting started
Requirements: Windows 10/11, [Rust](https://rustup.rs/) (stable, MSVC toolchain) and the Visual Studio C++ Build Tools.

```powershell
git clone https://github.com/Adrian-dev-sys/gamut
cd gamut
powershell -ExecutionPolicy Bypass -File .\install.ps1
```
`install.ps1` builds a release, copies it to `%LOCALAPPDATA%\Programs\Gamut\` and adds **Gamut** shortcuts to the
Desktop and Start Menu. Re-run it after pulling to update. (Or just `cargo build --release` and run
`target\release\gamut.exe`.)

Optional, in **Settings**:
- **Steam Web API key** ([get one](https://steamcommunity.com/dev/apikey)) – imports your whole owned library, not just
  installed games. Your SteamID is detected automatically.
- **SteamGridDB API key** ([get one](https://www.steamgriddb.com/profile/preferences/api)) – covers for non-Steam games
  and anything Steam has no art for.

Keys are stored locally in your data folder and never leave your machine except in requests to those services.

## Data
`%LOCALAPPDATA%\Gamut\data\` holds `library.db`, `config.toml` and the image cache.
Put an empty `portable.txt` next to `gamut.exe` to keep everything in a `data\` folder beside the exe instead.

## Development
```powershell
cargo test
cargo run -p gamut                      # debug build of the UI
cargo run -p gamut-core --bin gamut-cli -- scan
```
`gamut-cli [--data DIR] scan | list | fetch [N] | add NAME EXE | remove ID | launch ID | seed N | import-playnite [DIR] [--dry-run]` runs the core
without the UI (`seed` creates stress-test entries).

If the project lives in a very long path, the MSVC linker can fail with `LNK1104`; set `CARGO_TARGET_DIR` to a short
path.

### Layout
- `core/` – library: `steam.rs` (VDF/ACF parsing + Web API), `scrape.rs` (Steam store, SteamGridDB), `sync.rs`
  (per-game fetch steps), `db.rs`, `images.rs`, `launch.rs`, `vdf.rs`.
- `app/` – egui UI: `ui.rs` (panels, dialogs), `thumbs.rs` (texture loader/cache), `workers.rs` (background jobs).

See [TODO.md](TODO.md) for the roadmap.

## Disclaimer
Not affiliated with Valve, Steam, SteamGridDB or Playnite. Game names and artwork shown in the app are fetched at
runtime and belong to their respective owners; none are included in this repository.

## License
[MIT](LICENSE)
