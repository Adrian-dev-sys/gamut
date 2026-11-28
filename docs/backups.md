# Backups

Code: `core/src/backup.rs` (create, list, rotate, stage/apply restore), `app/src/ui/backups.rs` (Settings → Backups, restore dialog), `workers.rs` (`backup`, `spawn_backup_schedule`), `main.rs` (applies a staged restore).

- **Zip** `gamut-backup-YYYY-MM-DD_HHMMSS.zip` (local time): `library.db`, `config.toml` (holds the API keys), `local-ach.json`. Thumbs, logs, `ra`, `reshade`, `art-quarantine` and old `library.db.*` copies are left out. Screenshots live in `Pictures\Gamut` and aren't backed up.
- **Database snapshot**: `VACUUM INTO` through a read-only handle of its own (never a file copy of the live DB), then `PRAGMA integrity_check` + a `games` query on the snapshot. A failing check aborts before rotation, so good older backups stay.
- **Art**: `<backup dir>\art\{covers,heroes,icons}` is an incremental mirror (copied when missing or size/mtime differ; nothing deleted), shared by all zips.
- **Rotation**: newest `keep` zips stay (default 10). Names sort by time.
- **Schedule**: `backup.auto` (default on), every `days` (default 7): checked a minute after start, then hourly; due when the newest zip is older than that. Manual "Back up now" uses the settings as edited.
- **Folder**: `backup.dir`, default `<data>\backups`. Another drive also survives a dead disk.
- **Restore**: the dialog checks the zip and writes `<data>\restore.pending` (its path), then starts `gamut.exe --restarting` (waits up to 30 s for the lock) and quits. At start, after the lock and before anything opens the DB, `apply_pending` (marker removed first, never retried):
  - extracts and checks the DB; a damaged one changes nothing;
  - moves `library.db` (+ `-journal`/`-wal`/`-shm`, so a stale journal is never replayed into the restored DB) and each restored file to `*.pre-restore-<time>`;
  - puts back art missing from the data folder, only for game ids in the restored DB, and drops those covers' thumbs.
  - The result shows as a toast (`main::RESTORED`).
