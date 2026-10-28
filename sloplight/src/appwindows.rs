//! App-window streaming on the launcher side: the host's window list for the
//! selected host and the desk stream. The desk is a private host display that
//! mirrors this screen; streamed windows are *adopted* onto it and one helper
//! streams the whole desk, showing every window on it as a Mac window of its
//! own (the helper follows the desk's layout itself). See DESIGN-APPWINDOWS.md.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use futures::channel::mpsc::UnboundedSender;
use gpui::Context;

use crate::app::Root;
use crate::ui::ToastKind;
use crate::backend::Host;
use crate::backend::http::{DeskInfo, HostWindow};
use crate::settings;
use crate::stream::{self, AppWindowConfig, HelperEvent, StreamEvent, StreamProcess};

/// Window list of one host, as last fetched.
#[derive(Default)]
pub struct WindowsState {
    pub host_uuid: String,
    pub windows: Vec<HostWindow>,
    /// icon key -> cached PNG file
    pub icons: HashMap<String, PathBuf>,
    pub loading: bool,
    pub error: Option<String>,
    pub fetched_at: Option<Instant>,
}

/// The desk stream of one host: the helper that shows the desk's windows.
pub struct DeskStream {
    pub host_uuid: String,
    pub desk: DeskInfo,
    pub process: StreamProcess,
    pub connected: bool,
    pub error_lines: Vec<String>,
    /// Windows this launcher adopted (lower-case "0x..." handles)
    pub adopted: Vec<String>,
    /// Points per host pixel of the desk
    pub points_per_px: f32,
}

/// Stream events of the desk helper carry this in place of a window handle.
pub const DESK_STREAM_ID: &str = "desk";

pub type WindowsSender = UnboundedSender<WindowsEvent>;

pub enum WindowsEvent {
    List { host_uuid: String, result: Result<(Vec<HostWindow>, HashMap<String, PathBuf>), String> },
    /// A top-level window appeared on the host while windows were being streamed
    NewWindow { host_uuid: String, window: HostWindow },
}

fn icon_dir(cache_dir: &std::path::Path) -> PathBuf {
    cache_dir.join("window-icons")
}

impl Root {
    pub fn host_supports_windows(host: &Host) -> bool {
        crate::backend::apollo::has(host.apollo_permissions, crate::backend::apollo::APP_WINDOWS)
    }

    /// Fetches the host's window list (and any missing icons) in the background.
    pub fn refresh_host_windows(&mut self, uuid: &str, cx: &mut Context<Self>) {
        let Some(host) = self.host_by_uuid(uuid).cloned() else { return };
        if !Self::host_supports_windows(&host) {
            return;
        }
        if self.windows.host_uuid != uuid {
            self.windows = WindowsState { host_uuid: uuid.to_string(), ..Default::default() };
        }
        if self.windows.loading {
            return;
        }
        let Some(http) = host.http(self.identity.clone()) else { return };
        self.windows.loading = true;
        let tx = self.windows_tx.clone();
        let dir = icon_dir(&self.cache_dir);
        let known: HashMap<String, PathBuf> = self.windows.icons.clone();
        let uuid = uuid.to_string();
        std::thread::Builder::new()
            .name("host windows".into())
            .spawn(move || {
                let result = http.get_host_windows().map_err(|e| e.to_string()).map(|list| {
                    let _ = std::fs::create_dir_all(&dir);
                    let mut icons = known;
                    for w in &list {
                        if w.icon.is_empty() || icons.contains_key(&w.icon) {
                            continue;
                        }
                        let path = dir.join(format!("{}.png", w.icon.replace(['/', '\\', ':'], "_")));
                        if !path.exists() {
                            match http.get_window_icon(&w.hwnd) {
                                Ok(png) if !png.is_empty() => {
                                    let _ = std::fs::write(&path, png);
                                }
                                _ => continue,
                            }
                        }
                        icons.insert(w.icon.clone(), path);
                    }
                    (list, icons)
                });
                let _ = tx.unbounded_send(WindowsEvent::List { host_uuid: uuid, result });
            })
            .ok();
        cx.notify();
    }

