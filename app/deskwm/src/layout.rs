//! The host's view of the desk, as /actions/desk-layout reports it.

use serde::Deserialize;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Captioned (or frameless app window), unowned
    Window,
    /// Captioned, owned
    Dialog,
    /// Caption-less: menus, tooltips, drop-downs
    Popup,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Info {
    pub hwnd: u64,
    pub owner: u64,
    pub kind: Kind,
    /// Desk-relative outer rect (DWM frame), host px
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub minimized: bool,
    pub focused: bool,
    pub caption: i32,
    pub radius: i32,
    pub title: String,
    pub icon_key: String,
    /// Its picture in the frame (the atlas slot); w = 0: none
    pub slot: (i32, i32, i32, i32),
    /// The same picture on white, for exact alpha (-1: opaque)
    pub alpha: (i32, i32),
    pub scale: f32,
    /// When the host saw this, on the video frames' clock
    pub ts: u32,
}

impl Info {
    pub fn is_pane(&self) -> bool {
        self.kind != Kind::Popup
    }
}

#[derive(Deserialize)]
struct RawWindow {
    hwnd: String,
    #[serde(default)]
    owner: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    rect: Vec<i32>,
    #[serde(default)]
    state: String,
    #[serde(default)]
    focused: bool,
    #[serde(default)]
    caption: i32,
    #[serde(default)]
    radius: i32,
    #[serde(default)]
    title: String,
    #[serde(default)]
    icon: String,
    #[serde(default)]
    slot: Vec<i32>,
    #[serde(default)]
    alpha_slot: Vec<i32>,
    #[serde(default)]
    scale: Option<f64>,
}

#[derive(Deserialize)]
struct RawLayout {
    #[serde(default)]
    windows: Vec<RawWindow>,
    #[serde(default)]
    ts: f64,
    #[serde(default)]
    cursor: String,
}

pub struct Layout {
    pub windows: Vec<Info>,
    /// The host pointer's shape (arrow, ibeam, hand, we, ns, nwse, nesw, ...)
    pub cursor: String,
}

pub fn parse_hwnd(s: &str) -> u64 {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).unwrap_or(0)
    } else {
        s.parse().unwrap_or(0)
    }
}

/// The windows of a layout reply, bottom-most first. Full-desk popups (the
/// light-dismiss layer some XAML menus put behind them) are left out: they
/// are not windows anyone sees.
pub fn parse(json: &[u8], desk_w: i32, desk_h: i32) -> Option<Layout> {
    let raw: RawLayout = serde_json::from_slice(json).ok()?;
    let ts = raw.ts as i64 as u32;
    let mut out = Vec::with_capacity(raw.windows.len());
    for w in raw.windows {
        let hwnd = parse_hwnd(&w.hwnd);
        if hwnd == 0 || w.rect.len() != 4 || w.rect[2] <= 0 || w.rect[3] <= 0 {
            continue;
        }
        let kind = match w.kind.as_str() {
            "window" => Kind::Window,
            "dialog" => Kind::Dialog,
            _ => Kind::Popup,
        };
        if kind == Kind::Popup && (w.rect[2] as i64) * (w.rect[3] as i64) >= (desk_w as i64) * (desk_h as i64) * 6 / 10 {
            continue;
        }
        let slot = if w.slot.len() == 4 { (w.slot[0], w.slot[1], w.slot[2], w.slot[3]) } else { (0, 0, 0, 0) };
        let alpha = if w.alpha_slot.len() == 2 { (w.alpha_slot[0], w.alpha_slot[1]) } else { (-1, -1) };
        out.push(Info {
            hwnd,
            owner: parse_hwnd(&w.owner),
            kind,
            x: w.rect[0],
            y: w.rect[1],
            w: w.rect[2],
            h: w.rect[3],
            minimized: w.state == "minimized",
            focused: w.focused,
            caption: w.caption,
            radius: w.radius,
            title: w.title,
            icon_key: w.icon,
            slot,
            alpha,
            scale: w.scale.unwrap_or(1.0) as f32,
            ts,
        });
    }
    Some(Layout { windows: out, cursor: raw.cursor })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_windows_slots_and_cursor() {
        let json = br#"{"etag":"1","ts":123456,"cursor":"ibeam","windows":[
            {"hwnd":"0x430696","owner":"0x0","kind":"window","rect":[702,500,1441,1002],"state":"normal","focused":true,
             "caption":0,"radius":16,"title":"Explorer","icon":"k1","slot":[1664,0,1441,1002],"alpha_slot":[-1,-1],"scale":1.0},
            {"hwnd":"0x1e0e02","owner":"0x430696","kind":"popup","rect":[800,600,544,444],"state":"normal","focused":false,
             "slot":[0,1200,544,444],"alpha_slot":[600,1200],"scale":1.0}]}"#;
        let l = parse(json, 3840, 2160).expect("parses");
        assert_eq!(l.cursor, "ibeam");
        assert_eq!(l.windows.len(), 2);
        let w = &l.windows[0];
        assert_eq!(w.hwnd, 0x430696);
        assert_eq!(w.kind, Kind::Window);
        assert!(w.is_pane() && w.focused && !w.minimized);
        assert_eq!((w.x, w.y, w.w, w.h), (702, 500, 1441, 1002));
        assert_eq!(w.slot, (1664, 0, 1441, 1002));
        assert_eq!(w.alpha, (-1, -1));
        assert_eq!(w.ts, 123456);
        let p = &l.windows[1];
        assert_eq!(p.kind, Kind::Popup);
        assert_eq!(p.owner, 0x430696);
        assert_eq!(p.alpha, (600, 1200));
    }

    #[test]
    fn leaves_out_full_desk_popups_and_bad_rects() {
        let json = br#"{"windows":[
            {"hwnd":"0x1","kind":"popup","rect":[0,0,3840,2160]},
            {"hwnd":"0x2","kind":"window","rect":[0,0,0,10]},
            {"hwnd":"0x0","kind":"window","rect":[0,0,10,10]},
            {"hwnd":"0x3","kind":"dialog","rect":[1,2,3,4],"state":"minimized"}]}"#;
        let l = parse(json, 3840, 2160).expect("parses");
        assert_eq!(l.windows.len(), 1);
        assert_eq!(l.windows[0].kind, Kind::Dialog);
        assert!(l.windows[0].minimized);
        assert_eq!(l.windows[0].slot.2, 0);
    }

    #[test]
    fn hwnds_in_hex_and_decimal() {
        assert_eq!(parse_hwnd("0x1f0476"), 0x1f0476);
        assert_eq!(parse_hwnd("12345"), 12345);
        assert_eq!(parse_hwnd("junk"), 0);
    }
}
