use crate::model::{Game, Source};
use anyhow::{bail, Result};
use std::path::Path;
use std::process::Command;
use std::time::Instant;

/// Result of a launch: custom games report the session length when the process exits.
pub enum Launched {
    External,
    Tracked(std::thread::JoinHandle<Option<(i64, i64)>>),
}

/// Starts a game. For tracked processes the handle yields (minutes, start_unix) on exit.
pub fn launch(g: &Game) -> Result<Launched> {
    match g.source {
        Source::Steam => {
            let verb = if g.installed { "rungameid" } else { "install" };
            open::that_detached(format!("steam://{verb}/{}", g.source_id))?;
            Ok(Launched::External)
        }
        Source::Custom => {
            let Some(exe) = g.exe.as_deref().filter(|s| !s.trim().is_empty()) else {
                bail!("no executable set for {}", g.name)
            };
            let path = Path::new(exe);
            let is_exe = path
                .extension()
                .map(|e| ["exe", "bat", "cmd"].iter().any(|x| e.eq_ignore_ascii_case(x)))
                .unwrap_or(false);
            if !is_exe {
                // URLs, shortcuts, ROMs with file associations, ...
                open::that_detached(exe)?;
                return Ok(Launched::External);
            }
            let mut cmd = Command::new(path);
            if let Some(a) = g.args.as_deref().filter(|a| !a.trim().is_empty()) {
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    cmd.raw_arg(a);
                }
                #[cfg(not(windows))]
                cmd.args(a.split_whitespace());
            }
            let wd = g
                .work_dir
                .as_deref()
                .filter(|w| !w.trim().is_empty())
                .map(Path::new)
                .or_else(|| path.parent());
            if let Some(wd) = wd {
                cmd.current_dir(wd);
            }
            let mut child = cmd.spawn()?;
            let start_unix = crate::now();
            let t0 = Instant::now();
            Ok(Launched::Tracked(std::thread::spawn(move || {
                child.wait().ok()?;
                let mins = (t0.elapsed().as_secs() + 30) / 60;
                Some((mins as i64, start_unix))
            })))
        }
    }
}
