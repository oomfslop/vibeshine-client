//! The mouse over desk windows, sent to the host at the window's host position.
//!
//! A press on a window's title bar is held until the host says what is
//! there (its own hit test, so custom title bars, tabs and caption buttons
//! behave as the app defines them): a drag area starts a native drag when the
//! mouse moves (a click if it does not), caption buttons act on release.
//! A press on a spot another of our windows still covers on the host waits
//! until the host has raised the pressed one.

use std::cell::{Cell, RefCell};

use objc2_app_kit::{NSEvent, NSEventPhase, NSTouchPhase, NSView, NSWindow};
use objc2::msg_send;
use objc2_foundation::{NSPoint, NSSize, NSUserDefaults};

use crate::desk::{self, desk, win};
use crate::ffi::{self, info};

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Ev {
    Move,
    Drag,
    Down(i32),
    Up(i32),
    Scroll,
}

#[derive(Clone, Debug, PartialEq)]
enum State {
    /// Waiting for the host's hit test
    AwaitHit,
    /// A title bar press: a drag once the mouse moves, a click if not
    Caption { dragging: bool },
    /// Minimize, maximize or close, done on release over the button
    Control(String),
    /// Waiting for the host to raise the window before the click
    AwaitRaise,
    /// The host has the press
    Delivered,
    /// Handled here (title-bar double click): the release goes nowhere
    Swallowed,
}

struct Press {
    hwnd: u64,
    button: i32,
    /// Window-relative host px
    x: i32,
    y: i32,
    clicks: i32,
    token: u64,
    state: State,
    released: Option<(i32, i32)>,
    since: f64,
    /// Cmd held: the host gets a Ctrl-click
    ctrl: bool,
}

thread_local! {
    static PRESS: RefCell<Option<Press>> = const { RefCell::new(None) };
    static NEXT_TOKEN: Cell<u64> = const { Cell::new(1) };
}

fn with_press<R>(f: impl FnOnce(&mut Option<Press>) -> R) -> R {
    PRESS.with(|p| f(&mut p.borrow_mut()))
}

/// Where a window-relative host point is on the desk
fn desk_point(hwnd: u64, x: i32, y: i32) -> Option<(i32, i32)> {
    let w = win(hwnd)?;
    let i = w.info.borrow();
    Some((i.x + x, i.y + y))
}

fn move_to(hwnd: u64, x: i32, y: i32) {
    let Some(d) = desk() else { return };
    // The host holds the pointer: buttons and wheel only, where it is
    if crate::desk::pointer_locked() {
        return;
    }
    if let Some((px, py)) = desk_point(hwnd, x, y) {
        ffi::mouse_to(px, py, d.map.desk_w, d.map.desk_h);
    }
}

fn press_on_host(hwnd: u64, button: i32, x: i32, y: i32) {
    move_to(hwnd, x, y);
    if with_press(|p| p.as_ref().map(|p| p.ctrl).unwrap_or(false)) {
        ffi::host_ctrl(true);
    }
    ffi::mouse_button(true, button);
}

fn release_on_host(hwnd: u64, button: i32, x: i32, y: i32) {
    move_to(hwnd, x, y);
    ffi::mouse_button(false, button);
    if with_press(|p| p.as_ref().map(|p| p.ctrl).unwrap_or(false)) {
        ffi::host_ctrl(false);
    }
}

/// Tracking areas report moves even where another window (another app's
/// included) covers this one: the host must not see a hover there.
fn pointer_over(hwnd: u64) -> bool {
    let (Some(d), Some(w)) = (desk(), win(hwnd)) else { return false };
    NSWindow::windowNumberAtPoint_belowWindowWithWindowNumber(NSEvent::mouseLocation(), 0, d.mtm) == w.ns().windowNumber()
}

pub fn on_event(hwnd: u64, ev: Ev, p: NSPoint, clicks: i32, sx: f64, sy: f64, cmd: bool) {
    let Some(d) = desk() else { return };
    let x = (p.x / d.map.ppp).round() as i32;
    let y = (p.y / d.map.ppp).round() as i32;
    match ev {
        Ev::Move | Ev::Drag => moved(hwnd, ev, x, y),
        Ev::Down(button) => down(hwnd, button, x, y, clicks, cmd),
        Ev::Up(button) => up(hwnd, button, x, y),
        Ev::Scroll => {
            if !pointer_over(hwnd) {
                return;
            }
            move_to(hwnd, x, y);
            scroll_by(sx, sy);
        }
    }
}

thread_local! {
    static SCROLL_REST: Cell<(f64, f64)> = const { Cell::new((0.0, 0.0)) };
    static TRACKPAD: Cell<Trackpad> = const { Cell::new(Trackpad { down: false, gesture: false }) };
}

