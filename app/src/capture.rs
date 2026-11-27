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

/// scRGB (linear, 1.0 = 80 nits) → sRGB 8-bit. `white` is SDR white in scRGB units. The brightest channel
/// rolls off smoothly above a knee instead of clipping; scaling all three keeps hue and saturation.
pub fn tonemap(rgb: [f32; 3], white: f32) -> [u8; 3] {
    const KNEE: f32 = 0.75;
    let c = rgb.map(|v| (v / white).max(0.0));
    let m = c[0].max(c[1]).max(c[2]);
    let c = if m > KNEE {
        let mm = KNEE + (1.0 - KNEE) * (1.0 - (-(m - KNEE) / (1.0 - KNEE)).exp());
        c.map(|v| v * mm / m)
    } else {
        c
    };
    c.map(srgb)
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
            let hdr_on = output.cast::<IDXGIOutput6>().ok().and_then(|o| o.GetDesc1().ok()).is_some_and(|d| d.ColorSpace == DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020);
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
            let white = if hdr { sdr_white(&desc.DeviceName) } else { 1.0 };
            let mut px = vec![0u8; w * h * 4];
            let base = map.pData as *const u8;
            for y in 0..h {
                let row = base.add((y0 + y) * map.RowPitch as usize);
                for x in 0..w {
                    let o = (y * w + x) * 4;
                    let rgb = if hdr {
                        let p = row.add((x0 + x) * 8) as *const u16;
                        tonemap([f16(*p), f16(*p.add(1)), f16(*p.add(2))], white)
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
            log::info!("screenshot: {w}x{h}{}", if hdr { format!(", HDR tone-mapped at SDR white {:.0} nits", white * 80.0) } else { String::new() });
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
        // SDR white maps near white, mid grey stays, highlights roll off below clipping.
        assert!(tonemap([2.5, 2.5, 2.5], 2.5)[0] >= 240);
        assert_eq!(tonemap([0.5, 0.5, 0.5], 2.5), [124, 124, 124]);
        let hi = tonemap([25.0, 25.0, 25.0], 2.5);
        assert!(hi[0] >= 254);
        let bright_red = tonemap([10.0, 0.0, 0.0], 2.5);
        assert_eq!(bright_red, [255, 0, 0]);
        assert_eq!(tonemap([-1.0, 0.0, 0.0], 1.0), [0, 0, 0]);
    }
}
