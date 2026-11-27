# Special K helper

Modeled on Playnite's Special K Helper extension. The code is `core/src/specialk.rs`, and launches go through `Workers::play` in `app/src/workers.rs`.

## Flow
1. **Decide** with `specialk::decide`, checks in order:
   - the mode (`off` / `global` / `selective`);
   - the per-game `games.specialk` override (NULL = global setting, 1 = always, 0 = never);
   - the game is installed;
   - the anti-cheat scan: an `EasyAntiCheat*` / `BattlEye*` file or folder within 4 levels of the install dir or exe folder;
   - the hand-modded check (`own_setup`), skipped when the game is set to "always": the exe folder or install root has a ReShade file (`ReShade.ini`, `reshade-shaders`, …), a ReShade add-on (`*.addon*`, e.g. RenoDX), or a proxy DLL (`dxgi`, `d3d9/11/12`, `opengl32`, `dinput8` for REFramework, `dsound`, `SpecialK64/32`). A folder with Gamut's own `gamut-reshade.txt` doesn't count. Seen: SK on top of REFramework + ReShade + RenoDX crashed MH Wilds at the first frame (DXGI device removed).
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

## Per-launch files (`specialk/profile.rs`)
These run when SK is injected into a game (`specialk::prepare`). Every file is written only when something changed, and the rest of the file is kept as it was.
- **`Global\default_SpecialK.ini`** is the template SK copies into each new profile. Gamut sets these keys from the Settings checkboxes:
  - `[Steam.System] PreLoadSteamOverlay`;
  - for ReShade: `[Import.ReShade64/32]` (Filename is emptied when ReShade is off), `SleeplessRenderThread=false` and `ShowInVideoCapture=false`;
  - `[Render.FrameRate] TargetFPS`;
  - `[Compatibility.General] DisableBloatWare_NVIDIA`;
  - `[Render.DXGI] UseFlipDiscard`.

  It also removes the bogus `AppID=steamId` that Playnite's helper wrote.
- **ReShade**, when `PlugIns\ThirdParty\ReShade` exists:
  - `reshade-presets\<id>.ini` is created once per game. `<id>` is the Playnite GUID, so presets made under Playnite are reused, else `gamut-<id>`.
  - The preset's techniques come from `reshade-shaders\Shaders\**\*.fx`; if there are none, the built-in list is used.
  - `ReShade.ini` gets `[GENERAL] PresetPath` and `[APP] Force*=0`.
- **`steam_appid.txt`** is written in a non-Steam game's install (or exe) folder, only when it's missing and the game has a matched Steam appid (`source_id`). Gamut doesn't search Steam by name.

## Big Picture overlay
When the option is on and Steam's "Steam Big Picture Mode" window exists, non-Steam executables are launched with `SteamTenfoot=1`. Only directly started executables get it, not shortcuts or URLs.

## Updates (`specialk/update.rs`)
- **Check:** at start, every 6 h, and when the channel changes, Gamut reads `https://sk-data.special-k.info/repository.json`. It takes the first version whose `Branches` contains the channel (Website / Discord / Ancient) and compares it with `SpecialK64.dll`'s file version.
- **Notify:** a toast announces a newer release, and Settings shows an "Install update" button.
- **Install:**
  1. download the installer;
  2. check its SHA-256 against the feed;
  3. stop all SK services;
  4. run the installer interactively and wait for it to exit;
  5. delete the downloaded installer.
