//! What crosses into C: the helper's callbacks, the renderer's layers, the
//! host input calls of moonlight-common-c, and running code on the main thread.

use std::ffi::{c_char, c_void, CString};
use std::sync::Mutex;
use std::time::Duration;

use dispatch2::{DispatchQueue, DispatchTime};

#[repr(C)]
#[derive(Copy, Clone)]
pub struct Callbacks {
    pub ctx: *mut c_void,
    pub window_op: Option<extern "C" fn(*mut c_void, u64, *const c_char, i32, i32, i32, i32)>,
    pub order: Option<extern "C" fn(*mut c_void, *const u64, usize, bool)>,
    pub hittest: Option<extern "C" fn(*mut c_void, u64, i32, i32, u64)>,
    pub event: Option<extern "C" fn(*mut c_void, *const c_char)>,
    pub log: Option<extern "C" fn(*mut c_void, i32, *const c_char)>,
    pub redraw: Option<extern "C" fn(*mut c_void, u64)>,
}

// The helper's callbacks are thread-safe by contract (see deskwm.h)
unsafe impl Send for Callbacks {}

static CALLBACKS: Mutex<Option<Callbacks>> = Mutex::new(None);

pub fn set_callbacks(cb: Option<Callbacks>) {
    *CALLBACKS.lock().unwrap() = cb;
}

fn callbacks() -> Option<Callbacks> {
    *CALLBACKS.lock().unwrap()
}

/// Draw the window again now, from the latest frame (main thread, in step
/// with its new size)
pub fn redraw(hwnd: u64) {
    if let Some(cb) = callbacks() {
        if let Some(f) = cb.redraw {
            f(cb.ctx, hwnd);
        }
    }
}

pub fn log(level: i32, message: &str) {
    if let Some(cb) = callbacks() {
        if let (Some(f), Ok(text)) = (cb.log, CString::new(message.replace('\0', " "))) {
            f(cb.ctx, level, text.as_ptr());
        }
    }
}

macro_rules! info {
    ($($arg:tt)*) => { $crate::ffi::log(0, &format!($($arg)*)) };
}
pub(crate) use info;

pub fn window_op(hwnd: u64, op: &str, x: i32, y: i32, w: i32, h: i32) {
    if let Some(cb) = callbacks() {
        if let (Some(f), Ok(text)) = (cb.window_op, CString::new(op)) {
            f(cb.ctx, hwnd, text.as_ptr(), x, y, w, h);
        }
    }
}

pub fn order(top_first: &[u64], focus: bool) {
    if let Some(cb) = callbacks() {
        if let Some(f) = cb.order {
            f(cb.ctx, top_first.as_ptr(), top_first.len(), focus);
        }
    }
}

pub fn hittest(hwnd: u64, x: i32, y: i32, token: u64) -> bool {
    match callbacks() {
        Some(Callbacks { hittest: Some(f), ctx, .. }) => {
            f(ctx, hwnd, x, y, token);
            true
        }
        _ => false,
    }
}

pub fn event(json: &str) {
    if let Some(cb) = callbacks() {
        if let (Some(f), Ok(text)) = (cb.event, CString::new(json)) {
            f(cb.ctx, text.as_ptr());
        }
    }
}

// Host input: moonlight-common-c, linked into the helper
extern "C" {
    fn LiSendMousePositionEvent(x: i16, y: i16, reference_width: i16, reference_height: i16) -> i32;
    fn LiSendMouseButtonEvent(action: c_char, button: i32) -> i32;
    fn LiSendHighResScrollEvent(amount: i16) -> i32;
    fn LiSendHighResHScrollEvent(amount: i16) -> i32;
    fn LiSendKeyboardEvent2(key_code: i16, key_action: c_char, modifiers: c_char, flags: c_char) -> i32;
}

/// Ctrl held on the host around a click: Cmd-click on the Mac (add to the
/// selection) is Ctrl-click on Windows
pub fn host_ctrl(down: bool) {
    const VK_LCONTROL: i16 = 0xA2 | (0x8000u16 as i16);
    unsafe {
        LiSendKeyboardEvent2(VK_LCONTROL, if down { 0x03 } else { 0x04 }, if down { 0x02 } else { 0 }, 0);
    }
}

pub const BUTTON_ACTION_PRESS: c_char = 0x07;
pub const BUTTON_ACTION_RELEASE: c_char = 0x08;

pub fn mouse_to(x: i32, y: i32, desk_w: i32, desk_h: i32) {
    let x = x.clamp(0, (desk_w - 1).max(0)) as i16;
    let y = y.clamp(0, (desk_h - 1).max(0)) as i16;
    unsafe {
        LiSendMousePositionEvent(x, y, desk_w as i16, desk_h as i16);
    }
}

pub fn mouse_button(press: bool, button: i32) {
    unsafe {
        LiSendMouseButtonEvent(if press { BUTTON_ACTION_PRESS } else { BUTTON_ACTION_RELEASE }, button);
    }
}

pub fn scroll(dy: i16, dx: i16) {
    unsafe {
        if dy != 0 {
            LiSendHighResScrollEvent(dy);
        }
        if dx != 0 {
            LiSendHighResHScrollEvent(dx);
        }
    }
}

// --- The renderer's view of the windows --------------------------------------

