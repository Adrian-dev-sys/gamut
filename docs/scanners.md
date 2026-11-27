# Folder scanners

Find games in a folder and add them as custom entries (`core/src/scanner.rs`, UI `app/src/scan_ui.rs`).

## Scanners
- Saved in config (`scanners`, array of tables at the end of config.toml): name, folder, file types
  (comma separated, default `exe`), exclusions (one per line, `*`/`?`; a bare name matches any folder/file
  name, a pattern with `\` the path inside the scanned folder), depth (folders below each game folder,
  default 4), platform and tags given to added games.
- Top bar → "Scan folders" opens the list/editor; Scan runs `scanner::scan` on a thread.
- `gamut-cli scanfolder DIR` prints what a default scanner finds (read-only, safe while Gamut runs).

## Finding games
- Each folder in the scanned folder is a game; loose matching files there are games too.
- Collections (`split`): a folder with no matching file at its top and several subfolders with files splits
  into one game per subfolder, recursively ("NEKOPARA Collection\Vol. 1"…). A single subfolder
  (`Game\Binaries\Win64`) stays one game named after the outer folder.
- Skipped: installers, redistributables, crash reporters, anti-cheat, updaters (`JUNK_FILES`) and folders like
  `_CommonRedist`, `Redist`, `DirectX`, `Engine`, `steamapps`, `llvm` (`JUNK_DIRS`).
- Main file (`rank`): named like the folder (+100, partial +40), shallower (−15 per level, so a root
  launcher stub beats `Binaries\Win64\*-Shipping.exe`), bigger (log size, up to +25), "launcher"/"config"/
  "settings" −20. The review lets you pick another.
- Already in the library: a candidate whose folder is a game's install folder or whose file is a game's exe
  is shown greyed (hidden by default).

## Names (`clean_name`)
- Drops `[…]` always, `(…)` only when it holds tags/versions ("(GOG)", "(v1.2)", "(Win)") — "(The Final
  Cut)" stays.
- Store tags (GOG, repack, FitGirl…) anywhere; versions (`v1.5`, `1.2.7`, `Build 123`, `v1_1`); scene groups
  and "Setup" only when glued with a hyphen ("-CODEX"); language/platform words only at the end or glued
  ("…v1.13.ENG", "-pc") so "Minecraft for Windows" keeps "Windows".
- Run-together names: dots/underscores/hyphens become spaces; CamelCase splits only with 2+ humps
  ("HappyMemoriesISummonedAWhat", not "FlipWitch").

## Adding
- Ticked rows become custom games: exe, its folder as working dir, the scanner's platform/tags
  (`GamutApp::import_scanned`), then the usual pipeline fetches metadata and covers (docs/igdb.md).
- Not yet: automatic rescans, remembering unticked results, reporting games whose exe disappeared.
