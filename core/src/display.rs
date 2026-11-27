//! Per-game display mode (resolution / refresh rate) while a game runs, like Playnite's Display Helper.
use anyhow::{bail, Result};
use std::sync::Mutex;

/// What a game wants; 0 = keep the current value, empty device = the main display.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct DisplayMode {
    pub device: String,
    pub width: u32,
    pub height: u32,
    pub hz: u32,
}

impl DisplayMode {
    pub fn is_empty(&self) -> bool {
        self.width == 0 && self.hz == 0
    }

    /// The `display` column value; None when nothing changes.
    pub fn to_json(&self) -> Option<String> {
        (!self.is_empty()).then(|| serde_json::to_string(self).ok()).flatten()
    }

    /// "2560×1440 · 144 Hz on DISPLAY2".
    pub fn describe(&self) -> String {
        let mut v = Vec::new();
        if self.width > 0 {
            v.push(format!("{}×{}", self.width, self.height));
        }
        if self.hz > 0 {
            v.push(format!("{} Hz", self.hz));
        }
        let mut s = v.join(" · ");
        if !self.device.is_empty() {
            s += &format!(" on {}", short(&self.device));
        }
        s
    }

    /// From Display Helper's Playnite features: "[RC] 1920x1080", "[RC] 144Hz", "[RC] Display: \\.\DISPLAY1".
    pub fn from_playnite(features: &str) -> Option<DisplayMode> {
        let mut m = DisplayMode::default();
        for f in features.split(", ").filter_map(|f| f.strip_prefix("[RC] ")) {
            if let Some(d) = f.strip_prefix("Display: ") {
                m.device = d.trim().to_string();
            } else if let Some(hz) = f.strip_suffix("Hz") {
                m.hz = hz.trim().parse().unwrap_or(0);
            } else if let Some((w, h)) = f.split_once('x') {
                (m.width, m.height) = (w.trim().parse().unwrap_or(0), h.trim().parse().unwrap_or(0));
            }
        }
        (!m.is_empty()).then_some(m)
    }
}