#[repr(C)]
#[derive(Copy, Clone, PartialEq)]
pub struct Geo {
    pub ts: u32,
    /// The layout number this geometry came with (-1: none)
    pub seq: i32,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub ax: i32,
    pub ay: i32,
    pub scale: f32,
    pub alpha_scale: f32,
    pub piece_count: u32,
    pub pieces: [Piece; PIECE_MAX],
}

/// Part of a picture also in the frame 1:1: x, y, w, h in the picture (host
/// px), ax, ay where it is in the frame
#[repr(C)]
#[derive(Copy, Clone, Default, PartialEq)]
pub struct Piece {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub ax: i32,
    pub ay: i32,
}

pub const PIECE_MAX: usize = 192;

impl Default for Geo {
    fn default() -> Self {
        Geo { ts: 0, seq: -1, x: 0, y: 0, w: 0, h: 0, ax: 0, ay: 0, scale: 0.0, alpha_scale: 1.0, piece_count: 0, pieces: [Piece::default(); PIECE_MAX] }
    }
}

pub const GEO_MAX: usize = 12;

#[repr(C)]
pub struct CLayer {
    pub hwnd: u64,
    pub metal_layer: *mut c_void,
    pub view_w: i32,
    pub view_h: i32,
    pub radius: i32,
    pub visible: bool,
    pub anchor_right: bool,
    pub anchor_bottom: bool,
    pub cap_w: i32,
    pub cap_h: i32,
    pub geo_count: u32,
    pub geo: [Geo; GEO_MAX],
}

/// One window's layer as the renderer sees it. `metal` is a retained
/// CAMetalLayer (released when the entry goes).
pub struct Layer {
    pub hwnd: u64,
    pub metal: *mut c_void,
    pub view_w: i32,
    pub view_h: i32,
    pub radius: i32,
    pub visible: bool,
    pub anchor_right: bool,
    pub anchor_bottom: bool,
    pub cap: (i32, i32),
    pub geo: Vec<Geo>,
    pub shown: (i32, i32),
}

unsafe impl Send for Layer {}

extern "C" {
    fn CFRetain(cf: *const c_void) -> *const c_void;
    fn CFRelease(cf: *const c_void);
}

impl Drop for Layer {
    fn drop(&mut self) {
        if !self.metal.is_null() {
            unsafe { CFRelease(self.metal) };
        }
    }
}

pub static LAYERS: Mutex<Vec<Layer>> = Mutex::new(Vec::new());

pub fn add_layer(hwnd: u64, metal: *mut c_void, radius: i32) {
    let metal = if metal.is_null() { metal } else { unsafe { CFRetain(metal) as *mut c_void } };
    let mut layers = LAYERS.lock().unwrap();
    layers.retain(|l| l.hwnd != hwnd);
    layers.push(Layer {
        hwnd,
        metal,
        view_w: 0,
        view_h: 0,
        radius,
        visible: true,
        anchor_right: false,
        anchor_bottom: false,
        cap: (0, 0),
        geo: Vec::new(),
        shown: (0, 0),
    });
}

pub fn remove_layer(hwnd: u64) {
    LAYERS.lock().unwrap().retain(|l| l.hwnd != hwnd);
}

pub fn clear_layers() {
    LAYERS.lock().unwrap().clear();
}

pub fn with_layer<R>(hwnd: u64, f: impl FnOnce(&mut Layer) -> R) -> Option<R> {
    let mut layers = LAYERS.lock().unwrap();
    layers.iter_mut().find(|l| l.hwnd == hwnd).map(f)
}

/// Each frame is cut with the geometry that was true when it was captured:
/// the history keeps the last few, stamped on the frames' clock.
pub fn push_geo(hwnd: u64, g: Geo) {
    with_layer(hwnd, |l| {
        if let Some(last) = l.geo.last() {
            // Unchanged: the older entry stays valid for the newer frames
            let same = Geo { ts: last.ts, seq: last.seq, ..g };
            if *last == same {
                return;
            }
        }
        l.geo.push(g);
        if l.geo.len() > GEO_MAX {
            l.geo.remove(0);
        }
    });
}

pub fn shown(hwnd: u64) -> (i32, i32) {
    with_layer(hwnd, |l| l.shown).unwrap_or((0, 0))
}

// --- Threads -------------------------------------------------------------------

/// Run on the main thread (AppKit), soon.
pub fn on_main(f: impl FnOnce() + Send + 'static) {
    DispatchQueue::main().exec_async(f);
}

/// Run on the main thread after `secs`.
pub fn on_main_after(secs: f64, f: impl FnOnce() + Send + 'static) {
    let when = DispatchTime::try_from(Duration::from_secs_f64(secs.max(0.0))).unwrap_or(DispatchTime::NOW);
    let _ = DispatchQueue::main().after(when, f);
}

/// Seconds on a monotonic clock (CACurrentMediaTime's).
pub fn now() -> f64 {
    extern "C" {
        fn CACurrentMediaTime() -> f64;
    }
    unsafe { CACurrentMediaTime() }
}

/// Run `f`, logging instead of unwinding into C or Objective-C.
pub fn guard<R>(what: &str, f: impl FnOnce() -> R) -> Option<R> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(r) => Some(r),
        Err(e) => {
            let msg = e
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| e.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panic".into());
            log(2, &format!("deskwm {what}: {msg}"));
            None
        }
    }
}
