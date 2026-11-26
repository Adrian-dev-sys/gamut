# Statistics

Statistics tab (`app/src/stats_tab.rs`), next to Library and Achievements. Everything is computed on open and
reread (at most every 5 s) when the library, a game or achievements change.

## Data
- `sessions(game_id, start, minutes, kind)` in the library DB, deleted with the game.
  - kind 0: games Gamut starts and times (`add_playtime`), exact start and length.
  - kind 1: Steam playtime growth seen by a scan (`sync_steam`). Baseline per game in `steam_seen(game_id, minutes,
    since)`: the last API total and when it was first seen. Growth is a session only if it fits in the time since
    then (+10 min), so first sightings and imports never count, while a session still running during one scan is
    picked up whole at the next. Dated at Steam's `last_played` when that falls in the window, else back from now.
    With Gamut closed for days, the growth lands as one session on the last-played day.
- Steam is the source of truth for Steam playtime: every scan overwrites the total. The Steam watcher thread
  (`workers.rs`) rescans 30 s and 3 min after any Steam game stops running (started from Gamut or Steam), and hourly;
  these automatic scans log failures instead of toasting.
- Achievement unlock dates (`unlock_times`) go back as far as Steam/RA have them; playtime per day only starts
  when session logging did.
- Days and months are local time (`gamut_core::utc_offset`, today's offset for all dates).

## Charts
- Single-hue (accent) columns and bars, quiet grid, a tooltip on every bar (value + top 3 games).
- Playtime and achievements: 30 days / 12 months / years. Most played: all time or last 30 days (rows open the game).
- Playtime by platform (Steam games as "Steam") and genre, completion status, weekday, games added per month.
- Tiles: total playtime, games played, never played, this month, play streak (days in a row with a session).
