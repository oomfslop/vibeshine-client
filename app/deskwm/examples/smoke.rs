//! Offline smoke test: deskwm against a synthetic desk, no host needed.
//! Windows are placed far off-screen and the app never activates.
//! cargo run --release --example smoke

use deskwm as _;  // link the library (its C symbols are what this uses)
use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::Mutex;

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSWindow, NSWindowStyleMask};
use objc2_foundation::{NSDate, NSPoint, NSRect, NSRunLoop, NSSize};

static CALLS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static INPUT: Mutex<Vec<String>> = Mutex::new(Vec::new());

#[no_mangle]
pub extern "C" fn LiSendMousePositionEvent(x: i16, y: i16, w: i16, h: i16) -> i32 {
    INPUT.lock().unwrap().push(format!("pos {x},{y} of {w}x{h}"));
    0
}
#[no_mangle]
pub extern "C" fn LiSendMouseButtonEvent(action: c_char, button: i32) -> i32 {
    INPUT.lock().unwrap().push(format!("{} {button}", if action == 7 { "press" } else { "release" }));
    0
}
#[no_mangle]
pub extern "C" fn LiSendKeyboardEvent2(code: i16, action: c_char, _mods: c_char, _flags: c_char) -> i32 {
    INPUT.lock().unwrap().push(format!("key 0x{:x} {}", code as u16 & 0x7fff, if action == 3 { "down" } else { "up" }));
    0
}
#[no_mangle]
pub extern "C" fn LiSendHighResScrollEvent(a: i16) -> i32 {
    INPUT.lock().unwrap().push(format!("scroll {a}"));
    0
}
#[no_mangle]
pub extern "C" fn LiSendHighResHScrollEvent(a: i16) -> i32 {
    INPUT.lock().unwrap().push(format!("hscroll {a}"));
    0
}

extern "C" {
    fn deskwm_begin(key: *mut c_void, x: f64, y: f64, w: f64, h: f64, ppp: f64, dw: i32, dh: i32, cb: Callbacks);
    fn deskwm_end();
    fn deskwm_layout(json: *const c_char, len: usize);
    fn deskwm_hittest_result(token: u64, hit: *const c_char);
    fn deskwm_command(json: *const c_char);
    fn deskwm_for_each_layer(f: Option<extern "C" fn(*mut c_void, *const Layer)>, ctx: *mut c_void);
}

#[repr(C)]
struct Callbacks {
    ctx: *mut c_void,
    window_op: Option<extern "C" fn(*mut c_void, u64, *const c_char, i32, i32, i32, i32)>,
    order: Option<extern "C" fn(*mut c_void, *const u64, usize, bool)>,
    hittest: Option<extern "C" fn(*mut c_void, u64, i32, i32, u64)>,
    event: Option<extern "C" fn(*mut c_void, *const c_char)>,
    log: Option<extern "C" fn(*mut c_void, i32, *const c_char)>,
    redraw: Option<extern "C" fn(*mut c_void, u64)>,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct Geo {
    ts: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    ax: i32,
    ay: i32,
    scale: f32,
}

#[repr(C)]
struct Layer {
    hwnd: u64,
    metal_layer: *mut c_void,
    view_w: i32,
    view_h: i32,
    radius: i32,
    visible: bool,
    anchor_right: bool,
    anchor_bottom: bool,
    geo_count: u32,
    geo: [Geo; 12],
}

static PENDING_HIT: Mutex<Option<(u64, u64, i32, i32)>> = Mutex::new(None);

extern "C" fn op(_: *mut c_void, hwnd: u64, op: *const c_char, x: i32, y: i32, w: i32, h: i32) {
    let op = unsafe { CStr::from_ptr(op) }.to_string_lossy();
    CALLS.lock().unwrap().push(format!("op 0x{hwnd:x} {op} {x},{y} {w}x{h}"));
}
extern "C" fn order(_: *mut c_void, hwnds: *const u64, n: usize, focus: bool) {
    let list: Vec<String> = unsafe { std::slice::from_raw_parts(hwnds, n) }.iter().map(|h| format!("0x{h:x}")).collect();
    CALLS.lock().unwrap().push(format!("order {} focus={focus}", list.join(",")));
}
extern "C" fn hittest(_: *mut c_void, hwnd: u64, x: i32, y: i32, token: u64) {
    CALLS.lock().unwrap().push(format!("hittest 0x{hwnd:x} {x},{y}"));
    *PENDING_HIT.lock().unwrap() = Some((token, hwnd, x, y));
}
extern "C" fn event(_: *mut c_void, json: *const c_char) {
    CALLS.lock().unwrap().push(format!("event {}", unsafe { CStr::from_ptr(json) }.to_string_lossy()));
}
extern "C" fn log(_: *mut c_void, level: i32, m: *const c_char) {
    println!("  [log{level}] {}", unsafe { CStr::from_ptr(m) }.to_string_lossy());
}

extern "C" fn each(ctx: *mut c_void, l: *const Layer) {
    let out = unsafe { &mut *(ctx as *mut Vec<String>) };
    let l = unsafe { &*l };
    let g = l.geo[(l.geo_count.max(1) - 1) as usize];
    out.push(format!(
        "layer 0x{:x} view {}x{} r{} vis={} anchors={}{} geo#{} slot {},{} {}x{} alpha {},{}",
        l.hwnd, l.view_w, l.view_h, l.radius, l.visible, l.anchor_right as u8, l.anchor_bottom as u8, l.geo_count, g.x, g.y, g.w, g.h, g.ax, g.ay
    ));
}

fn pump(secs: f64) {
    let until = NSDate::dateWithTimeIntervalSinceNow(secs);
    NSRunLoop::currentRunLoop().runUntilDate(&until);
}

fn layout(json: &str) {
    unsafe { deskwm_layout(json.as_ptr() as *const c_char, json.len()) };
    pump(0.15);
}

fn command(json: &str) {
    let c = CString::new(json).unwrap();
    unsafe { deskwm_command(c.as_ptr()) };
}

fn take(m: &Mutex<Vec<String>>) -> Vec<String> {
    std::mem::take(&mut *m.lock().unwrap())
}

fn show(title: &str) {
    println!("--- {title}");
    for c in take(&CALLS) {
        println!("  {c}");
    }
    let input = take(&INPUT);
    if !input.is_empty() {
        println!("  input: {}", input.join(" | "));
    }
}

fn mac_order(mtm: MainThreadMarker) -> String {
    let app = NSApplication::sharedApplication(mtm);
    app.orderedWindows().iter().map(|w| w.title().to_string()).collect::<Vec<_>>().join(" > ")
}

fn app_frame(mtm: MainThreadMarker, title: &str) -> Option<(f64, f64, f64, f64)> {
    let app = NSApplication::sharedApplication(mtm);
    app.windows().iter().find(|w| w.title().to_string() == title).map(|w| {
        let f = w.frame();
        (f.origin.x, f.origin.y, f.size.width, f.size.height)
    })
}

fn layers() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    unsafe { deskwm_for_each_layer(Some(each), &mut out as *mut _ as *mut c_void) };
    out
}

