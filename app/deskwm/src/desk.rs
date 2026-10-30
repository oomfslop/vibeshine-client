//! The desk: one Mac window per host window, kept in step with the host.
//!
//! Pictures come from the atlas (each window in a slot of its own), so they
//! never depend on where windows are or how they overlap. Positions and
//! stacking still mirror between Mac and host, for input and for where
//! Windows opens menus and dialogs:
//!
//! - Positions: the Mac is the truth for what the user does (drags, tiling);
//!   each change goes to the host when it is over. Otherwise the host is the
//!   truth (an app moving its window). Sizes are the host's, except while the
//!   user resizes: then the host gets the size and the picture follows.
//! - Stacking and focus: the side that changed it wins. User actions on the
//!   Mac (clicks, Cmd-Tab, Cmd-`, Mission Control) go to the host; what the
//!   host does by itself (an app raising a window, a dialog taking focus)
//!   comes to the Mac, among our own windows only.
//!
//! Everything here runs on the main thread. Interior mutability is kept to
//! short borrows: AppKit calls back into us synchronously (a setFrame calls
//! the delegate), so no borrow is ever held across an AppKit call.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::{
    NSApplication, NSColor, NSCursor, NSCursorFrameResizeDirections, NSCursorFrameResizePosition, NSEvent,
    NSEventModifierFlags, NSEventType, NSImage, NSScreen, NSWindow,
    NSWindowCollectionBehavior, NSWindowOcclusionState, NSWindowOrderingMode, NSWindowStyleMask,
};
use objc2_foundation::{NSData, NSPoint, NSProcessInfo, NSRect, NSSize, NSString, NSUserDefaults};

use crate::classes::{Delegate, DeskView, DeskWindow};
use crate::ffi::{self, info, Geo};
use crate::layout::Info;

/// How long a change of ours may take to show on the host before the host's
/// state is believed again.
const PENDING_GRACE: f64 = 1.5;

#[derive(Copy, Clone)]
pub struct Mapping {
    /// The mirrored screen in global points, top-left origin
    pub sx: f64,
    pub sy: f64,
    pub sw: f64,
    pub sh: f64,
    /// Points per host pixel (one host pixel per device pixel)
    pub ppp: f64,
    pub desk_w: i32,
    pub desk_h: i32,
}

impl Mapping {
    fn primary_top(mtm: MainThreadMarker) -> f64 {
        NSScreen::screens(mtm)
            .firstObject()
            .map(|s| {
                let f = s.frame();
                f.origin.y + f.size.height
            })
            .unwrap_or(0.0)
    }

    /// Desk px (top-left origin) to a Cocoa frame (bottom-left origin)
    pub fn to_cocoa(&self, mtm: MainThreadMarker, x: i32, y: i32, w: i32, h: i32) -> NSRect {
        let top = Self::primary_top(mtm);
        let s = self.ppp;
        NSRect::new(
            NSPoint::new(self.sx + x as f64 * s, top - (self.sy + (y + h) as f64 * s)),
            NSSize::new(w as f64 * s, h as f64 * s),
        )
    }

    pub fn to_desk(&self, mtm: MainThreadMarker, r: NSRect) -> (i32, i32, i32, i32) {
        let top = Self::primary_top(mtm);
        let s = self.ppp;
        (
            ((r.origin.x - self.sx) / s).round() as i32,
            ((top - (r.origin.y + r.size.height) - self.sy) / s).round() as i32,
            (r.size.width / s).round() as i32,
            (r.size.height / s).round() as i32,
        )
    }

    /// A point in global Cocoa coordinates from top-left global points
    pub fn cocoa_point(&self, mtm: MainThreadMarker, x: f64, y: f64) -> NSPoint {
        NSPoint::new(x, Self::primary_top(mtm) - y)
    }
}

pub struct Win {
    pub hwnd: u64,
    pub pane: bool,
    pub window: Retained<DeskWindow>,
    _view: Retained<DeskView>,
    _delegate: Retained<Delegate>,
    pub info: RefCell<Info>,
    /// We are changing the window: its notifications are not the user's
    pub applying: Cell<bool>,
    pub gone_since: Cell<f64>,
    pub user_moving: Cell<bool>,
    move_generation: Cell<u64>,
    pub dragging: Cell<bool>,
    pub in_resize: Cell<bool>,
    settling: Cell<bool>,
    settle_deadline: Cell<f64>,
    pending_move: Cell<bool>,
    pending_resize: Cell<bool>,
    pending_minimize: Cell<bool>,
    pending_restore: Cell<bool>,
    pending_since: Cell<f64>,
    sent: Cell<(i32, i32)>,
    anchor_right: Cell<bool>,
    anchor_bottom: Cell<bool>,
    live_start: Cell<NSRect>,
    proposed: Cell<NSSize>,
    asked_size: Cell<bool>,
    resize_generation: Cell<u64>,
    last_resize_sent: Cell<f64>,
    shadow_fresh: Cell<bool>,
}

impl Win {
    pub fn info(&self) -> Info {
        self.info.borrow().clone()
    }

    pub fn ns(&self) -> &NSWindow {
        self.window.as_window()
    }

    fn set_frame(&self, r: NSRect) {
        self.applying.set(true);
        self.ns().setFrame_display_animate(r, true, false);
        self.applying.set(false);
    }

    /// The user is changing it, or a change of ours has not shown on the host yet
    fn busy(&self) -> bool {
        self.user_moving.get()
            || self.dragging.get()
            || self.in_resize.get()
            || self.ns().inLiveResize()
            || self.settling.get()
            || self.pending_move.get()
            || self.pending_resize.get()
    }

    fn set_anchors(&self, right: bool, bottom: bool) {
        self.anchor_right.set(right);
        self.anchor_bottom.set(bottom);
        ffi::with_layer(self.hwnd, |l| {
            l.anchor_right = right;
            l.anchor_bottom = bottom;
        });
    }
}

pub struct Desk {
    pub mtm: MainThreadMarker,
    pub map: Mapping,
    key_window: Retained<NSWindow>,
    pub wins: RefCell<HashMap<u64, Rc<Win>>>,
    pub closed: Cell<bool>,
    pub generation: u64,
    /// The host's stacking of our windows and dialogs, bottom-most first
    pub host_order: RefCell<Vec<u64>>,
    /// The host's rects of everything on the desk, bottom-most first
    pub host_rects: RefCell<Vec<(u64, bool, (i32, i32, i32, i32))>>,
    host_focused: Cell<u64>,
    last_order: RefCell<Vec<u64>>,
    order_pushed_at: Cell<f64>,
    focus_pushed_at: Cell<f64>,
    focus_mismatch_since: Cell<f64>,
    requested: RefCell<HashMap<u64, f64>>,
    prev_host_focused: Cell<u64>,
    follow: Cell<u64>,
    follow_at: Cell<f64>,
    host_owns_focus: Cell<bool>,
    prev_top: Cell<u64>,
    prev_active: Cell<bool>,
    host_diff_since: Cell<f64>,
    pub force_order_push: Cell<bool>,
    icons: RefCell<HashMap<String, Retained<NSImage>>>,
    front_icon: Cell<u64>,
    ticks: Cell<u64>,
    assume_active: bool,
    /// The host pointer's shape, shown while the pointer is over our windows
    cursor: RefCell<String>,
}

