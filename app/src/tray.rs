//! Notification-area icon: open Gamut, big screen, recently played games, exit. Also lets a second
//! launch of gamut.exe bring the running window back (`signal_running`).
use crossbeam_channel::{unbounded, Receiver};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

/// Games listed in the menu.
pub const RECENT: usize = 10;
const SHOW_EVENT: &str = "Local\\GamutShowWindow";
/// `gamut.exe --quit` (used by install.ps1): exit for real, even when closing would only hide.
const QUIT_EVENT: &str = "Local\\GamutQuit";

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Cmd {
    Show,
    BigScreen,
    Play(i64),
    Quit,
}

pub struct Tray {
    icon: TrayIcon,
    pub rx: Receiver<Cmd>,
    recent: Vec<(i64, String)>,
}

impl Tray {
    /// Must be called on the UI thread (its message loop drives the icon).
    pub fn new(ctx: &egui::Context) -> Option<Tray> {
        let (tx, rx) = unbounded();
        let (t, c) = (tx.clone(), ctx.clone());
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            let cmd = match e.id.0.as_str() {
                "show" => Cmd::Show,
                "big" => Cmd::BigScreen,
                "quit" => Cmd::Quit,
                s => match s.strip_prefix("play:").and_then(|n| n.parse().ok()) {
                    Some(id) => Cmd::Play(id),
                    None => return,
                },
            };
            let _ = t.send(cmd);
            c.request_repaint();
        }));
        let (t, c) = (tx.clone(), ctx.clone());
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            let left_up = matches!(e, TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. });
            if left_up || matches!(e, TrayIconEvent::DoubleClick { button: MouseButton::Left, .. }) {
                let _ = t.send(Cmd::Show);
                c.request_repaint();
            }
        }));

        let img = image::load_from_memory(include_bytes!("../assets/gamut.png")).ok()?.resize(32, 32, image::imageops::FilterType::Lanczos3).into_rgba8();
        let icon = tray_icon::Icon::from_rgba(img.to_vec(), img.width(), img.height()).ok()?;
        let icon = TrayIconBuilder::new()
            .with_icon(icon)
            .with_tooltip("Gamut")
            .with_menu(Box::new(menu(&[])))
            .with_menu_on_left_click(false)
            .build()
            .map_err(|e| log::warn!("tray icon unavailable: {e}"))
            .ok()?;
        log::info!("tray icon ready");
        listen(SHOW_EVENT, Cmd::Show, tx.clone(), ctx.clone());
        listen(QUIT_EVENT, Cmd::Quit, tx, ctx.clone());
        Some(Tray { icon, rx, recent: Vec::new() })
    }

    /// Rebuilds the menu when the recently played list changed.
    pub fn set_recent(&mut self, recent: Vec<(i64, String)>) {
        if recent != self.recent {
            self.icon.set_menu(Some(Box::new(menu(&recent))));
            self.recent = recent;
        }
    }
}

fn menu(recent: &[(i64, String)]) -> Menu {
    let m = Menu::new();
    let _ = m.append(&MenuItem::with_id("show", "Open Gamut", true, None));
    let _ = m.append(&MenuItem::with_id("big", "Big screen", true, None));
    if !recent.is_empty() {
        let _ = m.append(&PredefinedMenuItem::separator());
        for (id, name) in recent {
            // "&" marks a shortcut letter in Win32 menus.
            let mut label: String = name.replace('&', "&&").chars().take(48).collect();
            if name.chars().count() > 48 {
                label.push('…');
            }
            let _ = m.append(&MenuItem::with_id(format!("play:{id}"), label, true, None));
        }
    }
    let _ = m.append(&PredefinedMenuItem::separator());
    let _ = m.append(&MenuItem::with_id("quit", "Exit", true, None));
    m
}

/// Waits on a named event another gamut.exe sets (show the window / quit).
fn listen(event: &str, cmd: Cmd, tx: crossbeam_channel::Sender<Cmd>, ctx: egui::Context) {
    use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject, INFINITE};
    let name: Vec<u16> = event.encode_utf16().chain([0]).collect();
    // Auto-reset event; the handle lives as long as the process.
    let ev = unsafe { CreateEventW(std::ptr::null(), 0, 0, name.as_ptr()) } as usize;
    if ev == 0 {
        return;
    }
    std::thread::spawn(move || loop {
        if unsafe { WaitForSingleObject(ev as _, INFINITE) } != 0 {
            return;
        }
        let _ = tx.send(cmd);
        ctx.request_repaint();
    });
}

/// From a second launch: asks the running Gamut to show its window. False if none is listening.
pub fn signal_running() -> bool {
    signal(SHOW_EVENT)
}

/// `--quit`: asks the running Gamut to exit. False if none is listening.
pub fn signal_quit() -> bool {
    signal(QUIT_EVENT)
}

fn signal(event: &str) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenEventW, SetEvent, EVENT_MODIFY_STATE};
    let name: Vec<u16> = event.encode_utf16().chain([0]).collect();
    unsafe {
        let ev = OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr());
        if ev.is_null() {
            return false;
        }
        let ok = SetEvent(ev) != 0;
        CloseHandle(ev);
        ok
    }
}

/// Adds or removes Gamut from HKCU\...\Run, starting in the tray.
pub fn set_autostart(on: bool) -> anyhow::Result<()> {
    use windows_sys::Win32::System::Registry::{RegDeleteKeyValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};
    let w = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let key = w("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
    let name = w("Gamut");
    let r = if on {
        let exe = std::env::current_exe()?;
        let cmd = w(&format!("\"{}\" --tray", exe.display()));
        unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), REG_SZ, cmd.as_ptr().cast(), (cmd.len() * 2) as u32) }
    } else {
        match unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr()) } {
            2 => 0, // ERROR_FILE_NOT_FOUND: already off
            r => r,
        }
    };
    anyhow::ensure!(r == 0, "registry error {r}");
    log::info!("start with Windows {}", if on { "on" } else { "off" });
    Ok(())
}
