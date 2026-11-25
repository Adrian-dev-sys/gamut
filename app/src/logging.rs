//! Log file at `<data>\gamut.log` (previous run kept as `gamut.old.log`), plus panics.
use log::{Level, LevelFilter, Log, Metadata, Record};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

struct FileLog {
    file: Mutex<Option<std::fs::File>>,
    level: LevelFilter,
}

impl Log for FileLog {
    fn enabled(&self, m: &Metadata) -> bool {
        // Our crates at the chosen level; dependencies (egui, ureq, …) only when they warn.
        let ours = m.target().starts_with("gamut");
        m.level() <= if ours { self.level } else { LevelFilter::Warn }
    }

    fn log(&self, r: &Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        let line = format!("{} {:<5} {}: {}\n", now(), r.level(), r.target(), r.args());
        if cfg!(debug_assertions) || r.level() <= Level::Warn {
            eprint!("{line}");
        }
        if let Some(f) = self.file.lock().unwrap().as_mut() {
            let _ = f.write_all(line.as_bytes());
        }
    }

    fn flush(&self) {
        if let Some(f) = self.file.lock().unwrap().as_mut() {
            let _ = f.flush();
        }
    }
}

pub fn path(root: &Path) -> PathBuf {
    root.join("gamut.log")
}

/// Starts logging; `RUST_LOG=debug` raises the level (default info).
pub fn init(root: &Path) {
    let p = path(root);
    let _ = std::fs::rename(&p, root.join("gamut.old.log"));
    let file = std::fs::File::create(&p).ok();
    let level = std::env::var("RUST_LOG").ok().and_then(|v| v.parse().ok()).unwrap_or(LevelFilter::Info);
    if log::set_boxed_logger(Box::new(FileLog { file: Mutex::new(file), level })).is_ok() {
        log::set_max_level(level.max(LevelFilter::Warn));
    }
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("panic: {info}\n{}", std::backtrace::Backtrace::force_capture());
        log::logger().flush();
        default(info);
    }));
    log::info!("Gamut {} starting, data in {}", env!("CARGO_PKG_VERSION"), root.display());
}

/// Local time as "DD/MM/YYYY HH:MM:SS.mmm".
fn now() -> String {
    let mut t: windows_sys::Win32::Foundation::SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut t) };
    format!(
        "{:02}/{:02}/{:04} {:02}:{:02}:{:02}.{:03}",
        t.wDay, t.wMonth, t.wYear, t.wHour, t.wMinute, t.wSecond, t.wMilliseconds
    )
}
