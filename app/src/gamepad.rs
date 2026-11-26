//! Controller input (gilrs: XInput, DualShock/DualSense, …) turned into UI actions on a background thread.
use crate::ui::Nav;
use crossbeam_channel::{unbounded, Receiver, Sender};
use gilrs::{Axis, Button, EventType, Gilrs};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug)]
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
    /// View/Select (or Guide): toggle big screen.
    BigScreen,
    Start,
}

/// Held-direction repeat: first repeat after `DELAY`, then every `RATE`.
const DELAY: Duration = Duration::from_millis(380);
const RATE: Duration = Duration::from_millis(95);
const STICK: f32 = 0.55;

pub fn start(ctx: egui::Context) -> Receiver<Pad> {
    let (tx, rx) = unbounded();
    std::thread::Builder::new()
        .name("gamepad".into())
        .spawn(move || run(tx, ctx))
        .ok();
    rx
}

fn run(tx: Sender<Pad>, ctx: egui::Context) {
    let mut g = match Gilrs::new() {
        Ok(g) => g,
        Err(e) => {
            log::warn!("controller support unavailable: {e}");
            return;
        }
    };
    // A remapper (DS4Windows, DSX, Steam) can show one controller twice: real + virtual Xbox pad.
    // The same button within 60 ms counts once.
    let mut last: Option<(std::mem::Discriminant<Pad>, Instant)> = None;
    let mut send = |p: Pad| {
        let (key, now) = (std::mem::discriminant(&p), Instant::now());
        // Held-direction repeats are 95 ms apart, so they pass.
        if last.is_some_and(|(k, t)| k == key && now - t < Duration::from_millis(60)) {
            return;
        }
        last = Some((key, now));
        let _ = tx.send(p);
        ctx.request_repaint();
    };
    // Current held direction and when it next repeats.
    let mut held: Option<(Nav, Instant)> = None;
    loop {
        let wait = if held.is_some() { Duration::from_millis(16) } else { Duration::from_millis(500) };
        while let Some(ev) = g.next_event_blocking(Some(wait)) {
            if let EventType::ButtonPressed(b, _) = ev.event {
                let p = match b {
                    Button::South => Some(Pad::Confirm),
                    Button::East => Some(Pad::Back),
                    Button::West => Some(Pad::Favorite),
                    Button::LeftTrigger => Some(Pad::PrevTab),
                    Button::RightTrigger => Some(Pad::NextTab),
                    Button::LeftTrigger2 => Some(Pad::Nav(Nav::PageUp)),
                    Button::RightTrigger2 => Some(Pad::Nav(Nav::PageDown)),
                    Button::Select | Button::Mode => Some(Pad::BigScreen),
                    Button::Start => Some(Pad::Start),
                    _ => None,
                };
                if let Some(p) = p {
                    send(p);
                }
            }
            if !matches!(ev.event, EventType::AxisChanged(..) | EventType::ButtonPressed(..) | EventType::ButtonReleased(..)) {
                continue;
            }
            break;
        }
        // Direction from any connected pad: D-pad first, then the left stick.
        let dir = g.gamepads().find_map(|(_, p)| {
            let btn = [
                (Button::DPadUp, Nav::Up),
                (Button::DPadDown, Nav::Down),
                (Button::DPadLeft, Nav::Left),
                (Button::DPadRight, Nav::Right),
            ]
            .into_iter()
            .find(|(b, _)| p.is_pressed(*b))
            .map(|(_, n)| n);
            btn.or_else(|| {
                let (x, y) = (p.value(Axis::LeftStickX), p.value(Axis::LeftStickY));
                if x.abs() < STICK && y.abs() < STICK {
                    None
                } else if x.abs() > y.abs() {
                    Some(if x > 0.0 { Nav::Right } else { Nav::Left })
                } else {
                    Some(if y > 0.0 { Nav::Up } else { Nav::Down })
                }
            })
        });
        let now = Instant::now();
        held = match (dir, held) {
            (None, _) => None,
            // Same direction still held: repeat when due.
            (Some(d), Some((h, due))) if same(d, h) => {
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
    }
}

fn same(a: Nav, b: Nav) -> bool {
    std::mem::discriminant(&a) == std::mem::discriminant(&b)
}