    pub fn on_windows_event(&mut self, event: WindowsEvent, cx: &mut Context<Self>) {
        match event {
            WindowsEvent::List { host_uuid, result } => {
                if self.windows.host_uuid != host_uuid {
                    return;
                }
                self.windows.loading = false;
                self.windows.fetched_at = Some(Instant::now());
                match result {
                    Ok((list, icons)) => {
                        self.windows.windows = list;
                        self.windows.icons = icons;
                        self.windows.error = None;
                    }
                    Err(e) => self.windows.error = Some(e),
                }
            }
            WindowsEvent::NewWindow { host_uuid, window } => {
                if !self.settings.app_window_follow_new || self.window_streaming(&window.hwnd) {
                    return;
                }
                if self.windows.host_uuid == host_uuid && !self.windows.windows.iter().any(|w| w.hwnd.eq_ignore_ascii_case(&window.hwnd)) {
                    self.windows.windows.push(window.clone());
                }
                log::info!("new host window {} {:?} ({}); streaming it", window.hwnd, window.title, window.process);
                let hwnd = window.hwnd.clone();
                self.launch_window(&host_uuid, &hwnd, cx);
            }
        }
        cx.notify();
    }

    /// While windows of `host` are streamed, watch the host's window list and
    /// report windows that newly appear (opened from the streamed app, or
    /// dialogs of other processes grabbing focus) so they get streamed too.
    fn ensure_window_watch(&mut self, host: &Host) {
        if self.window_watch_stop.is_some() {
            return;
        }
        let Some(http) = host.http(self.identity.clone()) else { return };
        let stop = Arc::new(AtomicBool::new(false));
        self.window_watch_stop = Some(stop.clone());
        let tx = self.windows_tx.clone();
        let uuid = host.uuid.clone();
        let desk_display = self.desk_stream.as_ref().map(|d| d.desk.display.to_ascii_lowercase()).unwrap_or_default();
        std::thread::Builder::new()
            .name("host window watch".into())
            .spawn(move || {
                let mut known: Option<std::collections::HashSet<String>> = None;
                while !stop.load(Ordering::Relaxed) {
                    if let Ok(list) = http.get_host_windows() {
                        match &mut known {
                            None => known = Some(list.iter().map(|w| w.hwnd.to_ascii_lowercase()).collect()),
                            Some(seen) => {
                                for w in list {
                                    let on_desk = !desk_display.is_empty() && w.monitor.eq_ignore_ascii_case(&desk_display);
                                    if seen.insert(w.hwnd.to_ascii_lowercase()) && w.state != "minimized" && !on_desk {
                                        let _ = tx.unbounded_send(WindowsEvent::NewWindow { host_uuid: uuid.clone(), window: w });
                                    }
                                }
                            }
                        }
                    }
                    std::thread::sleep(Duration::from_millis(1500));
                }
            })
            .ok();
    }

    fn stop_window_watch(&mut self) {
        if let Some(stop) = self.window_watch_stop.take() {
            stop.store(true, Ordering::Relaxed);
        }
    }

    /// Is this host window streamed (adopted onto the desk)?
    pub fn window_streaming(&self, hwnd: &str) -> bool {
        let Some(ds) = &self.desk_stream else { return false };
        if ds.adopted.iter().any(|h| h.eq_ignore_ascii_case(hwnd)) {
            return true;
        }
        // Windows that opened on the desk by themselves (dialogs, new app windows)
        self.windows.windows.iter().any(|w| w.hwnd.eq_ignore_ascii_case(hwnd) && w.monitor.eq_ignore_ascii_case(&ds.desk.display))
    }

    /// Points per host pixel for a desk at this DPI.
    /// Device pixels per point of the primary screen (2 on Retina).
    pub fn screen_backing_scale() -> f32 {
        crate::display::displays()
            .first()
            .filter(|d| d.points.0 > 0 && d.current.0 > 0)
            .map(|d| d.current.0 as f32 / d.points.0 as f32)
            .unwrap_or(2.0)
    }

    fn desk_points_per_px(_dpi: i32) -> f32 {
        1.0 / Self::screen_backing_scale()
    }

