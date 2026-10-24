//! Color roles and typography tokens. Values come from the user's Zed theme
//! when it can be found, with the `gruvbox-poly` values as fallback.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;

use gpui::{App, Font, FontFeatures, FontStyle, FontWeight, Global, Hsla, Rgba, SharedString, px, rgb, rgba};

/// Zed's `assets/themes/one/one.json`, the base layer for every user theme.
const ZED_ONE_JSON: &str = include_str!("../assets/zed-one.json");

#[derive(Clone, Debug)]
pub struct Theme {
    pub bg: Hsla,
    pub surface: Hsla,
    pub elevated: Hsla,
    pub scrim: Hsla,
    pub shadow: Hsla,
    pub border: Hsla,
    pub border_focused: Hsla,
    pub border_selected: Hsla,
    pub focus_ring: Hsla,
    pub text: Hsla,
    pub text_muted: Hsla,
    pub text_disabled: Hsla,
    pub text_accent: Hsla,
    pub accent: Hsla,
    pub accent_hover: Hsla,
    pub accent_pressed: Hsla,
    pub accent_fg: Hsla,
    pub accent_subtle: Hsla,
    pub element: Hsla,
    pub element_hover: Hsla,
    pub element_pressed: Hsla,
    pub selection: Hsla,
    pub selection_unfocused: Hsla,
    pub text_selection: Hsla,
    pub success: Hsla,
    pub success_subtle: Hsla,
    pub warning: Hsla,
    pub warning_subtle: Hsla,
    pub error: Hsla,
    pub error_subtle: Hsla,
    pub error_subtle_hover: Hsla,
    pub modified: Hsla,
    pub scrollbar_thumb: Hsla,
    // state dots
    pub state_online: Hsla,
    pub state_streaming: Hsla,
    pub state_busy: Hsla,
    pub state_pairing: Hsla,
    pub state_offline: Hsla,
    pub state_error: Hsla,
    // badges
    pub badge_apollo: Hsla,
    pub badge_sunshine: Hsla,
    pub badge_gfe: Hsla,
    pub badge_net: Hsla,
    pub badge_running: Hsla,
    pub badge_hdr: Hsla,
    pub badge_direct: Hsla,
    pub art_scrim: Hsla,

    pub ui_font: SharedString,
    pub mono_font: SharedString,
    pub theme_name: String,
}

impl Global for Theme {}

fn h(c: u32) -> Hsla {
    rgb(c).into()
}

fn ha(c: u32) -> Hsla {
    rgba(c).into()
}

/// Mix `a` towards `b` by `t` in RGB space.
fn mix(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let a: Rgba = a.into();
    let b: Rgba = b.into();
    Rgba { r: a.r + (b.r - a.r) * t, g: a.g + (b.g - a.g) * t, b: a.b + (b.b - a.b) * t, a: 1.0 }.into()
}

impl Theme {
    /// Zed's bundled One Dark style, the base every user theme is layered on.
    fn one_dark_style() -> HashMap<String, String> {
        let mut style = HashMap::new();
        if let Ok(doc) = serde_json::from_str::<serde_json::Value>(ZED_ONE_JSON)
            && let Some(themes) = doc.get("themes").and_then(|t| t.as_array()) {
                for t in themes {
                    if t.get("name").and_then(|n| n.as_str()) == Some("One Dark")
                        && let Some(obj) = t.get("style").and_then(|s| s.as_object()) {
                            flatten(obj, "", &mut style);
                        }
                }
            }
        style
    }

