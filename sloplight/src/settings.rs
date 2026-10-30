//! Stream and application settings, persisted as JSON. Mirrors
//! `StreamingPreferences` so the C++ helper can consume them unchanged.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

macro_rules! enum_setting {
    ($name:ident { $($variant:ident = $val:expr => $label:expr),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(into = "i32", try_from = "i32")]
        #[repr(i32)]
        pub enum $name { $($variant = $val),+ }
        impl From<$name> for i32 {
            fn from(v: $name) -> i32 { v as i32 }
        }
        impl TryFrom<i32> for $name {
            type Error = String;
            fn try_from(v: i32) -> Result<Self, String> {
                $name::from_i32(v).ok_or_else(|| format!("invalid {} value {v}", stringify!($name)))
            }
        }
        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];
            pub fn label(self) -> &'static str { match self { $($name::$variant => $label),+ } }
            pub fn as_i32(self) -> i32 { self as i32 }
            pub fn from_i32(v: i32) -> Option<Self> { Self::ALL.iter().copied().find(|x| x.as_i32() == v) }
        }
    };
}

enum_setting!(AudioConfig { Stereo = 0 => "Stereo", Surround51 = 1 => "5.1", Surround71 = 2 => "7.1" });
enum_setting!(VideoCodec { Auto = 0 => "Auto", H264 = 1 => "H.264", Hevc = 2 => "HEVC", Av1 = 4 => "AV1" });
enum_setting!(VideoDecoder { Auto = 0 => "Auto", Hardware = 1 => "Hardware", Software = 2 => "Software" });
enum_setting!(Renderer { Auto = 0 => "Automatic", Vulkan = 1 => "Vulkan", Metal = 2 => "Metal", AvSampleBufferDisplayLayer = 3 => "AVSampleBufferDisplayLayer" });
enum_setting!(WindowMode { Fullscreen = 0 => "Fullscreen", Borderless = 1 => "Borderless", Windowed = 2 => "Windowed" });
enum_setting!(UiDisplayMode { Windowed = 0 => "Windowed", Maximized = 1 => "Maximized", Fullscreen = 2 => "Fullscreen" });
enum_setting!(CaptureSysKeys { Off = 0 => "Off", Fullscreen = 1 => "In fullscreen", Always = 2 => "Always" });
enum_setting!(WindowDensity { Retina = 0 => "Retina", Standard = 1 => "Standard" });

/// The settings that suit a network: a preset holds these, the rest are
/// shared. Choosing a preset copies its values into the settings; changing
/// any of them afterwards leaves the preset itself as it was.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preset {
    pub width: i32,
    pub height: i32,
    pub fps: i32,
    pub fractional_fps: f64,
    pub bitrate_kbps: i32,
    pub auto_adjust_bitrate: bool,
    pub enable_yuv444: bool,
    pub video_codec_config: VideoCodec,
    pub packet_size: i32,
    pub app_window_bitrate_kbps: i32,
    pub app_window_background_divisor: i32,
    pub app_window_skip_covered: bool,
    pub app_window_density: WindowDensity,
}

impl Default for WindowDensity {
    fn default() -> Self {
        WindowDensity::Retina
    }
}

impl Default for Preset {
    fn default() -> Self {
        Preset::of(&Settings::default())
    }
}

impl Preset {
    pub fn of(s: &Settings) -> Preset {
        Preset {
            width: s.width,
            height: s.height,
            fps: s.fps,
            fractional_fps: s.fractional_fps,
            bitrate_kbps: s.bitrate_kbps,
            auto_adjust_bitrate: s.auto_adjust_bitrate,
            enable_yuv444: s.enable_yuv444,
            video_codec_config: s.video_codec_config,
            packet_size: s.packet_size,
            app_window_bitrate_kbps: s.app_window_bitrate_kbps,
            app_window_background_divisor: s.app_window_background_divisor,
            app_window_skip_covered: s.app_window_skip_covered,
            app_window_density: s.app_window_density,
        }
    }