    /// Streams a host window: opens the host's desk for this screen (first
    /// window), starts the desk helper, and adopts the window onto the desk at
    /// the place it has on its host monitor.
    pub fn launch_window(&mut self, uuid: &str, hwnd: &str, cx: &mut Context<Self>) {
        if self.window_streaming(hwnd) {
            return;
        }
        let Some(host) = self.host_by_uuid(uuid).cloned() else { return };
        let Some(w) = self.windows.windows.iter().find(|w| w.hwnd.eq_ignore_ascii_case(hwnd)).cloned() else { return };
        let Some(http) = host.http(self.identity.clone()) else { return };
        if let Some(ds) = &mut self.desk_stream {
            if ds.host_uuid != host.uuid {
                self.toast(ToastKind::Error, "Windows of one host at a time: stop the other host's windows first", cx);
                return;
            }
            // Desk already streaming: adopt where the window is on its monitor
            let (x, y) = Self::adopt_position(&w, ds.points_per_px, cx);
            ds.adopted.push(w.hwnd.to_ascii_lowercase());
            std::thread::spawn(move || {
                if let Err(e) = http.window_action(&w.hwnd, "adopt", x, y, 0, 0) {
                    log::warn!("adopt {}: {e}", w.hwnd);
                }
            });
            cx.notify();
            return;
        }
        let helper = self.helper_path.clone().or_else(|| stream::find_helper(&self.settings));
        let Some(helper) = helper else {
            self.toast(ToastKind::Error, "Streaming helper not found; set it in Settings", cx);
            return;
        };
        self.helper_path = Some(helper.clone());
        let app = host.apps.iter().find(|a| a.id == host.current_game_id && a.id != 0).or_else(|| host.apps.first()).cloned();
        let Some(app) = app else {
            self.toast(ToastKind::Error, "The host lists no apps; can't start a window stream", cx);
            return;
        };
        // The desk mirrors the screen the launcher is on, at the host window's DPI
        let Some(display) = cx.primary_display() else {
            self.toast(ToastKind::Error, "No display to mirror", cx);
            return;
        };
        let b = display.bounds();
        let screen = [f32::from(b.origin.x) as i32, f32::from(b.origin.y) as i32, f32::from(b.size.width) as i32, f32::from(b.size.height) as i32];
        // The desk is the screen in device pixels; the host renders at the DPI
        // that makes windows the chosen fraction of the Mac's logical size.
        let backing = Self::screen_backing_scale();
        let dpi = (96.0 * backing * self.settings.app_window_scale.clamp(25, 400) as f32 / 100.0).round() as i32;
        let want = ((screen[2] as f32 * backing).round() as i32, (screen[3] as f32 * backing).round() as i32);
        let desk = match http.open_desk(want.0, want.1, self.settings.fps.max(30), dpi) {
            Ok(d) => d,
            Err(e) => {
                self.toast(ToastKind::Error, format!("Couldn't open the desk on the host: {e}"), cx);
                return;
            }
        };
        log::info!("desk {} {:?} @{} {} dpi for a {}x{} pt screen ({}x{} px wanted)", desk.display, desk.rect, desk.fps, desk.dpi, screen[2], screen[3], want.0, want.1);
        let icon_png = self.windows.icons.get(&w.icon).and_then(|p| std::fs::read(p).ok()).unwrap_or_default();
        let mut settings = self.settings.clone();
        settings.bitrate_kbps = settings.app_window_bitrate_kbps.clamp(5000, 150000);
        settings.width = desk.rect[2];
        settings.height = desk.rect[3];
        settings.fps = desk.fps;
        settings.window_mode = settings::WindowMode::Windowed;
        settings.hide_host_cursor = true;
        settings.enable_vsync = false;
        settings.frame_pacing = false;
        let cfg = AppWindowConfig { icon_png_base64: stream::base64_encode(&icon_png), dpi: desk.dpi, desk: true, screen };
        match stream::start_with_window(&helper, &self.identity, &host, &app, &settings, None, self.stream_tx.clone(), Some((DESK_STREAM_ID.to_string(), cfg))) {
            Ok(process) => {
                let points_per_px = Self::desk_points_per_px(desk.dpi);
                let (x, y) = Self::adopt_position(&w, points_per_px, cx);
                let adopt_http = http.clone();
                let hwnd_s = w.hwnd.clone();
                std::thread::spawn(move || {
                    if let Err(e) = adopt_http.window_action(&hwnd_s, "adopt", x, y, 0, 0) {
                        log::warn!("adopt {hwnd_s}: {e}");
                    }
                });
                self.desk_stream = Some(DeskStream {
                    host_uuid: host.uuid.clone(),
                    desk,
                    process,
                    connected: false,
                    error_lines: Vec::new(),
                    adopted: vec![w.hwnd.to_ascii_lowercase()],
                    points_per_px,
                });
                self.ensure_window_watch(&host);
            }
            Err(e) => {
                let _ = http.close_desk();
                self.toast(ToastKind::Error, format!("Couldn't stream {}: {e}", w.title), cx);
            }
        }
        cx.notify();
    }

