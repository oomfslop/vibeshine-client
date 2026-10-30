//! Settings view: section list + rows, with a filter across all sections.

use gpui::{AnyElement, Context, Entity, IntoElement, KeyDownEvent, ParentElement, Styled, Window, anchored, deferred, div, prelude::*, px};

use crate::app::{Popup, PopupKind, Root};
use crate::assets::icon;
use crate::settings::{self, AudioConfig, BackgroundQuality, CaptureSysKeys, LANGUAGES, Renderer, Settings, SystemPreset, UiDisplayMode, VideoCodec, VideoDecoder, WindowDensity, WindowMode};
use crate::theme::{Theme, type_scale};
use crate::ui::*;

#[allow(dead_code)]
pub struct Section {
    pub id: &'static str,
    pub title: &'static str,
    pub icon: &'static str,
    pub description: &'static str,
}

pub const SECTIONS: [Section; 9] = [
    Section { id: "stream", title: "Stream", icon: icon::MONITOR_PLAY, description: "Values your PC or network can't handle cause lag, stutter or errors." },
    Section { id: "apollo", title: "Apollo host", icon: icon::SERVER, description: "" },
    Section { id: "audio", title: "Audio", icon: icon::VOLUME, description: "" },
    Section { id: "input", title: "Input", icon: icon::KEYBOARD, description: "" },
    Section { id: "gamepad", title: "Gamepad", icon: icon::GAMEPAD, description: "" },
    Section { id: "host", title: "Host", icon: icon::MONITOR, description: "" },
    Section { id: "interface", title: "Interface", icon: icon::APP_WINDOW, description: "" },
    Section { id: "advanced", title: "Advanced", icon: icon::SLIDERS, description: "" },
    Section { id: "about", title: "About", icon: icon::INFO, description: "" },
];

