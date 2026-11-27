//! Per-game PowerShell scripts around a launch.
use crate::model::Game;
use anyhow::{bail, Result};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

fn spawn(script: &str, g: &Game) -> Result<Child> {
    let mut cmd = Command::new("powershell.exe");
    cmd.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script])
        .env("GAMUT_GAME_NAME", &g.name)
        .env("GAMUT_GAME_DIR", g.folder().map(|p| p.display().to_string()).unwrap_or_default())
        .env("GAMUT_GAME_EXE", g.exe.as_deref().unwrap_or(""));
    if let Some(d) = g.folder().filter(|d| d.is_dir()) {
        cmd.current_dir(d);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: no console flashing up.
        cmd.creation_flags(0x0800_0000);
    }
    Ok(cmd.spawn()?)
}

/// Runs the "before" script and waits for it (up to `limit`; then the game starts anyway).
pub fn before(g: &Game, limit: Duration) -> Result<()> {
    let Some(s) = g.script_before.as_deref().filter(|s| !s.trim().is_empty()) else { return Ok(()) };
    let mut child = spawn(s, g)?;
    let t0 = Instant::now();
    loop {
        if let Some(st) = child.try_wait()? {
            if !st.success() {
                bail!("the script before {} exited with {st}", g.name);
            }
            return Ok(());
        }
        if t0.elapsed() > limit {
            log::warn!("script before {} still running after {limit:?}; starting the game", g.name);
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Runs the "after" script when dropped (when the game has exited).
pub struct After(Option<Game>);

impl After {
    pub fn new(g: &Game) -> Self {
        After(g.script_after.as_deref().is_some_and(|s| !s.trim().is_empty()).then(|| g.clone()))
    }
    pub fn is_some(&self) -> bool {
        self.0.is_some()
    }
}

impl Drop for After {
    fn drop(&mut self) {
        if let Some(g) = self.0.take() {
            if let Err(e) = spawn(g.script_after.as_deref().unwrap_or(""), &g) {
                log::warn!("script after {} failed: {e:#}", g.name);
            }
        }
    }
}
