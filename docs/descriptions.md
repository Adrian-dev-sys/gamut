# Description media

Descriptions are stored as plain text with light markup from `scrape::strip_html`, rendered by `ui::description`:
- a blank line separates paragraphs; a single newline is a `<br>`. Newlines in the HTML source count as spaces; Playnite writes `<br />` followed by a newline.
- `## ` heading (`<h1>`–`<h6>`), `• ` list item (`<li>`, also `<li><p>…</p></li>`), `**bold**` (`<strong>`/`<b>`). egui has no bold weight, so bold is drawn in the bright text color.
- `[img]URL` for each `<img src>` (`scrape::IMG_MARKER`).

`text_only` ignores this markup, so "Update descriptions only" replaces older stored text that is the same apart from formatting.

- Text stored before this markup has no `## `. Short lines without sentence punctuation (`is_heading`) are treated as headings.
- Images are sized from their real dimensions (shrunk to the panel width). egui would otherwise fit them to the visible space left in the scroll area, which is zero below the fold. Images that fail to load are skipped.
- **AVIF** (Steam's store art format): `app/src/avif.rs` registers an `image` crate hook, decoded by Windows WIC (AV1 Video Extension). WIC ignores the alpha item, so it is pulled out with `avif-parse`, wrapped as its own AVIF with `avif-serialize`, decoded and copied into the alpha channel.
- **Clips**: Steam replaces GIFs with videos. Playnite stores only their `*.poster.avif` still, and the Steam scraper keeps the `poster` of `<video>` tags as an `[img]` line. `app/src/video.rs` maps a poster to `.webm`, then `.mp4` (same `?t=`), streams it into memory (kept only while it plays; nothing on disk), and decodes it with Media Foundation (RGB32 via the source reader's video processing).
  - Playback is looped and paced by frame timestamps on a thread.
  - The page scrolls as a whole (`details`), so the description is not boxed into the space under the banner.
  - Only on-screen clips play, at most 3 at a time; a clip stops 1.5 s after it was last drawn.
  - The poster shows until the first frame arrives, or for good if the clip can't play (logged once).
- **GIFs** use the same player (`video::show_gif`). Frames are decoded lazily on a thread and paced by their delays (under 20 ms counts as 100 ms, like browsers). Only the current frame is kept. The size comes from the GIF header, and single-frame GIFs stop after one frame. egui_extras' GIF loader is not used: it decodes every frame up front on the UI thread and keeps them all, which stalls and uses hundreds of MB, and it didn't animate in the description.
