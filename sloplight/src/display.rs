//! Connected displays and their modes, for offering "stream at the size of
//! this screen" presets. Only macOS is implemented (CoreGraphics); elsewhere
//! the list is empty and the UI shows the generic presets only.

#[derive(Clone, Debug, PartialEq)]
pub struct DisplayInfo {
    pub name: String,
    pub builtin: bool,
    /// Largest pixel mode the panel supports (its native resolution).
    pub native: (i32, i32),
    /// Backing pixel size of the mode currently in use.
    pub current: (i32, i32),
    /// Logical (point) size of the current mode: what the UI "looks like".
    pub points: (i32, i32),
    /// Refresh rate of the current mode, 0 when unknown (some panels report 0).
    pub refresh_hz: i32,
    /// Native width with the height below the camera housing, for built-in
    /// displays with a notch (macOS full-screen Spaces use this area).
    pub below_notch: Option<(i32, i32)>,
}

impl DisplayInfo {
    /// Distinct (label, size) choices for this display, native first.
    pub fn resolutions(&self) -> Vec<(String, (i32, i32))> {
        let mut out: Vec<(String, (i32, i32))> = Vec::new();
        let mut push = |label: &str, size: (i32, i32)| {
            if size.0 > 0 && size.1 > 0 && !out.iter().any(|(_, s)| *s == size) {
                out.push((format!("{} · {label}", self.name), size));
            }
        };
        push("Native", self.native);
        if let Some(b) = self.below_notch {
            push("Below notch", b);
        }
        push("Current", self.current);
        push("Points", self.points);
        out
    }
}

pub fn displays() -> Vec<DisplayInfo> {
    #[cfg(target_os = "macos")]
    {
        macos::displays()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Vec::new()
    }
}

/// Every display-derived resolution choice, de-duplicated across displays.
pub fn display_resolutions() -> Vec<(String, (i32, i32))> {
    let mut out: Vec<(String, (i32, i32))> = Vec::new();
    for d in displays() {
        for (label, size) in d.resolutions() {
            if !out.iter().any(|(_, s)| *s == size) {
                out.push((label, size));
            }
        }
    }
    out
}

/// Refresh rates of connected displays (deduplicated, ascending).
pub fn display_refresh_rates() -> Vec<(String, i32)> {
    let mut out: Vec<(String, i32)> = Vec::new();
    for d in displays() {
        if d.refresh_hz > 0 && !out.iter().any(|(_, r)| *r == d.refresh_hz) {
            out.push((d.name.clone(), d.refresh_hz));
        }
    }
    out.sort_by_key(|(_, r)| *r);
    out
}

#[cfg(target_os = "macos")]
mod macos {
    use super::DisplayInfo;
    use std::ffi::c_void;

    type CGDirectDisplayID = u32;
    type CGDisplayModeRef = *mut c_void;
    type CFArrayRef = *const c_void;
    type CFTypeRef = *const c_void;
    type CFIndex = isize;

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGGetActiveDisplayList(max: u32, displays: *mut CGDirectDisplayID, count: *mut u32) -> i32;
        fn CGMainDisplayID() -> CGDirectDisplayID;
        fn CGDisplayIsBuiltin(display: CGDirectDisplayID) -> i32;
        fn CGDisplayCopyDisplayMode(display: CGDirectDisplayID) -> CGDisplayModeRef;
        fn CGDisplayCopyAllDisplayModes(display: CGDirectDisplayID, options: CFTypeRef) -> CFArrayRef;
        fn CGDisplayModeGetWidth(mode: CGDisplayModeRef) -> usize;
        fn CGDisplayModeGetHeight(mode: CGDisplayModeRef) -> usize;
        fn CGDisplayModeGetPixelWidth(mode: CGDisplayModeRef) -> usize;
        fn CGDisplayModeGetPixelHeight(mode: CGDisplayModeRef) -> usize;
        fn CGDisplayModeGetRefreshRate(mode: CGDisplayModeRef) -> f64;
        fn CGDisplayModeRelease(mode: CGDisplayModeRef);
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFArrayGetCount(array: CFArrayRef) -> CFIndex;
        fn CFArrayGetValueAtIndex(array: CFArrayRef, idx: CFIndex) -> *const c_void;
        fn CFRelease(cf: CFTypeRef);
    }

    pub fn displays() -> Vec<DisplayInfo> {
        let mut ids = [0u32; 16];
        let mut count = 0u32;
        // SAFETY: plain C calls with a correctly sized output buffer; every
        // CF/CG object we copy is released before returning.
        unsafe {
            if CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut count) != 0 {
                return Vec::new();
            }
            let main = CGMainDisplayID();
            let mut out = Vec::new();
            let mut external = 0;
            for &id in &ids[..count as usize] {
                let mode = CGDisplayCopyDisplayMode(id);
                if mode.is_null() {
                    continue;
                }
                let points = (CGDisplayModeGetWidth(mode) as i32, CGDisplayModeGetHeight(mode) as i32);
                let current = (CGDisplayModeGetPixelWidth(mode) as i32, CGDisplayModeGetPixelHeight(mode) as i32);
                let refresh_hz = CGDisplayModeGetRefreshRate(mode).round() as i32;
                CGDisplayModeRelease(mode);

                let builtin = CGDisplayIsBuiltin(id) != 0;
                let mut native = current;
                let mut sizes: Vec<(i32, i32)> = Vec::new();
                let modes = CGDisplayCopyAllDisplayModes(id, std::ptr::null());
                if !modes.is_null() {
                    for i in 0..CFArrayGetCount(modes) {
                        let m = CFArrayGetValueAtIndex(modes, i) as CGDisplayModeRef;
                        let size = (CGDisplayModeGetPixelWidth(m) as i32, CGDisplayModeGetPixelHeight(m) as i32);
                        if (size.0 as i64 * size.1 as i64) > (native.0 as i64 * native.1 as i64) {
                            native = size;
                        }
                        sizes.push(size);
                    }
                    CFRelease(modes);
                }
                // Same heuristic as Moonlight's StreamUtils::getNativeDesktopMode: on a
                // built-in panel, a mode with the native width but a slightly smaller
                // height is the "below the camera housing" mode.
                let below_notch = if builtin {
                    sizes.iter().copied().filter(|s| s.0 == native.0 && s.1 < native.1 && s.1 + 200 >= native.1).max_by_key(|s| s.1)
                } else {
                    None
                };
                let name = if builtin {
                    "Built-in display".to_string()
                } else {
                    external += 1;
                    if id == main { format!("External display {external} (main)") } else { format!("External display {external}") }
                };
                out.push(DisplayInfo { name, builtin, native, current, points, refresh_hz, below_notch });
            }
            // Built-in / main first so the most likely target is on top.
            out.sort_by_key(|d| !d.builtin);
            out
        }
    }
}