#[derive(Clone, Copy)]
struct Trackpad {
    down: bool,     // two fingers on it, going to the host
    gesture: bool,  // the last scroll gesture went as fingers (its momentum too)
}

/// Two fingers on the trackpad over a desk window go to the host as they
/// are, on a touchpad of the same size: Windows pans, zooms and coasts with
/// its own touchpad handling, exactly as with a touchpad of its own. One
/// finger is the pointer; three and more are macOS's gestures.
pub fn touches(hwnd: u64, e: &NSEvent, view: &NSView) {
    if !ffi::trackpad_supported() || crate::desk::pointer_locked() {
        return;
    }
    let set = e.touchesMatchingPhase_inView(NSTouchPhase::Touching, Some(view));
    let touches: Vec<_> = set.allObjects().iter().collect();
    let mut t = TRACKPAD.with(|t| t.get());
    if touches.len() == 2 && (t.down || pointer_over(hwnd)) {
        // As the fingers are on the touchpad, y down; with macOS's natural
        // scrolling off, as if turned around (Windows' default is natural)
        let natural = natural_scrolling();
        let mut contacts = [ffi::TrackpadContact { id: 0, x: 0.0, y: 0.0 }; 2];
        let mut size = NSSize::new(0.0, 0.0);
        for (c, touch) in contacts.iter_mut().zip(&touches) {
            let p = touch.normalizedPosition();
            let (x, y) = if natural { (p.x, 1.0 - p.y) } else { (1.0 - p.x, p.y) };
            // The same finger has an equal identity from event to event
            let identity = touch.identity();
            let id: usize = unsafe { msg_send![&*identity, hash] };
            *c = ffi::TrackpadContact { id: id as u32, x: x as f32, y: y as f32 };
            size = touch.deviceSize();
        }
        // (device size in points: 72 to the inch)
        let mm = |pt: f64| (pt * 25.4 / 72.0) as f32;
        if !t.down {
            move_to_pointer(hwnd);
        }
        ffi::trackpad(&contacts, mm(size.width), mm(size.height), e.timestamp());
        t = Trackpad { down: true, gesture: true };
    } else if t.down {
        ffi::trackpad(&[], 0.0, 0.0, e.timestamp());
        t.down = false;
    }
    TRACKPAD.with(|c| c.set(t));
}

/// Is this scroll event the trackpad's, already on the host as fingers
/// (with the host's own momentum after)?
pub fn trackpad_scroll(e: &NSEvent) -> bool {
    if !ffi::trackpad_supported() || !e.hasPreciseScrollingDeltas() {
        return false;
    }
    let mut t = TRACKPAD.with(|t| t.get());
    let momentum = !e.momentumPhase().is_empty();
    if !momentum && e.phase().contains(NSEventPhase::Began) && !t.down {
        // A scroll gesture without fingers coming here (a Magic Mouse)
        t.gesture = false;
        TRACKPAD.with(|c| c.set(t));
    }
    t.down || (momentum && t.gesture)
}

fn natural_scrolling() -> bool {
    let defaults = NSUserDefaults::standardUserDefaults();
    let key = objc2_foundation::ns_string!("com.apple.swipescrolldirection");
    defaults.objectForKey(key).is_none() || defaults.boolForKey(key)
}

/// The host's pointer where the Mac's is (a touchpad gesture goes where the
/// pointer is)
fn move_to_pointer(hwnd: u64) {
    let (Some(d), Some(w)) = (desk(), win(hwnd)) else { return };
    let m = NSEvent::mouseLocation();
    let f = w.ns().frame();
    let (x, y) = (m.x - f.origin.x, f.origin.y + f.size.height - m.y);
    move_to(hwnd, (x / d.map.ppp).round() as i32, (y / d.map.ppp).round() as i32);
}

/// Trackpad and wheel deltas (points, as macOS gives them, momentum included)
/// as high-resolution wheel deltas (120 a notch): every fraction kept for the
/// next event, so slow scrolls move smoothly instead of in steps, and at most
/// four notches an event (acceleration gives wild deltas on fast flicks).
/// Windows apps that scroll smoothly (browsers, WinUI) take these as is.
fn scroll_by(sx: f64, sy: f64) {
    let (rx, ry) = SCROLL_REST.with(|r| r.get());
    let x = (rx + sx * 12.0).clamp(-480.0, 480.0);
    let y = (ry + sy * 12.0).clamp(-480.0, 480.0);
    let (ix, iy) = (x.trunc(), y.trunc());
    SCROLL_REST.with(|r| r.set((x - ix, y - iy)));
    if ix != 0.0 || iy != 0.0 {
        ffi::scroll(iy as i16, ix as i16);
    }
}

