# TODO

## Next up
- [x] **Import from Playnite** – read Playnite's library (games, custom entries, edited metadata, covers, playtime) so an existing setup carries over in one go.
- [x] **Themes** – load colors, fonts, spacing and corner radius from a `theme.toml`; hot-reload while editing.

## More sources
- [x] **Folder scanners** – saved scanners, review with checkboxes, best-guess exe and name, metadata fetched after adding
- [x] Scanner tweaks: no depth limit, platform dropdown, tag suggestions, unticked results remembered, new games shown first
- [ ] Scanners step 2: automatic rescans; emulator scanners
- [ ] GOG (GOG Galaxy / installed games)
- [ ] Epic Games Store
- [ ] Xbox / Microsoft Store (PC Game Pass)
- [ ] **Emulator auto-scanners** – point at ROM folders, detect the system (RetroArch, Dolphin, PCSX2, RPCS3, Ryujinx/yuzu forks, Cemu, DuckStation, PPSSPP…), create entries with the right launch command, then fetch metadata and covers per platform
- [ ] Battle.net, EA app, Ubisoft Connect

## Library & UI
- [x] Keyboard navigation in the grid (arrow keys, Home/End, PageUp/PageDown)
- [x] Natural sort order ("#56" before "#556")
- [x] List view with a details side panel, alongside the grid
- [x] Tags/categories and user collections (hand-filled and dynamic, Steam-style tiles)
- [x] Multi-select + bulk edit (hide, favorite, refresh metadata)
- [x] Choose between several covers/banners per game (SteamGridDB, IGDB, VNDB, Steam alternatives)
- [x] Remember window size/position and last selected filter/sort
- [ ] Library briefly flashes behind open popups. Gamut's own frames were verified correct (pixel readback), so likely G-Sync/VRR with uneven frame pacing; try Fixed Refresh for gamut.exe, then steady repaints while a popup is open

## Library health & maintenance
- [x] **Library health** – games with no launcher, missing exe/install folder (incl. scanned games whose exe disappeared), or likely duplicates; per-platform exclusions (e.g. PS5 games played on the console, unless an emulator is set up)
- [x] **Disk usage** – install size per game (done: per game, Steam + Calculate), Gamut's own data (covers, thumbnails, caches) with clear-cache buttons
- [x] **Uninstall from Gamut** – Steam first (`steam://uninstall/<appid>`)
- [x] **Backups** – library database, settings and art; scheduled + manual, restore
- [ ] **Security** – review: API key storage (plain config today), downloads (ReShade/Special K), launching untrusted paths, update checks

## Metadata
- [x] IGDB as an additional metadata source (richer data for non-Steam games)
- [x] Manual "search and match" when automatic name matching fails (Fill from Steam/GOG/IGDB/VNDB, Compare all)

## Playtime & stats
- [x] Playtime for custom games (tracked while the launched process runs)
- [x] Per-game play sessions tab; play count; editable playtime/play count
- [x] Track the real game process when a custom entry starts a launcher that exits immediately (follow child processes / watch the install folder)
- [x] **Statistics** – total/recent playtime, most played, per-platform and per-genre breakdowns, completion status

## Integrations (later, once the core UI and basics are solid)
- [x] **Special K** – optionally inject Special K automatically when launching a game (like Playnite's Special K Helper), with per-game on/off
- [x] **RetroAchievements** – show achievement progress for emulated games
- [x] **Achievements (Success Story-style)** – Steam and other platforms' achievements in the details view
- [x] **HowLongToBeat** – main/extra/completionist times in the details view

## Extras
- [x] Fullscreen / controller mode (gamepad navigation, big-picture style)
- [x] Tray icon (minimize to tray, quick-launch recent games), start with Windows
- [x] App icon
- [ ] Installer/portable zip release (GitHub Actions)
