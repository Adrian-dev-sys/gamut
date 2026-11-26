# Achievements

Per-game achievements on the details page, cached in the library DB. Steam games use the Steam Web API; custom (emulated) games use RetroAchievements.

## Storage (`core/src/db.rs`)
- `achievements(game_id, key, ord, name, description, icon, icon_locked, hidden, unlocked, percent, points)`; `unlocked` is a unix time, 0 = locked.
- `achievement_state(game_id, fetched, schema_at, total, unlocked, error)`: one row per fetched game. `total = 0` means "no achievements"; `error = 'private'` means Steam refused.
- Both are a cache: `delete` clears them with the game (ids get reused), a DB salvage doesn't copy them.

## Steam (`core/src/achievements.rs`)
- `GetPlayerAchievements` (key + SteamID, `l=english`) gives unlocks plus names/descriptions. 400 = the game has no stats → `Fetched::None`.
- 403 "Profile is not public" comes back both for a private profile and for a game the account doesn't own, so one 403 only marks that game.
- `full` fetches add `GetSchemaForGame` (icons, hidden flag, order) and `GetGlobalAchievementPercentagesForApp` (rarity; `percent` is a string).
- Error messages never include the URL (it carries the API key).

## RetroAchievements (`core/src/retro.rs`)
- Settings: `ra_user` + `ra_api_key`. Auth is `y=<key>`; `u=` is the user whose progress is read. Errors never include the URL.
- `games.ra_id`: NULL = not looked up, `-1` = platform not on RA (section hidden), `0` = not found (section offers "Pick the game…"), `>0` = RA game id. Editing a game's platform (or the name of an unmatched game) resets it to NULL.
- Lookup: `API_GetConsoleIDs` and `API_GetGameList` (f=1) are cached as JSON in the `ra` folder of the data dir for a week. Platform → system via `console_for` (Playnite names, aliases, maker prefix stripped; comma lists tried in order). Title match is exact on `title_key` (no tags in ()/[], articles, accents, punctuation); RA `~Hack~`/`~Homebrew~` and `[Subset …]` entries are skipped; ` | ` alternative titles count.
- The picker dialog lets the user choose the RA game by hand; a different game drops the cached list.
- `API_GetGameInfoAndUserProgress`: `Achievements` is an object keyed by id (or `[]` when empty). Unlock = `DateEarnedHardcore` else `DateEarned` (UTC). Rarity = NumAwarded / NumDistinctPlayers. Badges: `media.retroachievements.org/Badge/<BadgeName>.png`, locked `<BadgeName>_lock.png`.
- No ROM hashing yet; that would replace title matching for exact ids.

## When it fetches (`app/src/workers.rs`, one thread)
- Opening a game's details: when never fetched or older than 6 h (RA: also when not looked up yet). Errors from these show in the section.
- 15 s after a launched game exits (Steam games are watched via the registry's RunningAppID; custom games via their process).
- Background sweep (`achievements_due`), one game at a time, 30 s after start and after each rescan: played Steam games never fetched or played since; custom games with a platform that were never looked up on RA, and matched ones that were played and never fetched or played since. A source without credentials is skipped.
- Refreshes fetch unlocks only; names, icons and rarity are re-fetched weekly or when the cached list doesn't match.
- 8 refusals in a row pause the sweep (private profile) until settings are saved.
- Only `Source::Steam` games get Steam achievements; a custom entry matched to a Steam appid (e.g. a console copy) doesn't.

CLI: `gamut-cli --data <dir> achievements APPID` prints what Steam returns, without saving.

## Achievements tab (`app/src/ach_tab.rs`)
- Switch next to the logo (Library / Achievements). Reads only the cache: `achievement_progress` (per-game totals) and `unlocked_achievements` (recent / rarest).
- Totals (unlocked, perfect games, average completion over started games, RA points), recent and rarest unlocks, "Almost there" (60%+), and every game with a sort. Hidden games are left out; clicking opens the game.
- Worker updates mark it dirty; it rereads at most every 3 s.

## Details page and Settings
- The game page has About / Achievements tabs (the choice sticks while browsing games); the header tile shows the count.
- Settings shows a live Status line per key (`app/src/keycheck.rs` + `core/src/status.rs`): checked on open and 1 s after an edit. Steam: GetPlayerSummaries (persona, profile visibility); SteamGridDB: an autocomplete call; RA: API_GetUserProfile (`TotalSoftcorePoints`; `TotalPoints` is hardcore only). `gamut-cli status` runs the same checks.
- RA systems with no achievement sets yet (empty game list: Wii U, 3DS, PS3) count as unsupported; a "Version" suffix is optional when matching titles.
