# Game details: extra fields, actions, scripts, HowLongToBeat, saves

Columns on `games` (added in `db.rs` `ADDED_COLS`): `sort_name`, `user_score`, `notes`, `links` (JSON `[Link]`), `actions` (JSON `[PlayAction]`), `script_before`, `script_after`, `install_size` (bytes), `save_dir`, `display` (JSON `display::DisplayMode`). Icons are files at `Paths::icon(id)` (`icons/<id>.img`), no column.

## Editor tabs
- **General**: Sorting name (grid "Name" sort and its tiebreaks use it; cleared when equal to the name), Your score.
- **Launch**: install folder (custom games only; Steam keeps its own), size (Steam fills it from the manifest's `SizeOnDisk`; other installed games are measured in the background after each Steam scan, `workers::sizes`; "Recalculate" redoes one), save folder (Browse or "Find" on PCGamingWiki).
- **Actions & scripts**: extra actions (name, path, args, working folder, "Counts as playing"); PowerShell before/after scripts.
- **Links & notes**: own links (shown after the automatic ones), notes (shown above the description).
- **Media**: icon (png/ico/jpg/webp copied in on Save, or "Search online…": the art picker in Icon mode, SteamGridDB icons plus Steam's cached client icon).

## Folders
- `Game::folder()`: install folder, else the ROM's folder (first quoted absolute path in the arguments), else the exe's. Used by Open install folder, `{{p|game}}` save paths and scripts.
- `game_size` (lib.rs): a ROM in a folder shared with other games (`shared_folders`) counts only its own files (`Name.*`, `Name (Track n).*`); otherwise the ROM's folder (PS3 `PS3_GAME\USRDIR` and Wii U `code` climb to the game root) or the install folder. An emulator's own folder is never measured. Launcher links (GOG Galaxy, Ubisoft) get no size.

## Launching
- Extra actions appear in the More / right-click menu. "Counts as playing" goes through `Workers::play` with `Game::with_action` (time tracked, Special K/ReShade/HDR, scripts); otherwise the file is just opened. A tracked action is refused while the game runs.
- `scripts::before` runs `powershell -NoProfile -ExecutionPolicy Bypass -Command`, hidden, waited for up to 2 minutes; a failure is a toast and the game still starts. `scripts::After` runs on drop, i.e. when tracking sees the game exit. Env: `GAMUT_GAME_NAME`, `GAMUT_GAME_DIR`, `GAMUT_GAME_EXE`.

## Display mode (`core/src/display.rs`)
- Launch tab "Display": screen, resolution, refresh rate (0 = keep). Applied before launch with `ChangeDisplaySettingsExW(CDS_FULLSCREEN)` (temporary; Windows undoes it if Gamut dies), restored on exit by resetting to the registry mode. Skipped while another game holds a change. Shown in the details Install card.
- Playnite's Display Helper stores it as features `[RC] WxH`, `[RC] NHz`, `[RC] Display: \.\DISPLAYn`; read by the import and "Links, notes and more".

## HowLongToBeat (`core/src/hltb.rs`)
- Unofficial API: the search path is found in the site's `/_next/static/chunks/*.js` (a `fetch("/api/…", {method: "POST"})`), cached per session, fallback `api/s`. `GET <path>/init?t=<ms>` gives a token (`x-auth-token`; a key/val pair, if present, goes in headers and body). POST the search JSON with a browser User-Agent and Referer/Origin. Times are seconds.
- Matching is strict (`scrape::same_game` on the name, its port variants, or the HLTB aliases). Not found is cached as id 0.
- Cache table `hltb`; fetched lazily when a game is opened, refreshed after 60 days (14 for not found). Shown in the details column; the HowLongToBeat link goes to the game's page once matched.
- `gamut-cli hltb NAME` checks it.

## Save folders (`core/src/pcgw.rs`)
- Page: Steam games by `api/appid.php?appid=` redirect, others by name. Cargo queries aren't allowed for anonymous clients; `action=parse&prop=wikitext` is.
- Reads `{{Game data/saves|Windows|…}}` (then `Steam` cloud) entries, expands `{{p|…}}` (game, steam, appdata, localappdata, userprofile\Documents with OneDrive fallback, uid as `*`), drops file patterns, picks the newest match for wildcards, keeps the first folder that exists.
- "Open save folder" uses `save_dir` if it exists, else looks it up, stores and opens it. `gamut-cli saves NAME [APPID]` checks it.

## Playnite
- Import reads SortingName, Notes, UserScore, InstallSize, Links, Icon (.ico decodes too), every GameAction except the first play action, and Display Helper's `[RC]` features. Settings → Playnite → "Links, notes and more" fills these for already imported games, only where Gamut has none (`Library::import_extras`). Playnite must be closed (its files are locked). PreScript/PostScript are not imported: they use Playnite's runspace (`$Game`, `$PlayniteApi`) and would fail in plain PowerShell.

## Uninstall
`core/src/uninstall.rs`, `app/src/ui/uninstall.rs`. Menu item for installed Steam games and non-emulator games whose folder exists.
- Steam: `steam://uninstall/<appid>` (digits only); Steam confirms.
- Others: the Windows Uninstall key (HKCU, HKLM 64/32) whose `InstallLocation`, or the `UninstallString` exe's folder, is exactly the game's folder (no prefix matches: a folder like `D:\Games` would hit every app in it). Gamut shows the command and confirms, then runs it via `cmd /C start "" …` (ShellExecute, so UAC prompts work). Optional: remove from the library too. None found → error toast.
