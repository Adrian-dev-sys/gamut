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
