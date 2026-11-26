# Statistics

Statistics tab (`app/src/stats_tab.rs`), next to Library and Achievements. Everything is computed on open and
reread (at most every 5 s) when the library, a game or achievements change.

## Data
- `sessions(game_id, start, minutes, kind)` in the library DB, deleted with the game.
  - kind 0: games Gamut starts and times (`add_playtime`), exact start and length.
  - kind 1: Steam playtime growth between two Web API scans (`sync_steam`), dated at the game's `last_played`
    (clamped to the scan window). Growth bigger than the time between scans (+10 min) is ignored, so imports and
    first scans never count as play. `meta.steam_scan` is the last API scan, `meta.steam_scan_first` the first.
- Steam is the source of truth for Steam playtime: every scan overwrites the total. The Steam watcher thread
  (`workers.rs`) rescans 30 s and 3 min after any Steam game stops running (started from Gamut or Steam), and hourly.
- Achievement unlock dates (`unlock_times`) go back as far as Steam/RA have them; playtime per day only starts
  when session logging did.
- Days and months are local time (`gamut_core::utc_offset`, today's offset for all dates).

## Charts
- Single-hue (accent) columns and bars, quiet grid, a tooltip on every bar (value + top 3 games).
- Playtime and achievements: 30 days / 12 months / years. Most played: all time or last 30 days (rows open the game).
- Playtime by platform (Steam games as "Steam") and genre, completion status, weekday, games added per month.
- Tiles: total playtime, games played, never played, this month, play streak (days in a row with a session).
