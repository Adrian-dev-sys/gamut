# Library health

**Health** in the top bar opens a dialog listing games that can't start and likely duplicates (`core/src/health.rs`, UI in `app/src/health_ui.rs`). The check runs on a thread (a sleeping or network drive can stall file checks) and re-runs after an edit or removal.

## Checks
- Only installed custom games are checked for files; Steam handles its own, and uninstalled entries (owned console games) are expected to have no exe.
- **No launcher**: no exe/link set.
- **Exe / file / folder missing**: the exe, and every double-quoted absolute path in the arguments (emulator ROMs). If the parent folder is gone too, the folder is reported instead.
- **Drive missing**: the drive root doesn't exist (unplugged disk). Not ignorable, since it fixes itself.
- URLs (`goggalaxy://`), store names and Playnite `{placeholders}` aren't checked.
- **Duplicates**: same normalized name on the same platform (Steam counts as PC). The same game on different platforms is never a duplicate. A group where every copy is from a different store (Steam + GOG) is skipped as bought twice.

## Dialog
- Two sections: **Can't start** (Edit / Remove / Ignore) and **Possible duplicates**.
- Each duplicate copy shows how it starts (emulator + ROM file, since copies share the emulator exe), where it came from (Playnite import, store, added in Gamut), added date, playtime and cover.
- ★ marks the suggested copy (`keep_score`: can start, playtime, cover, description, older). **Keep only this** removes the other copies after an inline confirm, optionally adding their playtime to the kept one. **Not a duplicate** ignores the group.

## Settings (`[health]` in config.toml)
- `skip_platforms`: platforms not checked at all (games played on the console).
- `ignored`: dismissed issue keys, `file:<game id>` or `dup:<platform>|<name>`. "Show N ignored again" clears them.

`gamut-cli health` prints the same report.