fn moved(hwnd: u64, ev: Ev, x: i32, y: i32) {
    // A title bar press: the mouse moves the Mac window, not the host's pointer
    let start_drag = with_press(|p| match p {
        Some(press) if press.hwnd == hwnd => match press.state {
            State::Caption { dragging: false } if (x - press.x).abs() + (y - press.y).abs() > 8 => {
                press.state = State::Caption { dragging: true };
                Some(true)
            }
            State::Caption { .. } | State::AwaitHit | State::Control(_) => Some(false),
            _ => None,
        },
        _ => None,
    });
    match start_drag {
        Some(true) => {
            desk::begin_drag(hwnd);
            return;
        }
        Some(false) => return,
        None => {}
    }
    // While a button is held the pointer belongs to the pressed window (a
    // selection dragged out of it); otherwise only where it is in front
    if ev == Ev::Move && !pointer_over(hwnd) {
        return;
    }
    move_to(hwnd, x, y);
    if let Some(d) = desk() {
        desk::show_cursor(&d);
    }
}

fn down(hwnd: u64, button: i32, x: i32, y: i32, clicks: i32, cmd: bool) {
    let Some(w) = win(hwnd) else { return };
    desk::note_press(hwnd);
    let info = w.info();
    // A press that never got its release (a drag the system swallowed): forget it
    with_press(|p| *p = None);
    let token = NEXT_TOKEN.with(|t| {
        let v = t.get();
        t.set(v + 1);
        v
    });
    let press = Press { hwnd, button, x, y, clicks, token, state: State::AwaitHit, released: None, since: ffi::now(), ctrl: cmd };
    if w.pane && button == 1 && y < info.caption.max(96) {
        with_press(|p| *p = Some(press));
        if !ffi::hittest(hwnd, x, y, token) {
            hit_result(token, "client");
        }
        return;
    }
    with_press(|p| *p = Some(press));
    deliver(hwnd);
}

/// Send the held press to the host, once the spot is the pressed window's there
fn deliver(hwnd: u64) {
    let Some(d) = desk() else { return };
    let Some(w) = win(hwnd) else { return };
    let Some((button, x, y, released)) = with_press(|p| p.as_ref().filter(|p| p.hwnd == hwnd).map(|p| (p.button, p.x, p.y, p.released))) else {
        return;
    };
    let (px, py) = desk_point(hwnd, x, y).unwrap_or((0, 0));
    let waited = with_press(|p| p.as_ref().map(|p| ffi::now() - p.since).unwrap_or(0.0));
    if w.pane && desk::covered_on_host(&d, hwnd, px, py) && waited < 0.25 {
        // On the Mac this window is in front here; on the host another of
        // ours still covers the spot. Raised there first (the order push),
        // then clicked.
        with_press(|p| {
            if let Some(p) = p.as_mut() {
                p.state = State::AwaitRaise;
            }
        });
        d.force_order_push.set(true);
        desk::sync_order(&d);
        ffi::on_main_after(0.26, move || {
            ffi::guard("raise timeout", || retry_raise(hwnd));
        });
        return;
    }
    press_on_host(hwnd, button, x, y);
    with_press(|p| {
        if let Some(p) = p.as_mut() {
            p.state = State::Delivered;
        }
    });
    if let Some((rx, ry)) = released {
        release_on_host(hwnd, button, rx, ry);
        with_press(|p| *p = None);
    }
}

fn retry_raise(hwnd: u64) {
    let waiting = with_press(|p| p.as_ref().map(|p| p.hwnd == hwnd && p.state == State::AwaitRaise).unwrap_or(false));
    if waiting {
        deliver(hwnd);
    }
}

/// A new layout: a press waiting for its window to be raised may go now
pub fn layout_changed() {
    let waiting = with_press(|p| p.as_ref().filter(|p| p.state == State::AwaitRaise).map(|p| p.hwnd));
    if let Some(hwnd) = waiting {
        deliver(hwnd);
    }
}

