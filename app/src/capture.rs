//! F12 screenshots: grabs the monitor with DXGI desktop duplication (FP16 when HDR is on, then tone-mapped to
//! SDR at the user's SDR white level) and crops it to the game window. Falls back to a GDI screen copy.
use anyhow::{bail, Context, Result};

/// The window's client area as an SDR image.
pub fn grab(hwnd: isize) -> Result<image::RgbaImage> {
    match dxgi::grab(hwnd) {
        Ok(img) => Ok(img),
        Err(e) => {
            log::warn!("screenshot: desktop duplication failed ({e:#}); using a screen copy");
            gamut_core::screenshots::capture(hwnd)
        }
    }
}

fn f16(h: u16) -> f32 {
    let (s, e, m) = ((h >> 15) as u32, ((h >> 10) & 0x1f) as u32, (h & 0x3ff) as u32);
    let v = match e {
        0 => m as f32 / 1024.0 * 2f32.powi(-14),
        31 => f32::INFINITY,
        _ => (1.0 + m as f32 / 1024.0) * 2f32.powi(e as i32 - 15),
    };
    if s == 1 { -v } else { v }
}

fn srgb(v: f32) -> u8 {
    let v = v.clamp(0.0, 1.0);
    let e = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
    (e * 255.0 + 0.5) as u8
}

fn pq(nits: f32) -> f32 {
    const M1: f32 = 0.159_301_76;
    const M2: f32 = 78.843_75;
    let y = (nits / 10_000.0).clamp(0.0, 1.0).powf(M1);
    ((0.835_937_5 + 18.851_563 * y) / (1.0 + 18.6875 * y)).powf(M2)
}

fn pq_nits(e: f32) -> f32 {
    let p = e.clamp(0.0, 1.0).powf(1.0 / 78.843_75);
    ((p - 0.835_937_5).max(0.0) / (18.851_563 - 18.6875 * p)).powf(1.0 / 0.159_301_76) * 10_000.0
}

