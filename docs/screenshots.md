# Screenshots

## Index (`core/src/screenshots.rs`)
- The files are the truth; `index(games)` rebuilds the list from disk (newest first) at start, after each library change and after a capture. No DB table.
- Gamut's own: `Pictures\Gamut\<folder_name>` (Pictures read from `User Shell Folders`, `%VARS%` expanded; `Shell Folders` can be stale). `folder_name` = the game's name without characters Windows refuses, plus ` (<platform>)` (or ` (Steam)`) when another game has the same name, so duplicates stay apart. Previews in `.thumbs\<stem>.jpg`. Renaming a game leaves its old folder behind.
- Steam's: `<Steam>\userdata\<account>\760\remote\<appid>\screenshots\*.jpg` (+ `thumbnails\`), for Steam games by appid. Only read; never written or deleted.
- `gamut-cli shots` prints counts per game.

## F12 (`app/src/shots.rs`, `app/src/capture.rs`)
- A `WH_KEYBOARD_LL` hook on its own thread watches F12 (never swallows it; one shot per press; injected keys ignored) and hands off to a worker. Off with `no_screenshot_key`.
- Game in front (`game_for`): a game Gamut started whose exe (emulators) or folder holds the foreground process, else the library game whose folder holds it (deepest; exes shared by several games are emulators and are skipped). Steam games, `steamapps\common` and Gamut itself are left to Steam / ignored; unknown apps are ignored (F12 is browser dev tools too).
- Capture: DXGI desktop duplication of the window's monitor, cropped to its client area. HDR is checked per monitor first (`IDXGIOutput6` color space PQ/BT.2020): then the FP16 scRGB desktop is tone-mapped to SDR (`tonemap`: divided by Windows' SDR white level from `DisplayConfigGetDeviceInfo`, brightest channel rolled off above 0.75 so hue and saturation stay, sRGB encoded); SDR desktops are copied as they are. Fallback: GDI screen copy.
- Saved as PNG (`YYYY-MM-DD HH-MM-SS.png`, local time) with a 480 px JPEG preview; a popup shows the game and a square preview. Popups use `WDA_EXCLUDEFROMCAPTURE`, so they never end up in shots.

## Views (`app/src/shots_view.rs`)
- Top-bar tab "Screenshots": games by latest shot, one row of the newest each (virtualized rows); the name or "All N" opens the game on its Screenshots tab.
- Details tab "Screenshots N": grouped by local day.
- Viewer: full window, ←/→/Esc, Open, Show in folder; neighbours preload. Thumbnails decode on 3 threads, only when visible; unseen textures are dropped past 300.