pub fn hit_result(token: u64, hit: &str) {
    let Some((hwnd, clicks, released, x, y, button)) = with_press(|p| {
        p.as_ref().filter(|p| p.token == token && p.state == State::AwaitHit).map(|p| (p.hwnd, p.clicks, p.released, p.x, p.y, p.button))
    }) else {
        return;
    };
    match hit {
        "drag" => {
            if clicks >= 2 {
                desk::title_action(hwnd);
                with_press(|p| *p = if released.is_some() { None } else { p.take().map(|mut p| { p.state = State::Swallowed; p }) });
                return;
            }
            if released.is_some() {
                // Released before the answer came: a click (tabs, custom title bars)
                press_on_host(hwnd, button, x, y);
                release_on_host(hwnd, button, x, y);
                with_press(|p| *p = None);
                return;
            }
            with_press(|p| {
                if let Some(p) = p.as_mut() {
                    p.state = State::Caption { dragging: false };
                }
            });
            // The mouse may have moved far already while the host answered
            let m = NSEvent::pressedMouseButtons();
            if m & 1 == 0 {
                with_press(|p| *p = None);
            }
        }
        "min" | "max" | "close" => {
            with_press(|p| {
                if let Some(p) = p.as_mut() {
                    p.state = State::Control(hit.to_string());
                }
            });
            if let Some((rx, ry)) = released {
                control_released(hwnd, rx, ry);
            }
        }
        _ => {
            with_press(|p| {
                if let Some(p) = p.as_mut() {
                    p.state = State::Delivered;
                }
            });
            deliver(hwnd);
        }
    }
}

fn control_released(hwnd: u64, x: i32, y: i32) {
    let Some((hit, px, py)) = with_press(|p| {
        p.take().and_then(|p| match p.state {
            State::Control(hit) => Some((hit, p.x, p.y)),
            _ => None,
        })
    }) else {
        return;
    };
    // Like a button: only if the release is still on it
    if (x - px).abs() + (y - py).abs() > 12 {
        return;
    }
    match hit.as_str() {
        "min" => desk::minimize(hwnd),
        "max" => desk::zoom(hwnd),
        _ => {
            info!("deskwm: close 0x{hwnd:x}");
            ffi::window_op(hwnd, "close", 0, 0, 0, 0);
            ffi::event(&format!("{{\"kind\":\"close\",\"hwnd\":\"0x{hwnd:x}\"}}"));
        }
    }
}

fn up(hwnd: u64, button: i32, x: i32, y: i32) {
    let state = with_press(|p| p.as_ref().filter(|p| p.hwnd == hwnd && p.button == button).map(|p| p.state.clone()));
    match state {
        None => {
            // A release without a press of ours (the press began elsewhere)
            release_on_host(hwnd, button, x, y);
        }
        Some(State::AwaitHit) | Some(State::AwaitRaise) => with_press(|p| {
            if let Some(p) = p.as_mut() {
                p.released = Some((x, y));
            }
        }),
        Some(State::Caption { dragging: false }) => {
            let (px, py) = with_press(|p| p.as_ref().map(|p| (p.x, p.y)).unwrap_or((x, y)));
            press_on_host(hwnd, button, px, py);
            release_on_host(hwnd, button, px, py);
            with_press(|p| *p = None);
        }
        Some(State::Caption { dragging: true }) => {}  // the drag's end comes from polling
        Some(State::Control(_)) => control_released(hwnd, x, y),
        Some(State::Delivered) => {
            release_on_host(hwnd, button, x, y);
            with_press(|p| *p = None);
        }
        Some(State::Swallowed) => with_press(|p| *p = None),
    }
}

pub fn drag_ended(hwnd: u64) {
    with_press(|p| {
        if p.as_ref().map(|p| p.hwnd == hwnd).unwrap_or(false) {
            *p = None;
        }
    });
}

// --- Test hooks ---------------------------------------------------------------------------

/// A press and release at window-relative host px, as the user would make them
pub fn test_click(hwnd: u64, x: i32, y: i32, button: i32) {
    down(hwnd, button, x, y, 1, false);
    ffi::on_main_after(0.03, move || {
        ffi::guard("test click", || up(hwnd, button, x, y));
    });
}

/// A press, a drag of (dx, dy) in 20 steps (window-relative host px, may
/// leave the window) and the release, through the same path as real events
pub fn test_mouse_drag(hwnd: u64, x: i32, y: i32, dx: i32, dy: i32) {
    down(hwnd, 1, x, y, 1, false);
    for k in 1..=20 {
        ffi::on_main_after(0.1 + k as f64 * 0.03, move || {
            ffi::guard("test drag", || moved(hwnd, Ev::Drag, x + dx * k / 20, y + dy * k / 20));
        });
    }
    ffi::on_main_after(0.9, move || {
        ffi::guard("test drag up", || up(hwnd, 1, x + dx, y + dy));
    });
}

pub fn test_scroll(hwnd: u64, x: i32, y: i32, dy: f64, n: i32) {
    for k in 0..n.max(1) {
        ffi::on_main_after(k as f64 * 0.016, move || {
            move_to(hwnd, x, y);
            scroll_by(0.0, dy);
        });
    }
}
