# ReShade

The code is `core/src/reshade.rs`. It runs from `with_reshade` in `app/src/workers.rs` before each launch.

- **Decide.** The global `[reshade] enabled` setting applies, unless the per-game `games.reshade` override says otherwise (NULL = global, 1 = install, 0 = never). The game must be installed. Games are skipped when an EasyAntiCheat or BattlEye file or folder is found within 4 levels of the game folder.
- **Download, once.** Gamut downloads into `<data>\reshade\`:
  - `ReShade64.dll` and `ReShade32.dll`, unpacked from the zip appended to `ReShade_Setup_<ver>.exe` (the newest non-Addon setup linked on reshade.me);
  - the `Shaders/` and `Textures/` folders of the Standard effects package (crosire/reshade-shaders, slim branch) and SweetFX (CeeJayDK/SweetFX), placed under `reshade-shaders\`.

  Settings → "Download latest" downloads them again.
- **Find the folder.** ReShade goes next to the game's exe: `games.exe`, or for Steam games `procs::main_exe`, the largest non-helper exe within 3 levels of the install dir.
- **Skip existing setups.** Gamut skips the game if the exe folder or the install root already has any of these: `ReShade.ini`, `ReShadePreset.ini`, `reshade-shaders`, `ReShade.log`, `dxgi.dll`, `d3d9.dll`, `d3d11.dll`, `opengl32.dll`, `reshade.asi`.
- **Pick the DLL name.** It comes from the PE imports of the exe, and if those show no graphics API, of the DLLs next to it:
  - d3d10, d3d11, d3d12 or dxgi → `dxgi.dll`;
  - d3d9 → `d3d9.dll`;
  - opengl32 → `opengl32.dll`;
  - Vulkan only → skipped;
  - unknown → `dxgi.dll`.

  The 64- or 32-bit build is chosen from the exe's PE header.
- **Write the files.**
  - `ReShade.ini`: absolute `EffectSearchPaths` and `TextureSearchPaths` into the shared shader folder, plus `PresetPath=.\ReShadePreset.ini`.
  - `ReShadePreset.ini`: no techniques enabled, like a fresh install.
  - `gamut-reshade.txt`: the list of files Gamut added. Edit → Launch → "Remove ReShade" deletes only those files.