/// "\\.\DISPLAY2" -> "DISPLAY2".
pub fn short(device: &str) -> &str {
    device.trim_start_matches(['\\', '.'])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Mode {
    pub width: u32,
    pub height: u32,
    pub hz: u32,
}

#[derive(Debug, Clone)]
pub struct Monitor {
    pub device: String,
    /// The monitor's name as Windows reports it.
    pub name: String,
    pub primary: bool,
    pub current: Mode,
}

/// The device currently changed by a game; another game won't change it again.
static CHANGED: Mutex<Option<String>> = Mutex::new(None);

/// Restores the display's own mode when dropped.
pub struct Guard(String);

impl Guard {
    /// Switches the display; None when there's nothing to do or another game already changed it.
    pub fn apply(m: &DisplayMode) -> Result<Option<Guard>> {
        if m.is_empty() {
            return Ok(None);
        }
        let mut changed = CHANGED.lock().unwrap();
        if changed.is_some() {
            log::info!("display: another game changed the display mode already; leaving it");
            return Ok(None);
        }
        let mons = monitors();
        let Some(mon) = mons.iter().find(|x| x.device == m.device).or_else(|| mons.iter().find(|x| x.primary)) else {
            bail!("no display found")
        };
        let (w, h) = if m.width > 0 { (m.width, m.height) } else { (mon.current.width, mon.current.height) };
        // A resolution without a rate keeps the current rate when it's offered, else Windows picks.
        let modes = modes(&mon.device);
        let hz = match m.hz {
            0 if modes.contains(&Mode { width: w, height: h, hz: mon.current.hz }) => mon.current.hz,
            hz => hz,
        };
        let want = Mode { width: w, height: h, hz };
        if want == mon.current {
            return Ok(None);
        }
        sys::set(&mon.device, want)?;
        log::info!("display: {} set to {w}x{h} {hz} Hz", short(&mon.device));
        *changed = Some(mon.device.clone());
        // Let the display settle before the game picks its mode.
        std::thread::sleep(std::time::Duration::from_secs(2));
        Ok(Some(Guard(mon.device.clone())))
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        match sys::restore(&self.0) {
            Ok(()) => log::info!("display: {} restored", short(&self.0)),
            Err(e) => log::warn!("display: restoring {} failed: {e:#}", short(&self.0)),
        }
        *CHANGED.lock().unwrap() = None;
    }
}

pub fn monitors() -> Vec<Monitor> {
    sys::monitors()
}

/// Modes a display offers, largest first.
pub fn modes(device: &str) -> Vec<Mode> {
    let mut v = sys::modes(device);
    v.sort_unstable_by(|a, b| b.cmp(a));
    v.dedup();
    v
}

#[cfg(windows)]
mod sys {
    use super::{Mode, Monitor};
    use anyhow::{bail, Result};
    use windows_sys::Win32::Graphics::Gdi::*;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    fn text(b: &[u16]) -> String {
        String::from_utf16_lossy(&b[..b.iter().position(|c| *c == 0).unwrap_or(b.len())])
    }

    fn devmode() -> DEVMODEW {
        DEVMODEW { dmSize: std::mem::size_of::<DEVMODEW>() as u16, ..Default::default() }
    }

    fn mode(d: &DEVMODEW) -> Mode {
        Mode { width: d.dmPelsWidth, height: d.dmPelsHeight, hz: d.dmDisplayFrequency }
    }

    pub fn monitors() -> Vec<Monitor> {
        let mut out = Vec::new();
        for i in 0.. {
            let mut dd = DISPLAY_DEVICEW { cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
            // SAFETY: dd is a properly sized out struct.
            if unsafe { EnumDisplayDevicesW(std::ptr::null(), i, &mut dd, 0) } == 0 {
                break;
            }
            if dd.StateFlags & DISPLAY_DEVICE_ATTACHED_TO_DESKTOP == 0 {
                continue;
            }
            let device = text(&dd.DeviceName);
            let w = wide(&device);
            let mut dm = devmode();
            // SAFETY: w is NUL-terminated; dm is sized.
            if unsafe { EnumDisplaySettingsW(w.as_ptr(), ENUM_CURRENT_SETTINGS, &mut dm) } == 0 {
                continue;
            }
            let mut mon = DISPLAY_DEVICEW { cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
            // SAFETY: as above; index 0 is the monitor on this adapter output.
            let name = if unsafe { EnumDisplayDevicesW(w.as_ptr(), 0, &mut mon, 0) } != 0 { text(&mon.DeviceString) } else { String::new() };
            out.push(Monitor { device, name, primary: dd.StateFlags & DISPLAY_DEVICE_PRIMARY_DEVICE != 0, current: mode(&dm) });
        }
        out
    }

    pub fn modes(device: &str) -> Vec<Mode> {
        let w = wide(device);
        let mut out = Vec::new();
        for i in 0.. {
            let mut dm = devmode();
            // SAFETY: w is NUL-terminated; dm is sized.
            if unsafe { EnumDisplaySettingsW(w.as_ptr(), i, &mut dm) } == 0 {
                break;
            }
            if dm.dmBitsPerPel >= 32 {
                out.push(mode(&dm));
            }
        }
        out
    }

    /// Temporary change (CDS_FULLSCREEN): not saved, and Windows undoes it if Gamut exits.
    pub fn set(device: &str, m: Mode) -> Result<()> {
        let w = wide(device);
        let mut dm = devmode();
        dm.dmPelsWidth = m.width;
        dm.dmPelsHeight = m.height;
        dm.dmFields = DM_PELSWIDTH | DM_PELSHEIGHT;
        if m.hz > 0 {
            dm.dmDisplayFrequency = m.hz;
            dm.dmFields |= DM_DISPLAYFREQUENCY;
        }
        for flags in [CDS_TEST, CDS_FULLSCREEN] {
            // SAFETY: w is NUL-terminated; dm is a sized DEVMODEW.
            let r = unsafe { ChangeDisplaySettingsExW(w.as_ptr(), &dm, std::ptr::null_mut(), flags, std::ptr::null()) };
            if r != DISP_CHANGE_SUCCESSFUL {
                bail!("{} doesn't accept {}x{} {} Hz (error {r})", super::short(device), m.width, m.height, m.hz);
            }
        }
        Ok(())
    }

    /// Back to the mode saved in Windows' settings.
    pub fn restore(device: &str) -> Result<()> {
        let w = wide(device);
        // SAFETY: a null DEVMODE resets the device to its registry mode.
        let r = unsafe { ChangeDisplaySettingsExW(w.as_ptr(), std::ptr::null(), std::ptr::null_mut(), 0, std::ptr::null()) };
        if r != DISP_CHANGE_SUCCESSFUL {
            bail!("error {r}");
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod sys {
    use super::{Mode, Monitor};
    pub fn monitors() -> Vec<Monitor> {
        Vec::new()
    }
    pub fn modes(_: &str) -> Vec<Mode> {
        Vec::new()
    }
    pub fn set(_: &str, _: Mode) -> anyhow::Result<()> {
        anyhow::bail!("not supported")
    }
    pub fn restore(_: &str) -> anyhow::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_display_helper_features() {
        let m = DisplayMode::from_playnite("Single Player, [RC] 1920x1080, [RC] 120Hz, [RC] Display: \\\\.\\DISPLAY2").unwrap();
        assert_eq!(m, DisplayMode { device: "\\\\.\\DISPLAY2".into(), width: 1920, height: 1080, hz: 120 });
        assert_eq!(m.describe(), "1920×1080 · 120 Hz on DISPLAY2");
        assert!(DisplayMode::from_playnite("Single Player").is_none());
    }

    #[test]
    fn lists_displays() {
        let mons = monitors();
        if let Some(m) = mons.iter().find(|m| m.primary) {
            assert!(modes(&m.device).contains(&m.current), "{m:?}");
        }
    }
}
