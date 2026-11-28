# Security notes

Review (2026-11): what Gamut trusts and how.

- **API keys** (Steam, SteamGridDB, RetroAchievements, IGDB secret): `config.toml` stores them DPAPI-encrypted for the Windows user (`core/src/secret.rs`, `dpapi:<hex>`). Plain values from older configs load and are encrypted on the next save. Another user/PC (a restored backup) can't decrypt them: they load empty with a log warning. Keys never go into logs; Steam's and RA's APIs need them in the query string, SteamGridDB and IGDB get a Bearer header.
- **Downloads**: all HTTPS. Special K's installer is checked against the release's checksum before running. ReShade's setup comes from reshade.me (version page, then the linked exe) with no signature check; only its DLLs are extracted (`enclosed_name`, so no zip path escapes) and they're only ever loaded by games.
- **Links** on the details page open only when they're `http(s)://` (imported links could be file paths or other protocols).
- **Launching**: games, extra actions and before/after scripts run what the library says; that comes from the user, Steam or their Playnite import. Uninstall runs the registry's `UninstallString` only after showing it. `steam://` ids must be digits.
- **Restores** extract only fixed file names from the zip.
