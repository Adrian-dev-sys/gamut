# Launching, closing, HDR

- **`Workers::play`** runs on a worker thread and does these in order:
  1. turn HDR on (`hdr::Guard`);
  2. install ReShade (docs/reshade.md);
  3. start Special K (docs/specialk.md);
  4. launch the game (`launch::launch_with`, which carries the Big Picture env var).

  The game counts as "running" from the click. For Steam games, `steam_starting` holds it until Steam reports the app running, or 90 s pass.
- **Game exit.**
  - Tracked exes: the spawned child exiting.
  - Steam games: `steam::running_appid` changing away from the app.
  - Shortcuts and URLs: no process under `procs::game_dirs` any more.

  `wait_exit` waits up to 10 minutes for the game to appear first.
- **Close dialog.** Clicking "Running" asks to close the game. Both actions target the processes whose image path is under the game's install or exe folder (`procs::under`):
  - "Close game" posts WM_CLOSE to their visible windows;
  - "Force close" terminates them.
- **HDR.** A per-game `games.auto_hdr` setting.
  - `hdr::Guard` switches "Use HDR" on (DisplayConfigSetDeviceInfo, advanced color state) on HDR-capable displays, if it was off, then waits 2 s.
  - It turns HDR back off when the last HDR game exits, and only if Gamut was the one that turned it on.
- **Second game.** Games may run side by side, except two Steam games: Steam runs one at a time, so
  starting one while another Steam game runs asks first (`play_window`, Cancel focused).

## Tray and autostart
- `app/src/tray.rs`: tray icon (tray-icon crate, created on the UI thread). Left click opens Gamut; the menu
  has Open Gamut, Big screen, the 10 most recently played (non-hidden) games, Exit.
- Closing the window hides it (`App::logic`, which eframe keeps calling while hidden: CancelClose +
  Visible(false)) unless Settings → General "keeps Gamut in the tray" is off (`quit_on_close`). Exit sets
  `quitting` and closes for real. While hidden, worker events are still handled and controller input is dropped.
- A second gamut.exe sets the named event `Local\GamutShowWindow` and exits; the running one shows its window.
- Start with Windows writes `HKCU\...\Run\Gamut = "<exe>" --tray`; `--tray` starts hidden (`keep_hidden`
  re-hides for a few frames because eframe shows the window after its first paint).
