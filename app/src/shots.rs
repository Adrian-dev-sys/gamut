//! F12 screenshots for non-Steam games: a low-level keyboard hook that only watches F12 (never swallows it),
//! the game in front, capture, save, and a popup. Steam games keep Steam's own F12.
use gamut_core::{screenshots, Game, Library, Paths, Source};
use std::collections::HashSet;
use std::path::Path;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex, OnceLock};

static KEY: OnceLock<Sender<()>> = OnceLock::new();

/// Takes a screenshot as if F12 was pressed (the controller chord).
pub fn trigger() {
    if let Some(tx) = KEY.get() {
        let _ = tx.send(());
    }
}

/// Starts the hook; `on_saved` runs after each saved shot. `settings` (enabled, sound) is read at each press.
pub fn start(paths: Paths, running: Arc<Mutex<HashSet<i64>>>, settings: impl Fn() -> (bool, bool) + Send + 'static, on_saved: impl Fn(i64) + Send + 'static) {
    let (tx, rx) = channel::<()>();
    if KEY.set(tx).is_err() {
        return;
    }
    std::thread::spawn(move || {
        for () in rx {
            let (on, sound) = settings();
            if on {
                if let Some(id) = take(&paths, &running, sound) {
                    on_saved(id);
                }
            }
        }
    });
    std::thread::spawn(sys::hook);
}

/// The library game behind the foreground process: a game Gamut started (its exe, e.g. an emulator, or its
/// folder), else the game whose folder holds the exe (the deepest one).
fn game_for(exe: &Path, games: &[Game], running: &HashSet<i64>) -> Option<Game> {
    let lower = |p: &Path| p.to_string_lossy().to_lowercase();
    let e = lower(exe);
    let in_dirs = |g: &Game| gamut_core::procs::game_dirs(g).iter().map(|d| lower(d)).filter(|d| e.starts_with(&format!("{}\\", d.trim_end_matches('\\')))).map(|d| d.len()).max();
    let same_exe = |g: &Game| g.exe.as_deref().is_some_and(|x| x.to_lowercase() == e);
    if let Some(g) = games.iter().filter(|g| running.contains(&g.id)).find(|g| same_exe(g) || in_dirs(g).is_some()) {
        return Some(g.clone());
    }
    // An exe several games share is an emulator: which game it runs is only known for ones Gamut started.
    let shared = |g: &Game| games.iter().filter(|o| o.exe.is_some() && o.exe == g.exe).count() > 1;
    games.iter().filter(|g| !shared(g)).filter_map(|g| Some((in_dirs(g)?, g))).max_by_key(|(n, _)| *n).map(|(_, g)| g.clone())
}

/// Takes and saves one screenshot; Some(game id) when saved.
fn take(paths: &Paths, running: &Arc<Mutex<HashSet<i64>>>, sound: bool) -> Option<i64> {
    let (hwnd, exe) = sys::foreground()?;
    let name = exe.file_name()?.to_string_lossy().to_lowercase();
    if name == "gamut.exe" || exe.to_string_lossy().to_lowercase().contains(r"\steamapps\common\") {
        return None;
    }
    let games = Library::open(&paths.db).and_then(|l| l.all()).ok()?;
    let ids = running.lock().unwrap().clone();
    let Some(g) = game_for(&exe, &games, &ids) else {
        log::info!("screenshot: {name} isn't a library game; F12 left alone");
        return None;
    };
    if g.source == Source::Steam {
        return None;
    }
    let shot = crate::capture::grab(hwnd).and_then(|img| {
        if sound {
            sys::shutter();
        }
        let path = screenshots::save(&img, &screenshots::root().join(screenshots::folder_name(&g, &games)))?;
        Ok((img, path))
    });
    match shot {
        Ok((img, path)) => {
            log::info!("screenshot: {} saved to {}", g.name, path.display());
            // Square preview for the popup.
            let s = img.width().min(img.height());
            let crop = image::imageops::crop_imm(&img, (img.width() - s) / 2, (img.height() - s) / 2, s, s).to_image();
            let icon = image::imageops::thumbnail(&crop, 64, 64);
            crate::popup::show(crate::popup::Popup {
                caption: "Screenshot saved".into(),
                title: g.name.clone(),
                text: path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default(),
                icon: Some((icon.into_raw(), 64, 64)),
            });
            Some(g.id)
        }
        Err(e) => {
            log::warn!("screenshot of {} failed: {e:#}", g.name);
            crate::popup::show(crate::popup::Popup { caption: "Screenshot failed".into(), title: g.name, text: format!("{e:#}"), icon: None });
            None
        }
    }
}