fn luma(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// HDR → SDR. The brightest channel goes through BT.2390's roll-off in the PQ domain, from the frame's peak
/// down to Windows' SDR white (255), averaged with each channel rolled off on its own: the first keeps hue and
/// saturation (no pastel skies), the second lets bright colors lighten like they do on the HDR screen (no red skin).
pub struct Tonemap {
    white: f32,
    src: f32,
    target: f32,
    knee: f32,
}

impl Tonemap {
    /// `white` and `peak` in nits.
    pub fn new(white: f32, peak: f32) -> Tonemap {
        let src = pq(peak.max(white));
        let target = pq(white) / src;
        Tonemap { white, src, target, knee: (1.5 * target - 0.5).max(0.0) }
    }

    fn eetf(&self, nits: f32) -> f32 {
        let e = (pq(nits) / self.src).min(1.0);
        if e <= self.knee || self.target >= 1.0 {
            return nits.min(pq_nits(self.src));
        }
        let (ks, t) = (self.knee, (e - self.knee) / (1.0 - self.knee));
        let (t2, t3) = (t * t, t * t * t);
        let e2 = (2.0 * t3 - 3.0 * t2 + 1.0) * ks + (t3 - 2.0 * t2 + t) * (1.0 - ks) + (-2.0 * t3 + 3.0 * t2) * self.target;
        pq_nits(e2 * self.src)
    }

    /// One scRGB pixel (linear, 1.0 = 80 nits).
    pub fn map(&self, c: [f32; 3]) -> [u8; 3] {
        let mut n = c.map(|v| v * 80.0);
        let y = luma(n);
        if y <= 0.0 {
            return [0; 3];
        }
        let lo = n[0].min(n[1]).min(n[2]);
        if lo < 0.0 {
            n = n.map(|v| y + (v - y) * y / (y - lo));
        }
        let hi = n[0].max(n[1]).max(n[2]);
        let k = self.eetf(hi) / hi;
        n.map(|v| srgb((v * k + self.eetf(v.max(0.0))) * 0.5 / self.white))
    }
}

/// The frame's highlight level: 99.9th percentile of the brightest channel (nits), from a log histogram.
pub fn frame_peak(lum: impl Iterator<Item = f32>) -> f32 {
    let mut hist = [0u32; 256];
    let mut n = 0u32;
    for l in lum {
        let b = ((l.max(0.01).log2() + 7.0) * 12.0).clamp(0.0, 255.0) as usize;
        hist[b] += 1;
        n += 1;
    }
    let mut left = (n as f32 * 0.001) as u32;
    for b in (0..256).rev() {
        if hist[b] > left {
            return 2f32.powf((b + 1) as f32 / 12.0 - 7.0);
        }
        left -= hist[b];
    }
    0.0
}

mod dxgi {
    use super::*;
    use windows::core::Interface;
    use windows::Win32::Devices::Display::*;
    use windows::Win32::Foundation::{HWND, POINT, RECT};
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
    use windows::Win32::Graphics::Direct3D11::*;
    use windows::Win32::Graphics::Dxgi::Common::*;
    use windows::Win32::Graphics::Dxgi::*;
    use windows::Win32::Graphics::Gdi::{ClientToScreen, MonitorFromWindow, MONITOR_DEFAULTTONEAREST};
    use windows::Win32::UI::WindowsAndMessaging::GetClientRect;

    pub fn grab(hwnd: isize) -> Result<image::RgbaImage> {
        // SAFETY: COM calls on objects created here; the mapped buffer is read within its pitch and size.
        unsafe {
            let hwnd = HWND(hwnd as _);
            let mut rc = RECT::default();
            GetClientRect(hwnd, &mut rc)?;
            let mut tl = POINT::default();
            let _ = ClientToScreen(hwnd, &mut tl);
            let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
            let (mut adapter, mut output) = (None, None);
            'find: for a in 0.. {
                let Ok(ad) = factory.EnumAdapters1(a) else { break };
                for o in 0.. {
                    let Ok(out) = ad.EnumOutputs(o) else { break };
                    if out.GetDesc()?.Monitor == mon {
                        (adapter, output) = (Some(ad), Some(out));
                        break 'find;
                    }
                }
            }
            let (Some(adapter), Some(output)) = (adapter, output) else { bail!("monitor not found") };
            let desc = output.GetDesc()?;
            let mut device = None;
            let mut ctx = None;
            D3D11CreateDevice(&adapter, D3D_DRIVER_TYPE_UNKNOWN, Default::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut ctx))?;
            let (device, ctx) = (device.context("no device")?, ctx.context("no context")?);
            // HDR (PQ / BT.2020) output: ask for the FP16 desktop and tone-map; SDR: plain 8-bit, untouched.
            let desc1 = output.cast::<IDXGIOutput6>().ok().and_then(|o| o.GetDesc1().ok());
            let hdr_on = desc1.is_some_and(|d| d.ColorSpace == DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020);
            let display_peak = desc1.map_or(0.0, |d| d.MaxLuminance);
            let formats: &[DXGI_FORMAT] = if hdr_on { &[DXGI_FORMAT_R16G16B16A16_FLOAT] } else { &[DXGI_FORMAT_B8G8R8A8_UNORM] };
            let out5: IDXGIOutput5 = output.cast()?;
            let dup = out5.DuplicateOutput1(&device, 0, formats)?;
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut res = None;
            dup.AcquireNextFrame(500, &mut info, &mut res)?;
            let tex: ID3D11Texture2D = res.context("no frame")?.cast()?;
            let mut td = D3D11_TEXTURE2D_DESC::default();
            tex.GetDesc(&mut td);
            let st = D3D11_TEXTURE2D_DESC {
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
                MipLevels: 1,
                ArraySize: 1,
                ..td
            };
            let mut staging = None;
            device.CreateTexture2D(&st, None, Some(&mut staging))?;
            let staging = staging.context("no staging texture")?;
            ctx.CopyResource(&staging, &tex);
            let _ = dup.ReleaseFrame();
            let mut map = D3D11_MAPPED_SUBRESOURCE::default();
            ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut map))?;
            // Crop to the window, in output coordinates.
            let d = desc.DesktopCoordinates;
            let x0 = (tl.x - d.left).clamp(0, td.Width as i32) as usize;
            let y0 = (tl.y - d.top).clamp(0, td.Height as i32) as usize;
            let w = ((rc.right - rc.left) as usize).min(td.Width as usize - x0);
            let h = ((rc.bottom - rc.top) as usize).min(td.Height as usize - y0);
            let hdr = td.Format == DXGI_FORMAT_R16G16B16A16_FLOAT;
            if td.Format != DXGI_FORMAT_B8G8R8A8_UNORM && !hdr {
                ctx.Unmap(&staging, 0);
                bail!("unexpected desktop format {:?}", td.Format);
            }
            let base = map.pData as *const u8;
            let pixel = |x: usize, y: usize| -> [f32; 3] {
                let p = base.add((y0 + y) * map.RowPitch as usize + (x0 + x) * 8) as *const u16;
                [f16(*p), f16(*p.add(1)), f16(*p.add(2))]
            };
            let (white, peak) = if hdr {
                let white = sdr_white(&desc.DeviceName) * 80.0;
                let frame = frame_peak((0..h).step_by(2).flat_map(|y| (0..w).step_by(2).map(move |x| (x, y))).map(|(x, y)| pixel(x, y).into_iter().fold(0f32, f32::max) * 80.0));
                // Games master to the display's peak; never beyond it.
                (white, if display_peak > white { frame.min(display_peak) } else { frame })
            } else {
                (80.0, 80.0)
            };
            let tm = Tonemap::new(white, peak);
            let mut px = vec![0u8; w * h * 4];
            for y in 0..h {
                let row = base.add((y0 + y) * map.RowPitch as usize);
                for x in 0..w {
                    let o = (y * w + x) * 4;
                    let rgb = if hdr {
                        tm.map(pixel(x, y))
                    } else {
                        let p = row.add((x0 + x) * 4);
                        [*p.add(2), *p.add(1), *p]
                    };
                    px[o..o + 4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
                }
            }
            ctx.Unmap(&staging, 0);
            if w < 16 || h < 16 {
                bail!("the window is off this monitor");
            }
            log::info!("screenshot: {w}x{h}{}", if hdr { format!(", HDR tone-mapped: SDR white {white:.0} nits, frame peak {peak:.0} nits (display {display_peak:.0})") } else { String::new() });
            image::RgbaImage::from_raw(w as u32, h as u32, px).context("bad size")
        }
    }

    /// Windows' "SDR content brightness" for the display, in scRGB units (1.0 = 80 nits); 2.5 if unknown.
    fn sdr_white(gdi_name: &[u16; 32]) -> f32 {
        let want = String::from_utf16_lossy(&gdi_name[..gdi_name.iter().position(|c| *c == 0).unwrap_or(32)]);
        // SAFETY: buffers sized from GetDisplayConfigBufferSizes; request packets carry their own size.
        unsafe {
            let (mut np, mut nm) = (0u32, 0u32);
            if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm).is_err() {
                return 2.5;
            }
            let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
            let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
            if QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &mut np, paths.as_mut_ptr(), &mut nm, modes.as_mut_ptr(), None).is_err() {
                return 2.5;
            }
            for p in &paths[..np as usize] {
                let mut src = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
                src.header = DISPLAYCONFIG_DEVICE_INFO_HEADER {
                    r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                    size: std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                    adapterId: p.sourceInfo.adapterId,
                    id: p.sourceInfo.id,
                };
                if DisplayConfigGetDeviceInfo(&mut src.header) != 0 {
                    continue;
                }
                let n = &src.viewGdiDeviceName;
                if String::from_utf16_lossy(&n[..n.iter().position(|c| *c == 0).unwrap_or(32)]) != want {
                    continue;
                }
                let mut wl = DISPLAYCONFIG_SDR_WHITE_LEVEL::default();
                wl.header = DISPLAYCONFIG_DEVICE_INFO_HEADER {
                    r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL,
                    size: std::mem::size_of::<DISPLAYCONFIG_SDR_WHITE_LEVEL>() as u32,
                    adapterId: p.targetInfo.adapterId,
                    id: p.targetInfo.id,
                };
                if DisplayConfigGetDeviceInfo(&mut wl.header) == 0 && wl.SDRWhiteLevel > 0 {
                    return wl.SDRWhiteLevel as f32 / 1000.0;
                }
            }
            2.5
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tonemaps_hdr_to_sdr() {
        assert_eq!(f16(0x3c00), 1.0);
        assert_eq!(f16(0xc000), -2.0);
        assert!((pq_nits(pq(203.0)) - 203.0).abs() < 0.5);
        let n = |nits: f32| nits / 80.0;
        // No highlights above SDR white: SDR white is 255, darker values are untouched.
        let flat = Tonemap::new(260.0, 260.0);
        assert_eq!(flat.map([n(260.0); 3]), [255; 3]);
        assert_eq!(flat.map([n(52.0); 3]), [124; 3]);
        // 1000-nit highlights roll into white; shadows and midtones stay; out-of-gamut doesn't go black.
        let tm = Tonemap::new(260.0, 1000.0);
        assert_eq!(tm.map([n(1000.0); 3]), [255; 3]);
        assert!(tm.map([n(500.0); 3])[0] < 255);
        assert!(tm.map([n(260.0); 3])[0] < tm.map([n(500.0); 3])[0]);
        assert_eq!(tm.map([n(52.0); 3]), [124; 3]);
        let sky = tm.map([n(150.0), n(400.0), n(900.0)]);
        assert!(sky[2] >= 250 && sky[0] < 180 && sky[0] < sky[1], "{sky:?}");
        let g = tm.map([n(-20.0), n(100.0), n(20.0)]);
        assert!(g[1] > 150 && g[2] > 0, "{g:?}");
        assert_eq!(tm.map([-1.0, 0.0, 0.0]), [0, 0, 0]);
        assert!((frame_peak((0..1000).map(|i| if i == 999 { 5000.0 } else { 100.0 })) - 100.0).abs() < 10.0);
    }
}
