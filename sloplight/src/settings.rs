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
    /// App windows: resize the host window on every Mac resize step (true) or
    /// only once the drag settles (false).
    #[serde(default = "default_true")]
    pub app_window_live_resize: bool,
    /// While any window is streamed, windows that newly appear on the host
    /// (opened from the streamed app, focus-grabbing dialogs of other
    /// processes) are streamed too.
    #[serde(default = "default_true")]
    pub app_window_follow_new: bool,
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
            app_window_live_resize: true,
            app_window_follow_new: true,
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

fn default_true() -> bool {
    true
}
