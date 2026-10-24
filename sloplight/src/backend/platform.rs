//! Platform-specific explanations and fixes for connectivity failures.

use super::http::HttpError;

/// Name of the setting the user has to flip when the OS blocks LAN access.
#[cfg(target_os = "macos")]
pub const LOCAL_NETWORK_HINT: &str = "macOS is blocking local network access for this app. Allow Sloplight under System Settings \u{2192} Privacy & Security \u{2192} Local Network, then relaunch. When started from a terminal, the terminal app needs the permission instead.";

/// A deep link that opens the relevant privacy pane in System Settings.
#[cfg(target_os = "macos")]
pub const LOCAL_NETWORK_SETTINGS_URL: &str = "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_LocalNetwork";

pub fn resolution_for(e: &HttpError) -> Option<&'static str> {
    #[cfg(target_os = "macos")]
    {
        match e {
            HttpError::HostUnreachable => Some(LOCAL_NETWORK_HINT),
            _ => None,
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = e;
        None
    }
}

/// One-line reminder for the discovery empty state on gated platforms.
pub fn discovery_hint() -> Option<&'static str> {
    #[cfg(target_os = "macos")]
    {
        Some("Nothing showing up? Sloplight needs the Local Network permission in System Settings \u{2192} Privacy & Security.")
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}
