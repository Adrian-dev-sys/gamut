# Stores (GOG, …)

`core/src/stores/` (one file per store + `sync.rs`), `app/src/ui/stores.rs` (Settings → Stores, review dialog), `workers.rs` (`store_review`, `sync_store`, `watch_install`).

## Model
- Store games are custom rows with `store` = the store's name (Playnite's source name, e.g. "GOG"). The link lives in `store_links (store, store_id, game_id)`; `store_skipped` remembers games the user chose not to import. Both survive `salvage`; `delete` drops a game's links.
- Installed: `exe`/`args`/`work_dir` from the store, `install_dir` its folder. Not installed: `exe` = the store link (e.g. `goggalaxy://openGameView/<id>`), `installed = 0`; `Game::needs_install` makes Play read "Install". Install opens the link (a missing protocol handler gives "needs GOG Galaxy"), and `watch_install` syncs the store every minute for up to 3 h until the game is installed.
- A linked row's launch only changes while it isn't a working exe of the user's (empty, a link, or a missing file), so custom launch setups survive.

## Matching (`sync::plan`)
1. Linked before → follows the store.
2. Exact: an unlinked custom row whose exe/args contain the store link (`Store::launches`, e.g. Playnite's `openGameView/<id>`) → linked.
3. Doubt: same normalized name (letters/digits) or same folder as an unlinked custom row → the user picks (same game / import / don't), never merged automatically.
4. New.

First import goes through the review dialog (Settings → Stores → Import games…), which turns on "Keep in sync" (`config.stores`). Synced on start, hourly and with Refresh after the Steam scan: links follow, new games import, doubts wait ("Review…").

## GOG (`stores/gog.rs`)
- Owned: Galaxy's `C:\ProgramData\GOG.com\Galaxy\storage\galaxy-2.0.db`, read from a temp copy (with -wal/-shm). `LibraryReleases` ⨝ `ReleaseProperties` where `isDlc = 0 AND isVisibleInLibrary = 1` (drops DLCs, packs/bundles and duplicate editions, matching Playnite's list), title from `GamePieces` type `title`.
- Installed: `HKLM\SOFTWARE\WOW6432Node\GOG.com\Games\<id>` (`gameID`, `gameName`, `path`; `dependsOn` set = DLC, skipped). Launch: the primary `FileTask` in `<path>\goggame-<id>.info` (path, arguments, workingDir relative to the folder), else the registry `exe`.
- Launchers: a tracked exe that exits within 90 s (e.g. Cyberpunk's REDprelauncher) is followed by watching processes in the game's folder (up to 3 min to appear).
- Without Galaxy only registry-installed games are found. Uninstall uses the Windows uninstaller for the folder (GOG installers register one).
