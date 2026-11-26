//! Controller input turned into UI actions on a background thread.
//! PlayStation pads are read over HID (like games do; Windows.Gaming.Input doesn't see DSX's virtual
//! DualSense); everything else (Xbox/XInput, …) through gilrs.
use crate::ui::Nav;
use crossbeam_channel::{unbounded, Receiver, Sender};
use gilrs::{Axis, Button, EventType, Gilrs};
use std::ffi::CString;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pad {
    Nav(Nav),
    /// South (A / Cross).
    Confirm,
    /// East (B / Circle).
    Back,
    /// West (X / Square).
    Favorite,
    /// LB / RB: previous/next filter.
    PrevTab,
    NextTab,
    /// View/Select (Create) or Guide (PS): toggle big screen.
    BigScreen,
    Start,
    /// North (Y / Triangle).
    Search,
}

/// Held-direction repeat: first repeat after `DELAY`, then every `RATE`.
const DELAY: Duration = Duration::from_millis(380);
const RATE: Duration = Duration::from_millis(95);
const STICK: f32 = 0.55;
const SONY: u16 = 0x054c;
/// DualSense, DualSense Edge.
const DUALSENSE: [u16; 2] = [0x0ce6, 0x0df2];
/// DualShock 4 (v1, v2), its USB wireless adapter.
const DUALSHOCK4: [u16; 3] = [0x05c4, 0x09cc, 0x0ba0];

pub fn start(ctx: egui::Context) -> Receiver<Pad> {
    let (tx, rx) = unbounded();
    std::thread::Builder::new()
        .name("gamepad".into())
        .spawn(move || run(tx, ctx))
        .ok();
    rx
}

/// Pressed buttons that map to actions, plus the direction a pad is pushing.
#[derive(Default, Clone, Copy, PartialEq)]
struct PadState {
    buttons: [bool; 11],
    dir: Option<Nav>,
}

/// Order of `PadState::buttons`.
const ACTIONS: [Pad; 11] = [
    Pad::Confirm,
    Pad::Back,
    Pad::Favorite,
    Pad::PrevTab,
    Pad::NextTab,
    Pad::Nav(Nav::PageUp),
    Pad::Nav(Nav::PageDown),
    Pad::BigScreen,
    Pad::BigScreen,
    Pad::Start,
    Pad::Search,
];

struct HidPad {
    dev: hidapi::HidDevice,
    path: CString,
    dualsense: bool,
    state: PadState,
}

fn run(tx: Sender<Pad>, ctx: egui::Context) {
    let mut gil = Gilrs::new().map_err(|e| log::warn!("gilrs unavailable: {e}")).ok();
    let mut hid = hidapi::HidApi::new().map_err(|e| log::warn!("HID unavailable: {e}")).ok();
    let mut pads: Vec<HidPad> = Vec::new();
    let mut scanned = Instant::now() - Duration::from_secs(60);

    // A remapper (DSX, DS4Windows, Steam) can show one controller twice (real + virtual);
    // the same action within 60 ms counts once. Held-direction repeats are 95 ms apart.
    let mut last: Option<(Pad, Instant)> = None;
    let mut send = |p: Pad| {
        let now = Instant::now();
        if last.is_some_and(|(q, t)| q == p && now - t < Duration::from_millis(60)) {
            return;
        }
        last = Some((p, now));
        let _ = tx.send(p);
        ctx.request_repaint();
    };
    let mut held: Option<(Nav, Instant)> = None;
    loop {
        // gilrs: button presses as events, direction from state.
        let mut dir = None;
        if let Some(g) = gil.as_mut() {
            while let Some(ev) = g.next_event() {
                if let EventType::ButtonPressed(b, _) = ev.event {
                    if let Some(p) = gilrs_action(b) {
                        send(p);
                    }
                }
            }
            dir = g.gamepads().find_map(|(_, p)| gilrs_dir(&p));
        }

        // HID PlayStation pads: rescan every 2 s for (re)connects.
        if let Some(api) = hid.as_mut() {
            if scanned.elapsed() > Duration::from_secs(2) {
                scanned = Instant::now();
                open_new(api, &mut pads);
            }
            pads.retain_mut(|p| {
                let before = p.state;
                let mut buf = [0u8; 128];
                loop {
                    match p.dev.read_timeout(&mut buf, 0) {
                        Ok(0) => break,
                        Ok(n) => {
                            if let Some(s) = parse(&buf[..n], p.dualsense) {
                                p.state = s;
                            }
                        }
                        Err(e) => {
                            log::info!("controller disconnected ({e})");
                            return false;
                        }
                    }
                }
                for (i, a) in ACTIONS.iter().enumerate() {
                    if p.state.buttons[i] && !before.buttons[i] {
                        send(*a);
                    }
                }
                true
            });
            dir = dir.or_else(|| pads.iter().find_map(|p| p.state.dir));
        }

        let now = Instant::now();
        held = match (dir, held) {
            (None, _) => None,
            // Same direction still held: repeat when due.
            (Some(d), Some((h, due))) if d == h => {
                if now >= due {
                    send(Pad::Nav(d));
                    Some((d, now + RATE))
                } else {
                    Some((h, due))
                }
            }
            (Some(d), _) => {
                send(Pad::Nav(d));
                Some((d, now + DELAY))
            }
        };
        let idle = pads.is_empty() && gil.as_ref().is_none_or(|g| g.gamepads().next().is_none());
        std::thread::sleep(Duration::from_millis(if idle { 250 } else { 12 }));
    }
}