    pub fn apply_to(&self, s: &mut Settings) {
        s.width = self.width;
        s.height = self.height;
        s.fps = self.fps;
        s.fractional_fps = self.fractional_fps;
        s.bitrate_kbps = self.bitrate_kbps;
        s.auto_adjust_bitrate = self.auto_adjust_bitrate;
        s.enable_yuv444 = self.enable_yuv444;
        s.video_codec_config = self.video_codec_config;
        s.packet_size = self.packet_size;
        s.app_window_bitrate_kbps = self.app_window_bitrate_kbps;
        s.app_window_background_divisor = self.app_window_background_divisor;
        s.app_window_skip_covered = self.app_window_skip_covered;
        s.app_window_density = self.app_window_density;
    }

    /// Full quality (home Wi-Fi, LAN): the screen's own resolution at its
    /// refresh rate, windows at Retina density, bitrates a LAN carries
    pub fn full_quality(screen_px: (i32, i32), refresh: i32) -> Preset {
        let (width, height) = screen_px;
        let fps = refresh.clamp(60, 120);
        Preset {
            width,
            height,
            fps,
            fractional_fps: 0.0,
            bitrate_kbps: default_bitrate(width, height, fps, false).min(150_000),
            auto_adjust_bitrate: true,
            enable_yuv444: false,
            video_codec_config: VideoCodec::Auto,
            packet_size: 0,
            app_window_bitrate_kbps: 40000,
            app_window_background_divisor: 1,
            app_window_skip_covered: true,
            app_window_density: WindowDensity::Retina,
        }
    }

    /// Data saver (a phone's hotspot, slow or metered links: a few Mbps to a
    /// few tens, latency that varies, some loss): half the screen's pixels
    /// each way (sharp enough, 4x less to encode and send), 60 fps for smooth
    /// scrolling, HEVC, background windows at a quarter rate
    pub fn data_saver(screen_pts: (i32, i32)) -> Preset {
        let (width, height) = (screen_pts.0 & !1, screen_pts.1 & !1);
        Preset {
            width,
            height,
            fps: 60,
            fractional_fps: 0.0,
            bitrate_kbps: 12000,
            auto_adjust_bitrate: false,
            enable_yuv444: false,
            video_codec_config: VideoCodec::Hevc,
            packet_size: 0,
            app_window_bitrate_kbps: 12000,
            app_window_background_divisor: 4,
            app_window_skip_covered: true,
            app_window_density: WindowDensity::Standard,
        }
    }
}

/// The built-in presets: always there, never changed
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemPreset {
    FullQuality,
    DataSaver,
}

impl SystemPreset {
    pub const ALL: [SystemPreset; 2] = [SystemPreset::FullQuality, SystemPreset::DataSaver];

    pub fn id(self) -> &'static str {
        match self {
            SystemPreset::FullQuality => "full-quality",
            SystemPreset::DataSaver => "data-saver",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            SystemPreset::FullQuality => "Full quality",
            SystemPreset::DataSaver => "Data saver",
        }
    }

    /// Its values for this Mac's main screen
    pub fn values(self) -> Preset {
        let d = crate::display::displays().into_iter().next();
        let px = d.as_ref().map(|d| d.current).filter(|p| p.0 > 0).unwrap_or((1920, 1080));
        let pts = d.as_ref().map(|d| d.points).filter(|p| p.0 > 0).unwrap_or(px);
        let hz = d.as_ref().map(|d| d.refresh_hz).filter(|h| *h > 0).unwrap_or(60);
        match self {
            SystemPreset::FullQuality => Preset::full_quality(px, hz),
            SystemPreset::DataSaver => Preset::data_saver(pts),
        }
    }
}

/// A preset of the user's own, made from the settings at the time
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CustomPreset {
    pub name: String,
    pub values: Preset,
}

