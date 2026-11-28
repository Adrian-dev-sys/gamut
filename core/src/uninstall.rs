//! Uninstalling a game: Steam's own dialog for Steam games, else the uninstaller Windows lists for its folder.
use crate::{Game, Source};
use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub enum Uninstaller {
    Steam(String),
    /// From Windows' installed apps: display name and the command it runs.
    App { name: String, command: String },
}

/// How this game would be uninstalled, if Gamut knows a way. Reads the registry for non-Steam games.
pub fn find(g: &Game) -> Option<Uninstaller> {
    match g.source {
        Source::Steam => g.installed.then(|| Uninstaller::Steam(g.source_id.clone())),
        // An emulator game's "folder" is its ROM folder, which no uninstaller owns.
        _ if g.rom().is_some() => None,
        _ => find_app(&g.folder()?),
    }
}

/// Starts it. Steam asks for confirmation itself; most uninstallers do too.
pub fn run(u: &Uninstaller) -> Result<()> {
    match u {
        Uninstaller::Steam(appid) => {
            if appid.is_empty() || !appid.bytes().all(|b| b.is_ascii_digit()) {
                bail!("not a Steam app id: {appid}");
            }
            open::that_detached(format!("steam://uninstall/{appid}"))?;
        }
        Uninstaller::App { command, .. } => run_command(command)?,
    }
    Ok(())
}

/// `start` goes through ShellExecute, so uninstallers that need admin rights get their UAC prompt.
#[cfg(windows)]
fn run_command(command: &str) -> Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new("cmd").raw_arg(format!("/C start \"\" {}", quoted(command))).creation_flags(CREATE_NO_WINDOW).spawn()?;
    Ok(())
}

#[cfg(not(windows))]
fn run_command(_: &str) -> Result<()> {
    bail!("uninstalling is Windows-only")
}

/// The command with its exe quoted, so `start` doesn't stop at a space (C:\Program Files\…).
fn quoted(cmd: &str) -> String {
    let cmd = cmd.trim();
    if cmd.starts_with('"') {
        return cmd.to_string();
    }
    match command_exe(cmd) {
        Some(exe) => {
            let n = exe.as_os_str().len();
            format!("\"{}\"{}", &cmd[..n], &cmd[n..])
        }
        None => cmd.to_string(),
    }
}

fn same_dir(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| p.to_string_lossy().trim_end_matches(['\\', '/']).replace('/', "\\").to_lowercase();
    norm(a) == norm(b)
}

/// The exe a command line starts: the quoted part, else everything up to ".exe".
fn command_exe(cmd: &str) -> Option<PathBuf> {
    let cmd = cmd.trim();
    if let Some(rest) = cmd.strip_prefix('"') {
        return rest.split('"').next().map(PathBuf::from);
    }
    let end = cmd.to_lowercase().find(".exe")? + 4;
    Some(PathBuf::from(&cmd[..end]))
}

/// The installed app whose install folder, or whose uninstaller's folder, is exactly `folder`.
#[cfg(windows)]
fn find_app(folder: &Path) -> Option<Uninstaller> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY};
    const KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";
    let roots = [(HKEY_CURRENT_USER, KEY_READ), (HKEY_LOCAL_MACHINE, KEY_READ | KEY_WOW64_64KEY), (HKEY_LOCAL_MACHINE, KEY_READ | KEY_WOW64_32KEY)];
    for (hive, flags) in roots {
        let Ok(list) = winreg::RegKey::predef(hive).open_subkey_with_flags(KEY, flags) else { continue };
        for sub in list.enum_keys().flatten() {
            let Ok(k) = list.open_subkey_with_flags(&sub, flags) else { continue };
            let get = |n: &str| k.get_value::<String, _>(n).ok().filter(|s| !s.trim().is_empty());
            let Some(command) = get("UninstallString") else { continue };
            let location = get("InstallLocation").map(|l| PathBuf::from(l.trim().trim_matches('"')));
            let exe_dir = command_exe(&command).and_then(|e| e.parent().map(Path::to_path_buf));
            let hit = location.is_some_and(|l| same_dir(&l, folder)) || exe_dir.is_some_and(|d| same_dir(&d, folder));
            if hit {
                return Some(Uninstaller::App { name: get("DisplayName").unwrap_or(sub), command });
            }
        }
    }
    None
}

#[cfg(not(windows))]
fn find_app(_: &Path) -> Option<Uninstaller> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_exes() {
        assert_eq!(command_exe(r#""D:\Games\X\unins000.exe" /SILENT"#), Some(PathBuf::from(r"D:\Games\X\unins000.exe")));
        assert_eq!(command_exe(r"D:\Games\X\uninstall.exe"), Some(PathBuf::from(r"D:\Games\X\uninstall.exe")));
        assert_eq!(command_exe(r"MsiExec.exe /X{1234}"), Some(PathBuf::from("MsiExec.exe")));
        assert_eq!(quoted(r"C:\Program Files\X\uninstall.exe /S"), r#""C:\Program Files\X\uninstall.exe" /S"#);
        assert_eq!(quoted(r#""C:\a b\u.exe" /S"#), r#""C:\a b\u.exe" /S"#);
        assert_eq!(quoted("MsiExec.exe /X{12}"), r#""MsiExec.exe" /X{12}"#);
        assert!(same_dir(Path::new(r"D:\Games\X\"), Path::new("d:/games/x")));
        assert!(!same_dir(Path::new(r"D:\Games"), Path::new(r"D:\Games\X")));
    }

    #[test]
    fn steam_needs_install_and_digits() {
        let mut g = Game::new_custom("Portal 2".into(), None);
        (g.source, g.source_id, g.installed) = (Source::Steam, "620".into(), false);
        assert_eq!(find(&g), None);
        g.installed = true;
        assert_eq!(find(&g), Some(Uninstaller::Steam("620".into())));
        assert!(run(&Uninstaller::Steam("620 & calc".into())).is_err());
    }
}
