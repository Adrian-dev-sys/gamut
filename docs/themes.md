# Themes

`app/src/theme.rs`. `<data>\theme.toml` overrides the built-in look; Settings → General → Appearance writes a template with the built-in values ("Customize theme") and opens it, or renames it to `theme.toml.bak` ("Built-in theme").

- `[colors]`: `accent, accent_2, background, panel, card, card_hover, border, text, text_dim, good, danger` as `#rrggbb[aa]`. Code reads them through `ACCENT()`, `CARD()`, … (functions over an atomic palette; built-in values until a file loads).
- `[style]`: `corner_radius` (widgets; windows ×2, menus ×4/3), `spacing` (multiplier for item spacing, button padding, window margin), `text_size` (body; small −2, heading +4), `font` / `bold_font` (file paths; bold defaults to Segoe UI Bold). Fonts are only rebuilt when they change.
- Hot reload: a `Watcher` thread checks the file's mtime once a second and wakes the UI (works while idle); a bad file (unknown key, bad color, missing font) shows an error toast and keeps the previous theme. Deleting the file returns to the built-in theme.
- Not themed: shadows, backdrops, a few one-off paint colors.
