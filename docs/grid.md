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
  controller with DSX/HidHide, while HID sees DSX's virtual DualSense, like games do. The same action twice
  within 60 ms counts once (remappers show a pad twice). D-pad/left stick repeat after 380 ms, every 95 ms.
  `GamutApp::handle_pad` ignores input while Gamut's window isn't focused (a game is in front) and while
  dialogs are open. A = open/Play, B = back/clear, X = favorite, LB/RB = filter tabs, LT/RT = page,
  View/Guide = big screen, Start = like A.
- Big screen (`toggle_big`, F11, top-bar button, View): full screen, zoom ×1.35, filter tabs instead of the
  normal bar, a button-hint footer, cursor always placed. Leaving restores zoom and windowed mode;
  `track_window` pauses meanwhile so the saved placement isn't overwritten.
- Details popup with controller: `raw_input_hook` turns D-pad/A into arrow/Enter key events while it's open,
  so egui's own focus moves between its buttons/tabs and Enter clicks the focused one. Opening a game from the
  controller or keyboard focuses Play (`focus_play`); the focused widget gets a white border (`kb_focus`).
  L2/R2 scroll the page (`detail_scroll`).
- Search: Y/Triangle focuses the search box; in big screen it opens `search_pop.rs`, a centered on-screen
  keyboard (A types, X deletes, B/Y close) that filters the grid live.
- The grid keeps `GRID_MARGIN` above the first row and below the last so the cursor outline isn't clipped.
