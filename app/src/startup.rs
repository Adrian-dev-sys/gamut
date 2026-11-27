//! Restores a maximized window without the white flash: Windows shows a window the moment it is
//! maximized, before anything is drawn, so we keep it cloaked (invisible to the compositor) until then.
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// The main window's handle (for cloaking).
pub fn hwnd(cc: &eframe::CreationContext) -> Option<isize> {
    let RawWindowHandle::Win32(h) = cc.window_handle().ok()?.as_raw() else { return None };
    Some(h.hwnd.get())
}

pub struct Startup {
    hwnd: isize,
    frames: u32,
    ready: u32,
}

impl Startup {
    /// Cloaks the window and asks for maximize; call before the first frame.
    pub fn maximize_hidden(cc: &eframe::CreationContext) -> Option<Self> {
        let RawWindowHandle::Win32(h) = cc.window_handle().ok()?.as_raw() else { return None };
        let hwnd = h.hwnd.get();
        cloak(hwnd, true);
        cc.egui_ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
        Some(Startup { hwnd, frames: 0, ready: 0 })
    }

    /// Uncloaks once a couple of frames have been drawn maximized. Returns true when done.
    pub fn tick(&mut self, ctx: &egui::Context) -> bool {
        self.frames += 1;
        if ctx.input(|i| i.viewport().maximized == Some(true)) {
            self.ready += 1;
        }
        // Fallback so a refused maximize can never leave the window invisible.
        if self.ready >= 2 || self.frames >= 60 {
            cloak(self.hwnd, false);
            return true;
        }
        ctx.request_repaint();
        false
    }
}

pub fn cloak(hwnd: isize, on: bool) {
    use windows_sys::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_CLOAK};
    let v = i32::from(on);
    // SAFETY: hwnd is our live top-level window; the attribute is a 4-byte BOOL.
    unsafe {
        DwmSetWindowAttribute(hwnd as _, DWMWA_CLOAK as u32, (&raw const v).cast(), 4);
    }
}