    /// Builds the theme from a Zed style map (`"background": "#161819"` etc).
    /// Keys the user theme doesn't set fall back to One Dark, as in Zed.
    pub fn from_zed_style(user: &HashMap<String, String>, name: &str) -> Theme {
        let mut style = Theme::one_dark_style();
        style.extend(user.iter().map(|(k, v)| (k.clone(), v.clone())));
        let color = |key: &str| -> Option<Hsla> { style.get(key).and_then(|v| parse_hex(v)) };
        // First key that resolves wins
        let pick = |keys: &[&str], fallback: u32| -> Hsla { keys.iter().find_map(|k| color(k)).unwrap_or_else(|| ha(fallback)) };

        let bg = pick(&["background"], 0x161819FF);
        let surface = pick(&["panel.background", "surface.background"], 0x1E2021FF);
        let elevated = pick(&["elevated_surface.background", "panel.background"], 0x1E2021FF);
        let text = pick(&["text"], 0xDCE0E5FF);
        let text_muted = pick(&["text.muted"], 0xA9AFBCFF);
        let text_disabled = pick(&["text.disabled", "text.placeholder"], 0x878A98FF);
        let accent = pick(&["text.accent"], 0x7DAEA3FF);
        let border = pick(&["border"], 0x3B3836FF);
        let success = pick(&["success", "created"], 0xA1C181FF);
        let warning = pick(&["warning"], 0xDEC184FF);
        let error = pick(&["error"], 0xD07277FF);
        let players: Vec<Option<Hsla>> = (0..8).map(|i| color(&format!("players.{i}.cursor"))).collect();
        let player = |i: usize, fallback: u32| players[i].unwrap_or_else(|| h(fallback));
        let orange = player(1, 0xE78A4E);
        let pink = player(2, 0xD3869B);
        let purple = player(4, 0xB16286);
        let aqua = player(6, 0x89B482);

        Theme {
            bg,
            surface,
            elevated,
            scrim: bg.alpha(0.70),
            shadow: bg.alpha(0.80),
            border,
            border_focused: pick(&["border.focused"], 0x3E5A52FF),
            border_selected: pick(&["border.selected"], 0x31443FFF),
            focus_ring: accent,
            text,
            text_muted,
            text_disabled,
            text_accent: accent,
            accent,
            accent_hover: mix(accent, text, 0.12),
            accent_pressed: mix(accent, bg, 0.12),
            accent_fg: bg,
            accent_subtle: pick(&["info.background"], 0x22302DFF),
            element: pick(&["element.background"], 0x2E343EFF),
            element_hover: pick(&["ghost_element.hover", "element.hover"], 0x363C46FF),
            element_pressed: pick(&["ghost_element.active", "element.active"], 0x454A56FF),
            selection: pick(&["element.selected", "ghost_element.selected"], 0x2D2C2BFF),
            selection_unfocused: text.alpha(0.04),
            text_selection: pick(&["players.0.selection"], 0x7DAEA33D),
            success,
            success_subtle: success.alpha(0.12),
            warning,
            warning_subtle: warning.alpha(0.12),
            error,
            error_subtle: error.alpha(0.12),
            error_subtle_hover: error.alpha(0.24),
            modified: pick(&["modified"], 0xC6A837FF),
            scrollbar_thumb: pick(&["scrollbar.thumb.hover_background", "scrollbar.thumb.background"], 0x9283744C),
            state_online: success,
            state_streaming: accent,
            state_busy: orange,
            state_pairing: warning,
            state_offline: text_muted,
            state_error: error,
            badge_apollo: accent,
            badge_sunshine: warning,
            badge_gfe: text_muted,
            badge_net: aqua,
            badge_running: orange,
            badge_hdr: pink,
            badge_direct: purple,
            art_scrim: bg.alpha(0.80),
            ui_font: ".SystemUIFont".into(),
            mono_font: "Menlo".into(),
            theme_name: name.to_string(),
        }
    }

    pub fn fallback() -> Theme {
        Theme::from_zed_style(&HashMap::new(), "gruvbox-poly (built-in)")
    }

