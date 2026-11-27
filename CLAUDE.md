# Gamut

Native Windows game library (Playnite alternative). Rust workspace: `core/` (lib + `gamut-cli`), `app/` (egui UI).

## Rules
- No Electron/web UI. eframe uses the `glow` backend (smaller/lighter than wgpu); keep it unless there's a reason.
- UI thread never blocks: network, disk scans and image decoding run on worker threads (`app/src/workers.rs`, `thumbs.rs`).
- Grid must stay virtualized (`ScrollArea::show_rows`) and use pre-scaled thumbnails only.
- One SQLite handle per thread (`Library::open`), rollback journal (WAL got corrupted twice by a second process). Never open the live `library.db` from another process (Python, sqlite3, a second Gamut) while Gamut runs; the CLI refuses to. Scraped metadata never overwrites user edits (`COALESCE`).
- Verify egui/eframe APIs against the crate source in `~/.cargo/registry` — they change between releases.
- Keep comments to 1–2 lines.
- Claude's tools run MSIX-sandboxed: new files they create under `%LOCALAPPDATA%` go to `Packages\Claude_*\LocalCache\Local\` and the real Gamut never sees them. Never import/fetch/copy into the real data dir from tools; ask the user to run it in their own terminal.
- Never drive the user's desktop (mouse, keyboard, screenshots) without asking first; verify with tests/CLI/logs.
- Closing a running Gamut to install a new build is allowed: `install.ps1` runs `gamut.exe --quit` itself (closing the window or a plain `taskkill` only hides it in the tray).

## Test
`cargo test`; `gamut-cli --data <tmp> scan|fetch|list|seed N` exercises core against the real Steam install.
Debugging: the app logs to `<data>\gamut.log` (last run in `gamut.old.log`; `RUST_LOG=debug` for more). Use `log::` in code and `Toasts` (`app/src/notify.rs`) for user-facing success/error messages.
Long checkout paths break MSVC linking (LNK1104): set `CARGO_TARGET_DIR` to a short path.

## Docs
- `docs/playnite-import.md` – Playnite import: LiteDB reader, field mapping, emulator launch resolution, merge rules.
- `docs/specialk.md` – Special K helper: service start/stop, injection signal, per-game override, anti-cheat skip.
- `docs/reshade.md` – automatic per-game ReShade install (download, API detection, existing-setup skip).
- `docs/launching.md` – launch pipeline, exit detection, close/force-close, auto HDR, tray icon, start with Windows.
- `docs/descriptions.md` – description images (AVIF via WIC, alpha) and Steam clips (Media Foundation playback).
- `docs/achievements.md` – achievement cache, Steam + RetroAchievements fetching and title matching, refresh rules, Achievements tab.
- `docs/statistics.md` – Statistics tab, play session logging (Gamut-timed + Steam scan deltas), Steam as playtime master.
- `docs/grid.md` – grid sort order, keyboard cursor, multi-select, bulk actions, controller, big screen.
- `docs/scanners.md` – folder scanners: finding game folders/collections, main exe guess, name cleanup, review and import.
- `docs/igdb.md` – IGDB metadata/art for non-Steam games: Twitch credentials, strict matching, pipeline, manual fill (Steam, GOG, IGDB, VNDB).
