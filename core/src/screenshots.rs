//! Screenshots: Gamut's own (F12, `Pictures\Gamut\<game>`) and Steam's (`userdata\<id>\760\remote\<appid>`).
//! The files are the truth; the index is rebuilt from disk. Steam's folders are only read.
use crate::model::{Game, Source};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Shot {
    pub game_id: i64,
    pub path: PathBuf,
    /// Small preview (Steam's own, or Gamut's in `.thumbs`).
    pub thumb: Option<PathBuf>,
    /// Unix seconds.
    pub taken: i64,
    pub steam: bool,
}

/// `Pictures\Gamut`, following a moved (e.g. OneDrive) Pictures folder.
pub fn root() -> PathBuf {
    // "User Shell Folders" is current ("Shell Folders" can be stale); expand %VARS% ourselves.
    let expand = |s: String| -> String {
        let mut parts = s.split('%');
        let mut out = parts.next().unwrap_or_default().to_string();
        for (i, p) in parts.enumerate() {
            match std::env::var(p) {
                Ok(v) if i % 2 == 0 => out += &v,
                _ if i % 2 == 0 => out += &format!("%{p}%"),
                _ => out += p,
            }
        }
        out
    };
    let pics = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Explorer\User Shell Folders")
        .and_then(|k| k.get_value::<String, _>("My Pictures"))
        .map(|s| PathBuf::from(expand(s)))
        .ok()
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join("Pictures")))
        .unwrap_or_else(|| PathBuf::from("."));
    pics.join("Gamut")
}

/// The game's folder name: its name without characters Windows refuses, plus the platform when another game
/// has the same name (duplicates stay apart).
pub fn folder_name(g: &Game, games: &[Game]) -> String {
    let clean = |s: &str| -> String {
        let s: String = s.chars().map(|c| if "<>:\"/\\|?*".contains(c) || c.is_control() { ' ' } else { c }).collect();
        s.split_whitespace().collect::<Vec<_>>().join(" ").trim_end_matches('.').to_string()
    };
    let name = clean(&g.name);
    let twin = games.iter().any(|o| o.id != g.id && o.name.eq_ignore_ascii_case(&g.name));
    match (twin, g.platform.as_deref()) {
        (true, Some(p)) => format!("{name} ({})", clean(p)),
        (true, None) if g.source == Source::Steam => format!("{name} (Steam)"),
        _ => name,
    }
}

fn is_image(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| ["png", "jpg", "jpeg", "webp", "bmp"].contains(&e.to_ascii_lowercase().as_str()))
}

fn mtime(p: &Path) -> i64 {
    std::fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs() as i64)
}

/// Every screenshot of the library's games, newest first.
pub fn index(games: &[Game]) -> Vec<Shot> {
    let mut out = Vec::new();
    // Gamut's own.
    let root = root();
    for g in games {
        let dir = root.join(folder_name(g, games));
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let p = e.path();
            if is_image(&p) {
                let thumb = Some(dir.join(".thumbs").join(format!("{}.jpg", p.file_stem().unwrap_or_default().to_string_lossy()))).filter(|t| t.is_file());
                out.push(Shot { game_id: g.id, taken: mtime(&p), path: p, thumb, steam: false });
            }
        }
    }
    // Steam's, for Steam games.
    let by_app: HashMap<&str, i64> = games.iter().filter(|g| g.source == Source::Steam).map(|g| (g.source_id.as_str(), g.id)).collect();
    let users = crate::steam::steam_path().map(|s| s.join("userdata"));
    for user in users.iter().flat_map(|u| std::fs::read_dir(u).into_iter().flatten().flatten()) {
        for app in std::fs::read_dir(user.path().join("760").join("remote")).into_iter().flatten().flatten() {
            let Some(&id) = by_app.get(app.file_name().to_string_lossy().as_ref()) else { continue };
            let dir = app.path().join("screenshots");
            for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let p = e.path();
                if is_image(&p) {
                    let thumb = Some(dir.join("thumbnails").join(e.file_name())).filter(|t| t.is_file());
                    out.push(Shot { game_id: id, taken: mtime(&p), path: p, thumb, steam: true });
                }
            }
        }
    }
    out.sort_by(|a, b| b.taken.cmp(&a.taken));
    out
}

