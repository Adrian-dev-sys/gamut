# Gamut

Native Windows game library (Playnite alternative). Rust workspace: `core/` (lib + `gamut-cli`), `app/` (egui UI).

## Rules
- No Electron/web UI. eframe uses the `glow` backend (smaller/lighter than wgpu); keep it unless there's a reason.
- UI thread never blocks: network, disk scans and image decoding run on worker threads (`app/src/workers.rs`, `thumbs.rs`).
- Grid must stay virtualized (`ScrollArea::show_rows`) and use pre-scaled thumbnails only.
- One SQLite handle per thread (`Library::open`), WAL mode. Scraped metadata never overwrites user edits (`COALESCE`).
- Verify egui/eframe APIs against the crate source in `~/.cargo/registry` — they change between releases.
- Keep comments to 1–2 lines.
- Never drive the user's desktop (mouse, keyboard, screenshots, closing their Gamut) without asking first; verify with tests/CLI/logs.

## Test
`cargo test`; `gamut-cli --data <tmp> scan|fetch|list|seed N` exercises core against the real Steam install.
Long checkout paths break MSVC linking (LNK1104): set `CARGO_TARGET_DIR` to a short path.
