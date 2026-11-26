# IGDB

Metadata and art for games not on Steam (`core/src/igdb.rs`).

## Access
- IGDB only accepts requests signed by a Twitch developer app (Playnite hides this behind its own server).
  Settings → IGDB takes the app's Client ID + secret (`igdb_client_id`, `igdb_client_secret`).
- Token: `POST https://id.twitch.tv/oauth2/token` with client credentials in the form body (never the URL),
  kept in memory until shortly before `expires_in`. A 401 from the API refreshes it once.
- Bad credentials (`AUTH_ERR`) are remembered for 5 min so a library sweep doesn't hit Twitch per game; the
  pipeline then falls back to Steam-only behavior instead of marking games not found.
- Queries: `POST https://api.igdb.com/v4/games`, Apicalypse text (`FIELDS` + `search "…"; where version_parent = null;`).
  4 req/s limit: `http::min_gap` spaces api.igdb.com 300 ms.
- Images: `images.igdb.com/igdb/image/upload/t_<size>/<image_id>.jpg`; covers `t_cover_big_2x`, banners `t_1080p`.

## Matching (automatic)
- Strict, since fills are hard to undo: a main game (not DLC/bundle/mod/…) whose title or an alternative name
  equals the game's after `retro::title_key`, on the game's platform when it has one (`platform_variants`:
  maker prefixes and aliases like "Sony PSP", "PC (Windows)"). Several candidates (remakes share titles) need
  the game's release year to pick exactly one. Searches drop region/dump tags ("(USA) [!]").
- Result stored in `igdb_match(game_id, igdb_id)`: >0 id, 0 = no confident match. Dropped on rename/platform
  change and delete.
- Anything else is left to the manual pick.

## Pipeline (`sync.rs`)
- `fetch_meta`: custom games on consoles use IGDB first; PC custom games try the exact Steam match, then IGDB.
  `save_metadata` only fills empty fields (now also series, critic/community score), so user edits win.
- `fetch_cover`: Steam CDN, then SteamGridDB, then IGDB (cover; artwork or screenshot as banner).
- `Library::igdb_requeue`: once per new Client ID + secret (hash in `meta.igdb_client`) and after a passing check, custom
  games with gaps (not found, missing description/developer/genres/date, or no cover) are queued again and old
  "no match" results forgotten. Runs at startup and when the credentials are saved.

## Manual
- Cover/banner picker lists IGDB matches next to SteamGridDB and VNDB.
- Editor → "Fill from IGDB…": search, pick, and the entry's details replace the form's fields (unsaved until
  Save). Save stores the pick as the match and queues its cover if the game has none.
- Editor → "Fill from VNDB…" (no key needed, `vndb::details`): description (BBCode stripped, spoilers
  dropped), developers, most frequent official publisher, full release date, genre "Visual Novel", VNDB
  rating as community score, and up to 10 non-spoiler content tags added to the game's tags. Not stored as a match.
- `gamut-cli vndb NAME` prints VNDB results and the first one's details.
- `gamut-cli igdb NAME [PLATFORM]` prints search results and marks the automatic pick; `gamut-cli status` checks keys.