thread_local! {
    static DESK: RefCell<Option<Rc<Desk>>> = const { RefCell::new(None) };
    static KEY_MONITOR: RefCell<Option<Retained<objc2::runtime::AnyObject>>> = const { RefCell::new(None) };
}

/// The Window menu's shortcuts act on the desk windows (the key window is a
/// hidden one): Cmd-M minimizes the front window, Cmd-` and Cmd-Shift-`
/// cycle through them.
fn install_key_monitor() {
    let block = block2::RcBlock::new(|event: std::ptr::NonNull<NSEvent>| -> *mut NSEvent {
        let e = unsafe { event.as_ref() };
        let flags = e.modifierFlags();
        let handled = flags.contains(NSEventModifierFlags::Command)
            && !flags.contains(NSEventModifierFlags::Control)
            && !flags.contains(NSEventModifierFlags::Option)
            && ffi::guard("key monitor", || window_shortcut(e.keyCode(), flags.contains(NSEventModifierFlags::Shift))).unwrap_or(false);
        if handled {
            std::ptr::null_mut()
        } else {
            event.as_ptr()
        }
    });
    let monitor = unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(objc2_app_kit::NSEventMask::KeyDown, &block) };
    KEY_MONITOR.with(|m| *m.borrow_mut() = monitor);
}

fn remove_key_monitor() {
    if let Some(monitor) = KEY_MONITOR.with(|m| m.borrow_mut().take()) {
        unsafe { NSEvent::removeMonitor(&monitor) };
    }
}

fn window_shortcut(key_code: u16, shift: bool) -> bool {
    const KEY_M: u16 = 46;
    const KEY_GRAVE: u16 = 50;
    let Some(d) = desk() else { return false };
    let Some(order) = mac_order(&d) else { return false };
    match key_code {
        KEY_M => {
            if let Some(front) = order.last().and_then(|h| d.wins.borrow().get(h).cloned()) {
                front.ns().miniaturize(None);
            }
            true
        }
        KEY_GRAVE => {
            if order.len() >= 2 {
                let wins = d.wins.borrow().clone();
                if shift {
                    // The front one goes behind the others
                    let back = wins.get(&order[0]).map(|w| w.ns().windowNumber());
                    if let (Some(front), Some(back)) = (order.last().and_then(|h| wins.get(h)), back) {
                        front.ns().orderWindow_relativeTo(NSWindowOrderingMode::Below, back);
                    }
                } else if let Some(next) = wins.get(&order[0]) {
                    // The one at the back comes to the front
                    next.ns().makeMainWindow();
                    next.ns().orderFront(None);
                }
            }
            true
        }
        _ => false,
    }
}

static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub fn desk() -> Option<Rc<Desk>> {
    DESK.with(|d| d.borrow().clone())
}

pub fn win(hwnd: u64) -> Option<Rc<Win>> {
    desk().and_then(|d| d.wins.borrow().get(&hwnd).cloned())
}

pub fn begin(mtm: MainThreadMarker, key_window: Retained<NSWindow>, mut map: Mapping) {
    end();
    // One desk pixel per device pixel: the mirrored screen's backing scale
    let corner = map.cocoa_point(mtm, map.sx + 1.0, map.sy + 1.0);
    for screen in NSScreen::screens(mtm).iter() {
        let f = screen.frame();
        if corner.x >= f.origin.x
            && corner.x < f.origin.x + f.size.width
            && corner.y > f.origin.y
            && corner.y <= f.origin.y + f.size.height
            && screen.backingScaleFactor() > 0.0
        {
            map.ppp = 1.0 / screen.backingScaleFactor();
        }
    }
    if map.ppp <= 0.0 {
        map.ppp = 0.5;
    }
    crate::set_points_per_px(map.ppp);
    // The key window carries the keyboard and nothing else: invisible, out of
    // the way of the mouse, absent from Mission Control and window cycling.
    key_window.setStyleMask(NSWindowStyleMask::Borderless);
    key_window.setAlphaValue(0.0);
    key_window.setOpaque(false);
    key_window.setBackgroundColor(Some(&NSColor::clearColor()));
    key_window.setHasShadow(false);
    key_window.setIgnoresMouseEvents(true);
    key_window.setExcludedFromWindowsMenu(true);
    key_window.setCollectionBehavior(
        NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::IgnoresCycle
            | NSWindowCollectionBehavior::Transient
            | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );
    key_window.setTitle(&NSString::from_str("Moonlight desk"));
    let at = map.to_cocoa(mtm, 0, 0, 1, 1);
    key_window.setFrame_display_animate(NSRect::new(at.origin, NSSize::new(1.0, 1.0)), false, false);
    let generation = GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let desk = Rc::new(Desk {
        mtm,
        map,
        key_window,
        wins: RefCell::new(HashMap::new()),
        closed: Cell::new(false),
        generation,
        host_order: RefCell::new(Vec::new()),
        host_rects: RefCell::new(Vec::new()),
        host_focused: Cell::new(0),
        last_order: RefCell::new(Vec::new()),
        order_pushed_at: Cell::new(0.0),
        focus_pushed_at: Cell::new(0.0),
        focus_mismatch_since: Cell::new(0.0),
        requested: RefCell::new(HashMap::new()),
        prev_host_focused: Cell::new(0),
        follow: Cell::new(0),
        follow_at: Cell::new(0.0),
        host_owns_focus: Cell::new(false),
        prev_top: Cell::new(0),
        prev_active: Cell::new(false),
        host_diff_since: Cell::new(0.0),
        force_order_push: Cell::new(false),
        icons: RefCell::new(HashMap::new()),
        front_icon: Cell::new(0),
        ticks: Cell::new(0),
        assume_active: std::env::var("DESK_TEST_ASSUME_ACTIVE").map(|v| v == "1").unwrap_or(false),
        cursor: RefCell::new("arrow".into()),
    });
    DESK.with(|d| *d.borrow_mut() = Some(desk));
    install_key_monitor();
    info!(
        "deskwm: screen {:.0},{:.0} {:.0}x{:.0} pt, {:.4} pt/px, desk {}x{}",
        map.sx, map.sy, map.sw, map.sh, map.ppp, map.desk_w, map.desk_h
    );
    schedule_tick(generation);
}

pub fn end() {
    remove_key_monitor();
    let Some(d) = DESK.with(|d| d.borrow_mut().take()) else {
        return;
    };
    d.closed.set(true);
    let wins: Vec<Rc<Win>> = d.wins.borrow_mut().drain().map(|(_, w)| w).collect();
    for w in wins {
        close_window(&w);
    }
    ffi::clear_layers();
}

