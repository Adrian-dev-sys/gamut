# Library grid

- Sort: names use `util::nat_key` (digit runs compare as numbers, case-insensitive) for the Name sort and
  every name tie-break.
- Cursor (`GamutApp::cursor`, drawn white only while `kb_focus`): arrows, Home/End, PageUp/PageDown move it
  (`nav`, also the entry point for controller input), Enter opens the details popup, Space toggles it in the
  selection. Keys are ignored while typing, in another tab, or with any popup open. `nav` scrolls the cursor's
  row into view using last frame's layout (`grid_cols`, `grid_page`, `grid_pitch`, `grid_scroll`).
- Multi-select (`multi`): Ctrl+click toggles, Shift+click / Shift+arrows add a range from `anchor`, Ctrl+A
  selects the view, Esc clears. A plain click clears it and opens the game. Games leaving the view
  (filter, hide) drop out of it.
- Bulk actions (bar above the grid, or right-click a selected card): favorite, hide (and undo), refresh
  metadata. Refresh asks first since `requeue(force)` replaces description/developer/publisher/date/genres.

## Controller and big screen
- `app/src/gamepad.rs`: one thread polls PlayStation pads over HID (hidapi; DualSense/Edge/DS4, USB and BT
  report layouts in `parse`) and everything else through gilrs. Windows.Gaming.Input (gilrs' backend) sees no
  controller with DSX/HidHide, while HID sees DSX's virtual DualSense, like games do. Every HID report is
  edge-checked (`Src::update`), so quick taps between polls still count. Remappers show a pad twice (real +
  virtual): `Lead` follows the device that acted first and ignores others until it's quiet for 400 ms.
  When a Bluetooth pad and a non-Bluetooth one are both present (DSX: real BT pad + virtual USB copy), the
  Bluetooth one is muted so only the remapped copy counts (`mirrored`; bus types are logged on connect).
  D-pad/left stick repeat after 380 ms, every 95 ms. `GamutApp::handle_pad` ignores input while Gamut's
  window isn't focused (a game is in front). A = open/Play, B = back/clear, X = favorite, Y = search,
  Start/Options/Create = game menu (`game_menu_window`, the right-click menu as a dialog), LB/RB = top tabs
  (filter tabs in big screen), LT/RT = page, View/PS = big screen.
- Dialogs with controller: `raw_input_hook` sends Tab/Shift+Tab for the D-pad (text fields keep arrows),
  Enter for A, Escape for B (B also closes the simple confirm dialogs). A dialog opened from the controller
  gets a Tab so its first widget is focused; ✕ buttons aren't focusable. One focus move per frame; extra
  input waits in `pad_later`.
- Big screen (`toggle_big`, F11, top-bar button, View): full screen, zoom ×1.35, filter tabs instead of the
  normal bar, a button-hint footer, cursor always placed. Leaving restores zoom and windowed mode;
  `track_window` pauses meanwhile so the saved placement isn't overwritten.
- Details popup with controller: `raw_input_hook` turns D-pad/A into arrow/Enter key events while it's open,
  so egui's own focus moves between its buttons/tabs and Enter clicks the focused one. Opening a game from the
  controller or keyboard focuses Play (`focus_play`); the focused widget gets a white border (`kb_focus`).
  L2/R2 page and the right stick scrolls (`scroll_details`); a D-pad up/down that finds nothing to focus
  that way scrolls instead (`nav_probe`: focus unchanged next frame).
- On-screen keyboard (`osk.rs`): A on any focused text field (and Y outside big screen, which focuses the
  search box) opens it at the bottom. It's painted, not focusable, and types by injecting `Event::Text` /
  Backspace / Ctrl+A into the focused field. A types, X deletes, Y space, LB/RB shift, B closes.
- Search: Y/Triangle focuses the search box; in big screen it opens `search_pop.rs`, a centered on-screen
  keyboard (A types, X deletes, B/Y close) that filters the grid live.
- The grid keeps `GRID_MARGIN` above the first row and below the last so the cursor outline/glow isn't clipped.

## Animations
Kept cheap: they only repaint while running.
- `theme::popup` fades and slides every dialog in (frame drawn inside so it fades too); `theme::appear` is the
  shared 0→1 timer (restarts when an id wasn't drawn for 2 passes).
- Cards ease their lift/glow on hover and cursor (`animate_bool`); the glow uses the cover's main color
  (`ThumbCache::color`, computed off-thread from the thumbnail). Covers fade in as they load; tab switches
  cross-fade.

## Random game
- "Random game" (top bar and big screen bar, Ctrl+R) opens a random game from the current view, so filters, search and tags narrow it; never the game already open. The grid cursor moves to it.

## List mode
- Grid / List buttons next to the cover-size slider (`Config::list_view`, saved with the other UI prefs). Big screen always uses the grid.
- `app/src/ui/list.rs`: one 44 px row per game (small cover crop, favorite star, RUNNING badge) with columns Name, Platform, Status, Time played, Last played, Added, Score. Clicking a header picks that sort (same `Sort` as the dropdown); Platform isn't sortable. Narrow windows drop Added, Score, Status, Last played, Platform in that order.
- With a game open, the list moves to a resizable left panel (`list_side`, 240–900 px, like Playnite) and the details fill the rest instead of the popup (`side()`; `popup()` is "the popup is up" for keys and controller). When only Name fits, rows get a second line (last played · time). Details wider than 1400 px grow the banner (28% of the width, up to half the height) and cover (up to 320 px). The open game follows the cursor (`nav`) and is scrolled into view when the list moves; Enter / A play; Esc / B close it.
- It's a one-column grid to `nav`: cursor, keys, controller, multi-select, context/bulk menus and clicks (`apply_clicks`) are shared with the grid.