/// Saves `img` as lossy WebP (quality 88, sharp YUV: about half a good JPEG, no visible loss) in `dir`, named by
/// local date and time, with a 640 px JPEG preview in `.thumbs`.
pub fn save(img: &image::RgbaImage, dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dir.join(".thumbs")).with_context(|| format!("creating {}", dir.display()))?;
    let t = crate::now() + crate::utc_offset();
    let (days, secs) = (t.div_euclid(86_400), t.rem_euclid(86_400));
    let (y, m, d) = civil(days);
    let mut stem = format!("{y:04}-{m:02}-{d:02} {:02}-{:02}-{:02}", secs / 3600, secs / 60 % 60, secs % 60);
    let mut n = 2;
    while dir.join(format!("{stem}.webp")).exists() {
        stem = format!("{} ({n})", stem.split(" (").next().unwrap_or(&stem));
        n += 1;
    }
    let path = dir.join(format!("{stem}.webp"));
    let rgb = image::DynamicImage::ImageRgba8(img.clone()).to_rgb8();
    let jpeg = |p: &Path, im: &image::RgbImage, q: u8| -> Result<()> {
        let f = std::io::BufWriter::new(std::fs::File::create(p)?);
        image::codecs::jpeg::JpegEncoder::new_with_quality(f, q).encode_image(im)?;
        Ok(())
    };
    let mut cfg = webp::WebPConfig::new().map_err(|_| anyhow::anyhow!("WebP setup failed"))?;
    (cfg.quality, cfg.method, cfg.use_sharp_yuv) = (88.0, 4, 1);
    let data = webp::Encoder::from_rgb(rgb.as_raw(), rgb.width(), rgb.height())
        .encode_advanced(&cfg)
        .map_err(|e| anyhow::anyhow!("WebP encoding failed: {e:?}"))?;
    std::fs::write(&path, &*data).context("saving the screenshot")?;
    let w = rgb.width().min(640);
    let thumb = image::imageops::resize(&rgb, w, (rgb.height() as u64 * w as u64 / rgb.width() as u64).max(1) as u32, image::imageops::FilterType::Triangle);
    jpeg(&dir.join(".thumbs").join(format!("{stem}.jpg")), &thumb, 85)?;
    Ok(path)
}

/// Days since 1970 → (year, month, day).
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// The foreground window: its process's exe and a capture of its client area.
#[cfg(windows)]
pub fn foreground() -> Option<(isize, PathBuf)> {
    use windows_sys::Win32::System::Threading::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    // SAFETY: plain queries; the process handle is closed.
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return None;
        }
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) != 0;
        windows_sys::Win32::Foundation::CloseHandle(h);
        ok.then(|| (hwnd as isize, PathBuf::from(String::from_utf16_lossy(&buf[..len as usize]))))
    }
}

/// Copies the window's client area from the screen (GDI; what's on screen, so overlays show and
/// exclusive fullscreen may come out black).
#[cfg(windows)]
pub fn capture(hwnd: isize) -> Result<image::RgbaImage> {
    use windows_sys::Win32::Foundation::*;
    use windows_sys::Win32::Graphics::Gdi::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    // SAFETY: GDI objects are created, used and released here; buffers are sized for the bitmap.
    unsafe {
        let hwnd = hwnd as HWND;
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut rc);
        let mut tl = POINT { x: 0, y: 0 };
        ClientToScreen(hwnd, &mut tl);
        let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
        if w < 16 || h < 16 {
            anyhow::bail!("the window is too small to capture");
        }
        let screen = GetDC(std::ptr::null_mut());
        let mem = CreateCompatibleDC(screen);
        let bmp = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(mem, bmp as _);
        let ok = BitBlt(mem, 0, 0, w, h, screen, tl.x, tl.y, SRCCOPY) != 0;
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..std::mem::zeroed()
            },
            ..std::mem::zeroed()
        };
        let mut px = vec![0u8; (w * h * 4) as usize];
        let rows = GetDIBits(mem, bmp, 0, h as u32, px.as_mut_ptr().cast(), &mut bmi, DIB_RGB_COLORS);
        SelectObject(mem, old);
        DeleteObject(bmp as _);
        DeleteDC(mem);
        ReleaseDC(std::ptr::null_mut(), screen);
        if !ok || rows == 0 {
            anyhow::bail!("couldn't copy the screen");
        }
        for c in px.chunks_exact_mut(4) {
            c.swap(0, 2);
            c[3] = 255;
        }
        image::RgbaImage::from_raw(w as u32, h as u32, px).context("bad capture size")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_webp_with_preview() {
        let dir = std::env::temp_dir().join(format!("gamut-shot-{}", std::process::id()));
        let img = image::RgbaImage::from_fn(800, 450, |x, y| image::Rgba([(x % 256) as u8, (y % 256) as u8, 128, 255]));
        let p = save(&img, &dir).unwrap();
        assert_eq!(p.extension().unwrap(), "webp");
        assert_eq!(image::image_dimensions(&p).unwrap(), (800, 450));
        assert_eq!(image::image_dimensions(dir.join(".thumbs").join(format!("{}.jpg", p.file_stem().unwrap().to_string_lossy()))).unwrap(), (640, 360));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn folder_names_keep_duplicates_apart() {
        let mut a = Game::new_custom("Slay the Spire".into(), None);
        a.id = 1;
        a.platform = Some("Sony PlayStation 4".into());
        let mut b = Game::new_custom("Slay the Spire".into(), None);
        b.id = 2;
        b.source = Source::Steam;
        let mut c = Game::new_custom("Re:Zero? <Test>".into(), None);
        c.id = 3;
        let all = [a.clone(), b.clone(), c.clone()];
        assert_eq!(folder_name(&a, &all), "Slay the Spire (Sony PlayStation 4)");
        assert_eq!(folder_name(&b, &all), "Slay the Spire (Steam)");
        assert_eq!(folder_name(&c, &all), "Re Zero Test");
        assert_eq!(civil(20_419), (2025, 11, 27));
    }
}