/// Row labels per section (for the palette and the filter).
pub fn section_rows(id: &str) -> Vec<&'static str> {
    match id {
        "stream" => vec!["Preset", "Resolution", "Frame rate", "Video bitrate", "Display mode", "V-Sync", "Frame pacing", "HDR", "Performance stats overlay"],
        "apollo" => vec!["Stream from a virtual display", "Host render scale", "App window size", "App window density", "App window bitrate", "Background window frame rate", "Background window quality", "Efficient encoding", "Quiet when idle", "Skip covered windows", "Stream new windows automatically", "Sync clipboard"],
        "audio" => vec!["Audio channels", "Mute host speakers while streaming", "Mute when Moonlight isn't focused"],
        "input" => vec!["Remote desktop mouse mode", "Capture system shortcuts", "Touchscreen as trackpad", "Swap left and right mouse buttons", "Reverse scroll direction"],
        "gamepad" => vec!["Swap A/B and X/Y", "Always connect gamepad 1", "Mouse control by holding Start", "Gamepad input in background"],
        "host" => vec!["Optimize game settings for streaming", "Quit app on host when the stream ends"],
        "interface" => vec!["Language", "Window at launch", "Connection quality warnings", "Configuration warnings", "Discord Rich Presence", "Keep display awake while streaming"],
        "advanced" => vec!["Video decoder", "Video codec", "Renderer", "YUV 4:4:4", "Unlock bitrate limit", "Discover hosts on the local network", "Detect blocked connections", "Exclusive fullscreen", "Streaming helper"],
        "about" => vec!["Version", "Setup guide", "Troubleshooting", "Log file", "Gamepad mapping guide", "Moonlight Discord"],
        _ => vec![],
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dropdown {
    Preset,
    Resolution,
    Fps,
    Language,
    Renderer,
}

pub struct SettingsViewState {
    pub section: usize,
    pub filter: Entity<TextInput>,
    pub filter_text: String,
    pub bitrate: Entity<TextInput>,
    pub scale: Entity<TextInput>,
    pub custom_fps: Entity<TextInput>,
    pub width: Entity<TextInput>,
    pub height: Entity<TextInput>,
    pub helper: Entity<TextInput>,
    /// The name of a preset being saved ("Save as new preset…")
    pub preset_name: Entity<TextInput>,
    pub naming_preset: bool,
    pub custom_res: bool,
    pub custom_fps_mode: bool,
    pub last_change: Option<Settings>,
}

const PRESET_RES: [(&str, i32, i32); 4] = [("720p", 1280, 720), ("1080p", 1920, 1080), ("1440p", 2560, 1440), ("4K", 3840, 2160)];
const PRESET_FPS: [i32; 2] = [30, 60];

fn numeric(cx: &mut Context<Root>, decimal: bool, width: f32) -> Entity<TextInput> {
    cx.new(|cx| {
        let mut t = TextInput::new(cx, "", InputKind::Numeric { decimal, negative: false });
        t.align_right = true;
        let _ = width;
        t
    })
}

impl SettingsViewState {
    pub fn new(cx: &mut Context<Root>, settings: &Settings) -> Self {
        let filter = cx.new(|cx| TextInput::new(cx, "Filter settings", InputKind::Text));
        let bitrate = numeric(cx, true, 72.);
        let scale = numeric(cx, false, 72.);
        let custom_fps = numeric(cx, true, 72.);
        let width = numeric(cx, false, 64.);
        let height = numeric(cx, false, 64.);
        let helper = cx.new(|cx| TextInput::new(cx, "Auto-detect", InputKind::Mono));
        let preset_name = cx.new(|cx| TextInput::new(cx, "Preset name", InputKind::Text));

        cx.subscribe(&preset_name, |this: &mut Root, input, ev: &TextInputEvent, cx| match ev {
            TextInputEvent::Submit => {
                let name = input.read(cx).text().to_string();
                this.save_preset_as(&name, cx);
            }
            TextInputEvent::Escape => {
                this.settings_view.naming_preset = false;
                this.focus_main(cx);
                cx.notify();
            }
            _ => {}
        })
        .detach();

        cx.subscribe(&filter, |this: &mut Root, input, ev: &TextInputEvent, cx| match ev {
            TextInputEvent::Changed => {
                this.settings_view.filter_text = input.read(cx).text().to_string();
                cx.notify();
            }
            TextInputEvent::Escape => {
                this.settings_view.filter_text.clear();
                input.update(cx, |t, cx| t.clear(cx));
                this.focus_main(cx);
            }
            TextInputEvent::Arrow(1) | TextInputEvent::Submit => this.focus_main(cx),
            _ => {}
        })
        .detach();

        macro_rules! commit_on {
            ($entity:expr, $apply:expr) => {
                cx.subscribe(&$entity, |this: &mut Root, input, ev: &TextInputEvent, cx| match ev {
                    TextInputEvent::Submit | TextInputEvent::Blur => {
                        let text = input.read(cx).text().to_string();
                        let apply: fn(&mut Root, &str, &mut Context<Root>) = $apply;
                        apply(this, &text, cx);
                        this.settings_view.sync_from(&this.settings, cx);
                        this.save_settings();
                        if matches!(ev, TextInputEvent::Submit) {
                            this.focus_main(cx);
                        }
                        cx.notify();
                    }
                    TextInputEvent::Escape => {
                        this.settings_view.sync_from(&this.settings, cx);
                        this.focus_main(cx);
                    }
                    _ => {}
                })
                .detach();
            };
        }
        commit_on!(bitrate, |root, text, _cx| {
            if let Ok(mbps) = text.trim().parse::<f64>() {
                let max = root.settings.max_bitrate_kbps();
                let kbps = ((mbps * 1000.0).round() as i32).clamp(500, max);
                if kbps != root.settings.bitrate_kbps {
                    root.settings.bitrate_kbps = kbps;
                    root.settings.auto_adjust_bitrate = false;
                }
            }
        });
        commit_on!(scale, |root, text, _cx| {
            if let Ok(v) = text.trim().parse::<i32>() {
                root.settings.resolution_scale_factor = (v.clamp(50, 200) / 5) * 5;
            }
        });
        commit_on!(custom_fps, |root, text, _cx| {
            if let Ok(v) = text.trim().parse::<f64>()
                && (10.0..=9999.0).contains(&v) {
                    if (v - v.round()).abs() < 1e-6 {
                        root.settings.fps = v.round() as i32;
                        root.settings.fractional_fps = 0.0;
                    } else {
                        root.settings.fps = v.round() as i32;
                        root.settings.fractional_fps = v;
                    }
                    root.settings.apply_auto_bitrate();
                }
        });
        commit_on!(width, |root, text, _cx| {
            if let Ok(v) = text.trim().parse::<i32>() {
                root.settings.width = v.clamp(256, 8192);
                root.settings.apply_auto_bitrate();
            }
        });
        commit_on!(height, |root, text, _cx| {
            if let Ok(v) = text.trim().parse::<i32>() {
                root.settings.height = v.clamp(256, 8192);
                root.settings.apply_auto_bitrate();
            }
        });
        commit_on!(helper, |root, text, _cx| {
            root.settings.helper_path = text.trim().to_string();
            root.helper_path = crate::stream::find_helper(&root.settings);
        });

        let mut state = SettingsViewState {
            section: 0,
            filter,
            filter_text: String::new(),
            bitrate,
            scale,
            custom_fps,
            width,
            height,
            helper,
            preset_name,
            naming_preset: false,
            custom_res: false,
            custom_fps_mode: false,
            last_change: None,
        };
        state.sync_from(settings, cx);
        state
    }

    /// Copies setting values into the text fields.
    pub fn sync_from(&mut self, s: &Settings, cx: &mut Context<Root>) {
        let bitrate = settings::format_mbps(s.bitrate_kbps);
        self.bitrate.update(cx, |t, cx| t.set_text(bitrate, cx));
        self.scale.update(cx, |t, cx| t.set_text(s.resolution_scale_factor.to_string(), cx));
        self.custom_fps.update(cx, |t, cx| t.set_text(s.fps_label(), cx));
        self.width.update(cx, |t, cx| t.set_text(s.width.to_string(), cx));
        self.height.update(cx, |t, cx| t.set_text(s.height.to_string(), cx));
        self.helper.update(cx, |t, cx| t.set_text(s.helper_path.clone(), cx));
        if !is_preset_res(s.width, s.height) {
            self.custom_res = true;
        }
        if s.fractional_fps > 0.0 || !is_preset_fps(s.fps) {
            self.custom_fps_mode = true;
        }
    }
}

fn res_label(s: &Settings) -> String {
    for (name, w, h) in PRESET_RES {
        if w == s.width && h == s.height {
            return format!("{name} ({w}×{h})");
        }
    }
    for (name, (w, h)) in crate::display::display_resolutions() {
        if w == s.width && h == s.height {
            return format!("{name} ({w}×{h})");
        }
    }
    format!("Custom ({}×{})", s.width, s.height)
}

fn is_preset_res(w: i32, h: i32) -> bool {
    PRESET_RES.iter().any(|(_, pw, ph)| *pw == w && *ph == h) || crate::display::display_resolutions().iter().any(|(_, s)| *s == (w, h))
}

fn is_preset_fps(fps: i32) -> bool {
    PRESET_FPS.contains(&fps) || crate::display::display_refresh_rates().iter().any(|(_, r)| *r == fps)
}

fn fps_label(s: &Settings) -> String {
    if s.fractional_fps == 0.0 && is_preset_fps(s.fps) {
        format!("{} FPS", s.fps)
    } else {
        format!("Custom ({} fps)", s.fps_label())
    }
}

pub fn dropdown_entries(root: &mut Root, kind: Dropdown, cx: &mut Context<Root>) -> Vec<MenuEntry> {
    let s = root.settings.clone();
    let mut entries = Vec::new();
    let close = |this: &mut Root, cx: &mut Context<Root>| {
        this.popup = None;
        this.settings_view.sync_from(&this.settings, cx);
        this.save_settings();
        cx.notify();
    };
    match kind {
        Dropdown::Preset => {
            for p in SystemPreset::ALL {
                entries.push(
                    MenuEntry::new(p.name(), cx.listener(move |this, _, _, cx| {
                        this.settings.apply_preset(p.id());
                        close(this, cx);
                    }))
                    .checked(s.active_preset == p.id()),
                );
            }
            for (i, c) in s.custom_presets.iter().enumerate() {
                let id = format!("custom:{}", c.name);
                let checked = s.active_preset == id;
                let mut entry = MenuEntry::new(c.name.clone(), cx.listener(move |this, _, _, cx| {
                    this.settings.apply_preset(&id);
                    close(this, cx);
                }))
                .checked(checked);
                if i == 0 {
                    entry = entry.separator();
                }
                entries.push(entry);
            }
            entries.push(
                MenuEntry::new("Save as new preset…", cx.listener(move |this, _, _, cx| {
                    this.popup = None;
                    this.settings_view.naming_preset = true;
                    this.settings_view.preset_name.update(cx, |t, cx| t.clear(cx));
                    let h = this.settings_view.preset_name.read(cx).focus_handle.clone();
                    this.pending_focus = Some(h);
                    cx.notify();
                }))
                .separator(),
            );
            if let Some(name) = s.active_preset.strip_prefix("custom:").map(str::to_string) {
                if s.preset_modified() {
                    let n = name.clone();
                    entries.push(MenuEntry::new(format!("Save changes to “{name}”"), cx.listener(move |this, _, _, cx| {
                        this.settings.save_custom_preset(&n);
                        close(this, cx);
                    })));
                }
                entries.push(
                    MenuEntry::new(format!("Delete “{name}”"), cx.listener(move |this, _, _, cx| {
                        this.settings.delete_custom_preset(&name);
                        close(this, cx);
                    }))
                    .destructive(),
                );
            }
        }
        Dropdown::Resolution => {
            for (name, w, h) in PRESET_RES {
                let cur = s.width == w && s.height == h && !root.settings_view.custom_res;
                entries.push(
                    MenuEntry::new(format!("{name}  {w}×{h}"), cx.listener(move |this, _, _, cx| {
                        this.settings.width = w;
                        this.settings.height = h;
                        this.settings.apply_auto_bitrate();
                        this.settings_view.custom_res = false;
                        close(this, cx);
                    }))
                    .checked(cur),
                );
            }
            for (i, (name, (w, h))) in crate::display::display_resolutions().into_iter().enumerate() {
                let cur = s.width == w && s.height == h && !root.settings_view.custom_res;
                let mut entry = MenuEntry::new(format!("{name}  {w}×{h}"), cx.listener(move |this, _, _, cx| {
                    this.settings.width = w;
                    this.settings.height = h;
                    this.settings.apply_auto_bitrate();
                    this.settings_view.custom_res = false;
                    close(this, cx);
                }))
                .checked(cur);
                if i == 0 {
                    entry = entry.separator();
                }
                entries.push(entry);
            }
            let custom = root.settings_view.custom_res;
            entries.push(
                MenuEntry::new("Custom…", cx.listener(move |this, _, _, cx| {
                    this.settings_view.custom_res = true;
                    this.popup = None;
                    let h = this.settings_view.width.read(cx).focus_handle.clone();
                    this.pending_focus = Some(h);
                    cx.notify();
                }))
                .checked(custom)
                .separator(),
            );
        }
        Dropdown::Fps => {
            for fps in PRESET_FPS {
                let cur = s.fps == fps && s.fractional_fps == 0.0 && !root.settings_view.custom_fps_mode;
                entries.push(
                    MenuEntry::new(format!("{fps} FPS"), cx.listener(move |this, _, _, cx| {
                        this.settings.fps = fps;
                        this.settings.fractional_fps = 0.0;
                        this.settings.apply_auto_bitrate();
                        this.settings_view.custom_fps_mode = false;
                        close(this, cx);
                    }))
                    .checked(cur),
                );
            }
            let mut first_display_rate = true;
            for (name, fps) in crate::display::display_refresh_rates() {
                if PRESET_FPS.contains(&fps) {
                    continue;
                }
                let cur = s.fps == fps && s.fractional_fps == 0.0 && !root.settings_view.custom_fps_mode;
                let mut entry = MenuEntry::new(format!("{fps} FPS  {name}"), cx.listener(move |this, _, _, cx| {
                    this.settings.fps = fps;
                    this.settings.fractional_fps = 0.0;
                    this.settings.apply_auto_bitrate();
                    this.settings_view.custom_fps_mode = false;
                    close(this, cx);
                }))
                .checked(cur);
                if first_display_rate {
                    entry = entry.separator();
                    first_display_rate = false;
                }
                entries.push(entry);
            }
            let custom = root.settings_view.custom_fps_mode;
            entries.push(
                MenuEntry::new("Custom…", cx.listener(move |this, _, _, cx| {
                    this.settings_view.custom_fps_mode = true;
                    this.popup = None;
                    let h = this.settings_view.custom_fps.read(cx).focus_handle.clone();
                    this.pending_focus = Some(h);
                    cx.notify();
                }))
                .checked(custom)
                .separator(),
            );
        }
        Dropdown::Language => {
            for (i, (_, name)) in LANGUAGES.iter().enumerate() {
                let cur = s.language == i as i32;
                entries.push(
                    MenuEntry::new(*name, cx.listener(move |this, _, _, cx| {
                        this.settings.language = i as i32;
                        close(this, cx);
                    }))
                    .checked(cur),
                );
            }
        }
        Dropdown::Renderer => {
            for r in Renderer::ALL {
                let r = *r;
                entries.push(
                    MenuEntry::new(r.label(), cx.listener(move |this, _, _, cx| {
                        this.settings.renderer_selection = r;
                        close(this, cx);
                    }))
                    .checked(s.renderer_selection == r),
                );
            }
        }
    }
    entries
}

struct Row {
    label: &'static str,
    help: String,
    modified: bool,
    disabled_reason: Option<&'static str>,
    badge: Option<(&'static str, bool)>,
    control: AnyElement,
    extra: Option<AnyElement>,
    reset: Option<Box<dyn Fn(&mut Settings)>>,
}

fn toggle_row(root: &mut Root, cx: &mut Context<Root>, theme: &Theme, id: &'static str, label: &'static str, help: &str, get: fn(&Settings) -> bool, set: fn(&mut Settings, bool), disabled: Option<&'static str>) -> Row {
    let on = get(&root.settings);
    let default = get(&Settings::default());
    let control = toggle(theme, id, on, disabled.is_some(), cx.listener(move |this: &mut Root, v: &bool, _, cx| {
        set(&mut this.settings, *v);
        this.after_settings_change(cx);
    }))
    .into_any_element();
    Row {
        label,
        help: help.to_string(),
        modified: on != default,
        disabled_reason: disabled,
        badge: None,
        control,
        extra: None,
        reset: Some(Box::new(move |s| set(s, default))),
    }
}

fn segmented_row<T: Copy + PartialEq + 'static>(
    root: &mut Root,
    cx: &mut Context<Root>,
    theme: &Theme,
    id: &'static str,
    label: &'static str,
    help: &str,
    options: &'static [T],
    labels: &[&str],
    get: fn(&Settings) -> T,
    set: fn(&mut Settings, T),
    disabled: Option<&'static str>,
) -> Row {
    let cur = get(&root.settings);
    let default = get(&Settings::default());
    let selected = options.iter().position(|o| *o == cur).unwrap_or(0);
    let control = segmented(theme, id, labels, selected, disabled.is_some(), cx.listener(move |this: &mut Root, i: &usize, _, cx| {
        set(&mut this.settings, options[*i]);
        this.after_settings_change(cx);
    }))
    .into_any_element();
    Row {
        label,
        help: help.to_string(),
        modified: cur != default,
        disabled_reason: disabled,
        badge: None,
        control,
        extra: None,
        reset: Some(Box::new(move |s| set(s, default))),
    }
}

fn dropdown_control(root: &mut Root, theme: &Theme, id: &'static str, label: String, kind: Dropdown, cx: &mut Context<Root>) -> AnyElement {
    let open = matches!(&root.popup, Some(Popup { kind: PopupKind::Dropdown(k), .. }) if *k == kind);
    let highlighted = if open { root.popup.as_ref().and_then(|p| p.highlighted) } else { None };
    // The open menu hangs from the control's bottom-left corner (anchored uses its
    // own layout origin), not from the pointer. The overlay layer only draws the scrim.
    let menu_el: Option<AnyElement> = if open {
        let entries = dropdown_entries(root, kind, cx);
        Some(
            div()
                .absolute()
                .left_0()
                .top(px(28.))
                .child(
                    deferred(
                        anchored()
                            .snap_to_window_with_margin(px(8.))
                            .child(div().occlude().on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation()).child(menu(theme, "dd-menu", entries, highlighted, 160.))),
                    )
                    .with_priority(3),
                )
                .into_any_element(),
        )
    } else {
        None
    };
    let hover_border = theme.text_muted.alpha(0.5);
    div()
        .relative()
        .child(
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.))
                .h(px(24.))
                .min_w(px(160.))
                .max_w(px(260.))
                .px(px(8.))
                .rounded(px(4.))
                .bg(theme.bg)
                .border_1()
                .border_color(if open { theme.border_focused } else { theme.border })
                .cursor_pointer()
                .hover(move |s| s.border_color(hover_border))
                .child(div().whitespace_nowrap().overflow_hidden().text_ellipsis().child(label))
                .child(icon_el(icon::CHEVRONS_UP_DOWN, 12., theme.text_muted))
                .on_click(cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                    if matches!(&this.popup, Some(Popup { kind: PopupKind::Dropdown(k), .. }) if *k == kind) {
                        this.popup = None;
                    } else {
                        this.popup = Some(Popup { kind: PopupKind::Dropdown(kind), position: gpui::point(px(0.), px(0.)), highlighted: None });
                    }
                    cx.notify();
                })),
        )
        .children(menu_el)
        .into_any_element()
}

