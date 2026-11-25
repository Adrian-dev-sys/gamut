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
  - If the ROM or exe is missing, or a variable can't be expanded, the game is kept as not installed and
    listed in the report.
- **Merge rules:** playtime, last played and play count take the max; added takes the min; favorite and hidden are
  OR'd; Playnite's metadata wins where it has a value.
- **Art:** Playnite's cover and background replace Gamut's. They are copied on worker threads after the rows are
  committed. Imported custom games skip the scrapers (meta and cover marked done or not found); a custom game whose
  cover fails to decode goes back to the cover queue.

## Schema
The Playnite columns (`playnite_id, platform, store, completion, play_count, critic_score, community_score, tags,
series`) are added by `Library::migrate` on open. Salvage reads older files by substituting defaults for missing columns.