/// Ten times a second: stacking and focus (they change without a layout:
/// clicks, Cmd-Tab), and twice a second the host's frames again (a size the
/// host refused, a resize that never matched).
fn schedule_tick(generation: u64) {
    ffi::on_main_after(0.1, move || {
        let Some(d) = desk() else { return };
        if d.generation != generation || d.closed.get() {
            return;
        }
        ffi::guard("tick", || {
            sync_order(&d);
            let t = d.ticks.get() + 1;
            d.ticks.set(t);
            if t % 5 == 0 {
                let wins: Vec<Rc<Win>> = d.wins.borrow().values().cloned().collect();
                for w in wins {
                    apply_frame(&d, &w);
                    settle(w.hwnd, false);
                }
            }
        });
        schedule_tick(generation);
    });
}

// --- Layout ---------------------------------------------------------------------

pub fn apply_layout(infos: Vec<Info>, cursor: String) {
    let Some(d) = desk() else { return };
    if d.closed.get() {
        return;
    }
    if !cursor.is_empty() && *d.cursor.borrow() != cursor {
        *d.cursor.borrow_mut() = cursor;
        if pointer_over_any(&d) {
            show_cursor(&d);
        }
    }
    let now = ffi::now();
    let present: HashSet<u64> = infos.iter().map(|i| i.hwnd).collect();
    // Gone from the desk: hidden at once, closed if it stays gone (layered
    // skin parts and some menus flicker out and back within a frame)
    let gone: Vec<Rc<Win>> = d.wins.borrow().values().filter(|w| !present.contains(&w.hwnd)).cloned().collect();
    for w in gone {
        if w.gone_since.get() == 0.0 {
            w.gone_since.set(now);
            w.applying.set(true);
            w.ns().orderOut(None);
            w.applying.set(false);
            update_visible(w.hwnd);
            let hwnd = w.hwnd;
            ffi::on_main_after(0.6, move || {
                ffi::guard("close gone", || close_if_gone(hwnd));
            });
        }
    }
    for i in &infos {
        let existing = d.wins.borrow().get(&i.hwnd).cloned();
        match existing {
            Some(w) => update_window(&d, &w, i, now),
            None => {
                let w = create_window(&d, i);
                d.wins.borrow_mut().insert(i.hwnd, w);
            }
        }
    }
    *d.host_order.borrow_mut() = infos.iter().filter(|i| i.is_pane() && !i.minimized).map(|i| i.hwnd).collect();
    *d.host_rects.borrow_mut() = infos.iter().filter(|i| !i.minimized).map(|i| (i.hwnd, i.is_pane(), (i.x, i.y, i.w, i.h))).collect();
    d.host_focused.set(infos.iter().find(|i| i.focused).map(|i| i.hwnd).unwrap_or(0));
    sync_order(&d);
    update_icon(&d);
    crate::input::layout_changed();
}

fn close_if_gone(hwnd: u64) {
    let Some(d) = desk() else { return };
    let Some(w) = d.wins.borrow().get(&hwnd).cloned() else { return };
    if w.gone_since.get() != 0.0 && ffi::now() - w.gone_since.get() > 0.5 {
        d.wins.borrow_mut().remove(&hwnd);
        close_window(&w);
    }
}

fn close_window(w: &Win) {
    ffi::remove_layer(w.hwnd);
    w.applying.set(true);
    let ns = w.ns();
    if let Some(parent) = ns.parentWindow() {
        parent.removeChildWindow(ns);
    }
    ns.setDelegate(None);
    ns.orderOut(None);
    ns.close();
}

fn geo_of(i: &Info) -> Geo {
    let mut pieces = [ffi::Piece::default(); ffi::PIECE_MAX];
    for (k, p) in i.pieces.iter().take(ffi::PIECE_MAX).enumerate() {
        pieces[k] = ffi::Piece { x: p[0], y: p[1], w: p[2], h: p[3], ax: p[4], ay: p[5] };
    }
    Geo {
        ts: i.ts,
        x: i.slot.0,
        y: i.slot.1,
        w: if i.minimized { 0 } else { i.slot.2 },
        h: if i.minimized { 0 } else { i.slot.3 },
        ax: i.alpha.0,
        ay: i.alpha.1,
        scale: i.scale,
        alpha_scale: i.alpha_scale,
        piece_count: i.pieces.len().min(ffi::PIECE_MAX) as u32,
        pieces,
    }
}

fn create_window(d: &Desk, i: &Info) -> Rc<Win> {
    let mtm = d.mtm;
    let frame = d.map.to_cocoa(mtm, i.x, i.y, i.w, i.h);
    let pane = i.is_pane();
    let mut style = NSWindowStyleMask::Borderless;
    if pane {
        style |= NSWindowStyleMask::Resizable | NSWindowStyleMask::Miniaturizable;
    }
    let window = DeskWindow::new(mtm, frame, style, pane);
    let ns = window.as_window();
    unsafe { ns.setReleasedWhenClosed(false) };
    ns.setOpaque(false);
    ns.setBackgroundColor(Some(&NSColor::clearColor()));
    // Windows get the macOS shadow; popups carry their own (in their alpha)
    ns.setHasShadow(pane);
    ns.setTitle(&NSString::from_str(&i.title));
    if !pane {
        ns.setCollectionBehavior(NSWindowCollectionBehavior::Transient | NSWindowCollectionBehavior::IgnoresCycle);
    }
    let view = DeskView::new(mtm, NSRect::new(NSPoint::new(0.0, 0.0), frame.size), i.hwnd);
    ns.setContentView(Some(&view));
    let delegate = Delegate::new(mtm, i.hwnd);
    ns.setDelegate(Some(delegate.as_protocol()));
    view.update_drawable_size();
    let metal = view.metal_layer().map(|l| Retained::as_ptr(&l) as *mut c_void).unwrap_or(std::ptr::null_mut());
    ffi::add_layer(i.hwnd, metal, i.radius);
    ffi::with_layer(i.hwnd, |l| {
        l.view_w = i.w;
        l.view_h = i.h;
    });
    ffi::push_geo(i.hwnd, geo_of(i));
    let w = Rc::new(Win {
        hwnd: i.hwnd,
        pane,
        window,
        _view: view,
        _delegate: delegate,
        info: RefCell::new(i.clone()),
        applying: Cell::new(false),
        gone_since: Cell::new(0.0),
        user_moving: Cell::new(false),
        move_generation: Cell::new(0),
        dragging: Cell::new(false),
        in_resize: Cell::new(false),
        settling: Cell::new(false),
        settle_deadline: Cell::new(0.0),
        pending_move: Cell::new(false),
        pending_resize: Cell::new(false),
        pending_minimize: Cell::new(false),
        pending_restore: Cell::new(false),
        pending_since: Cell::new(0.0),
        sent: Cell::new((i.x, i.y)),
        anchor_right: Cell::new(false),
        anchor_bottom: Cell::new(false),
        live_start: Cell::new(frame),
        proposed: Cell::new(frame.size),
        asked_size: Cell::new(false),
        resize_generation: Cell::new(0),
        last_resize_sent: Cell::new(0.0),
        shadow_fresh: Cell::new(false),
    });
    w.applying.set(true);
    let owner = if pane { None } else { d.wins.borrow().get(&i.owner).cloned() };
    if let Some(owner) = owner.filter(|o| o.pane) {
        // Menus, tooltips, drop-downs ride on their owner
        unsafe { owner.ns().addChildWindow_ordered(w.ns(), NSWindowOrderingMode::Above) };
        w.ns().orderFront(None);
    } else if i.minimized {
        w.ns().miniaturize(None);
    } else {
        // A new window or dialog appears in front, as it did on the host
        w.ns().orderFront(None);
    }
    w.applying.set(false);
    update_visible(i.hwnd);
    info!(
        "deskwm: window 0x{:x} ({}, {:?}) at {},{} {}x{} px, slot {},{} {}x{}{}",
        i.hwnd, i.title, i.kind, i.x, i.y, i.w, i.h, i.slot.0, i.slot.1, i.slot.2, i.slot.3,
        if i.alpha.0 >= 0 { " +alpha" } else { "" }
    );
    w
}

