//! deskwm: host windows of a Vibeshine desk as native macOS windows.
//! See deskwm.h for the C interface and desk.rs for how the two sides are
//! kept in step.

mod classes;
mod desk;
mod ffi;
mod input;
mod layout;

use std::ffi::{c_char, c_void, CStr};
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};

use objc2::rc::Retained;
use objc2::MainThreadMarker;
use objc2_app_kit::NSWindow;

use ffi::{guard, Callbacks, LAYERS};
/// The layer and geometry types of the C interface (deskwm.h), for Rust users
pub use ffi::{CLayer, Geo, Piece, TrackpadContact, GEO_MAX, PIECE_MAX};

static PPP_BITS: AtomicU64 = AtomicU64::new(0);
static FRONT: AtomicU64 = AtomicU64::new(0);
static DESK_W: AtomicI32 = AtomicI32::new(0);
static DESK_H: AtomicI32 = AtomicI32::new(0);
/// The desk itself (the Mac screen in host px); DESK_W/H are the stream's
/// size, the reference for pointer positions
static SCREEN_W: AtomicI32 = AtomicI32::new(0);
static SCREEN_H: AtomicI32 = AtomicI32::new(0);
static LAYOUT_SEQ: AtomicI32 = AtomicI32::new(-1);

pub(crate) fn set_layout_seq(seq: i32) {
    LAYOUT_SEQ.store(seq, Ordering::Release);
}

/// The newest layout number whose geometry is in (-1: none yet)
#[no_mangle]
pub extern "C" fn deskwm_layout_seq() -> i32 {
    LAYOUT_SEQ.load(Ordering::Acquire)
}

pub(crate) fn set_points_per_px(ppp: f64) {
    PPP_BITS.store(ppp.to_bits(), Ordering::Relaxed);
}

pub(crate) fn set_front_window(hwnd: u64) {
    FRONT.store(hwnd, Ordering::Relaxed);
}

/// # Safety
/// `key_window` is a live NSWindow; call on the main thread.
#[no_mangle]
pub unsafe extern "C" fn deskwm_begin(
    key_window: *mut c_void,
    screen_x: f64,
    screen_y: f64,
    screen_w: f64,
    screen_h: f64,
    points_per_px: f64,
    desk_w: i32,
    desk_h: i32,
    callbacks: Callbacks,
) {
    ffi::set_callbacks(Some(callbacks));
    guard("begin", || {
        let Some(mtm) = MainThreadMarker::new() else {
            ffi::log(2, "deskwm_begin: not on the main thread");
            return;
        };
        let Some(key) = (unsafe { Retained::retain(key_window as *mut NSWindow) }) else {
            ffi::log(2, "deskwm_begin: no key window");
            return;
        };
        DESK_W.store(desk_w, Ordering::Relaxed);
        DESK_H.store(desk_h, Ordering::Relaxed);
        if points_per_px > 0.0 {
            SCREEN_W.store((screen_w / points_per_px).round() as i32, Ordering::Relaxed);
            SCREEN_H.store((screen_h / points_per_px).round() as i32, Ordering::Relaxed);
        }
        desk::begin(
            mtm,
            key,
            desk::Mapping { sx: screen_x, sy: screen_y, sw: screen_w, sh: screen_h, ppp: points_per_px, desk_w, desk_h },
        );
    });
}

#[no_mangle]
pub extern "C" fn deskwm_end() {
    if MainThreadMarker::new().is_some() {
        guard("end", desk::end);
    } else {
        ffi::on_main(|| {
            guard("end", desk::end);
        });
    }
}

/// # Safety
/// `json` points to `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn deskwm_layout(json: *const c_char, len: usize) {
    if json.is_null() {
        return;
    }
    let bytes = unsafe { std::slice::from_raw_parts(json as *const u8, len) };
    let Some(layout) = layout::parse(bytes, SCREEN_W.load(Ordering::Relaxed), SCREEN_H.load(Ordering::Relaxed)) else {
        ffi::log(1, "deskwm: a layout that does not parse");
        return;
    };
    ffi::on_main(move || {
        guard("layout", || desk::apply_layout(layout.windows, layout.cursor, layout.seq, layout.pointer_lock));
    });
}

/// # Safety
/// `hit` is a NUL-terminated string (or null).
#[no_mangle]
pub unsafe extern "C" fn deskwm_hittest_result(token: u64, hit: *const c_char) {
    let hit = if hit.is_null() { "client".to_string() } else { unsafe { CStr::from_ptr(hit) }.to_string_lossy().into_owned() };
    ffi::on_main(move || {
        guard("hit result", || input::hit_result(token, &hit));
    });
}

