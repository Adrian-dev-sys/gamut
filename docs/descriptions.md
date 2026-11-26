# Description media

Descriptions are plain text; `scrape::strip_html` keeps each `<img src>` as an `[img]URL` line (`scrape::IMG_MARKER`), rendered by `ui::description`.

- Images are sized from their real dimensions (shrunk to the panel width). egui would otherwise fit them to the visible space left in the scroll area, which is zero below the fold. Images that fail to load are skipped.
- **AVIF** (Steam's store art format): `app/src/avif.rs` registers an `image` crate hook, decoded by Windows WIC (AV1 Video Extension). WIC ignores the alpha item, so it is pulled out with `avif-parse`, wrapped as its own AVIF with `avif-serialize`, decoded and copied into the alpha channel.
- **Clips**: Steam replaces GIFs with videos. Playnite stores only their `*.poster.avif` still. `app/src/video.rs` maps a poster to `.webm`, then `.mp4` (same `?t=`), downloads it to `%TEMP%\gamut-clips`, and decodes it with Media Foundation (RGB32 via the source reader's video processing).
  - Playback is looped and paced by frame timestamps on a thread.
  - Only on-screen clips play, at most 3 at a time; a clip stops 1.5 s after it was last drawn.
  - The poster shows until the first frame arrives, or for good if the clip can't play (logged once).
