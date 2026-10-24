use std::borrow::Cow;

use anyhow::Result;
use gpui::{AssetSource, SharedString};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "assets"]
#[include = "icons/*.svg"]
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(Self::get(path).map(|f| f.data))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(Self::iter().filter(|p| p.starts_with(path)).map(SharedString::from).collect())
    }
}

/// Icon names used by the UI (Lucide). `icon::PLUS` → "icons/plus.svg".
#[allow(dead_code)]
pub mod icon {
    macro_rules! icons { ($($name:ident = $file:literal),* $(,)?) => { $(pub const $name: &str = concat!("icons/", $file, ".svg");)* }; }
    icons! {
        PLUS = "plus", SEARCH = "search", X = "x", EYE = "eye", EYE_OFF = "eye-off", REFRESH = "refresh-cw",
        INFO = "info", ARROW_LEFT = "arrow-left", SETTINGS = "settings", LOCK = "lock", POWER = "power",
        PLAY = "play", SQUARE = "square", ZAP = "zap", GAMEPAD = "gamepad-2", LAYOUT_GRID = "layout-grid",
        SHIELD_ALERT = "shield-alert", CIRCLE_ALERT = "circle-alert", CIRCLE_X = "circle-x", CIRCLE_CHECK = "circle-check",
        TRIANGLE_ALERT = "triangle-alert", LOADER = "loader-circle", COPY = "copy", CHECK = "check",
        CHEVRONS_UP_DOWN = "chevrons-up-down", ROTATE_CCW = "rotate-ccw", RADAR = "radar", EXTERNAL_LINK = "external-link",
        MONITOR_PLAY = "monitor-play", SERVER = "server", VOLUME = "volume-2", KEYBOARD = "keyboard", MONITOR = "monitor",
        APP_WINDOW = "app-window", SLIDERS = "sliders-horizontal", NETWORK = "network", PENCIL = "pencil", TRASH = "trash-2",
        DOWNLOAD = "download", CHEVRON_DOWN = "chevron-down", CHEVRON_RIGHT = "chevron-right", WIFI_OFF = "wifi-off",
    }
}