    /// Where a newly adopted window goes on the desk: its place on its host
    /// monitor, so the layout comes over as it was (the host clamps it in).
    fn adopt_position(w: &HostWindow, points_per_px: f32, cx: &Context<Self>) -> (i32, i32) {
        let (x, y) = (w.rect[0] - w.monitor_rect[0], w.rect[1] - w.monitor_rect[1]);
        // Inside the mirrored screen (desk px), not just inside the desk
        let Some(d) = cx.primary_display() else { return (x, y) };
        let b = d.bounds();
        let (sw, sh) = ((f32::from(b.size.width) / points_per_px) as i32, (f32::from(b.size.height) / points_per_px) as i32);
        // The window rescales to the desk's DPI when it lands there
        let k = (96.0 / points_per_px) / w.dpi.max(48) as f32;
        let (ww, wh) = ((w.rect[2] as f32 * k) as i32, (w.rect[3] as f32 * k) as i32);
        (x.min((sw - ww).max(0)).max(0), y.min((sh - wh).max(0)).max(0))
    }

    /// Stops streaming one window: it goes back to where it was on the host.
    /// The desk stream ends with its last window.
    pub fn close_window_stream(&mut self, hwnd: &str, cx: &mut Context<Self>) {
        let Some(ds) = &mut self.desk_stream else { return };
        let host_uuid = ds.host_uuid.clone();
        let desk_display = ds.desk.display.clone();
        ds.adopted.retain(|h| !h.eq_ignore_ascii_case(hwnd));
        let adopted_left = ds.adopted.len();
        let others_on_desk = self.windows.windows.iter().any(|w| {
            !w.hwnd.eq_ignore_ascii_case(hwnd) && w.monitor.eq_ignore_ascii_case(&desk_display) && w.state != "minimized"
        });
        let last = adopted_left == 0 && !others_on_desk;
        let Some(host) = self.host_by_uuid(&host_uuid).cloned() else { return };
        let Some(ds) = &mut self.desk_stream else { return };
        if let Some(http) = host.http(self.identity.clone()) {
            let hwnd = hwnd.to_string();
            std::thread::spawn(move || {
                if let Err(e) = http.window_action(&hwnd, "release", 0, 0, 0, 0) {
                    log::warn!("release {hwnd}: {e}");
                }
                if last {
                    let _ = http.close_desk();
                }
            });
        }
        if last {
            ds.process.terminate();
        }
        cx.notify();
    }

    /// Ends the desk stream: every adopted window goes back to the host's own display.
    pub fn close_desk_stream(&mut self, cx: &mut Context<Self>) {
        let Some(ds) = &self.desk_stream else { return };
        if let Some(http) = self.host_by_uuid(&ds.host_uuid).and_then(|h| h.http(self.identity.clone())) {
            std::thread::spawn(move || {
                let _ = http.close_desk();
            });
        }
        ds.process.terminate();
        cx.notify();
    }

    /// Events from the desk helper (routed here by `on_stream_event`).
    pub fn on_desk_stream_event(&mut self, event: StreamEvent, cx: &mut Context<Self>) {
        let Some(ds) = &mut self.desk_stream else { return };
        match event.event {
            HelperEvent::ConnectionStarted => ds.connected = true,
            HelperEvent::StageFailed { stage, error_code, .. } => ds.error_lines.push(format!("Starting {stage} failed: Error {error_code}")),
            HelperEvent::LaunchError { text } => ds.error_lines.push(text),
            HelperEvent::AppWindow { kind, hwnd, .. } => {
                // The helper talks to the host itself; this is bookkeeping.
                if kind == "close" {
                    if let Some(h) = hwnd {
                        ds.adopted.retain(|a| !a.eq_ignore_ascii_case(&h));
                    } else {
                        // The hidden key window was closed: end the desk
                        self.close_desk_stream(cx);
                        return;
                    }
                }
            }
            HelperEvent::Exited { code, .. } => {
                let ds = self.desk_stream.take().unwrap();
                if !ds.connected && code != Some(0) {
                    let why = if ds.error_lines.is_empty() { format!("helper exited with code {code:?}") } else { ds.error_lines.join(" ") };
                    self.toast(ToastKind::Error, format!("Couldn't stream the desk: {why}"), cx);
                }
                if let Some(http) = self.host_by_uuid(&ds.host_uuid).and_then(|h| h.http(self.identity.clone())) {
                    std::thread::spawn(move || {
                        let _ = http.close_desk();
                    });
                }
                self.stop_window_watch();
            }
            _ => {}
        }
        cx.notify();
    }
}