fn update_window(d: &Desk, w: &Rc<Win>, i: &Info, now: f64) {
    let before = w.info.replace(i.clone());
    if w.gone_since.get() != 0.0 {
        // Back before it was closed
        w.gone_since.set(0.0);
        w.applying.set(true);
        if !w.pane {
            if let Some(owner) = d.wins.borrow().get(&i.owner).cloned() {
                unsafe { owner.ns().addChildWindow_ordered(w.ns(), NSWindowOrderingMode::Above) };
            }
        }
        w.ns().orderFront(None);
        w.applying.set(false);
    }
    if before.title != i.title {
        w.ns().setTitle(&NSString::from_str(&i.title));
    }
    // Changes of ours: confirmed when the host shows them, dropped when it never does
    let (_, _, fw, fh) = d.map.to_desk(d.mtm, w.ns().frame());
    if w.pending_move.get() && (i.x - w.sent.get().0).abs() <= 1 && (i.y - w.sent.get().1).abs() <= 1 {
        w.pending_move.set(false);
    }
    if w.pending_resize.get() && (fw - i.w).abs() <= 1 && (fh - i.h).abs() <= 1 {
        w.pending_resize.set(false);
    }
    if w.pending_minimize.get() && i.minimized {
        w.pending_minimize.set(false);
    }
    if w.pending_restore.get() && !i.minimized {
        w.pending_restore.set(false);
    }
    if (w.pending_move.get() || w.pending_resize.get() || w.pending_minimize.get() || w.pending_restore.get())
        && now - w.pending_since.get() > PENDING_GRACE
    {
        w.pending_move.set(false);
        w.pending_resize.set(false);
        w.pending_minimize.set(false);
        w.pending_restore.set(false);
    }
    if i.minimized != w.ns().isMiniaturized() && !w.pending_minimize.get() && !w.pending_restore.get() {
        w.applying.set(true);
        if i.minimized {
            w.ns().miniaturize(None);
        } else {
            w.ns().deminiaturize(None);
        }
        w.applying.set(false);
    }
    apply_frame(d, w);
    if !w.pane {
        let want = d.wins.borrow().get(&i.owner).cloned().filter(|o| o.pane);
        let current = w.ns().parentWindow();
        let same = match (&want, &current) {
            (Some(o), Some(p)) => std::ptr::eq(o.ns(), &**p),
            (None, None) => true,
            _ => false,
        };
        if !same {
            if let Some(p) = current {
                p.removeChildWindow(w.ns());
            }
            if let Some(o) = want {
                unsafe { o.ns().addChildWindow_ordered(w.ns(), NSWindowOrderingMode::Above) };
            }
        }
    }
    ffi::push_geo(i.hwnd, geo_of(i));
    if before.radius != i.radius {
        ffi::with_layer(i.hwnd, |l| l.radius = i.radius);
        w.ns().invalidateShadow();
    }
    update_visible(i.hwnd);
    settle(i.hwnd, false);
}

/// The host's frame, unless the user is changing it or a change of ours is on
/// its way.
fn apply_frame(d: &Desk, w: &Win) {
    if w.gone_since.get() != 0.0 || w.busy() || w.ns().isMiniaturized() {
        return;
    }
    let i = w.info.borrow().clone();
    if i.minimized {
        return;
    }
    let want = d.map.to_cocoa(d.mtm, i.x, i.y, i.w, i.h);
    let cur = w.ns().frame();
    if (cur.origin.x - want.origin.x).abs() > 0.5
        || (cur.origin.y - want.origin.y).abs() > 0.5
        || (cur.size.width - want.size.width).abs() > 0.5
        || (cur.size.height - want.size.height).abs() > 0.5
    {
        w.set_frame(want);
    }
}

pub fn update_visible(hwnd: u64) {
    let Some(w) = win(hwnd) else { return };
    let ns = w.ns();
    let visible = w.gone_since.get() == 0.0
        && ns.isVisible()
        && !ns.isMiniaturized()
        && ns.occlusionState().contains(NSWindowOcclusionState::Visible);
    ffi::with_layer(hwnd, |l| l.visible = visible);
}

/// The renderer drew a picture of a new size: a resize may be done
pub fn shown_changed(hwnd: u64) {
    let Some(w) = win(hwnd) else { return };
    if !w.shadow_fresh.get() && w.pane {
        // The macOS shadow follows the window's shape, known once it has a picture
        w.shadow_fresh.set(true);
        w.ns().invalidateShadow();
    }
    settle(hwnd, false);
}

// --- What the user does to a window ---------------------------------------------------

pub fn did_move(hwnd: u64) {
    let Some(w) = win(hwnd) else { return };
    // Popups move with their owner (child windows): never the user's doing
    if !w.pane || w.applying.get() || w.dragging.get() {
        return;
    }
    if !w.user_moving.get() {
        dismiss_menus(hwnd);  // a move by other means (tiling, Mission Control)
    }
    w.user_moving.set(true);
    let generation = w.move_generation.get() + 1;
    w.move_generation.set(generation);
    ffi::on_main_after(0.05, move || {
        ffi::guard("move settled", || move_settled(hwnd, generation));
    });
}

/// The host is told once a move is over: moving it live would only make the
/// host's popups and hover lag behind
fn move_settled(hwnd: u64, generation: u64) {
    let (Some(d), Some(w)) = (desk(), win(hwnd)) else { return };
    if w.move_generation.get() != generation {
        return;
    }
    if NSEvent::pressedMouseButtons() & 1 != 0 {
        ffi::on_main_after(0.05, move || {
            ffi::guard("move settled", || move_settled(hwnd, generation));
        });
        return;
    }
    w.user_moving.set(false);
    send_position(&d, &w);
}

fn send_position(d: &Desk, w: &Win) {
    let (x, y, _, _) = d.map.to_desk(d.mtm, w.ns().frame());
    let i = w.info.borrow().clone();
    if x == i.x && y == i.y && !w.pending_move.get() {
        return;
    }
    w.pending_move.set(true);
    w.pending_since.set(ffi::now());
    w.sent.set((x, y));
    ffi::window_op(w.hwnd, "adopt", x, y, 0, 0);
}

