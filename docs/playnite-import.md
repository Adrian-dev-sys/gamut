# Playnite import

Settings → **Import from Playnite…** (or `gamut-cli import-playnite [DIR] [--dry-run] [--verbose]`).
The source folder is the one that contains `library\games.db`: `%APPDATA%\Playnite` for installed Playnite,
or the Playnite folder itself for portable installs. Playnite must be closed, because it locks its database.

## How it reads Playnite
- `core/src/litedb.rs` is a small read-only LiteDB v4 reader: it walks the 4 KB data pages, follows extend-page
  chains and decodes the BSON. Indexes are ignored.
- `core/src/playnite.rs` loads `games.db` and the lookup tables (companies, genres, platforms, sources, tags,
  series, completion statuses, emulators), then maps each game to a `Game`. Nothing is written by `load`, which
  also powers the preview report.

## Mapping
- **Steam games** (Steam library plugin) merge into Gamut's Steam row by appid. Gamut's own Steam scan still
  decides the installed state.
- **Everything else** becomes a custom game keyed on `playnite_id` (unique index), so re-imports update the same rows.
  `source_id` stays empty: it is reserved for Steam appids.
- **Launch:**
  - File actions → exe, arguments and working dir, with `{InstallDir}` expanded.
  - URL actions → opened via the shell.
  - GOG → `goggalaxy://openGameView/ID`; Ubisoft → `uplay://launch/ID/0`.
  - Other store games (PlayStation, Battle.net, EA…) have no launch and import as not installed.
- **Emulator actions** are resolved once at import time:
  - Custom profiles use their executable, arguments and working dir.
  - Built-in profiles take the arguments and executable regex from `Emulation\Emulators\*\emulator.yaml`;
    the exe is searched in the emulator folder (and one level down), and the working dir is the emulator folder.
  - Profile `OverrideDefaultArgs` uses `CustomArguments`. Action `OverrideDefaultArgs` replaces the arguments;
    `AdditionalArguments` is appended.
  - A missing install folder that was moved into a sibling folder (e.g. `Games\X` → `Games\Ports\X`) is found
    again by `relocate`.
  - If the ROM or exe is missing, or a variable can't be expanded, the game is kept as not installed and
    listed in the report.
- **Merge rules:** playtime, last played and play count take the max; added takes the min; favorite and hidden are
  OR'd; Playnite's metadata wins where it has a value.
- **Art:** Playnite's cover and background replace Gamut's. They are copied on worker threads after the rows are
  committed (`apply_imported_art`). A cover that isn't roughly 2:3 (`images::is_cover_shaped`) is kept but marked
  `state::REPLACE`, so the cover scraper looks for a better one; games missing a cover or banner go back to the art
  queue. `sync::fetch_cover` only fetches what's missing, borrows a demo's full-game art, and falls back to a
  SteamGridDB name search.

## Schema
The Playnite columns (`playnite_id, platform, store, completion, play_count, critic_score, community_score, tags,
series`) are added by `Library::migrate` on open. `user_version` 2 requeued games with a cover but no banner once. Salvage reads older files by substituting defaults for missing columns.

## Preview
`playnite::summarize` returns a `Summary` (counts plus playable / unplayable / reason groups). The app renders it as
tables, and `playnite::report` formats it as text for the CLI.

## Confirmation
The import dialog asks twice: **Import** shows what will be overwritten (metadata including Gamut edits, covers, backgrounds, launch settings of non-Steam games), and **Yes, overwrite** runs it. "Update descriptions only" is separate and keeps edited descriptions.