/// Languages in `StreamingPreferences::Language` order (index = enum value).
pub const LANGUAGES: &[(&str, &str)] = &[
    ("auto", "Automatic"),
    ("en", "English"),
    ("fr", "Français"),
    ("zh_CN", "简体中文"),
    ("de", "Deutsch"),
    ("nb_NO", "Norsk bokmål"),
    ("ru", "Русский"),
    ("es", "Español"),
    ("ja", "日本語"),
    ("vi", "Tiếng Việt"),
    ("th", "ภาษาไทย"),
    ("ko", "한국어"),
    ("hu", "Magyar"),
    ("nl", "Nederlands"),
    ("sv", "Svenska"),
    ("tr", "Türkçe"),
    ("uk", "Українська"),
    ("zh_TW", "繁體中文"),
    ("pt", "Português"),
    ("pt_BR", "Português do Brasil"),
    ("el", "Ελληνικά"),
    ("it", "Italiano"),
    ("hi", "हिन्दी"),
    ("pl", "Polski"),
    ("cs", "Čeština"),
    ("he", "עברית"),
    ("ckb", "کوردی"),
    ("lt", "Lietuvių"),
    ("et", "Eesti"),
    ("bg", "Български"),
    ("eo", "Esperanto"),
    ("ta", "தமிழ்"),
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    // Stream
    pub width: i32,
    pub height: i32,
    pub fps: i32,
    /// Fractional refresh rate (Apollo), 0 = use `fps`
    pub fractional_fps: f64,
    pub bitrate_kbps: i32,
    pub unlock_bitrate: bool,
    pub auto_adjust_bitrate: bool,
    pub enable_vsync: bool,
    pub frame_pacing: bool,
    pub enable_hdr: bool,
    pub enable_yuv444: bool,
    pub show_performance_overlay: bool,
    pub window_mode: WindowMode,
    // Apollo
    pub use_virtual_display: bool,
    /// Vibeshine/Vibepollo: capture right after each present (1000 Hz virtual display).
    pub vrr_capture: bool,
    /// While any window is streamed, windows that newly appear on the host
    /// (opened from the streamed app, focus-grabbing dialogs of other
    /// processes) are streamed too.
    #[serde(default = "default_true")]
    pub app_window_follow_new: bool,
    /// App windows: size of host windows on this screen, percent of the Mac's
    /// own logical size (100 = the host renders at 200%, one host pixel per
    /// screen pixel, so a window is as many points wide as it is 100%-pixels). Pixels
    /// stay 1:1 with the screen at every setting; the host renders at the
    /// matching DPI.
    #[serde(default = "default_app_window_scale")]
    pub app_window_scale: i32,
    /// App windows: bitrate of the desk stream (kbps). Windows are mostly
    /// still, so far less than a game needs; too much only invites packet
    /// loss, and every lost frame freezes the picture until it recovers.
    #[serde(default = "default_app_window_bitrate")]
    pub app_window_bitrate_kbps: i32,
    /// App windows: windows other than the focused one at 1/n of the frame
    /// rate (1 = full). Saves bits on what animates in the background.
    #[serde(default = "default_one")]
    pub app_window_background_divisor: i32,
    /// App windows: windows covered whole by others stay out of the stream
    /// (the Mac keeps showing their last picture).
    #[serde(default = "default_true")]
    pub app_window_skip_covered: bool,
    /// App windows: Retina draws host windows with two pixels to the point
    /// (sharp; a 4K atlas); Standard with one (like a non-Retina screen;
    /// 4x fewer pixels, for slow links)
    #[serde(default)]
    pub app_window_density: WindowDensity,
    pub resolution_scale_factor: i32,
    pub clipboard_sync: bool,
    /// Draw only the Mac cursor; the host leaves its cursor out of the
    /// capture. Always on for app windows.
    #[serde(default)]
    pub hide_host_cursor: bool,
    // Audio
    pub audio_config: AudioConfig,
    pub play_audio_on_host: bool,
    pub mute_on_focus_loss: bool,
    // Input
    pub absolute_mouse_mode: bool,
    pub absolute_touch_mode: bool,
    pub swap_mouse_buttons: bool,
    pub reverse_scroll_direction: bool,
    pub capture_sys_keys_mode: CaptureSysKeys,
    // Gamepad
    pub multi_controller: bool,
    pub swap_face_buttons: bool,
    pub gamepad_mouse: bool,
    pub background_gamepad: bool,
    // Host
    pub game_optimizations: bool,
    pub quit_app_after: bool,
    // Interface
    pub language: i32,
    pub ui_display_mode: UiDisplayMode,
    pub connection_warnings: bool,
    pub configuration_warnings: bool,
    pub rich_presence: bool,
    pub keep_awake: bool,
    // Advanced
    pub video_decoder_selection: VideoDecoder,
    pub video_codec_config: VideoCodec,
    pub renderer_selection: Renderer,
    pub enable_mdns: bool,
    pub detect_network_blocking: bool,
    /// macOS only: real display-mode fullscreen instead of a borderless
    /// window. Upstream Moonlight disables it because it can hang.
    pub exclusive_fullscreen: bool,
    pub packet_size: i32,
    /// Explicit path to the streaming helper (moonlight binary). Empty = auto-detect.
    pub helper_path: String,

    /// The preset last chosen: a system preset's id, or "custom:" and a custom
    /// preset's name; empty: none (the settings as they are)
    pub active_preset: String,
    pub custom_presets: Vec<CustomPreset>,

    // UI state (not stream settings)
    pub last_host_uuid: String,
    pub inspector_open: bool,
    pub window_width: f32,
    pub window_height: f32,
}

