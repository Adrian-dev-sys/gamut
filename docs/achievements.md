# Achievements

Per-game achievements on the details page, cached in the library DB. Steam only so far (RetroAchievements next).

## Storage (`core/src/db.rs`)
- `achievements(game_id, key, ord, name, description, icon, icon_locked, hidden, unlocked, percent, points)`; `unlocked` is a unix time, 0 = locked.
- `achievement_state(game_id, fetched, schema_at, total, unlocked, error)`: one row per fetched game. `total = 0` means "no achievements"; `error = 'private'` means Steam refused.
- Both are a cache: `delete` clears them with the game (ids get reused), a DB salvage doesn't copy them.

## Steam (`core/src/achievements.rs`)
- `GetPlayerAchievements` (key + SteamID, `l=english`) gives unlocks plus names/descriptions. 400 = the game has no stats → `Fetched::None`.
- 403 "Profile is not public" comes back both for a private profile and for a game the account doesn't own, so one 403 only marks that game.
- `full` fetches add `GetSchemaForGame` (icons, hidden flag, order) and `GetGlobalAchievementPercentagesForApp` (rarity; `percent` is a string).
- Error messages never include the URL (it carries the API key).

## When it fetches (`app/src/workers.rs`, one thread)
- Opening a game's details: when never fetched or older than 6 h.
- 15 s after a launched game exits (Steam games are watched via the registry's RunningAppID).
- Background sweep: Steam games with playtime or a last-played date that were never fetched or were played since (`achievements_due`), one at a time, 30 s after start and after each rescan.
- Refreshes fetch unlocks only; names, icons and rarity are re-fetched weekly or when the cached list doesn't match.
- 8 refusals in a row pause the sweep (private profile) until settings are saved.
- Only `Source::Steam` games get Steam achievements; a custom entry matched to a Steam appid (e.g. a console copy) doesn't.

CLI: `gamut-cli --data <dir> achievements APPID` prints what Steam returns, without saving.