fn numeric_control(theme: &Theme, input: Entity<TextInput>, width: f32, unit: &str) -> AnyElement {
    div().flex().items_center().gap(px(6.)).child(div().w(px(width)).child(input)).child(text_sm(theme, unit.to_string())).into_any_element()
}

impl Root {
    /// "Save as new preset…": the settings as they are, under this name
    pub fn save_preset_as(&mut self, name: &str, cx: &mut Context<Self>) {
        if name.trim().is_empty() {
            return;
        }
        self.settings.save_custom_preset(name);
        self.settings_view.naming_preset = false;
        self.focus_main(cx);
        self.after_settings_change(cx);
    }

    pub fn after_settings_change(&mut self, cx: &mut Context<Self>) {
        if !self.settings.enable_vsync {
            self.settings.frame_pacing = false;
        }
        if !self.settings.unlock_bitrate && self.settings.bitrate_kbps > 150_000 {
            self.settings.bitrate_kbps = 150_000;
        }
        self.manager.set_mdns_enabled(self.settings.enable_mdns);
        self.settings_view.sync_from(&self.settings, cx);
        self.save_settings();
        cx.notify();
    }
}

fn build_rows(root: &mut Root, theme: &Theme, section: &str, cx: &mut Context<Root>) -> Vec<Row> {
    let s = root.settings.clone();
    let d = Settings::default();
    let is_macos = cfg!(target_os = "macos");
    let mut rows = Vec::new();
    match section {
        "stream" => {
            // Preset
            let preset_label = if s.active_preset.is_empty() || s.preset_by_id(&s.active_preset).is_none() {
                "Custom".to_string()
            } else if s.preset_modified() {
                format!("{} (modified)", Settings::preset_name(&s.active_preset))
            } else {
                Settings::preset_name(&s.active_preset)
            };
            let extra = root.settings_view.naming_preset.then(|| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(div().w(px(180.)).child(root.settings_view.preset_name.clone()))
                    .child(button("preset-save", "Save").primary().compact().on_click(cx.listener(|this: &mut Root, _, _, cx| {
                        let name = this.settings_view.preset_name.read(cx).text().to_string();
                        this.save_preset_as(&name, cx);
                    })))
                    .child(button("preset-cancel", "Cancel").ghost().compact().on_click(cx.listener(|this: &mut Root, _, _, cx| {
                        this.settings_view.naming_preset = false;
                        cx.notify();
                    })))
                    .into_any_element()
            });
            rows.push(Row {
                label: "Preset",
                help: "Full quality is for home Wi-Fi and wired networks; Data saver for phone hotspots and slow links. Choosing one sets the settings it covers (resolution, frame rate, bitrates, codec and the app window ones); change any of them freely, and keep the result as a preset of your own if you like.".into(),
                modified: s.preset_modified(),
                disabled_reason: None,
                badge: None,
                control: dropdown_control(root, theme, "dd-preset", preset_label, Dropdown::Preset, cx),
                extra,
                reset: Some(Box::new(|s| {
                    let id = s.active_preset.clone();
                    s.apply_preset(&id);
                })),
            });
            // Resolution
            let extra = root.settings_view.custom_res.then(|| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(div().w(px(64.)).child(root.settings_view.width.clone()))
                    .child(text_sm(theme, "×"))
                    .child(div().w(px(64.)).child(root.settings_view.height.clone()))
                    .child(text_sm(theme, "px"))
                    .into_any_element()
            });
            rows.push(Row {
                label: "Resolution",
                help: if root.settings_view.custom_res { "GeForce Experience won't change the host resolution for custom values. Unsupported resolutions may fail to stream.".into() } else { String::new() },
                modified: s.width != d.width || s.height != d.height,
                disabled_reason: None,
                badge: None,
                control: dropdown_control(root, theme, "dd-res", res_label(&s), Dropdown::Resolution, cx),
                extra,
                reset: Some(Box::new(|s| {
                    s.width = 1920;
                    s.height = 1080;
                    s.apply_auto_bitrate();
                })),
            });
            let extra = root.settings_view.custom_fps_mode.then(|| numeric_control(theme, root.settings_view.custom_fps.clone(), 72., "fps"));
            rows.push(Row {
                label: "Frame rate",
                help: if root.settings_view.custom_fps_mode { "Fractional rates like 59.94 need an Apollo host.".into() } else { String::new() },
                modified: s.fps != d.fps || s.fractional_fps != 0.0,
                disabled_reason: None,
                badge: None,
                control: dropdown_control(root, theme, "dd-fps", fps_label(&s), Dropdown::Fps, cx),
                extra,
                reset: Some(Box::new(|s| {
                    s.fps = 60;
                    s.fractional_fps = 0.0;
                    s.apply_auto_bitrate();
                })),
            });
            let default_kbps = s.current_default_bitrate();
            rows.push(Row {
                label: "Video bitrate",
                help: "Lower on slow connections, raise for quality.".into(),
                modified: s.bitrate_kbps != default_kbps,
                disabled_reason: None,
                badge: None,
                control: numeric_control(theme, root.settings_view.bitrate.clone(), 72., "Mbps"),
                extra: None,
                reset: Some(Box::new(|s| {
                    s.auto_adjust_bitrate = true;
                    s.apply_auto_bitrate();
                })),
            });
            let window_help = if is_macos {
                if s.exclusive_fullscreen {
                    "Fullscreen switches the display mode (exclusive). Borderless is an overlay that covers the whole panel, notch included."
                } else {
                    "Fullscreen is a macOS Space placed below the notch; pick the \"Below notch\" resolution to fill it without bars. Borderless is an overlay that covers the whole panel, notch included."
                }
            } else {
                "Borderless works better with Spaces, Alt+Tab and overlays."
            };
            rows.push(segmented_row(root, cx, theme, "seg-window", "Display mode", window_help, WindowMode::ALL, &["Fullscreen", "Borderless", "Windowed"], |s| s.window_mode, |s, v| s.window_mode = v, None));
            rows.push(toggle_row(root, cx, theme, "tg-vsync", "V-Sync", "Off allows sub-frame latency but may tear.", |s| s.enable_vsync, |s, v| s.enable_vsync = v, None));
            rows.push(toggle_row(root, cx, theme, "tg-pacing", "Frame pacing", "Needs V-Sync. Reduces micro-stutter.", |s| s.frame_pacing, |s, v| s.frame_pacing = v, if s.enable_vsync { None } else { Some("Needs V-Sync.") }));
            rows.push(toggle_row(root, cx, theme, "tg-hdr", "HDR", "Some games need an HDR monitor on the host.", |s| s.enable_hdr, |s, v| s.enable_hdr = v, None));
            rows.push(toggle_row(root, cx, theme, "tg-perf", "Performance stats overlay", "Ctrl+Alt+Shift+S or Select+L1+R1+X while streaming.", |s| s.show_performance_overlay, |s, v| s.show_performance_overlay = v, None));
        }
        "apollo" => {
            rows.push(toggle_row(root, cx, theme, "tg-vd", "Stream from a virtual display", "The host creates a display matching your stream instead of capturing a monitor.", |s| s.use_virtual_display, |s, v| s.use_virtual_display = v, None));
            rows.push(segmented_row(root, cx, theme, "seg-aw-scale", "App window size", "How big host windows are on this screen, relative to the Mac's own interface. Every setting keeps one host pixel per screen pixel; the host renders at the matching scale.", &[50, 75, 100, 125], &["50%", "75%", "100%", "125%"], |s| s.app_window_scale, |s, v| s.app_window_scale = v, None));
            rows.push(segmented_row(root, cx, theme, "seg-aw-density", "App window density", "Retina draws host windows with two pixels to the point, as sharp as the Mac's own (a 4K stream). Standard draws them with one, like a non-Retina screen: a quarter of the pixels to encode and send, for slow links.", WindowDensity::ALL, &["Retina", "Standard"], |s| s.app_window_density, |s, v| s.app_window_density = v, None));
            rows.push(segmented_row(root, cx, theme, "seg-aw-bitrate", "App window bitrate", "The desk stream carries still windows most of the time; a lower rate than for games avoids packet loss, and every lost frame freezes the picture until it recovers.", &[8000, 12000, 20000, 40000, 80000], &["8 Mbps", "12 Mbps", "20 Mbps", "40 Mbps", "80 Mbps"], |s| s.app_window_bitrate_kbps, |s, v| s.app_window_bitrate_kbps = v, None));
            rows.push(segmented_row(root, cx, theme, "seg-aw-background", "Background window frame rate", "Windows other than the one you work in update at a fraction of the stream's frame rate. Pixels that do not change cost almost nothing, so this saves bandwidth on what animates in the background (videos, spinners).", &[1, 2, 4, 8, 16], &["Full", "1/2", "1/4", "1/8", "1/16"], |s| s.app_window_background_divisor, |s, v| s.app_window_background_divisor = v, None));
            rows.push(segmented_row(root, cx, theme, "seg-aw-bgquality", "Background window quality", "Windows other than the one you work in are encoded coarser, so their changes cost fewer bits. The window you click becomes sharp as soon as it has the focus.", BackgroundQuality::ALL, &["Full", "Lower", "Low"], |s| s.app_window_background_quality, |s, v| s.app_window_background_quality = v, None));
            rows.push(toggle_row(root, cx, theme, "tg-aw-quiet", "Quiet when idle", "While nothing on the screen changes, the host sends a frame a second instead of repeating the picture many times a second, and its text cursor stops blinking (it stays visible). Saves most of the data an idle stream uses.", |s| s.app_window_quiet_idle, |s, v| s.app_window_quiet_idle = v, None));
            rows.push(toggle_row(root, cx, theme, "tg-aw-efficient", "Efficient encoding", "The host's encoder takes more time per frame (a slower preset and a pre-pass) and stops refining still areas once they look right: noticeably less data for the same picture, a few milliseconds more latency.", |s| s.app_window_efficient_encoding, |s, v| s.app_window_efficient_encoding = v, None));
            rows.push(toggle_row(root, cx, theme, "tg-aw-covered", "Skip covered windows", "Windows that other windows cover completely are left out of the stream; the Mac keeps their last picture until they show again.", |s| s.app_window_skip_covered, |s, v| s.app_window_skip_covered = v, None));
            rows.push(toggle_row(root, cx, theme, "tg-aw-follow", "Stream new windows automatically", "While a window is streamed, windows that newly appear on the host (opened from it, or dialogs grabbing focus) get a Mac window too.", |s| s.app_window_follow_new, |s, v| s.app_window_follow_new = v, None));
            rows.push(toggle_row(root, cx, theme, "tg-vrr", "Capture on present", "Vibeshine/Vibepollo run the virtual display at 1000 Hz and grab each frame right after the game presents it, instead of on the display's refresh grid.", |s| s.vrr_capture, |s, v| s.vrr_capture = v, None));
            rows.push(Row {
                label: "Host render scale",
                help: "Below 100% is faster, above supersamples. 50–200.".into(),
                modified: s.resolution_scale_factor != 100,
                disabled_reason: None,
                badge: None,
                control: numeric_control(theme, root.settings_view.scale.clone(), 72., "%"),
                extra: None,
                reset: Some(Box::new(|s| s.resolution_scale_factor = 100)),
            });
            rows.push(toggle_row(root, cx, theme, "tg-clip", "Sync clipboard", "Sends your clipboard on stream start and focus, fetches the host's on focus loss.", |s| s.clipboard_sync, |s, v| s.clipboard_sync = v, None));
            rows.push(toggle_row(root, cx, theme, "tg-hide-cursor", "Show only the Mac cursor", "The host leaves its cursor out of the picture and your Mac cursor stays visible over the stream. Always on for app windows.", |s| s.hide_host_cursor, |s, v| s.hide_host_cursor = v, None));
        }
        "audio" => {
            rows.push(segmented_row(root, cx, theme, "seg-audio", "Audio channels", "", AudioConfig::ALL, &["Stereo", "5.1", "7.1"], |s| s.audio_config, |s, v| s.audio_config = v, None));
            rows.push(toggle_row(root, cx, theme, "tg-hostaudio", "Mute host speakers while streaming", "Restart a running game for this to take effect.", |s| !s.play_audio_on_host, |s, v| s.play_audio_on_host = !v, None));
            rows.push(toggle_row(root, cx, theme, "tg-mutefocus", "Mute when Moonlight isn't focused", "Mutes the stream when you switch to another window.", |s| s.mute_on_focus_loss, |s, v| s.mute_on_focus_loss = v, None));
        }
        "input" => {
            rows.push(toggle_row(root, cx, theme, "tg-absmouse", "Remote desktop mouse mode", "Mouse isn't captured. Good for desktop use, breaks most games. Ctrl+Alt+Shift+M in stream.", |s| s.absolute_mouse_mode, |s, v| s.absolute_mouse_mode = v, None));
            rows.push(segmented_row(root, cx, theme, "seg-syskeys", "Capture system shortcuts", "Sends shortcuts like Alt+Tab to the host.", CaptureSysKeys::ALL, &["Off", "In fullscreen", "Always"], |s| s.capture_sys_keys_mode, |s, v| s.capture_sys_keys_mode = v, None));
            rows.push(toggle_row(root, cx, theme, "tg-touch", "Touchscreen as trackpad", "On: touch acts like a trackpad. Off: touch moves the pointer directly.", |s| !s.absolute_touch_mode, |s, v| s.absolute_touch_mode = !v, None));
            rows.push(toggle_row(root, cx, theme, "tg-swapmouse", "Swap left and right mouse buttons", "", |s| s.swap_mouse_buttons, |s, v| s.swap_mouse_buttons = v, None));
            rows.push(toggle_row(root, cx, theme, "tg-revscroll", "Reverse scroll direction", "", |s| s.reverse_scroll_direction, |s, v| s.reverse_scroll_direction = v, None));
        }
        "gamepad" => {
            rows.push(toggle_row(root, cx, theme, "tg-swapface", "Swap A/B and X/Y", "Nintendo-style button layout.", |s| s.swap_face_buttons, |s, v| s.swap_face_buttons = v, None));
            rows.push(toggle_row(root, cx, theme, "tg-single", "Always connect gamepad 1", "For games that don't detect gamepads plugged in after launch.", |s| !s.multi_controller, |s, v| s.multi_controller = !v, None));
            rows.push(toggle_row(root, cx, theme, "tg-gpmouse", "Mouse control by holding Start", "", |s| s.gamepad_mouse, |s, v| s.gamepad_mouse = v, None));
            rows.push(toggle_row(root, cx, theme, "tg-bggp", "Gamepad input in background", "Captures gamepads while Moonlight isn't the focused window.", |s| s.background_gamepad, |s, v| s.background_gamepad = v, None));
        }
        "host" => {
            rows.push(toggle_row(root, cx, theme, "tg-gameopts", "Optimize game settings for streaming", "Lets GeForce Experience adjust in-game settings.", |s| s.game_optimizations, |s, v| s.game_optimizations = v, None));
            let mut r = toggle_row(root, cx, theme, "tg-quitafter", "Quit app on host when the stream ends", "Closes the app when you disconnect. Unsaved progress is lost.", |s| s.quit_app_after, |s, v| s.quit_app_after = v, None);
            r.badge = Some(("Danger", true));
            rows.push(r);
        }
        "interface" => {
            let lang = LANGUAGES.get(s.language.max(0) as usize).map(|l| l.1).unwrap_or("Automatic").to_string();
            rows.push(Row {
                label: "Language",
                help: String::new(),
                modified: s.language != 0,
                disabled_reason: None,
                badge: if s.language != 0 { Some(("Restart required", false)) } else { None },
                control: dropdown_control(root, theme, "dd-lang", lang, Dropdown::Language, cx),
                extra: None,
                reset: Some(Box::new(|s| s.language = 0)),
            });
            rows.push(segmented_row(root, cx, theme, "seg-uimode", "Window at launch", "", UiDisplayMode::ALL, &["Windowed", "Maximized", "Fullscreen"], |s| s.ui_display_mode, |s, v| s.ui_display_mode = v, None));
            rows.push(toggle_row(root, cx, theme, "tg-connwarn", "Connection quality warnings", "Shows a warning in the stream when the network struggles.", |s| s.connection_warnings, |s, v| s.connection_warnings = v, None));
            rows.push(toggle_row(root, cx, theme, "tg-confwarn", "Configuration warnings", "Shows warnings about unsupported settings when a stream starts.", |s| s.configuration_warnings, |s, v| s.configuration_warnings = v, None));
            rows.push(toggle_row(root, cx, theme, "tg-discord", "Discord Rich Presence", "Shows the game you're streaming in your Discord status.", |s| s.rich_presence, |s, v| s.rich_presence = v, None));
            rows.push(toggle_row(root, cx, theme, "tg-awake", "Keep display awake while streaming", "Prevents the screensaver and display sleep.", |s| s.keep_awake, |s, v| s.keep_awake = v, None));
        }
        "advanced" => {
            rows.push(segmented_row(root, cx, theme, "seg-decoder", "Video decoder", "Forcing software decoding may hurt performance.", VideoDecoder::ALL, &["Auto", "Hardware", "Software"], |s| s.video_decoder_selection, |s, v| s.video_decoder_selection = v, None));
            rows.push(segmented_row(root, cx, theme, "seg-codec", "Video codec", "", VideoCodec::ALL, &["Auto", "H.264", "HEVC", "AV1"], |s| s.video_codec_config, |s, v| s.video_codec_config = v, None));
            if is_macos {
                rows.push(Row {
                    label: "Renderer",
                    help: String::new(),
                    modified: s.renderer_selection != d.renderer_selection,
                    disabled_reason: None,
                    badge: None,
                    control: dropdown_control(root, theme, "dd-renderer", s.renderer_selection.label().to_string(), Dropdown::Renderer, cx),
                    extra: None,
                    reset: Some(Box::new(|s| s.renderer_selection = Renderer::Auto)),
                });
            }
            rows.push(toggle_row(root, cx, theme, "tg-yuv444", "YUV 4:4:4", "Sharper text, not for fast games. Raises the default bitrate.", |s| s.enable_yuv444, |s, v| {
                s.enable_yuv444 = v;
                s.apply_auto_bitrate();
            }, None));
            let mut r = toggle_row(root, cx, theme, "tg-unlock", "Unlock bitrate limit", "Up to 500 Mbps for Sunshine/Apollo. Wired LAN only.", |s| s.unlock_bitrate, |s, v| s.unlock_bitrate = v, None);
            r.badge = Some(("Experimental", true));
            rows.push(r);
            rows.push(toggle_row(root, cx, theme, "tg-mdns", "Discover hosts on the local network", "Recommended.", |s| s.enable_mdns, |s, v| s.enable_mdns = v, None));
            rows.push(toggle_row(root, cx, theme, "tg-netblock", "Detect blocked connections", "Explains failures caused by firewalls or filters.", |s| s.detect_network_blocking, |s, v| s.detect_network_blocking = v, None));
            if is_macos {
                rows.push(toggle_row(root, cx, theme, "tg-exclusive-fs", "Exclusive fullscreen", "Real display-mode switching for the Fullscreen display mode. Moonlight disables it on macOS because it can hang or deadlock.", |s| s.exclusive_fullscreen, |s, v| s.exclusive_fullscreen = v, if s.window_mode == WindowMode::Fullscreen { None } else { Some("Needs the Fullscreen display mode.") }));
            }
            let detected = root.helper_path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "not found".into());
            rows.push(Row {
                label: "Streaming helper",
                help: format!("Moonlight binary that runs the stream. Detected: {detected}"),
                modified: !s.helper_path.is_empty(),
                disabled_reason: None,
                badge: None,
                control: div().w(px(260.)).child(root.settings_view.helper.clone()).into_any_element(),
                extra: None,
                reset: Some(Box::new(|s| s.helper_path.clear())),
            });
        }
        "about" => {
            let link_row = |label: &'static str, url: &str| Row {
                label,
                help: String::new(),
                modified: false,
                disabled_reason: None,
                badge: None,
                control: {
                    let url = url.to_string();
                    button(label, "Open").ghost().icon(icon::EXTERNAL_LINK).on_click(move |_, _, cx| cx.open_url(&url)).into_any_element()
                },
                extra: None,
                reset: None,
            };
            rows.push(Row {
                label: "Version",
                help: "Moonlight Qt streaming core with the sloplight launcher.".into(),
                modified: false,
                disabled_reason: None,
                badge: None,
                control: mono_sm(theme, format!("sloplight {}", env!("CARGO_PKG_VERSION"))).into_any_element(),
                extra: None,
                reset: None,
            });
            rows.push(link_row("Setup guide", "https://github.com/moonlight-stream/moonlight-docs/wiki/Setup-Guide"));
            rows.push(link_row("Troubleshooting", "https://github.com/moonlight-stream/moonlight-docs/wiki/Troubleshooting"));
            rows.push(link_row("Log file", &format!("file://{}", crate::log_path().display())));
            rows.push(link_row("Gamepad mapping guide", "https://github.com/moonlight-stream/moonlight-docs/wiki/Gamepad-Mapping"));
            rows.push(link_row("Moonlight Discord", "https://moonlight-stream.org/discord"));
        }
        _ => {}
    }
    rows
}