impl Default for Settings {
    fn default() -> Self {
        let width = 1920;
        let height = 1080;
        let fps = 60;
        Settings {
            width,
            height,
            fps,
            fractional_fps: 0.0,
            bitrate_kbps: default_bitrate(width, height, fps, false),
            unlock_bitrate: false,
            auto_adjust_bitrate: true,
            enable_vsync: true,
            frame_pacing: false,
            enable_hdr: false,
            enable_yuv444: false,
            show_performance_overlay: false,
            window_mode: recommended_window_mode(),
            use_virtual_display: false,
            vrr_capture: false,
            app_window_follow_new: true,
            app_window_scale: 100,
            app_window_bitrate_kbps: 20000,
            app_window_background_divisor: 1,
            app_window_skip_covered: true,
            app_window_density: WindowDensity::Retina,
            resolution_scale_factor: 100,
            clipboard_sync: false,
            hide_host_cursor: false,
            audio_config: AudioConfig::Stereo,
            play_audio_on_host: false,
            mute_on_focus_loss: false,
            absolute_mouse_mode: false,
            absolute_touch_mode: true,
            swap_mouse_buttons: false,
            reverse_scroll_direction: false,
            capture_sys_keys_mode: CaptureSysKeys::Off,
            multi_controller: true,
            swap_face_buttons: false,
            gamepad_mouse: true,
            background_gamepad: false,
            game_optimizations: true,
            quit_app_after: false,
            language: 0,
            ui_display_mode: UiDisplayMode::Windowed,
            connection_warnings: true,
            configuration_warnings: true,
            rich_presence: true,
            keep_awake: true,
            video_decoder_selection: VideoDecoder::Auto,
            video_codec_config: VideoCodec::Auto,
            renderer_selection: Renderer::Auto,
            enable_mdns: true,
            detect_network_blocking: true,
            exclusive_fullscreen: false,
            packet_size: 0,
            helper_path: String::new(),
            active_preset: String::new(),
            custom_presets: Vec::new(),
            last_host_uuid: String::new(),
            inspector_open: false,
            window_width: 960.0,
            window_height: 620.0,
        }
    }
}

pub fn recommended_window_mode() -> WindowMode {
    if cfg!(target_os = "macos") { WindowMode::Borderless } else { WindowMode::Fullscreen }
}

