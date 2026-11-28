# Screenshots

## Index (`core/src/screenshots.rs`)
- The files are the truth; `index(games)` rebuilds the list from disk (newest first) at start, after each library change and after a capture. No DB table.
- Gamut's own: `Pictures\Gamut\<folder_name>` (Pictures read from `User Shell Folders`, `%VARS%` expanded; `Shell Folders` can be stale). `folder_name` = the game's name without characters Windows refuses, plus ` (<platform>)` (or ` (Steam)`) when another game has the same name, so duplicates stay apart. Previews in `.thumbs\<stem>.jpg`. Renaming a game leaves its old folder behind.
- Steam's: `<Steam>\userdata\<account>\760\remote\<appid>\screenshots\*.jpg` (+ `thumbnails\`), for Steam games by appid. Only read; never written or deleted.
- `gamut-cli shots` prints counts per game.

## F12 (`app/src/shots.rs`, `app/src/capture.rs`)
- A `WH_KEYBOARD_LL` hook on its own thread watches F12 (never swallows it; one shot per press; injected keys ignored) and hands off to a worker. Off with `no_screenshot_key`.
- Controller: Steam's chord, PS/Guide + R1, or Create/View + R1 (Windows hides an Xbox pad's Guide button), read on the gamepad thread even while a game has focus (`Pad::Screenshot` → `shots::trigger`).
- Game in front (`game_for`): a game Gamut started whose exe (emulators) or folder holds the foreground process, else the library game whose folder holds it (deepest; exes shared by several games are emulators and are skipped). Steam games, `steamapps\common` and Gamut itself are left to Steam / ignored; unknown apps are ignored (F12 is browser dev tools too).
- Capture: DXGI desktop duplication of the window's monitor, cropped to its client area. HDR is checked per monitor first (`IDXGIOutput6` color space PQ/BT.2020); SDR desktops are copied as they are. HDR (`Tonemap`): the FP16 scRGB desktop in nits; colors outside sRGB are desaturated toward their luminance (not clipped); the brightest channel goes through the BT.2390 EETF in the PQ domain from the frame's peak (99.9th percentile, capped at the display's MaxLuminance) to the geometric mean of Windows' SDR white (`DisplayConfigGetDeviceInfo`) and that peak, which becomes 255 (Special K HDR puts most of the scene above SDR white; tuned on raw XC2 frames); that ratio-preserving result is averaged 50/50 with each channel rolled off on its own (luminance mapping plus desaturation washed skies out; ratio-only made bright skin red). The log line gives white, frame peak and display peak. Fallback: GDI screen copy.
- Saved as lossy WebP quality 88 with sharp YUV (`YYYY-MM-DD HH-MM-SS.webp`, local time; `webp` crate = libwebp): about half the size of a well-encoded JPEG (tested: 3440x1440 PNG 4.7 MB → JPEG 1.07 MB → WebP 0.4–0.5 MB), opened by Windows Photos and browsers with a 640 px preview; a popup shows the game and a square preview. Popups use `WDA_EXCLUDEFROMCAPTURE`, so they never end up in shots.

## Views (`app/src/shots_view.rs`)
- Top-bar tab "Screenshots": games by latest shot, every shot grouped by game (only groups in view are laid out); the name opens the game on its Screenshots tab.
- Details tab "Screenshots N": grouped by local day.
- Viewer: full window, ←/→/Esc, Full screen (undone on close), Open, Show in folder (`explorer /select,"path"` via `raw_arg`); neighbours preload. Thumbnails are 640 px with mipmaps, decoded on 3 threads only when visible; Steam's own previews (200 px) are too small, so the full image is decoded instead; unseen textures are dropped past 300.
- Viewer "Delete" (or Del) sends a Gamut shot and its `.thumbs` preview to the Recycle Bin; Steam shots can't be deleted here (Steam Cloud re-syncs them).
- A short synthesized shutter click (`shots::sys::shutter`, `PlaySoundW` from memory) plays when the frame is grabbed; Settings can turn it off (`no_screenshot_sound`).

- The details About tab shows the newest screenshots under the description (up to 2 rows, 320 px tiles; "All N" opens the Screenshots tab).
