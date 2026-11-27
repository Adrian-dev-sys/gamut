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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_before_and_after_scripts() {
        let dir = std::env::temp_dir().join(format!("gamut script test {}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (b, a) = (dir.join("before.txt"), dir.join("after.txt"));
        let mut g = Game::new_custom("It's \"quoted\"".into(), None);
        g.script_before = Some(format!("$n = $env:GAMUT_GAME_NAME\nSet-Content -LiteralPath '{}' -Value $n", b.display()));
        g.script_after = Some(format!("Set-Content -LiteralPath '{}' -Value 'done'", a.display()));
        before(&g, Duration::from_secs(60)).unwrap();
        assert_eq!(std::fs::read_to_string(&b).unwrap().trim(), "It's \"quoted\"");
        drop(After::new(&g));
        let t0 = Instant::now();
        while !a.exists() && t0.elapsed() < Duration::from_secs(30) {
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(a.exists());
        g.script_before = Some("exit 3".into());
        assert!(before(&g, Duration::from_secs(60)).is_err());
        std::fs::remove_dir_all(&dir).ok();
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