/// Port of `StreamingPreferences::getDefaultBitrate`.
pub fn default_bitrate(width: i32, height: i32, fps: i32, yuv444: bool) -> i32 {
    // Don't scale bitrate linearly beyond 60 FPS. It's definitely not a linear
    // bitrate increase for frame rate once we get to values that high.
    let frame_rate_factor = if fps <= 60 { fps as f64 } else { (fps as f64 / 60.0).sqrt() * 60.0 } / 30.0;

    struct Res { pixels: i64, factor: f64 }
    let resolutions = [
        Res { pixels: 640 * 360, factor: 1.0 },
        Res { pixels: 854 * 480, factor: 2.0 },
        Res { pixels: 1280 * 720, factor: 5.0 },
        Res { pixels: 1920 * 1080, factor: 10.0 },
        Res { pixels: 2560 * 1440, factor: 20.0 },
        Res { pixels: 3840 * 2160, factor: 40.0 },
        Res { pixels: 7680 * 4320, factor: 80.0 },
    ];
    let pixels = width as i64 * height as i64;
    let mut resolution_factor = resolutions.last().unwrap().factor;
    for (i, r) in resolutions.iter().enumerate() {
        if pixels == r.pixels {
            resolution_factor = r.factor;
            break;
        } else if pixels < r.pixels {
            if i == 0 {
                resolution_factor = r.factor;
            } else {
                let prev = &resolutions[i - 1];
                resolution_factor = (r.factor - prev.factor) * ((pixels - prev.pixels) as f64 / (r.pixels - prev.pixels) as f64) + prev.factor;
            }
            break;
        }
    }
    if yuv444 {
        resolution_factor *= 2.0;
    }
    ((resolution_factor * frame_rate_factor).round() as i32) * 1000
}

impl Settings {
    pub fn path(dir: &Path) -> PathBuf {
        dir.join("settings.json")
    }

    pub fn load(dir: &Path) -> Settings {
        match fs::read(Self::path(dir)) {
            Ok(bytes) => match serde_json::from_slice::<Settings>(&bytes) {
                Ok(mut s) => {
                    s.sanitize();
                    // No preset chosen yet: the one the settings are, if any
                    if s.active_preset.is_empty() {
                        let now = Preset::of(&s);
                        if let Some(p) = SystemPreset::ALL.iter().find(|p| p.values() == now) {
                            s.active_preset = p.id().to_string();
                        }
                    }
                    s
                }
                Err(e) => {
                    log::error!("settings.json unreadable, using defaults: {e}");
                    Settings::default()
                }
            },
            Err(_) => Settings::default(),
        }
    }

    pub fn save(&self, dir: &Path) {
        if let Err(e) = fs::create_dir_all(dir) {
            log::error!("create config dir: {e}");
            return;
        }
        match serde_json::to_vec_pretty(self) {
            Ok(bytes) => {
                let tmp = dir.join("settings.json.tmp");
                if let Err(e) = fs::write(&tmp, bytes).and_then(|_| fs::rename(&tmp, Self::path(dir))) {
                    log::error!("Failed to save settings: {e}");
                }
            }
            Err(e) => log::error!("Failed to serialize settings: {e}"),
        }
    }

    fn sanitize(&mut self) {
        self.width = self.width.clamp(256, 8192);
        self.height = self.height.clamp(256, 8192);
        self.fps = self.fps.clamp(10, 9999);
        self.resolution_scale_factor = self.resolution_scale_factor.clamp(50, 200);
        self.bitrate_kbps = self.bitrate_kbps.clamp(500, self.max_bitrate_kbps());
        if !self.enable_vsync {
            self.frame_pacing = false;
        }
    }

    /// The values of a preset by id (a system preset's, or "custom:name")
    pub fn preset_by_id(&self, id: &str) -> Option<Preset> {
        if let Some(name) = id.strip_prefix("custom:") {
            return self.custom_presets.iter().find(|p| p.name == name).map(|p| p.values.clone());
        }
        SystemPreset::ALL.iter().find(|p| p.id() == id).map(|p| p.values())
    }

    pub fn preset_name(id: &str) -> String {
        match id.strip_prefix("custom:") {
            Some(name) => name.to_string(),
            None => SystemPreset::ALL.iter().find(|p| p.id() == id).map(|p| p.name().to_string()).unwrap_or_default(),
        }
    }

    /// Choose a preset: its values become the settings
    pub fn apply_preset(&mut self, id: &str) {
        if let Some(p) = self.preset_by_id(id) {
            p.apply_to(self);
            self.active_preset = id.to_string();
        }
    }

    /// Have the settings been changed since the active preset was chosen?
    pub fn preset_modified(&self) -> bool {
        self.preset_by_id(&self.active_preset).map(|p| p != Preset::of(self)).unwrap_or(false)
    }

