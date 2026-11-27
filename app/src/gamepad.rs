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
    /// LB / RB: previous/next tab.
    PrevTab,
    NextTab,
    /// View/Select (Xbox) or PS: toggle big screen.
    BigScreen,
    /// Start / Options (and Create on PlayStation): the game's menu.
    Menu,
    /// North (Y / Triangle).
    Search,
    /// Right stick held up (-1) / down (1), repeated while held.
    Scroll(i8),
}

/// Held-direction repeat: first repeat after `DELAY`, then every `RATE`.
const DELAY: Duration = Duration::from_millis(380);
const RATE: Duration = Duration::from_millis(95);
/// Right-stick scroll steps while held.
const SCROLL_RATE: Duration = Duration::from_millis(45);
/// After input from one device, others are ignored this long (a remapper's mirrored copy).
const LEAD: Duration = Duration::from_millis(400);
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
#[derive(Default, Clone, Copy, PartialEq, Debug)]
struct PadState {
    buttons: [bool; 11],
    dir: Option<Nav>,
    /// Right stick vertical: -1 up, 1 down.
    scroll: i8,
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
    Pad::Menu,
    Pad::BigScreen,
    Pad::Menu,
    Pad::Search,
];

/// One device's last state and when its held direction repeats next.
#[derive(Default)]
struct Src {
    state: PadState,
    due: Option<Instant>,
    scroll_due: Option<Instant>,
}

impl Src {
    /// Applies a new state; presses and new directions become actions.
    fn update(&mut self, new: PadState, now: Instant, out: &mut Vec<Pad>) {
        for (i, a) in ACTIONS.iter().enumerate() {
            if new.buttons[i] && !self.state.buttons[i] {
                out.push(*a);
            }
        }
        if new.dir != self.state.dir {
            self.due = new.dir.map(|d| {
                out.push(Pad::Nav(d));
                now + DELAY
            });
        }
        if new.scroll != self.state.scroll {
            self.scroll_due = (new.scroll != 0).then(|| {
                out.push(Pad::Scroll(new.scroll));
                now + SCROLL_RATE
            });
        }
        self.state = new;
    }

    /// Repeats a held direction when due.
    fn tick(&mut self, now: Instant, out: &mut Vec<Pad>) {
        if let (Some(d), Some(due)) = (self.state.dir, self.due) {
            if now >= due {
                out.push(Pad::Nav(d));
                self.due = Some(now + RATE);
            }
        }
        if let Some(due) = self.scroll_due.filter(|d| now >= *d && self.state.scroll != 0) {
            out.push(Pad::Scroll(self.state.scroll));
            self.scroll_due = Some(due.max(now - SCROLL_RATE) + SCROLL_RATE);
        }
    }
}

/// A remapper (DSX, DS4Windows, Steam) can show one controller twice (real + virtual). The device
/// that acts first leads; others are ignored until it has been quiet for `LEAD`.
#[derive(Default)]
struct Lead(Option<(u64, Instant)>);

impl Lead {
    fn accept(&mut self, src: u64, now: Instant) -> bool {
        if let Some((l, t)) = self.0 {
            if l != src && now - t < LEAD {
                return false;
            }
        }
        self.0 = Some((src, now));
        true
    }

    /// Keeps the lead while its device is still holding something.
    fn hold(&mut self, src: u64, now: Instant) {
        if self.0.is_some_and(|(l, _)| l == src) {
            self.0 = Some((src, now));
        }
    }
}

struct HidPad {
    dev: hidapi::HidDevice,
    path: CString,
    dualsense: bool,
    /// Connected over Bluetooth (with DSX, the real pad; its virtual copy is USB).
    bt: bool,
    src: Src,
    id: u64,
}