fn render_row(theme: &Theme, row: Row, index: usize, last: bool, cx: &mut Context<Root>) -> AnyElement {
    let disabled = row.disabled_reason.is_some();
    let label_color = if disabled { theme.text_disabled } else { theme.text };
    let help = row.disabled_reason.map(|r| r.to_string()).unwrap_or(row.help);
    let reset = row.reset;
    let modified = row.modified;
    div()
        .id(gpui::ElementId::NamedInteger("setting-row".into(), index as u64))
        .group("row")
        .flex()
        .flex_col()
        .w_full()
        .when(!last, |d| d.border_b_1().border_color(theme.border))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(16.))
                .w_full()
                .min_w(px(0.))
                .pt(px(8.))
                .pb(px(8.))
                .px(px(8.))
                .mx(px(-8.))
                .rounded(px(4.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .flex_1()
                        .min_w(px(0.))
                        .max_w(gpui::relative(0.66))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .child(div().text_color(label_color).child(row.label))
                                .when_some(row.badge, |d, (text, _)| d.child(badge(theme, text, theme.warning, theme.warning_subtle)))
                                // The reset button's slot is always reserved so the
                                // label line doesn't shift when a setting becomes
                                // modified (or is reset back to its default).
                                .when(reset.is_some(), |d| {
                                    if modified {
                                        d.child(
                                            icon_button(gpui::ElementId::NamedInteger("reset".into(), index as u64), icon::ROTATE_CCW)
                                                .size(20., 12.)
                                                .tooltip("Reset to default", None)
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    if let Some(r) = &reset {
                                                        r(&mut this.settings);
                                                    }
                                                    this.after_settings_change(cx);
                                                })),
                                        )
                                    } else {
                                        d.child(div().flex_none().w(px(20.)).h(px(20.)))
                                    }
                                }),
                        )
                        .when(!help.is_empty(), |d| d.child(text_sm(theme, help).w_full().when(disabled, |t| t.text_color(theme.text_disabled)))),
                )
                .child(div().flex().items_center().gap(px(8.)).flex_none().children(row.extra).child(row.control)),
        )
        .into_any_element()
}