    /// Loads the dark theme named in Zed's settings from `~/.config/zed/themes`.
    pub fn load_from_zed() -> Theme {
        let Some(config) = dirs::home_dir().map(|h| h.join(".config/zed")) else { return Theme::fallback() };
        let settings = std::fs::read_to_string(config.join("settings.json")).ok().and_then(|s| parse_lenient_json(&s));
        let mut wanted: Option<String> = None;
        let mut mono_font: Option<String> = None;
        if let Some(settings) = &settings {
            wanted = match settings.get("theme") {
                Some(serde_json::Value::String(s)) => Some(s.clone()),
                Some(serde_json::Value::Object(o)) => o.get("dark").and_then(|v| v.as_str()).map(String::from),
                _ => None,
            };
            mono_font = settings.get("buffer_font_family").and_then(|v| v.as_str()).map(String::from);
        }
        let mut theme = None;
        if let Some(wanted) = &wanted {
            theme = find_zed_theme(&config.join("themes"), wanted);
            if theme.is_none()
                && let Some(ext_dir) = dirs::data_dir().map(|d| d.join("Zed/extensions/installed")) {
                    theme = find_zed_theme(&ext_dir, wanted);
                }
        }
        let mut theme = theme.unwrap_or_else(|| {
            log::info!("Zed theme {:?} not found, using built-in gruvbox-poly values", wanted);
            Theme::fallback()
        });
        if let Some(mono) = mono_font {
            theme.mono_font = mono.into();
        }
        theme
    }

    /// Picks fonts that actually exist on this machine.
    pub fn resolve_fonts(&mut self, cx: &App) {
        let names: Vec<String> = cx.text_system().all_font_names().into_iter().map(|n| n.to_lowercase()).collect();
        let exists = |family: &str| family == ".SystemUIFont" || names.iter().any(|n| n == &family.to_lowercase());
        if !exists(&self.mono_font) {
            for candidate in ["Iosevka Plxn", "Iosevka", "JetBrains Mono", "SF Mono", "Menlo", "monospace"] {
                if exists(candidate) {
                    self.mono_font = candidate.into();
                    break;
                }
            }
        }
        if !exists(&self.ui_font) {
            for candidate in [".SystemUIFont", "SF Pro Text", "Helvetica Neue", "Inter", "sans-serif"] {
                if exists(candidate) {
                    self.ui_font = candidate.into();
                    break;
                }
            }
        }
        log::info!("Fonts: ui={} mono={} theme={}", self.ui_font, self.mono_font, self.theme_name);
    }
}

pub fn font(family: &str, weight: FontWeight) -> Font {
    Font { family: family.to_string().into(), features: FontFeatures::default(), fallbacks: None, weight, style: FontStyle::Normal }
}

fn find_zed_theme(dir: &PathBuf, wanted: &str) -> Option<Theme> {
    let mut stack = vec![dir.clone()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().map(|n| n == "themes" || d == *dir).unwrap_or(false) || d == *dir {
                    stack.push(path);
                }
                continue;
            }
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                let Ok(text) = std::fs::read_to_string(&path) else { continue };
                let Some(doc) = parse_lenient_json(&text) else { continue };
                let Some(themes) = doc.get("themes").and_then(|t| t.as_array()) else { continue };
                for t in themes {
                    if t.get("name").and_then(|n| n.as_str()) == Some(wanted) {
                        let mut style = HashMap::new();
                        if let Some(obj) = t.get("style").and_then(|s| s.as_object()) {
                            flatten(obj, "", &mut style);
                        }
                        log::info!("Loaded Zed theme {wanted} from {}", path.display());
                        return Some(Theme::from_zed_style(&style, wanted));
                    }
                }
            }
        }
    }
    None
}

