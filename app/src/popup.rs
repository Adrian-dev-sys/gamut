//! Achievement popup: a small topmost window that never takes focus, at the top right of the
//! foreground window's monitor. Shows over windowed / borderless games, not exclusive fullscreen.
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Mutex, OnceLock};

pub struct Popup {
    pub caption: String,
    pub title: String,
    pub text: String,
    /// RGBA pixels, width, height.
    pub icon: Option<(Vec<u8>, u32, u32)>,
}

/// Queues a popup; they show one after another, 5 s each.
pub fn show(p: Popup) {
    static TX: OnceLock<Mutex<Sender<Popup>>> = OnceLock::new();
    let tx = TX.get_or_init(|| {
        let (tx, rx) = channel();
        std::thread::spawn(move || sys::run(rx));
        Mutex::new(tx)
    });
    let _ = tx.lock().unwrap().send(p);
}

#[cfg(not(windows))]
mod sys {
    pub fn run(rx: super::Receiver<super::Popup>) {
        for _ in rx {}
    }
}

#[cfg(windows)]
mod sys {
    use super::{Popup, Receiver};
    use std::cell::RefCell;
    use windows_sys::Win32::Foundation::*;
    use windows_sys::Win32::Graphics::Gdi::*;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    thread_local!(static CURRENT: RefCell<Option<Popup>> = const { RefCell::new(None) });

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
        r as u32 | (g as u32) << 8 | (b as u32) << 16
    }

    /// Screen DPI scale (Gamut is DPI aware).
    fn scale() -> f32 {
        // SAFETY: the screen DC is released right after use.
        unsafe {
            let dc = GetDC(std::ptr::null_mut());
            let dpi = GetDeviceCaps(dc, LOGPIXELSY as i32);
            ReleaseDC(std::ptr::null_mut(), dc);
            if dpi > 0 { dpi as f32 / 96.0 } else { 1.0 }
        }
    }

    pub fn run(rx: Receiver<Popup>) {
        let class = wide("GamutPopup");
        // SAFETY: plain Win32 window creation and a message loop on this thread.
        unsafe {
            let inst = GetModuleHandleW(std::ptr::null());
            let wc = WNDCLASSW {
                lpfnWndProc: Some(proc_),
                hInstance: inst,
                lpszClassName: class.as_ptr(),
                hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
                ..std::mem::zeroed()
            };
            RegisterClassW(&wc);
            for p in rx {
                CURRENT.with(|c| *c.borrow_mut() = Some(p));
                let s = scale();
                let (w, h) = ((400.0 * s) as i32, (84.0 * s) as i32);
                let mon = MonitorFromWindow(GetForegroundWindow(), MONITOR_DEFAULTTOPRIMARY);
                let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..std::mem::zeroed() };
                GetMonitorInfoW(mon, &mut mi);
                let m = (24.0 * s) as i32;
                let (x, y) = (mi.rcWork.right - w - m, mi.rcWork.top + m);
                let ex = WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_TRANSPARENT;
                let hwnd = CreateWindowExW(ex, class.as_ptr(), class.as_ptr(), WS_POPUP, x, y, w, h, std::ptr::null_mut(), std::ptr::null_mut(), inst, std::ptr::null());
                if hwnd.is_null() {
                    continue;
                }
                SetLayeredWindowAttributes(hwnd, 0, 242, LWA_ALPHA);
                let r = (16.0 * s) as i32;
                SetWindowRgn(hwnd, CreateRoundRectRgn(0, 0, w + 1, h + 1, r, r), 1);
                ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                SetTimer(hwnd, 1, 5000, None);
                let mut msg: MSG = std::mem::zeroed();
                while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }
    }

    unsafe extern "system" fn proc_(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        // SAFETY: called by Windows on this thread with a valid hwnd.
        unsafe {
            match msg {
                WM_PAINT => {
                    let mut ps: PAINTSTRUCT = std::mem::zeroed();
                    let dc = BeginPaint(hwnd, &mut ps);
                    CURRENT.with(|c| {
                        if let Some(p) = c.borrow().as_ref() {
                            paint(hwnd, dc, p);
                        }
                    });
                    EndPaint(hwnd, &ps);
                    0
                }
                WM_TIMER => {
                    DestroyWindow(hwnd);
                    0
                }
                WM_DESTROY => {
                    PostQuitMessage(0);
                    0
                }
                _ => DefWindowProcW(hwnd, msg, wp, lp),
            }
        }
    }

    unsafe fn paint(hwnd: HWND, dc: HDC, p: &Popup) {
        // SAFETY: dc is the paint DC of hwnd; GDI objects are deleted after use.
        unsafe {
            let s = scale();
            let px = |v: f32| (v * s) as i32;
            let mut rc: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut rc);
            let bg = CreateSolidBrush(rgb(28, 31, 38));
            FillRect(dc, &rc, bg);
            DeleteObject(bg as _);
            let gold = CreateSolidBrush(rgb(232, 184, 74));
            FillRect(dc, &RECT { left: 0, top: 0, right: px(4.0), bottom: rc.bottom }, gold);
            DeleteObject(gold as _);
            let mut left = px(16.0);
            if let Some((rgba, w, h)) = &p.icon {
                let bgra: Vec<u8> = rgba.chunks_exact(4).flat_map(|c| [c[2], c[1], c[0], 255]).collect();
                let bmi = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: *w as i32,
                        biHeight: -(*h as i32),
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB,
                        ..std::mem::zeroed()
                    },
                    ..std::mem::zeroed()
                };
                SetStretchBltMode(dc, HALFTONE);
                let sz = px(60.0);
                StretchDIBits(dc, left, px(12.0), sz, sz, 0, 0, *w as i32, *h as i32, bgra.as_ptr().cast(), &bmi, DIB_RGB_COLORS, SRCCOPY);
                left += sz + px(14.0);
            }
            SetBkMode(dc, TRANSPARENT as i32);
            let face = wide("Segoe UI");
            let line = |text: &str, size: f32, weight: i32, color: COLORREF, top: f32| {
                let font = CreateFontW(-px(size), 0, 0, 0, weight, 0, 0, 0, 1, 0, 0, 5, 0, face.as_ptr());
                let old = SelectObject(dc, font as _);
                SetTextColor(dc, color);
                let t: Vec<u16> = text.encode_utf16().collect();
                let mut r = RECT { left, top: px(top), right: rc.right - px(14.0), bottom: px(top + size * 1.6) };
                DrawTextW(dc, t.as_ptr(), t.len() as i32, &mut r, DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX);
                SelectObject(dc, old);
                DeleteObject(font as _);
            };
            line(&p.caption, 12.5, 400, rgb(232, 184, 74), 12.0);
            line(&p.title, 17.0, 600, rgb(240, 242, 246), 30.0);
            line(&p.text, 12.5, 400, rgb(160, 166, 178), 54.0);
        }
    }
}