    /// The settings as they are, as a custom preset of this name (replacing
    /// one of the same name); it becomes the active preset
    pub fn save_custom_preset(&mut self, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        let values = Preset::of(self);
        match self.custom_presets.iter_mut().find(|p| p.name == name) {
            Some(p) => p.values = values,
            None => self.custom_presets.push(CustomPreset { name: name.to_string(), values }),
        }
        self.active_preset = format!("custom:{name}");
    }

    pub fn delete_custom_preset(&mut self, name: &str) {
        self.custom_presets.retain(|p| p.name != name);
        if self.active_preset == format!("custom:{name}") {
            self.active_preset.clear();
        }
    }

    pub fn max_bitrate_kbps(&self) -> i32 {
        if self.unlock_bitrate { 500_000 } else { 150_000 }
    }

    pub fn current_default_bitrate(&self) -> i32 {
        default_bitrate(self.width, self.height, self.fps, self.enable_yuv444)
    }

    /// Applies the auto-bitrate rule after a resolution/fps/yuv change.
    pub fn apply_auto_bitrate(&mut self) {
        if self.auto_adjust_bitrate {
            self.bitrate_kbps = self.current_default_bitrate().min(self.max_bitrate_kbps());
        }
    }

    /// Effective frame rate for display.
    pub fn fps_label(&self) -> String {
        if self.fractional_fps > 0.0 {
            let s = format!("{:.3}", self.fractional_fps);
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        } else {
            self.fps.to_string()
        }
    }

    /// Summary for the status bar: `1920×1080 · 60 fps · 20 Mbps · HEVC`.
    pub fn profile_summary(&self, apollo_host: bool) -> String {
        let mut s = format!(
            "{}×{} · {} fps · {} Mbps · {}",
            self.width,
            self.height,
            self.fps_label(),
            format_mbps(self.bitrate_kbps),
            self.video_codec_config.label()
        );
        if self.enable_hdr {
            s.push_str(" · HDR");
        }
        if self.enable_yuv444 {
            s.push_str(" · 4:4:4");
        }
        if apollo_host {
            if self.use_virtual_display {
                s.push_str(" · VD");
            }
            if self.resolution_scale_factor != 100 {
                s.push_str(&format!(" · {}%", self.resolution_scale_factor));
            }
        }
        s
    }
}

pub fn format_mbps(kbps: i32) -> String {
    let mbps = kbps as f64 / 1000.0;
    if (mbps - mbps.round()).abs() < 0.001 { format!("{}", mbps.round() as i64) } else { format!("{mbps:.1}") }
}

fn default_app_window_scale() -> i32 {
    100
}

fn default_app_window_bitrate() -> i32 {
    20000
}

fn default_true() -> bool {
    true
}

fn default_one() -> i32 {
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_apply_save_delete() {
        let mut s = Settings::default();
        let saver = Preset::data_saver((1512, 982));
        // (system presets read the screen; a custom one stands in here)
        s.custom_presets.push(CustomPreset { name: "Saver".into(), values: saver.clone() });
        s.apply_preset("custom:Saver");
        assert_eq!((s.width, s.height, s.app_window_density), (1512, 982, WindowDensity::Standard));
        assert!(!s.preset_modified());
        s.app_window_bitrate_kbps = 8000;
        assert!(s.preset_modified());
        // The preset itself stays as it was
        assert_eq!(s.preset_by_id("custom:Saver").unwrap().app_window_bitrate_kbps, 12000);
        s.save_custom_preset("Train");
        assert_eq!(s.active_preset, "custom:Train");
        assert!(!s.preset_modified());
        s.apply_preset("custom:Saver");
        assert_eq!(s.app_window_bitrate_kbps, 12000);
        s.apply_preset("custom:Train");
        assert_eq!(s.app_window_bitrate_kbps, 8000);
        s.delete_custom_preset("Train");
        assert!(s.active_preset.is_empty() && s.custom_presets.len() == 1);
        assert_eq!(Settings::preset_name("data-saver"), "Data saver");
    }
}