pub fn handle_key(root: &mut Root, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Root>) {
    let key = event.keystroke.key.as_str();
    let cmd = event.keystroke.modifiers.platform;
    match (key, cmd) {
        ("escape", _) => root.action_back(&crate::app::Back, _window, cx),
        ("up", true) => {
            root.settings_view.section = root.settings_view.section.saturating_sub(1);
            cx.notify();
        }
        ("down", true) => {
            root.settings_view.section = (root.settings_view.section + 1).min(SECTIONS.len() - 1);
            cx.notify();
        }
        ("z", true) => {
            if let Some(prev) = root.settings_view.last_change.take() {
                root.settings = prev;
                root.after_settings_change(cx);
            }
        }
        _ => {}
    }
}

/// Section list, shown in the sidebar slot while settings are open.
pub fn render_sections(root: &mut Root, theme: &Theme, cx: &mut Context<Root>) -> impl IntoElement {
    let filter = root.settings_view.filter_text.trim().to_lowercase();
    let filtering = !filter.is_empty();
    let current = root.settings_view.section;
    let mut nav = div().flex().flex_col().px(px(6.)).gap(px(1.));
    for (i, section) in SECTIONS.iter().enumerate() {
        let selected = i == current && !filtering;
        let matches = !filtering || section_rows(section.id).iter().any(|r| r.to_lowercase().contains(&filter)) || section.title.to_lowercase().contains(&filter);
        let hover_bg = theme.element_hover;
        nav = nav.child(
            div()
                .id(gpui::ElementId::NamedInteger("section".into(), i as u64))
                .flex()
                .items_center()
                .h(px(28.))
                .px(px(6.))
                .rounded(px(4.))
                .cursor_pointer()
                .when(selected, |d| d.bg(theme.selection))
                .when(!selected, |d| d.hover(move |s| s.bg(hover_bg)))
                .when(filtering && !matches, |d| d.opacity(0.4))
                .child(div().flex_1().text_color(if selected { theme.text } else { theme.text_muted }).child(section.title))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.settings_view.section = i;
                    if !this.settings_view.filter_text.is_empty() {
                        this.settings_view.filter_text.clear();
                        this.settings_view.filter.update(cx, |f, cx| f.clear(cx));
                    }
                    this.focus_main(cx);
                    cx.notify();
                })),
        );
    }
    nav
}