pub fn will_start_live_resize(hwnd: u64) {
    let Some(w) = win(hwnd) else { return };
    if !w.pane {
        return;
    }
    w.in_resize.set(true);
    w.settling.set(false);
    dismiss_menus(hwnd);
    let f = w.ns().frame();
    w.live_start.set(f);
    w.proposed.set(f.size);
    // The edges being dragged are the ones next to the mouse; the picture
    // hugs the others from the very first step
    let m = NSEvent::mouseLocation();
    let (min_x, max_x, min_y, max_y) = (f.origin.x, f.origin.x + f.size.width, f.origin.y, f.origin.y + f.size.height);
    let right = (m.x - min_x).abs() < (m.x - max_x).abs() && (m.x - min_x).abs() < 16.0;
    let bottom = (m.y - max_y).abs() < (m.y - min_y).abs() && (m.y - max_y).abs() < 16.0;
    w.set_anchors(right, bottom);
}

/// The window never gets smaller than the picture the host has drawn: a
/// shrink waits for the host, so nothing is ever squeezed or cut. Growing
/// shows the picture against the edges that stay put.
pub fn will_resize(hwnd: u64, size: NSSize) -> NSSize {
    let (Some(d), Some(w)) = (desk(), win(hwnd)) else { return size };
    if w.applying.get() || !w.in_resize.get() {
        return size;
    }
    w.proposed.set(size);
    w.asked_size.set(true);
    schedule_resize_settled(&w, if ffi::now() - w.last_resize_sent.get() >= 0.016 { 0.0 } else { 0.016 });
    let (sw, sh) = ffi::shown(hwnd);
    NSSize::new(size.width.max(sw as f64 * d.map.ppp), size.height.max(sh as f64 * d.map.ppp))
}

pub fn did_resize(hwnd: u64) {
    let (Some(d), Some(w)) = (desk(), win(hwnd)) else { return };
    let f = w.ns().frame();
    ffi::with_layer(hwnd, |l| {
        l.view_w = (f.size.width / d.map.ppp).round() as i32;
        l.view_h = (f.size.height / d.map.ppp).round() as i32;
    });
    if w.applying.get() || !w.pane {
        return;
    }
    if w.in_resize.get() && !w.asked_size.get() {
        // macOS sized it by itself mid-resize: that size is the one to get
        w.proposed.set(f.size);
        schedule_resize_settled(&w, 0.016);
    }
    w.asked_size.set(false);
    if w.in_resize.get() {
        // The edges being dragged move; the picture hugs the others
        let a = w.live_start.get();
        let mut right = w.anchor_right.get();
        let mut bottom = w.anchor_bottom.get();
        if (f.origin.x - a.origin.x).abs() > 0.5 && ((f.origin.x + f.size.width) - (a.origin.x + a.size.width)).abs() < 0.5 {
            right = true;
        }
        if ((f.origin.y + f.size.height) - (a.origin.y + a.size.height)).abs() > 0.5 && (f.origin.y - a.origin.y).abs() < 0.5 {
            bottom = true;  // Cocoa y grows upwards: the top edge moved
        }
        w.set_anchors(right, bottom);
        return;
    }
    // Zoom, tiling, Fill: not a live resize
    dismiss_menus(hwnd);
    w.proposed.set(f.size);
    w.set_anchors(false, false);
    schedule_resize_settled(&w, 0.033);
}

fn schedule_resize_settled(w: &Win, delay: f64) {
    let generation = w.resize_generation.get() + 1;
    w.resize_generation.set(generation);
    let hwnd = w.hwnd;
    ffi::on_main_after(delay, move || {
        ffi::guard("resize settled", || {
            if let Some(w) = win(hwnd) {
                if w.resize_generation.get() == generation {
                    resize_settled(hwnd);
                }
            }
        });
    });
}

fn resize_settled(hwnd: u64) {
    let (Some(d), Some(w)) = (desk(), win(hwnd)) else { return };
    let p = w.proposed.get();
    let (pw, ph) = ((p.width / d.map.ppp).round() as i32, (p.height / d.map.ppp).round() as i32);
    if pw <= 0 || ph <= 0 {
        return;
    }
    let i = w.info.borrow().clone();
    let now = ffi::now();
    // Where the picture's top-left will be: against the edges that stay put
    let f = w.ns().frame();
    let left = if w.anchor_right.get() { f.origin.x + f.size.width - p.width } else { f.origin.x };
    let top = if w.anchor_bottom.get() { f.origin.y + p.height } else { f.origin.y + f.size.height };
    let (x, y, _, _) = d.map.to_desk(d.mtm, NSRect::new(NSPoint::new(left, top - p.height), p));
    let resized = pw != i.w || ph != i.h || w.pending_resize.get();
    if resized {
        w.pending_resize.set(true);
        w.pending_since.set(now);
        w.last_resize_sent.set(now);
    }
    if x != i.x || y != i.y {
        // A resize from the left or top: place and size in one step, so the
        // host never shows the window shifted in between
        w.pending_move.set(true);
        w.pending_since.set(now);
        w.sent.set((x, y));
        ffi::window_op(hwnd, "adopt", x, y, if resized { pw } else { 0 }, if resized { ph } else { 0 });
    } else if resized {
        ffi::window_op(hwnd, "resize", 0, 0, pw, ph);
    }
}

pub fn did_end_live_resize(hwnd: u64) {
    let Some(w) = win(hwnd) else { return };
    w.resize_generation.set(w.resize_generation.get() + 1);
    resize_settled(hwnd);
    w.in_resize.set(false);
    // The frame snaps to the picture once the host has drawn the final size
    w.settling.set(true);
    w.settle_deadline.set(ffi::now() + 3.0);
    settle(hwnd, false);
}

/// After a resize: the frame becomes the picture's size (the host may have
/// refused the size asked for: minimum sizes), against the edges that stayed.
fn settle(hwnd: u64, _deadline: bool) {
    let (Some(d), Some(w)) = (desk(), win(hwnd)) else { return };
    if !w.settling.get() || w.in_resize.get() {
        return;
    }
    let (sw, sh) = ffi::shown(hwnd);
    let p = w.proposed.get();
    let (want_w, want_h) = ((p.width / d.map.ppp).round() as i32, (p.height / d.map.ppp).round() as i32);
    let i = w.info.borrow().clone();
    // Done when the picture shows the size asked for; or when the host has
    // settled on a size (maybe another: minimum sizes) and the picture shows
    // it, or there is no picture to wait for (the window is not drawn now)
    let drawn = ffi::with_layer(hwnd, |l| l.visible).unwrap_or(false) && sw > 0 && sh > 0;
    let done = (sw - want_w).abs() <= 2 && (sh - want_h).abs() <= 2;
    let host_done = !w.pending_resize.get() && !w.pending_move.get();
    let picture_matches_host = (sw - i.w).abs() <= 2 && (sh - i.h).abs() <= 2;
    let expired = ffi::now() >= w.settle_deadline.get();
    if !done && !(host_done && (picture_matches_host || !drawn)) && !expired {
        return;
    }
    let (sw, sh) = if drawn && picture_matches_host || done { (sw, sh) } else { (i.w, i.h) };
    let (pw, ph) = if sw > 0 && sh > 0 { (sw as f64 * d.map.ppp, sh as f64 * d.map.ppp) } else { (i.w as f64 * d.map.ppp, i.h as f64 * d.map.ppp) };
    let cur = w.ns().frame();
    let x = if w.anchor_right.get() { cur.origin.x + cur.size.width - pw } else { cur.origin.x };
    let y = if w.anchor_bottom.get() { cur.origin.y } else { cur.origin.y + cur.size.height - ph };
    let r = NSRect::new(NSPoint::new(x, y), NSSize::new(pw, ph));
    if (cur.size.width - pw).abs() > 0.5 || (cur.size.height - ph).abs() > 0.5 || (cur.origin.x - x).abs() > 0.5 || (cur.origin.y - y).abs() > 0.5 {
        w.set_frame(r);
    }
    w.settling.set(false);
    w.set_anchors(false, false);
    send_position(&d, &w);
}