#[cfg(not(windows))]
mod sys {
    pub fn hook() {}
    pub fn shutter() {}
    pub fn foreground() -> Option<(isize, std::path::PathBuf)> {
        None
    }
}

#[cfg(windows)]
mod sys {
    use std::sync::atomic::{AtomicBool, Ordering};
    use windows_sys::Win32::Foundation::*;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_F12;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    pub use gamut_core::screenshots::foreground;

    static DOWN: AtomicBool = AtomicBool::new(false);

    /// A short synthesized shutter click (two decaying noise bursts), played without waiting.
    pub fn shutter() {
        use windows_sys::Win32::Media::Audio::*;
        static WAV: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
        let wav = WAV.get_or_init(|| {
            const RATE: u32 = 44_100;
            let (mut seed, mut prev) = (0x1234_5678u32, 0f32);
            let pcm: Vec<i16> = (0..RATE * 14 / 100)
                .map(|i| {
                    let t = i as f32 / RATE as f32;
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    let noise = (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0;
                    let env = |at: f32, len: f32| if t >= at { (-(t - at) / len).exp() } else { 0.0 };
                    // High-passed noise sounds like a click, not a hiss.
                    let v = noise * (env(0.0, 0.008) + 0.7 * env(0.06, 0.012));
                    let out = v - prev * 0.6;
                    prev = v;
                    (out * 0.3 * i16::MAX as f32) as i16
                })
                .collect();
            let data = (pcm.len() * 2) as u32;
            let mut w = Vec::with_capacity(44 + data as usize);
            w.extend(b"RIFF");
            w.extend((36 + data).to_le_bytes());
            w.extend(b"WAVEfmt ");
            w.extend(16u32.to_le_bytes());
            w.extend(1u16.to_le_bytes());
            w.extend(1u16.to_le_bytes());
            w.extend(RATE.to_le_bytes());
            w.extend((RATE * 2).to_le_bytes());
            w.extend(2u16.to_le_bytes());
            w.extend(16u16.to_le_bytes());
            w.extend(b"data");
            w.extend(data.to_le_bytes());
            pcm.iter().for_each(|s| w.extend(s.to_le_bytes()));
            w
        });
        // SAFETY: the WAV lives for the whole program, as SND_ASYNC needs.
        unsafe { PlaySoundW(wav.as_ptr().cast(), std::ptr::null_mut(), SND_MEMORY | SND_ASYNC | SND_NODEFAULT) };
    }

    /// Only compares the key and hands off: a slow hook lags all typing and Windows drops it.
    unsafe extern "system" fn proc_(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        // SAFETY: for HC_ACTION, lp points to a KBDLLHOOKSTRUCT.
        unsafe {
            if code == HC_ACTION as i32 {
                let k = &*(lp as *const KBDLLHOOKSTRUCT);
                if k.vkCode == VK_F12 as u32 && k.flags & LLKHF_INJECTED == 0 {
                    let down = wp == WM_KEYDOWN as usize || wp == WM_SYSKEYDOWN as usize;
                    // Key repeat sends more key-downs; one shot per press.
                    if down && !DOWN.swap(true, Ordering::Relaxed) {
                        if let Some(tx) = super::KEY.get() {
                            let _ = tx.send(());
                        }
                    } else if !down {
                        DOWN.store(false, Ordering::Relaxed);
                    }
                }
            }
            CallNextHookEx(std::ptr::null_mut(), code, wp, lp)
        }
    }

    pub fn hook() {
        // SAFETY: the hook lives as long as this thread's message loop.
        unsafe {
            let h = SetWindowsHookExW(WH_KEYBOARD_LL, Some(proc_), std::ptr::null_mut(), 0);
            if h.is_null() {
                log::warn!("screenshot key: couldn't watch the keyboard");
                return;
            }
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_game_in_front() {
        let mut emu = Game::new_custom("Xenoblade X".into(), Some(r"D:\Emu\Cemu\Cemu.exe".into()));
        emu.id = 1;
        let mut pc = Game::new_custom("XC2".into(), Some(r"D:\Jocuri\XC2\extracted\xc2.exe".into()));
        pc.id = 2;
        pc.install_dir = Some(r"D:\Jocuri\XC2".into());
        let games = [emu.clone(), pc.clone()];
        let none = HashSet::new();
        assert_eq!(game_for(Path::new(r"d:\jocuri\xc2\extracted\xc2.exe"), &games, &none).map(|g| g.id), Some(2));
        assert_eq!(game_for(Path::new(r"D:\Emu\Cemu\Cemu.exe"), &games, &HashSet::from([1])).map(|g| g.id), Some(1));
        assert!(game_for(Path::new(r"C:\Windows\explorer.exe"), &games, &none).is_none());
    }
}