/// Settings rows for the current section (or all matching sections while filtering).
pub fn render_content(root: &mut Root, theme: &Theme, content_w: f32, window: &mut Window, cx: &mut Context<Root>) -> impl IntoElement {
    let col_offset = crate::theme::layout(content_w, Root::body_height(window)).settings_offset;
    let filter = root.settings_view.filter_text.trim().to_lowercase();
    let filtering = !filter.is_empty();
    let current = root.settings_view.section;

    let mut content = div().flex().flex_col().w_full().max_w(px(640. + 24. + 16.)).min_w(px(0.)).pl(px(24.)).pr(px(16.)).pb(px(32.));
    let mut row_index = 0usize;
    let sections_to_show: Vec<usize> = if filtering { (0..SECTIONS.len()).collect() } else { vec![current] };
    for si in sections_to_show {
        let section = &SECTIONS[si];
        let rows: Vec<Row> = build_rows(root, theme, section.id, cx)
            .into_iter()
            .filter(|r| !filtering || r.label.to_lowercase().contains(&filter) || r.help.to_lowercase().contains(&filter))
            .collect();
        if rows.is_empty() {
            continue;
        }
        let mut description = section.description.to_string();
        if section.id == "apollo" {
            description = "These options need Apollo on the host. Sunshine and GeForce Experience ignore them.".into();
        }
        content = content.child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .pt(px(if filtering { 20. } else { 4. }))
                .pb(px(6.))
                .border_b_1()
                .border_color(theme.border)
                .child(text_lg(theme, section.title))
                .when(!description.is_empty(), |d| d.child(text_sm(theme, description).w_full())),
        );
        let n = rows.len();
        let mut items = div().flex().flex_col();
        for (k, row) in rows.into_iter().enumerate() {
            items = items.child(render_row(theme, row, row_index, k + 1 == n, cx));
            row_index += 1;
        }
        content = content.child(items);
    }
    if row_index == 0 {
        content = content.child(div().pt(px(24.)).child(text_sm(theme, format!("No settings match \"{}\"", root.settings_view.filter_text))));
    }
    div().id("settings-content").flex_1().min_h(px(0.)).overflow_y_scroll().child(div().pl(px(col_offset)).child(content))
}

#[allow(dead_code)]
pub fn type_sm() -> gpui::Pixels {
    type_scale::SM
}