fn run(tx: Sender<Pad>, ctx: egui::Context) {
    let mut gil = Gilrs::new().map_err(|e| log::warn!("gilrs unavailable: {e}")).ok();
    let mut hid = hidapi::HidApi::new().map_err(|e| log::warn!("HID unavailable: {e}")).ok();
    let mut pads: Vec<HidPad> = Vec::new();
    let mut next_id = 1u64 << 32;
    let mut gil_src: std::collections::HashMap<usize, Src> = Default::default();
    let mut scanned = Instant::now() - Duration::from_secs(60);
    let mut lead = Lead::default();
    let mut out: Vec<(u64, Pad)> = Vec::new();
    loop {
        let now = Instant::now();
        // gilrs: button presses as events (no missed quick taps), direction from state.
        if let Some(g) = gil.as_mut() {
            while let Some(ev) = g.next_event() {
                if let EventType::ButtonPressed(b, _) = ev.event {
                    if let Some(p) = gilrs_action(b) {
                        out.push((usize::from(ev.id) as u64, p));
                    }
                }
            }
            for (id, p) in g.gamepads() {
                let s = gil_src.entry(id.into()).or_default();
                let mut v = Vec::new();
                let ry = p.value(Axis::RightStickY);
                let scroll = if ry > STICK { -1 } else if ry < -STICK { 1 } else { 0 };
                s.update(PadState { buttons: s.state.buttons, dir: gilrs_dir(&p), scroll }, now, &mut v);
                s.tick(now, &mut v);
                out.extend(v.into_iter().map(|a| (usize::from(id) as u64, a)));
                if s.state.dir.is_some() {
                    lead.hold(usize::from(id) as u64, now);
                }
            }
        }

        // HID PlayStation pads: rescan every 2 s for (re)connects; every report counts (quick taps).
        if let Some(api) = hid.as_mut() {
            if scanned.elapsed() > Duration::from_secs(2) {
                scanned = Instant::now();
                open_new(api, &mut pads, &mut next_id);
            }
            // DSX & co. mirror a Bluetooth pad as a virtual USB one: use only the virtual one (it has the
            // remapper's mappings), and read the real one just to keep its buffer drained.
            let mirrored = pads.iter().any(|p| p.bt) && pads.iter().any(|p| !p.bt);
            pads.retain_mut(|p| {
                let muted = mirrored && p.bt;
                let mut buf = [0u8; 128];
                let mut v = Vec::new();
                loop {
                    match p.dev.read_timeout(&mut buf, 0) {
                        Ok(0) => break,
                        Ok(n) => {
                            if let Some(s) = parse(&buf[..n], p.dualsense) {
                                p.src.update(s, now, &mut v);
                            }
                        }
                        Err(e) => {
                            log::info!("controller disconnected ({e})");
                            return false;
                        }
                    }
                }
                p.src.tick(now, &mut v);
                if muted {
                    return true;
                }
                out.extend(v.into_iter().map(|a| (p.id, a)));
                if p.src.state != PadState::default() {
                    lead.hold(p.id, now);
                }
                true
            });
        }

        for (src, p) in out.drain(..) {
            if lead.accept(src, now) {
                let _ = tx.send(p);
                ctx.request_repaint();
            }
        }
        let idle = pads.is_empty() && gil.as_ref().is_none_or(|g| g.gamepads().next().is_none());
        std::thread::sleep(Duration::from_millis(if idle { 250 } else { 8 }));
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
        Button::Start => Pad::Menu,
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
fn open_new(api: &mut hidapi::HidApi, pads: &mut Vec<HidPad>, next_id: &mut u64) {
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
            let bt = matches!(d.bus_type(), hidapi::BusType::Bluetooth);
            log::info!("controller connected: {} ({pid:04x}, {:?})", d.product_string().unwrap_or("PlayStation pad"), d.bus_type());
            *next_id += 1;
            pads.push(HidPad { dev, path: d.path().to_owned(), dualsense: DUALSENSE.contains(&pid), bt, src: Src::default(), id: *next_id });
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
    let ry = axis(b(3));
    let scroll = if ry < -STICK { -1 } else if ry > STICK { 1 } else { 0 };
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
    Some(PadState { buttons, dir, scroll })
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

    #[test]
    fn quick_taps_within_one_poll_all_count() {
        let (t, mut s, mut out) = (Instant::now(), Src::default(), Vec::new());
        let right = PadState { dir: Some(Nav::Right), ..Default::default() };
        let mut cross = PadState::default();
        cross.buttons[0] = true;
        for st in [right, PadState::default(), right, PadState::default(), cross, PadState::default(), cross] {
            s.update(st, t, &mut out);
        }
        assert_eq!(out, [Pad::Nav(Nav::Right), Pad::Nav(Nav::Right), Pad::Confirm, Pad::Confirm]);
    }

    #[test]
    fn mirrored_device_is_ignored_but_fast_taps_pass() {
        let (t, mut l) = (Instant::now(), Lead::default());
        let ms = |n| t + Duration::from_millis(n);
        assert!(l.accept(1, t)); // tap on the real pad
        assert!(!l.accept(2, ms(30))); // its lagged copy from the virtual pad
        assert!(l.accept(1, ms(60))); // second quick tap
        assert!(!l.accept(2, ms(90)));
        assert!(l.accept(2, ms(1000))); // other device after the first went quiet
    }

    #[test]
    fn held_direction_repeats() {
        let (t, mut s, mut out) = (Instant::now(), Src::default(), Vec::new());
        s.update(PadState { dir: Some(Nav::Down), ..Default::default() }, t, &mut out);
        s.tick(t + Duration::from_millis(100), &mut out);
        s.tick(t + DELAY, &mut out);
        s.tick(t + DELAY + RATE, &mut out);
        assert_eq!(out.len(), 3);
    }
}
