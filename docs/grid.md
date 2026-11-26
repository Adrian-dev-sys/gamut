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
- `app/src/gamepad.rs`: a gilrs (Windows.Gaming.Input) thread sends `Pad` actions; the same button twice within
  60 ms counts once (remappers show a pad twice: real + virtual Xbox). HidHide hides a DualSense from Gamut
  unless gamut.exe is on its allow list. D-pad/left
  stick repeat after 380 ms, every 95 ms. `GamutApp::handle_pad` ignores them while Gamut's window isn't
  focused (a game is in front) and while dialogs are open. A = open/Play, B = back/clear, X = favorite,
  LB/RB = filter tabs, LT/RT = page, View/Guide = big screen, Start = like A.
- Big screen (`toggle_big`, F11, top-bar button, View): full screen, zoom ×1.35, filter tabs instead of the
  normal bar, a button-hint footer, cursor always placed. Leaving restores zoom and windowed mode;
  `track_window` pauses meanwhile so the saved placement isn't overwritten.
