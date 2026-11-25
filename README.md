# Gamut

A small, fast native game library for Windows (Playnite alternative). Rust + egui (OpenGL), SQLite, no web stack.

## Features
- **Steam**: installed games from local manifests; full owned library + playtime via the Steam Web API.
- **Custom games**: any exe / shortcut / URL, with arguments and working dir; playtime tracked while the process runs.
- **Metadata & art**: Steam store (description, developer, publisher, genres, release date), covers and banners from
  Steam's local cache → Steam CDN → SteamGridDB. Custom games are matched to Steam by name, or use SteamGridDB search.
- **Fast grid**: virtualized rows, covers pre-scaled to 300×450 thumbnails on disk, decoded on worker threads,
  bounded GPU texture cache.

## Build
```powershell
cargo build --release          # target\release\gamut.exe (+ gamut-cli.exe)
cargo test
```
If the project path is very long, MSVC's linker can fail with `LNK1104`; set `CARGO_TARGET_DIR` to a short path.

## Data
`%LOCALAPPDATA%\Gamut\` holds `library.db`, `config.toml` (API keys) and image caches.
Put an empty `portable.txt` next to `gamut.exe` to keep data in `.\data\` instead.

## CLI
`gamut-cli [--data DIR] scan | list | fetch [N] | add NAME EXE | seed N` (seed = stress-test data).

## Layout
- `core/` – library: `steam.rs` (VDF/ACF + Web API), `scrape.rs` (Steam store, SteamGridDB), `sync.rs` (per-game
  fetch steps), `db.rs`, `images.rs`, `launch.rs`, `vdf.rs`.
- `app/` – egui UI: `ui.rs` (panels, dialogs), `thumbs.rs` (texture loader/cache), `workers.rs` (background jobs).
