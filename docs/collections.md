# Collections

Steam-style groups of games, shown as tiles on the **Collections** tab (between Library and Achievements).

## Kinds
- **Hand-filled**: right-click a game (or a multi-selection) → *Add to collection* → tick a collection or *New collection…*.
  Bulk menu also has *Remove from <name>* while that collection is open.
- **Dynamic** (lightning badge): a saved filter set (All/Installed/Favorites/Steam/Custom + platform, tag, status).
  Create from the "+" tile, or *Save as collection* in the library controls while filters are active.
- **Favorites** is built in (id -1, not stored) and can't be renamed or deleted.

## Storage
- `collections (id, name, rule)`: `rule` is JSON (`model::Rule`), NULL for hand-filled.
- `collection_games (collection_id, game_id)`. Removing a game deletes its memberships; a database rebuild (`salvage`) copies both tables.

## UI
- Tiles: 3x3 tilted covers (most recently played first) under a wash tinted by the first cover, name + count. Empty collections are plain tiles.
- Counts and covers are cached in `coll_stats`, dropped on any game reload or collection edit.
- Clicking a tile opens the library narrowed to it (other filters and search reset; you can narrow further). A chip in the controls shows the collection; its X (or B on a controller) goes back.
- Right-click a tile: Rename… / Edit… (dynamic), Delete… (games stay in the library).
- The active collection isn't saved across restarts; big screen clears it.
- Code: `app/src/ui/collections.rs`; shared filter predicate `matches()` in `ui.rs`.