fn flatten(obj: &serde_json::Map<String, serde_json::Value>, prefix: &str, out: &mut HashMap<String, String>) {
    for (k, v) in obj {
        let key = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
        match v {
            serde_json::Value::String(s) => {
                out.insert(key, s.clone());
            }
            serde_json::Value::Object(o) => flatten(o, &key, out),
            serde_json::Value::Array(a) => {
                for (i, item) in a.iter().enumerate() {
                    if let serde_json::Value::Object(o) = item {
                        flatten(o, &format!("{key}.{i}"), out);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Parses `#RRGGBB` or `#RRGGBBAA`.
pub fn parse_hex(s: &str) -> Option<Hsla> {
    let s = s.trim().trim_start_matches('#');
    match s.len() {
        6 => u32::from_str_radix(s, 16).ok().map(|v| rgb(v).into()),
        8 => u32::from_str_radix(s, 16).ok().map(|v| rgba(v).into()),
        _ => None,
    }
}

/// Zed settings files allow comments and trailing commas.
pub fn parse_lenient_json(text: &str) -> Option<serde_json::Value> {
    serde_json::from_str(&strip_for_debug(text)).ok()
}

pub fn strip_for_debug(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let bytes: Vec<char> = text.chars().collect();
    let mut i = 0;
    let mut in_string = false;
    while i < bytes.len() {
        let c = bytes[i];
        if in_string {
            out.push(c);
            if c == '\\' && i + 1 < bytes.len() {
                out.push(bytes[i + 1]);
                i += 2;
                continue;
            }
            if c == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_string = true;
            out.push(c);
            i += 1;
        } else if c == '/' && i + 1 < bytes.len() && bytes[i + 1] == '/' {
            while i < bytes.len() && bytes[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && i + 1 < bytes.len() && bytes[i + 1] == '*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == '*' && bytes[i + 1] == '/') {
                i += 1;
            }
            i += 2;
        } else if c == ',' {
            // Drop trailing commas before } or ] (skipping whitespace and comments)
            let mut j = i + 1;
            loop {
                while j < bytes.len() && bytes[j].is_whitespace() {
                    j += 1;
                }
                if j + 1 < bytes.len() && bytes[j] == '/' && bytes[j + 1] == '/' {
                    while j < bytes.len() && bytes[j] != '\n' {
                        j += 1;
                    }
                } else if j + 1 < bytes.len() && bytes[j] == '/' && bytes[j + 1] == '*' {
                    j += 2;
                    while j + 1 < bytes.len() && !(bytes[j] == '*' && bytes[j + 1] == '/') {
                        j += 1;
                    }
                    j += 2;
                } else {
                    break;
                }
            }
            if j < bytes.len() && (bytes[j] == '}' || bytes[j] == ']') {
                i += 1;
                continue;
            }
            out.push(c);
            i += 1;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// Typography tokens from the design spec.
pub mod type_scale {
    use gpui::{Pixels, px};
    pub const XS: Pixels = px(11.);
    pub const SM: Pixels = px(12.);
    pub const BASE: Pixels = px(13.);
    pub const LG: Pixels = px(15.);
    pub const DISPLAY: Pixels = px(32.);
    pub const LH_XS: Pixels = px(14.);
    pub const LH_SM: Pixels = px(16.);
    pub const LH_BASE: Pixels = px(18.);
    pub const LH_LG: Pixels = px(20.);
}

pub const HEADER_H: f32 = 36.;
pub const SIDEBAR_W: f32 = 184.;
pub const CONTROL_H: f32 = 24.;
pub const ROW_H: f32 = 28.;
pub const TILE_MIN_W: f32 = 104.;
pub const GRID_GAP_X: f32 = 12.;
pub const GRID_GAP_Y: f32 = 16.;
pub const GRID_PAD: f32 = 16.;

pub fn active_theme(cx: &App) -> &Theme {
    cx.global::<Theme>()
}

#[allow(dead_code)]
pub fn px_(v: f32) -> gpui::Pixels {
    px(v)
}

#[cfg(test)]
mod tests {
    #[test]
    fn parse_zed_settings() {
        let path = dirs::home_dir().unwrap().join(".config/zed/settings.json");
        let text = std::fs::read_to_string(&path).unwrap();
        let stripped = super::strip_for_debug(&text);
        match serde_json::from_str::<serde_json::Value>(&stripped) {
            Ok(v) => println!("OK theme={:?}", v.get("theme")),
            Err(e) => {
                println!("ERR {e}");
                let line = e.line();
                for (i, l) in stripped.lines().enumerate() {
                    if i + 3 >= line && i <= line + 1 {
                        println!("{}: {}", i + 1, l);
                    }
                }
            }
        }
    }
}
