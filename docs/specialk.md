# Special K helper

Modeled on Playnite's Special K Helper extension. The code is `core/src/specialk.rs`, and launches go through `Workers::play` in `app/src/workers.rs`.

## Flow
1. **Decide** with `specialk::decide`, checks in order:
   - the mode (`off` / `global` / `selective`);
   - the per-game `games.specialk` override (NULL = global setting, 1 = always, 0 = never);
   - the game is installed;
   - the anti-cheat scan: an `EasyAntiCheat*` / `BattlEye*` file or folder within 4 levels of the install dir or exe folder.
2. **Find SK.** Use `Settings.path` if set (the folder or SKIF.exe). Otherwise read the registry value `HKCU\Software\Kaldaien\Special K` → `Path`. The folder must contain `Servlet\`.
3. **Create the named event** `Local\SKIF_InjectAckEx` (auto-reset), before the game starts. `SpecialK64.dll` signals it once injected.
4. **Start the services.** For each of `SKIFsvc32` and `SKIFsvc64` that isn't running, run `Servlet\SKIFsvcNN.exe Start` and poll up to 2 s for the process. Gamut stops only the services it started, and only once no other launch still holds a `Guard`.
5. **Launch the game**, then stop the services according to `stop`:
   - `injection`: once the event fires, or after 5 minutes;
   - `exit`: when the tracked process exits. Steam and URL launches can't be tracked, so they fall back to `injection`;
   - `never`: the services are left running.

## Playnite import
Playnite feature names map to the per-game override:
- `[SK] Selective Mode Enable` → always;
- `[SK] Global Mode Disable` → never;
- `Valve Anti-Cheat Enabled` → never.

## Not ported
These Playnite helper features were not ported:
- writing `Global\default_SpecialK.ini`;
- ReShade presets;
- `steam_appid.txt` creation;
- Steam Big Picture overlay env var;
- the SK updater.

Gamut leaves the user's SK config alone.
