# TODO

## Next up
- [x] **Import from Playnite** – read Playnite's library (games, custom entries, edited metadata, covers, playtime) so an existing setup carries over in one go.
- [ ] **Themes** – load colors, fonts, spacing and corner radius from a `theme.toml`; hot-reload while editing.

## More sources
- [ ] GOG (GOG Galaxy / installed games)
- [ ] Epic Games Store
- [ ] Xbox / Microsoft Store (PC Game Pass)
- [ ] **Emulator auto-scanners** – point at ROM folders, detect the system (RetroArch, Dolphin, PCSX2, RPCS3, Ryujinx/yuzu forks, Cemu, DuckStation, PPSSPP…), create entries with the right launch command, then fetch metadata and covers per platform
- [ ] Battle.net, EA app, Ubisoft Connect

## Library & UI
- [ ] Keyboard navigation in the grid (arrow keys, Home/End, PageUp/PageDown)
- [ ] Natural sort order ("#56" before "#556")
- [ ] List view and a compact/details view alongside the grid
- [ ] Tags/categories and user collections
- [ ] Multi-select + bulk edit (hide, favorite, refresh metadata)
- [ ] Choose between several covers/banners per game (SteamGridDB alternatives)
- [x] Remember window size/position and last selected filter/sort

## Metadata
- [ ] IGDB as an additional metadata source (richer data for non-Steam games)
- [ ] Manual "search and match" when automatic name matching fails

## Playtime & stats
- [x] Playtime for custom games (tracked while the launched process runs)
- [ ] Track the real game process when a custom entry starts a launcher that exits immediately (follow child processes / watch the install folder)
- [ ] **Statistics** – total/recent playtime, most played, per-platform and per-genre breakdowns, completion status

## Integrations (later, once the core UI and basics are solid)
- [ ] **Special K** – optionally inject Special K automatically when launching a game (like Playnite's Special K Helper), with per-game on/off
- [ ] **RetroAchievements** – show achievement progress for emulated games
- [ ] **Achievements (Success Story-style)** – Steam and other platforms' achievements in the details view
- [ ] **HowLongToBeat** – main/extra/completionist times in the details view

## Extras
- [ ] Fullscreen / controller mode (gamepad navigation, big-picture style)
- [ ] Tray icon (minimize to tray, quick-launch recent games)
- [ ] App icon and installer/portable zip release (GitHub Actions)