pub fn did_miniaturize(hwnd: u64, minimized: bool) {
    let Some(w) = win(hwnd) else { return };
    update_visible(hwnd);
    if w.applying.get() {
        return;
    }
    w.pending_since.set(ffi::now());
    if minimized {
        w.pending_minimize.set(true);
        ffi::window_op(hwnd, "minimize", 0, 0, 0, 0);
    } else {
        w.pending_restore.set(true);
        ffi::window_op(hwnd, "restore", 0, 0, 0, 0);
    }
    if let Some(d) = desk() {
        update_icon(&d);
    }
}

// --- Drags -----------------------------------------------------------------------------

/// A caption press turned into a drag: the system's own window drag (tiling at
/// edges and corners, Mission Control, other screens). The host window follows
/// along (pictures come from the atlas, so where it is does not change them,
/// but the host then knows what the drag uncovers).
pub fn begin_drag(hwnd: u64) {
    let Some(w) = win(hwnd) else { return };
    if NSEvent::pressedMouseButtons() & 1 == 0 {
        return;  // released already: it was a click after all
    }
    w.dragging.set(true);
    dismiss_menus(hwnd);
    let ns = w.ns();
    let at = ns.mouseLocationOutsideOfEventStream();
    let uptime = NSProcessInfo::processInfo().systemUptime();
    let down = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
        NSEventType::LeftMouseDown,
        at,
        NSEventModifierFlags::empty(),
        uptime,
        ns.windowNumber(),
        None,
        0,
        1,
        1.0,
    );
    if let Some(down) = down {
        ns.performWindowDragWithEvent(&down);
    }
    poll_drag_end(hwnd, 0);
}

/// Mid-drag: tell the host where the window is now (every poll: a move
/// costs the host well under a millisecond, and the sooner it knows, the
/// sooner what the drag uncovers is in the frame 1:1)
fn drag_follow(hwnd: u64, _tick: u32) {
    if let (Some(d), Some(w)) = (desk(), win(hwnd)) {
        send_position(&d, &w);
    }
}

/// The window server owns a native drag and swallows its mouse-up: watch the button
fn poll_drag_end(hwnd: u64, tick: u32) {
    ffi::on_main_after(0.016, move || {
        ffi::guard("drag end", || {
            let Some(w) = win(hwnd) else { return };
            if !w.dragging.get() {
                return;
            }
            if NSEvent::pressedMouseButtons() & 1 != 0 {
                drag_follow(hwnd, tick);
                poll_drag_end(hwnd, tick + 1);
                return;
            }
            end_drag(hwnd);
        });
    });
}

pub fn end_drag(hwnd: u64) {
    let (Some(d), Some(w)) = (desk(), win(hwnd)) else { return };
    crate::input::drag_ended(hwnd);
    if !w.dragging.get() {
        return;
    }
    w.dragging.set(false);
    send_position(&d, &w);
    // Dropped in front: the host stacks it there and gives it the focus
    d.force_order_push.set(true);
    sync_order(&d);
}

/// What a double click on the title bar does: the user's macOS preference
pub fn title_action(hwnd: u64) {
    let Some(w) = win(hwnd) else { return };
    let action = NSUserDefaults::standardUserDefaults()
        .stringForKey(&NSString::from_str("AppleActionOnDoubleClick"))
        .map(|s| s.to_string())
        .unwrap_or_else(|| "Maximize".into());
    match action.as_str() {
        "None" => {}
        "Minimize" => w.ns().miniaturize(None),
        "Fill" => {
            if let Some(screen) = w.ns().screen() {
                w.ns().setFrame_display_animate(screen.visibleFrame(), true, true);
            }
        }
        _ => w.ns().zoom(None),
    }
}

pub fn zoom(hwnd: u64) {
    if let Some(w) = win(hwnd) {
        w.ns().zoom(None);
    }
}

pub fn minimize(hwnd: u64) {
    if let Some(w) = win(hwnd) {
        w.ns().miniaturize(None);
    }
}

/// The pane a host window belongs to: itself, or the pane owning the popup
/// or dialog.
pub fn root_pane(d: &Desk, hwnd: u64) -> u64 {
    let wins = d.wins.borrow();
    let mut h = hwnd;
    for _ in 0..8 {
        let Some(w) = wins.get(&h) else { return 0 };
        if w.pane {
            return h;
        }
        let owner = w.info.borrow().owner;
        if owner == 0 {
            return 0;
        }
        h = owner;
    }
    0
}

/// The user moves or resizes a window here: on Windows the press on its title
/// bar would have closed its open menus first, and a menu left open stays
/// where it was while the window goes. Hidden here at once, closed on the
/// host (shown again if the host keeps it, e.g. a tooltip).
pub fn dismiss_menus(pane: u64) {
    let Some(d) = desk() else { return };
    let popups: Vec<Rc<Win>> = d.wins.borrow().values().filter(|w| !w.pane).cloned().collect();
    let mut any = false;
    for p in popups {
        if root_pane(&d, p.hwnd) != pane {
            continue;
        }
        any = true;
        p.ns().setAlphaValue(0.0);
        let hwnd = p.hwnd;
        ffi::on_main_after(1.0, move || {
            if let Some(p) = win(hwnd) {
                p.ns().setAlphaValue(1.0);
            }
        });
    }
    if any {
        ffi::window_op(pane, "dismiss", 0, 0, 0, 0);
    }
}

// --- Stacking and focus ----------------------------------------------------------------

/// Our panes as AppKit stacks them, bottom-most first; None mid-drag (the
/// drop pushes the order).
fn mac_order(d: &Desk) -> Option<Vec<u64>> {
    let app = NSApplication::sharedApplication(d.mtm);
    let by_ptr: HashMap<usize, Rc<Win>> =
        d.wins.borrow().values().map(|w| (Retained::as_ptr(&w.window) as usize, w.clone())).collect();
    let ordered: Vec<Retained<NSWindow>> = app.orderedWindows().iter().collect();
    let mut out = Vec::new();
    for ns in ordered.into_iter().rev() {
        let Some(w) = by_ptr.get(&(Retained::as_ptr(&ns) as usize)) else { continue };
        if !w.pane || w.gone_since.get() != 0.0 || ns.isMiniaturized() || !ns.isVisible() {
            continue;
        }
        if w.dragging.get() {
            return None;
        }
        out.push(w.hwnd);
    }
    Some(out)
}