fn gilrs_action(b: Button) -> Option<Pad> {
    Some(match b {
        Button::South => Pad::Confirm,
        Button::East => Pad::Back,
        Button::West => Pad::Favorite,
        Button::North => Pad::Search,
        Button::LeftTrigger => Pad::PrevTab,
        Button::RightTrigger => Pad::NextTab,
        Button::LeftTrigger2 => Pad::Nav(Nav::PageUp),
        Button::RightTrigger2 => Pad::Nav(Nav::PageDown),
        Button::Select | Button::Mode => Pad::BigScreen,
        Button::Start => Pad::Start,
        _ => return None,
    })
}

/// D-pad first, then the left stick.
fn gilrs_dir(p: &gilrs::Gamepad) -> Option<Nav> {
    let dpad = [
        (Button::DPadUp, Nav::Up),
        (Button::DPadDown, Nav::Down),
        (Button::DPadLeft, Nav::Left),
        (Button::DPadRight, Nav::Right),
    ];
    dpad.into_iter()
        .find(|(b, _)| p.is_pressed(*b))
        .map(|(_, n)| n)
        .or_else(|| stick_dir(p.value(Axis::LeftStickX), p.value(Axis::LeftStickY)))
}

/// `y` up is positive.
fn stick_dir(x: f32, y: f32) -> Option<Nav> {
    if x.abs() < STICK && y.abs() < STICK {
        None
    } else if x.abs() > y.abs() {
        Some(if x > 0.0 { Nav::Right } else { Nav::Left })
    } else {
        Some(if y > 0.0 { Nav::Up } else { Nav::Down })
    }
}

/// Opens Sony pads not open yet (the gamepad interface only; others are audio/touch).
fn open_new(api: &mut hidapi::HidApi, pads: &mut Vec<HidPad>) {
    if api.refresh_devices().is_err() {
        return;
    }
    for d in api.device_list() {
        let pid = d.product_id();
        let known = DUALSENSE.contains(&pid) || DUALSHOCK4.contains(&pid);
        if d.vendor_id() != SONY || !known || d.usage_page() != 1 || d.usage() != 5 {
            continue;
        }
        if pads.iter().any(|p| p.path.as_c_str() == d.path()) {
            continue;
        }
        if let Ok(dev) = d.open_device(api) {
            log::info!("controller connected: {} ({pid:04x})", d.product_string().unwrap_or("PlayStation pad"));
            pads.push(HidPad { dev, path: d.path().to_owned(), dualsense: DUALSENSE.contains(&pid), state: PadState::default() });
        }
    }
}

/// Decodes an input report. DualSense USB 0x01 (64 bytes) and BT 0x31 share one layout; DualShock 4
/// USB 0x01 / BT 0x11 and the DualSense's short BT 0x01 report share the other.
fn parse(r: &[u8], dualsense: bool) -> Option<PadState> {
    // (offset of the left stick X, DualSense layout?)
    let (base, ds) = match (r.first()?, dualsense) {
        (0x01, true) if r.len() >= 64 => (1, true),
        (0x31, true) => (2, true),
        (0x01, _) => (1, false),
        (0x11, false) => (3, false),
        _ => return None,
    };
    if r.len() < base + if ds { 10 } else { 7 } {
        return None;
    }
    let b = |i: usize| r[base + i];
    // Face buttons + hat, shoulders/menu, PS button.
    let (face, shoulder, ps) = if ds { (b(7), b(8), b(9)) } else { (b(4), b(5), b(6)) };
    let dpad = match face & 0x0f {
        0 | 1 | 7 => Some(Nav::Up),
        3..=5 => Some(Nav::Down),
        2 => Some(Nav::Right),
        6 => Some(Nav::Left),
        _ => None,
    };
    let axis = |v: u8| (v as f32 - 128.0) / 128.0;
    let dir = dpad.or_else(|| stick_dir(axis(b(0)), -axis(b(1))));
    let buttons = [
        face & 0x20 != 0,     // Cross
        face & 0x40 != 0,     // Circle
        face & 0x10 != 0,     // Square
        shoulder & 0x01 != 0, // L1
        shoulder & 0x02 != 0, // R1
        shoulder & 0x04 != 0, // L2
        shoulder & 0x08 != 0, // R2
        shoulder & 0x10 != 0, // Create / Share
        ps & 0x01 != 0,       // PS
        shoulder & 0x20 != 0, // Options
        face & 0x80 != 0,     // Triangle
    ];
    Some(PadState { buttons, dir })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dualsense_usb_report() {
        let mut r = [0u8; 64];
        r[0] = 0x01;
        (r[1], r[2]) = (128, 0); // left stick fully up
        r[8] = 0x08 | 0x20; // hat released, Cross
        r[9] = 0x02; // R1
        let s = parse(&r, true).unwrap();
        assert_eq!(s.dir, Some(Nav::Up));
        assert!(s.buttons[0] && s.buttons[4] && !s.buttons[1]);
    }

    #[test]
    fn dualshock_bt_report() {
        let mut r = [0u8; 78];
        r[0] = 0x11;
        (r[3], r[4]) = (128, 128);
        r[7] = 0x02 | 0x40; // hat right, Circle
        let s = parse(&r, false).unwrap();
        assert_eq!(s.dir, Some(Nav::Right));
        assert!(s.buttons[1]);
    }
}