fn win(hwnd: &str, kind: &str, owner: &str, x: i32, y: i32, w: i32, h: i32, focused: bool, slot: (i32, i32), alpha: (i32, i32)) -> String {
    format!(
        r#"{{"hwnd":"{hwnd}","owner":"{owner}","kind":"{kind}","rect":[{x},{y},{w},{h}],"state":"normal","focused":{focused},"caption":0,"radius":16,"title":"{hwnd}","icon":"","slot":[{},{},{w},{h}],"alpha_slot":[{},{}],"scale":1.0}}"#,
        slot.0, slot.1, alpha.0, alpha.1
    )
}

fn main() {
    let mtm = MainThreadMarker::new().unwrap();
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    let key: Retained<NSWindow> = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            NSRect::new(NSPoint::new(30000.0, 30000.0), NSSize::new(10.0, 10.0)),
            NSWindowStyleMask::Borderless,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    let cb = Callbacks { ctx: std::ptr::null_mut(), window_op: Some(op), order: Some(order), hittest: Some(hittest), event: Some(event), log: Some(log), redraw: None };
    // A mirrored "screen" far off to the right: nothing shows on the real one
    unsafe { deskwm_begin(Retained::as_ptr(&key) as *mut c_void, 30000.0, 0.0, 1512.0, 982.0, 0.5, 3840, 2160, cb) };
    pump(0.2);

    let a = win("0xa", "window", "0x0", 100, 100, 800, 600, false, (0, 0), (-1, -1));
    let b = win("0xb", "window", "0x0", 500, 400, 900, 700, true, (900, 0), (-1, -1));
    layout(&format!(r#"{{"ts":1000,"cursor":"arrow","windows":[{a},{b}]}}"#));
    show("two windows");
    println!("  AppKit front-to-back: {}", mac_order(mtm));
    for l in layers() {
        println!("  {l}");
    }

    // A click in the client area of A (not front on the host: covered by B there?)
    command(r#"{"cmd":"click","hwnd":"0xa","x":50,"y":300,"button":1}"#);
    pump(0.5);
    show("click A client area (covered on host by B? no: B starts at 500,400)");

    // A title-bar press on B: waits for the host's hit test
    command(r#"{"cmd":"click","hwnd":"0xb","x":300,"y":20,"button":1}"#);
    pump(0.1);
    if let Some((token, ..)) = PENDING_HIT.lock().unwrap().take() {
        let hit = CString::new("drag").unwrap();
        unsafe { deskwm_hittest_result(token, hit.as_ptr()) };
    }
    pump(0.3);
    show("caption click on B answered drag (released before the answer: a click)");

    // A test drag of B: the drop goes to the host as adopt
    command(r#"{"cmd":"drag","hwnd":"0xb","dx":100,"dy":50}"#);
    pump(0.8);
    show("drag B by 100,50 pt");

    // The host shows the move; then a popup of B appears (menu with alpha)
    let b2 = win("0xb", "window", "0x0", 700, 500, 900, 700, true, (900, 0), (-1, -1));
    let p = win("0xc", "popup", "0xb", 750, 600, 300, 400, false, (0, 800), (400, 800));
    layout(&format!(r#"{{"ts":2000,"cursor":"ibeam","windows":[{a},{b2},{p}]}}"#));
    show("host confirms the move, a menu opens");
    for l in layers() {
        println!("  {l}");
    }

    // A live resize of A from its left edge
    command(r#"{"cmd":"liveresize","hwnd":"0xa","dw":-100,"dh":50,"left":true}"#);
    pump(0.8);
    show("live resize of A from the left (menus of A: none)");

    // The host focuses A by itself (e.g. a file opened in it)
    let a_focused = win("0xa", "window", "0x0", 100, 100, 800, 600, true, (0, 0), (-1, -1));
    let b3 = win("0xb", "window", "0x0", 700, 500, 900, 700, false, (900, 0), (-1, -1));
    layout(&format!(r#"{{"ts":3000,"cursor":"arrow","windows":[{b3},{a_focused}]}}"#));
    pump(0.4);
    show("host focuses and raises A by itself");

    // B closes on the host
    layout(&format!(r#"{{"ts":4000,"cursor":"arrow","windows":[{a_focused}]}}"#));
    pump(0.8);
    show("B gone");
    println!("  layers now: {}", layers().len());

    // --- more paths -------------------------------------------------------------
    let c1 = win("0x10", "window", "0x0", 200, 200, 1000, 800, true, (0, 0), (-1, -1));
    let c2 = win("0x11", "window", "0x0", 400, 300, 1000, 800, false, (1100, 0), (-1, -1));
    let menu = win("0x12", "popup", "0x10", 300, 300, 300, 300, false, (0, 900), (400, 900));
    layout(&format!(r#"{{"ts":5000,"cursor":"arrow","windows":[{c2},{c1},{menu}]}}"#));
    pump(0.3);
    show("C1 (focused, top on host) over C2, C1 has a menu");

    command(r#"{"cmd":"drag","hwnd":"0x10","dx":20,"dy":0}"#);
    pump(0.8);
    show("drag C1: its menu is dismissed, then the drop");

    // Press C2 where C1 covers it on the host (500,400 is inside both)
    command(r#"{"cmd":"click","hwnd":"0x11","x":100,"y":200,"button":1}"#);
    pump(0.1);
    show("press on C2 at a spot C1 covers on the host: held, order pushed");
    // The host raises C2
    let c1b = win("0x10", "window", "0x0", 240, 200, 1000, 800, false, (0, 0), (-1, -1));
    let c2b = win("0x11", "window", "0x0", 400, 300, 1000, 800, true, (1100, 0), (-1, -1));
    layout(&format!(r#"{{"ts":6000,"cursor":"arrow","windows":[{c1b},{c2b}]}}"#));
    pump(0.3);
    show("host raised C2: the held press goes now");

    // The minimize button of C2 (hit test says min)
    command(r#"{"cmd":"click","hwnd":"0x11","x":900,"y":10,"button":1}"#);
    pump(0.05);
    if let Some((token, ..)) = PENDING_HIT.lock().unwrap().take() {
        let hit = CString::new("min").unwrap();
        unsafe { deskwm_hittest_result(token, hit.as_ptr()) };
    }
    pump(0.6);
    show("minimize button on C2");

    // macOS tiling of C1 into the left half (top-left origin points)
    command(r#"{"cmd":"tile","hwnd":"0x10","x":30000,"y":0,"w":756,"h":982}"#);
    pump(0.5);
    show("tile C1 to the left half");

    // The host moves C1 by itself
    let c1c = win("0x10", "window", "0x0", 0, 0, 1512, 1964, true, (0, 0), (-1, -1));
    layout(&format!(r#"{{"ts":7000,"cursor":"hand","windows":[{c1c}]}}"#));
    pump(2.0);
    let c1d = win("0x10", "window", "0x0", 50, 60, 1512, 1964, true, (0, 0), (-1, -1));
    layout(&format!(r#"{{"ts":8000,"cursor":"hand","windows":[{c1d}]}}"#));
    pump(0.3);
    show("host confirms tiling, then moves C1 by itself to 50,60");
    println!("  C1 frame: {:?}", app_frame(mtm, "0x10"));

    // A popup flickers out and back within a frame
    let pop = win("0x13", "popup", "0x10", 100, 100, 200, 100, false, (0, 2000), (300, 2000));
    layout(&format!(r#"{{"ts":9000,"cursor":"arrow","windows":[{c1d},{pop}]}}"#));
    layout(&format!(r#"{{"ts":9001,"cursor":"arrow","windows":[{c1d}]}}"#));
    layout(&format!(r#"{{"ts":9002,"cursor":"arrow","windows":[{c1d},{pop}]}}"#));
    pump(0.8);
    show("popup flicker");
    println!("  layers: {}", layers().join(" / "));

    unsafe { deskwm_end() };
    pump(0.1);
    println!("--- end: layers {}", layers().len());
}