/// Stack these panes (bottom-most first) on top of our others, each right
/// above our current front-most one: relative ordering keeps them below other
/// apps' windows, the host never pulls us over the app the user is in.
fn raise_panes(d: &Desk, bottom_first: &[u64]) {
    let Some(now) = mac_order(d) else { return };
    let Some(&top) = now.last() else { return };
    let wins = d.wins.borrow().clone();
    let mut anchor = wins.get(&top).map(|w| w.ns().windowNumber());
    for hwnd in bottom_first {
        let Some(w) = wins.get(hwnd) else { continue };
        let number = w.ns().windowNumber();
        if Some(number) == anchor {
            continue;
        }
        if let Some(a) = anchor {
            w.ns().orderWindow_relativeTo(NSWindowOrderingMode::Above, a);
        }
        anchor = Some(number);
    }
}

pub fn sync_order(d: &Desk) {
    let Some(mac) = mac_order(d) else { return };
    let now = ffi::now();
    let active = NSApplication::sharedApplication(d.mtm).isActive() || d.assume_active;
    let host: Vec<u64> = d.host_order.borrow().iter().copied().filter(|h| mac.contains(h)).collect();
    let host_focused = d.host_focused.get();
    let host_pane = root_pane(d, host_focused);
    let host_focus_moved = host_focused != d.prev_host_focused.get();
    d.prev_host_focused.set(host_focused);
    let request_recent = now - d.order_pushed_at.get() < 1.0 || now - d.focus_pushed_at.get() < 1.0;
    let mac_top = mac.last().copied().unwrap_or(0);
    let top_changed = mac_top != d.prev_top.get();
    // Focus that left our windows by itself (a file opened from Explorer in
    // an app on the host's own display) stays where the host put it until
    // the user acts here: picks another of our windows or comes back.
    if top_changed || active != d.prev_active.get() || (host_pane != 0 && host_pane == mac_top) {
        d.host_owns_focus.set(false);
    }
    d.prev_top.set(mac_top);
    d.prev_active.set(active);
    let last_order_same = *d.last_order.borrow() == mac;
    if host_focus_moved && host_pane == 0 && host_focused == 0 && last_order_same && !request_recent {
        d.host_owns_focus.set(true);
    }
    // The host focused one of our windows by itself (a file opened in a
    // running Notepad, a dialog): it comes up here. It may not be up yet
    // (restored from minimized by the host): kept for a moment.
    let requested_at = d.requested.borrow().get(&host_pane).copied().unwrap_or(-10.0);
    if host_focus_moved && host_pane != 0 && host_pane != mac_top && now - requested_at > 1.5 {
        d.follow.set(host_pane);
        d.follow_at.set(now);
    } else if d.follow.get() != 0 && top_changed {
        d.follow.set(0);  // the user picked a window here meanwhile
    }
    let follow = d.follow.get();
    if follow != 0 && (host_pane != follow || follow == mac_top || now - d.follow_at.get() > 2.0) {
        d.follow.set(0);
    }
    let follow = d.follow.get();
    if follow != 0 && mac.contains(&follow) {
        info!("deskwm: host focused 0x{follow:x} by itself; bringing it up here");
        raise_panes(d, &[follow]);
        d.follow.set(0);
        d.focus_mismatch_since.set(0.0);
        *d.last_order.borrow_mut() = mac_order(d).unwrap_or_default();
        return;  // the next tick pushes the rest of the order, if it differs
    }
    // The host restacked by itself without a focus change (a new window on
    // top, an app raising another): the Mac takes the host's order
    if !mac.is_empty() && last_order_same && !d.force_order_push.get() && !request_recent && host.len() == mac.len() && host != mac {
        if d.host_diff_since.get() == 0.0 {
            d.host_diff_since.set(now);
        } else if now - d.host_diff_since.get() > 0.3 {
            info!("deskwm: host restacked by itself; following");
            raise_panes(d, &host);
            *d.last_order.borrow_mut() = mac_order(d).unwrap_or_default();
            d.host_diff_since.set(0.0);
            return;
        }
    } else {
        d.host_diff_since.set(0.0);
    }
    // Keyboard focus: while we are the active app, the host focuses our
    // front-most window. A refused activation is retried.
    if active && !mac.is_empty() && !d.host_owns_focus.get() && d.follow.get() == 0 {
        if host_pane == mac_top {
            d.focus_mismatch_since.set(0.0);
        } else if d.focus_mismatch_since.get() == 0.0 {
            d.focus_mismatch_since.set(now);
        } else if now - d.focus_mismatch_since.get() > 0.15 && now - d.focus_pushed_at.get() > 0.6 {
            info!("deskwm: host has 0x{host_focused:x}, front here is 0x{mac_top:x}; activating");
            d.focus_pushed_at.set(now);
            d.requested.borrow_mut().insert(mac_top, now);
            ffi::window_op(mac_top, "activate", 0, 0, 0, 0);
        }
    }
    if mac.is_empty() || (last_order_same && !d.force_order_push.get()) {
        return;
    }
    d.force_order_push.set(false);
    *d.last_order.borrow_mut() = mac.clone();
    d.order_pushed_at.set(now);
    if mac == host && host_pane == mac_top {
        return;  // the host is there already
    }
    // While we are active, the front window takes the focus in the same
    // request: the host never shows the new order with the old focus
    if active {
        d.focus_pushed_at.set(now);
        d.focus_mismatch_since.set(0.0);
        d.requested.borrow_mut().insert(mac_top, now);
    }
    let top_first: Vec<u64> = mac.iter().rev().copied().collect();
    info!("deskwm: order {:x?} -> host{}", top_first, if active { " +focus" } else { "" });
    ffi::order(&top_first, active);
}

// --- Dock icon ---------------------------------------------------------------------------

pub fn set_icon(key: String, png: Vec<u8>) {
    let Some(d) = desk() else { return };
    let data = NSData::with_bytes(&png);
    let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) else { return };
    d.icons.borrow_mut().insert(key, image);
    d.front_icon.set(0);
    update_icon(&d);
}

/// The Dock shows the front window's host icon
fn update_icon(d: &Desk) {
    let front = mac_order(d).and_then(|o| o.last().copied()).unwrap_or(0);
    crate::set_front_window(front);
    if front == d.front_icon.get() {
        return;
    }
    let key = d.wins.borrow().get(&front).map(|w| w.info.borrow().icon_key.clone()).unwrap_or_default();
    let image = d.icons.borrow().get(&key).cloned();
    if let Some(image) = image {
        d.front_icon.set(front);
        unsafe { NSApplication::sharedApplication(d.mtm).setApplicationIconImage(Some(&image)) };
    }
}