/// # Safety
/// `key` is NUL-terminated; `png` points to `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn deskwm_set_icon(key: *const c_char, png: *const u8, len: usize) {
    if key.is_null() || png.is_null() || len == 0 {
        return;
    }
    let key = unsafe { CStr::from_ptr(key) }.to_string_lossy().into_owned();
    let png = unsafe { std::slice::from_raw_parts(png, len) }.to_vec();
    ffi::on_main(move || {
        guard("icon", || desk::set_icon(key, png));
    });
}

/// # Safety
/// `json` is NUL-terminated.
#[no_mangle]
pub unsafe extern "C" fn deskwm_command(json: *const c_char) {
    if json.is_null() {
        return;
    }
    let text = unsafe { CStr::from_ptr(json) }.to_string_lossy().into_owned();
    ffi::on_main(move || {
        guard("command", || command(&text));
    });
}

fn command(text: &str) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else { return };
    let num = |k: &str| v.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0);
    let hwnd = v.get("hwnd").and_then(|h| h.as_str()).map(layout::parse_hwnd).unwrap_or(0);
    match v.get("cmd").and_then(|c| c.as_str()).unwrap_or("") {
        "click" => input::test_click(hwnd, num("x") as i32, num("y") as i32, v.get("button").and_then(|b| b.as_i64()).unwrap_or(1) as i32),
        "scroll" => input::test_scroll(hwnd, num("x") as i32, num("y") as i32, v.get("dy").and_then(|x| x.as_f64()).unwrap_or(-3.0), num("n") as i32),
        "frame" => desk::test_frame(hwnd, num("x"), num("y"), num("w"), num("h")),
        "drag" => desk::test_drag(hwnd, num("dx"), num("dy")),
        "liveresize" => desk::test_live_resize(hwnd, num("dw"), num("dh"), v.get("left").and_then(|b| b.as_bool()).unwrap_or(false)),
        "tile" => desk::test_tile(hwnd, num("x"), num("y"), num("w"), num("h")),
        "front" => desk::test_front(hwnd),
        "relmove" => {
            // Test hook: motion as a locked pointer sends it (n steps of dx, dy points)
            if desk::pointer_locked() {
                for _ in 0..(num("n") as i32).max(1) {
                    desk::relative_motion(num("dx"), num("dy"));
                }
            } else {
                ffi::log(1, "deskwm: relmove while the pointer is not locked");
            }
        }
        "mousedrag" => input::test_mouse_drag(hwnd, num("x") as i32, num("y") as i32, num("dx") as i32, num("dy") as i32),
        "nsclick" => desk::test_nsclick(hwnd, num("x") as i32, num("y") as i32, v.get("command").and_then(|b| b.as_bool()).unwrap_or(false), num("count").max(1.0) as i32),
        other => ffi::log(1, &format!("deskwm: unknown test command {other}")),
    }
}

#[no_mangle]
pub extern "C" fn deskwm_front_window() -> u64 {
    FRONT.load(Ordering::Relaxed)
}

#[no_mangle]
pub extern "C" fn deskwm_points_per_px() -> f64 {
    f64::from_bits(PPP_BITS.load(Ordering::Relaxed))
}

/// # Safety
/// `f` is callable with `ctx`; it must not call back into deskwm.
#[no_mangle]
pub unsafe extern "C" fn deskwm_for_each_layer(f: Option<extern "C" fn(*mut c_void, *const CLayer)>, ctx: *mut c_void) {
    let Some(f) = f else { return };
    let layers = match LAYERS.lock() {
        Ok(l) => l,
        Err(poisoned) => poisoned.into_inner(),
    };
    for l in layers.iter() {
        let mut c = CLayer {
            hwnd: l.hwnd,
            metal_layer: l.metal,
            view_w: l.view_w,
            view_h: l.view_h,
            radius: l.radius,
            visible: l.visible,
            anchor_right: l.anchor_right,
            anchor_bottom: l.anchor_bottom,
            cap_w: l.cap.0,
            cap_h: l.cap.1,
            geo_count: l.geo.len().min(GEO_MAX) as u32,
            geo: [Geo::default(); GEO_MAX],
        };
        for (k, g) in l.geo.iter().take(GEO_MAX).enumerate() {
            c.geo[k] = *g;
        }
        f(ctx, &c);
    }
}

#[no_mangle]
pub extern "C" fn deskwm_shown(hwnd: u64, w: i32, h: i32) {
    let changed = ffi::with_layer(hwnd, |l| {
        let changed = l.shown != (w, h);
        l.shown = (w, h);
        changed
    })
    .unwrap_or(false);
    if changed {
        ffi::on_main(move || {
            guard("shown", || desk::shown_changed(hwnd));
        });
    }
}
