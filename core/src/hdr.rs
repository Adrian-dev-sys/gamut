//! Windows HDR ("Use HDR" in display settings) on/off for HDR-capable displays.
use anyhow::Result;
use std::sync::Mutex;

/// (games needing HDR, whether Gamut turned it on)
static USERS: Mutex<(usize, bool)> = Mutex::new((0, false));

/// Keeps HDR on while alive; HDR goes back off when the last guard drops, if Gamut turned it on.
pub struct Guard(());

impl Guard {
    /// Turns HDR on if it's off. Blocks ~2 s after switching so the game starts in HDR.
    pub fn acquire() -> Result<Guard> {
        let mut u = USERS.lock().unwrap();
        if u.0 == 0 && enabled() == Some(false) {
            set(true)?;
            u.1 = true;
            log::info!("HDR turned on");
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
        u.0 += 1;
        Ok(Guard(()))
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        let mut u = USERS.lock().unwrap();
        u.0 = u.0.saturating_sub(1);
        if u.0 == 0 && std::mem::take(&mut u.1) {
            match set(false) {
                Ok(_) => log::info!("HDR turned back off"),
                Err(e) => log::warn!("turning HDR back off failed: {e:#}"),
            }
        }
    }
}

/// Some(true/false) when at least one active display supports HDR; None if none does.
pub fn enabled() -> Option<bool> {
    let d = sys::displays();
    d.iter().any(|x| x.supported).then(|| d.iter().any(|x| x.supported && x.enabled))
}

/// Turns HDR on or off on every display that supports it; returns how many changed.
pub fn set(on: bool) -> Result<usize> {
    let mut n = 0;
    for d in sys::displays().into_iter().filter(|d| d.supported && d.enabled != on) {
        sys::set(&d, on)?;
        n += 1;
    }
    Ok(n)
}

#[allow(dead_code)]
struct Display {
    adapter: (u32, i32),
    id: u32,
    supported: bool,
    enabled: bool,
}

#[cfg(windows)]
mod sys {
    use super::Display;
    use anyhow::{bail, Result};
    use windows_sys::Win32::Devices::Display::{
        DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_DEVICE_INFO_HEADER,
        DISPLAYCONFIG_DEVICE_INFO_SET_ADVANCED_COLOR_STATE, DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_MODE_INFO,
        DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SET_ADVANCED_COLOR_STATE, DisplayConfigGetDeviceInfo, DisplayConfigSetDeviceInfo,
        GetDisplayConfigBufferSizes, QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
    };
    use windows_sys::Win32::Foundation::LUID;

    pub fn displays() -> Vec<Display> {
        let mut out = Vec::new();
        unsafe {
            let (mut np, mut nm) = (0u32, 0u32);
            if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm) != 0 {
                return out;
            }
            let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
            let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
            if QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &mut np, paths.as_mut_ptr(), &mut nm, modes.as_mut_ptr(), std::ptr::null_mut()) != 0 {
                return out;
            }
            for p in &paths[..np as usize] {
                let t = &p.targetInfo;
                let mut info = DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO {
                    header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                        r#type: DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO,
                        size: size_of::<DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO>() as u32,
                        adapterId: t.adapterId,
                        id: t.id,
                    },
                    ..Default::default()
                };
                if DisplayConfigGetDeviceInfo(&mut info.header) != 0 {
                    continue;
                }
                let v = info.Anonymous.value;
                // Bit 0: supported; bit 1: enabled; bit 2: wide color forced (SDR "advanced color").
                let wide_forced = v & 4 != 0;
                out.push(Display {
                    adapter: (t.adapterId.LowPart, t.adapterId.HighPart),
                    id: t.id,
                    supported: v & 1 != 0 && !wide_forced,
                    enabled: v & 2 != 0,
                });
            }
        }
        out
    }

    pub fn set(d: &Display, on: bool) -> Result<()> {
        let mut st = DISPLAYCONFIG_SET_ADVANCED_COLOR_STATE {
            header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_SET_ADVANCED_COLOR_STATE,
                size: size_of::<DISPLAYCONFIG_SET_ADVANCED_COLOR_STATE>() as u32,
                adapterId: LUID { LowPart: d.adapter.0, HighPart: d.adapter.1 },
                id: d.id,
            },
            ..Default::default()
        };
        st.Anonymous.value = on as u32;
        let r = unsafe { DisplayConfigSetDeviceInfo(&st.header) };
        if r != 0 {
            bail!("Windows refused to switch HDR (error {r})");
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod sys {
    use super::Display;
    pub fn displays() -> Vec<Display> {
        Vec::new()
    }
    pub fn set(_: &Display, _: bool) -> anyhow::Result<()> {
        anyhow::bail!("HDR switching is Windows-only")
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_state() {
        // Read-only: just make sure the query works on this machine.
        println!("HDR: {:?}", super::enabled());
    }
}