// --- The pointer ---------------------------------------------------------------------------

/// Is the pointer over one of our windows (in front there)?
pub fn pointer_over_any(d: &Desk) -> bool {
    let number = NSWindow::windowNumberAtPoint_belowWindowWithWindowNumber(NSEvent::mouseLocation(), 0, d.mtm);
    d.wins.borrow().values().any(|w| w.ns().windowNumber() == number)
}

/// The Mac's equivalent of the host pointer's shape: an I-beam over text, a
/// hand over links, resize arrows at window edges, as the host app says
pub fn show_cursor(d: &Desk) {
    let resize = |position| NSCursor::frameResizeCursorFromPosition_inDirections(position, NSCursorFrameResizeDirections::All);
    let cursor = match d.cursor.borrow().as_str() {
        "ibeam" => NSCursor::IBeamCursor(),
        "hand" => NSCursor::pointingHandCursor(),
        "we" => resize(NSCursorFrameResizePosition::Right),
        "ns" => resize(NSCursorFrameResizePosition::Bottom),
        "nwse" => resize(NSCursorFrameResizePosition::BottomRight),
        "nesw" => resize(NSCursorFrameResizePosition::BottomLeft),
        "all" => NSCursor::openHandCursor(),
        "cross" => NSCursor::crosshairCursor(),
        "no" => NSCursor::operationNotAllowedCursor(),
        _ => NSCursor::arrowCursor(),
    };
    cursor.set();
}

// --- Input helpers ------------------------------------------------------------------------

/// The keyboard goes to the hidden key window (and from there to the host);
/// a window or dialog the user presses comes to the front of the app.
pub fn note_press(hwnd: u64) {
    let (Some(d), Some(w)) = (desk(), win(hwnd)) else { return };
    if !d.key_window.isKeyWindow() {
        d.key_window.makeKeyWindow();
    }
    if w.pane {
        w.ns().orderFront(None);
        if !w.ns().isMainWindow() {
            w.ns().makeMainWindow();
        }
        update_icon(&d);
    }
}

/// Is desk pixel (x, y) of this window covered on the host by another of
/// our windows stacked above it there?
pub fn covered_on_host(d: &Desk, hwnd: u64, x: i32, y: i32) -> bool {
    let rects = d.host_rects.borrow();
    let mut above = false;
    for (h, pane, (rx, ry, rw, rh)) in rects.iter() {
        if *h == hwnd {
            above = true;
            continue;
        }
        if above && *pane && x >= *rx && y >= *ry && x < rx + rw && y < ry + rh {
            return true;
        }
    }
    false
}

// --- Test hooks ------------------------------------------------------------------------------

pub fn test_frame(hwnd: u64, x: f64, y: f64, w: f64, h: f64) {
    let (Some(d), Some(win)) = (desk(), win(hwnd)) else { return };
    let top = Mapping::primary_top(d.mtm);
    // Not "applying": the delegate takes it as the user's doing
    win.ns().setFrame_display_animate(NSRect::new(NSPoint::new(x, top - y - h), NSSize::new(w, h)), true, false);
}

pub fn test_drag(hwnd: u64, dx: f64, dy: f64) {
    let Some(w) = win(hwnd) else { return };
    w.dragging.set(true);
    dismiss_menus(hwnd);
    let origin = w.ns().frame().origin;
    for i in 1..=20 {
        ffi::on_main_after(i as f64 * 0.016, move || {
            if let Some(w) = win(hwnd) {
                if w.dragging.get() {
                    w.ns().setFrameOrigin(NSPoint::new(origin.x + dx * i as f64 / 20.0, origin.y - dy * i as f64 / 20.0));
                    drag_follow(hwnd, i);
                }
            }
        });
    }
    ffi::on_main_after(0.4, move || {
        ffi::guard("test drag end", || end_drag(hwnd));
    });
}

pub fn test_live_resize(hwnd: u64, dw: f64, dh: f64, from_left: bool) {
    let Some(w) = win(hwnd) else { return };
    let start = w.ns().frame();
    will_start_live_resize(hwnd);
    if from_left {
        w.set_anchors(true, w.anchor_bottom.get());
    }
    for i in 1..=30 {
        ffi::on_main_after(i as f64 * 0.016, move || {
            ffi::guard("test live resize", || {
                let Some(w) = win(hwnd) else { return };
                let want = NSSize::new(start.size.width + dw * i as f64 / 30.0, start.size.height + dh * i as f64 / 30.0);
                let got = will_resize(hwnd, want);
                let x = if from_left { start.origin.x + start.size.width - got.width } else { start.origin.x };
                w.ns().setFrame_display_animate(
                    NSRect::new(NSPoint::new(x, start.origin.y + start.size.height - got.height), got),
                    true,
                    false,
                );
            });
        });
    }
    ffi::on_main_after(0.55, move || {
        ffi::guard("test live resize end", || did_end_live_resize(hwnd));
    });
}

pub fn test_tile(hwnd: u64, x: f64, y: f64, w: f64, h: f64) {
    let (Some(d), Some(win)) = (desk(), win(hwnd)) else { return };
    let top = Mapping::primary_top(d.mtm);
    will_start_live_resize(hwnd);
    win.ns().setFrame_display_animate(NSRect::new(NSPoint::new(x, top - y - h), NSSize::new(w, h)), true, false);
    did_end_live_resize(hwnd);
}

/// A real click through AppKit: NSEvents sent to the window, as the window
/// server would (window-relative host px; cmd: with Command held)
pub fn test_nsclick(hwnd: u64, x: i32, y: i32, cmd: bool, count: i32) {
    let (Some(d), Some(w)) = (desk(), win(hwnd)) else { return };
    let ns = w.ns();
    let h = ns.frame().size.height;
    let at = NSPoint::new(x as f64 * d.map.ppp, h - y as f64 * d.map.ppp);
    let flags = if cmd { NSEventModifierFlags::Command } else { NSEventModifierFlags::empty() };
    for n in 0..count.max(1) {
        for (k, kind) in [NSEventType::LeftMouseDown, NSEventType::LeftMouseUp].into_iter().enumerate() {
            let number = ns.windowNumber();
            ffi::on_main_after(n as f64 * 0.12 + k as f64 * 0.05, move || {
                let Some(w) = win(hwnd) else { return };
                let uptime = NSProcessInfo::processInfo().systemUptime();
                if let Some(e) = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
                    kind, at, flags, uptime, number, None, 0, n as isize + 1, 1.0,
                ) {
                    w.ns().sendEvent(&e);
                }
            });
        }
    }
}

pub fn test_front(hwnd: u64) {
    let (Some(d), Some(w)) = (desk(), win(hwnd)) else { return };
    let others: Vec<Rc<Win>> = d.wins.borrow().values().filter(|o| o.pane && o.hwnd != hwnd).cloned().collect();
    for o in others {
        w.ns().orderWindow_relativeTo(NSWindowOrderingMode::Above, o.ns().windowNumber());
    }
}

