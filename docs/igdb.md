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
- Fan ports: Steam, SteamGridDB and IGDB lookups try `scrape::name_variants` in turn (the name, without
  recomp/decomp/port words, each part around " - ", a trailing " 64" dropped) and accept a title without its
  series prefix (`same_game`: "Tears of the Kingdom" = "The Legend of Zelda: Tears of the Kingdom"). Shortened
  names skip the platform check on IGDB. A forced Refresh clears a cached IGDB "not found".
- Cover/banner picker lists IGDB matches next to SteamGridDB and VNDB.
  Each source keeps its best 3 by `scrape::best_matches` (same name > prefix > shared words; under half the best
  dropped). VNDB matches on every title (English, romaji, Japanese, aliases), so an English name finds the
  Japanese entry; a game tagged or with genre "Visual Novel" gets VNDB first.
- Editor → "Fill from IGDB…": search, pick, and the entry's details replace the form's fields (unsaved until
  Save). Save stores the pick as the match and queues its cover if the game has none.
- Editor → "Fill from VNDB…" (no key needed, `vndb::details`): description (BBCode stripped, spoilers
  dropped), developers, most frequent official publisher, full release date, genre "Visual Novel", VNDB
  rating as community score, and up to 10 non-spoiler content tags added to the game's tags. Not stored as a match.
- Editor → "Fill from Steam…" (any game, owned or not): store search (`scrape::steam_find`), then the store
  page's description, developer, publisher, date, genres and Metacritic score (also used by the Steam fetch now).
- Editor → "Fill from GOG…" (`gog.rs`, public catalog, no account): developer, publisher, date, genres,
  rating (stars ×20) and up to 10 store tags from the catalog, description from `api.gog.com/products/{id}`.
  "Edition/bundle" marks packs. Steam/GOG/VNDB picks fetch details after the click; none is stored as a match.
- Picking any result opens a preview first (`editor::preview`: every field, "N of 9 fields", description
  word/image count); "Fill form" applies it, "Back to results" returns. Source tabs in the panel re-run the
  same query, so one game can be compared across sources.
- "Compare all…" (`app/src/lookup.rs`): searches all four sources at once, auto-picks each one's likeliest
  entry (IGDB's strict match, else exact title preferring games over bundles, else first; a dropdown switches
  it), and shows a field × source grid. ★ = most fields (longer description breaks ties); cells start on the
  ★ source where it has the field, else the first source that does. Clicking cells mixes sources; "Fill N
  fields" applies only selected cells. Taking any field from IGDB stores that entry as the match.
- `gamut-cli vndb|gog|steam NAME` prints results and the first one's details.
- `gamut-cli igdb NAME [PLATFORM]` prints search results and marks the automatic pick; `gamut-cli status` checks keys.
