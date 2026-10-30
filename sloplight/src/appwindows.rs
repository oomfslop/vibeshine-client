//! App-window streaming on the launcher side: the host's window list for the
//! selected host and the desk stream. The desk is a private host display that
//! mirrors this screen; streamed windows are *adopted* onto it and one helper
//! streams the whole desk, showing every window on it as a Mac window of its
//! own (the helper follows the desk's layout itself).

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
use crate::settings::{self, Settings, WindowDensity};
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
    /// A window to focus once the helper is connected (a launched app)
    pub focus_on_connect: Option<String>,
}

/// The desk being opened on the host (in the background: a slow link takes
/// a while), with the windows to stream once it is there
pub struct DeskOpening {
    pub host_uuid: String,
    /// The windows to adopt, the first one opens the stream; and whether to focus each
    pub windows: Vec<(HostWindow, bool)>,
}

/// A launch from the launcher until its window shows here
pub struct PendingLaunch {
    pub host_uuid: String,
    pub label: String,
    /// The app's window, once it is known
    pub hwnd: Option<String>,
    pub since: Instant,
}

/// Stream events of the desk helper carry this in place of a window handle.
pub const DESK_STREAM_ID: &str = "desk";

/// The desk for a screen of `screen_pts` points with `backing` device pixels
/// to the point: its size, the DPI host windows render at, Mac points per
/// desk pixel and the atlas (what is streamed). Retina density: a desk of the
/// screen's device pixels and a 4K atlas, windows as sharp as the Mac's own;
/// Standard: one pixel to the point (4x fewer pixels to encode and send, for
/// slow links) and a 1440p atlas.
#[derive(Clone, Copy)]
pub struct DeskPlan {
    pub size: (i32, i32),
    pub dpi: i32,
    pub points_per_px: f32,
    pub atlas: (i32, i32),
}

pub fn desk_plan(settings: &Settings, screen_pts: (i32, i32), backing: f32) -> DeskPlan {
    let k = match settings.app_window_density {
        WindowDensity::Retina => backing,
        WindowDensity::Standard => 1.0,
    };
    let size = ((screen_pts.0 as f32 * k).round() as i32, (screen_pts.1 as f32 * k).round() as i32);
    // (window size: host windows as large on this screen as the setting says,
    // at one desk pixel to `1 / k` points)
    let dpi = (96.0 * k * settings.app_window_scale.clamp(25, 400) as f32 / 100.0).round() as i32;
    // The atlas holds every window drawn whole: room to spare costs next to
    // nothing (what does not change is not sent again), while a full atlas
    // puts windows behind in as small copies and their visible parts as
    // tiles, which a window moved across them re-places (re-sent, blurry
    // until refined). 3840x2160 at most: the decoder's limit at 120 fps.
    let atlas = match settings.app_window_density {
        WindowDensity::Retina => (3840, 2160),
        // (16:9 sizes: the hardware decoder's own; odd shapes can fall back
        // to software decoding)
        WindowDensity::Standard if size.0 <= 1920 && size.1 <= 1200 => (2560, 1440),
        WindowDensity::Standard => (3840, 2160),
    };
    DeskPlan { size, dpi, points_per_px: 1.0 / k, atlas }
}

/// The host-side options of a desk planned so (`open_desk`)
pub fn desk_options(settings: &Settings, plan: &DeskPlan) -> crate::backend::http::DeskOptions {
    crate::backend::http::DeskOptions {
        background_divisor: settings.app_window_background_divisor,
        keep_covered: !settings.app_window_skip_covered,
        atlas: plan.atlas,
        background_qp: settings.app_window_background_quality.qp_delta(),
        efficient: settings.app_window_efficient_encoding,
        quiet_idle: settings.app_window_quiet_idle,
    }
}

pub type WindowsSender = UnboundedSender<WindowsEvent>;

pub enum WindowsEvent {
    List { host_uuid: String, result: Result<(Vec<HostWindow>, HashMap<String, PathBuf>), String> },
    /// A top-level window appeared on the host while windows were being streamed
    NewWindow { host_uuid: String, window: HostWindow },
    /// The app launcher's list of a host
    Launchables { host_uuid: String, result: Result<(crate::backend::http::Launchables, HashMap<String, PathBuf>), String> },
    /// The window of an app launched from the launcher
    Launched { host_uuid: String, window: HostWindow },
    LaunchFailed { message: String },
    /// The host opened (or couldn't open) the desk for the first window
    DeskOpened { host_uuid: String, plan: DeskPlan, screen: [i32; 4], result: Result<DeskInfo, String> },
    /// A window went onto the running desk
    Adopted { hwnd: String },
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
            WindowsEvent::Launchables { host_uuid, result } => self.on_launchables(host_uuid, result),
            WindowsEvent::Launched { host_uuid, window } => {
                log::info!("launched app's window {} {:?} ({}); streaming it", window.hwnd, window.title, window.process);
                if self.windows.host_uuid == host_uuid && !self.windows.windows.iter().any(|w| w.hwnd.eq_ignore_ascii_case(&window.hwnd)) {
                    self.windows.windows.push(window.clone());
                }
                if let Some(p) = &mut self.launch_pending {
                    p.hwnd = Some(window.hwnd.to_ascii_lowercase());
                }
                if self.window_streaming(&window.hwnd) {
                    // Already here (it opened on the desk, or the watch got it first)
                    self.launch_pending = None;
                    self.focus_streamed_window(&window.hwnd);
                } else {
                    self.stream_window(&host_uuid, window, true, cx);
                }
            }
            WindowsEvent::LaunchFailed { message } => {
                self.launch_pending = None;
                self.toast(ToastKind::Error, message, cx);
            }
            WindowsEvent::DeskOpened { host_uuid, plan, screen, result } => self.on_desk_opened(host_uuid, plan, screen, result, cx),
            WindowsEvent::Adopted { hwnd } => {
                if self.launch_pending.as_ref().and_then(|p| p.hwnd.as_deref()).is_some_and(|h| h.eq_ignore_ascii_case(&hwnd)) {
                    self.focus_streamed_window(&hwnd);
                }
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

    /// Device pixels per point of the primary screen (2 on Retina).
    pub fn screen_backing_scale() -> f32 {
        crate::display::displays()
            .first()
            .filter(|d| d.points.0 > 0 && d.current.0 > 0)
            .map(|d| d.current.0 as f32 / d.points.0 as f32)
            .unwrap_or(2.0)
    }

    /// Streams a host window: opens the host's desk for this screen (first
    /// window), starts the desk helper, and adopts the window onto the desk at
    /// the place it has on its host monitor.
    pub fn launch_window(&mut self, uuid: &str, hwnd: &str, cx: &mut Context<Self>) {
        let Some(w) = self.windows.windows.iter().find(|w| w.hwnd.eq_ignore_ascii_case(hwnd)).cloned() else { return };
        self.stream_window(uuid, w, false, cx);
    }

    /// `launch_window` for a window at hand; `focus`: bring it to the front
    /// here and on the host once it shows (one the user just launched)
    pub fn stream_window(&mut self, uuid: &str, w: HostWindow, focus: bool, cx: &mut Context<Self>) {
        if self.window_streaming(&w.hwnd) {
            return;
        }
        let Some(host) = self.host_by_uuid(uuid).cloned() else { return };
        let Some(http) = host.http(self.identity.clone()) else { return };
        if let Some(opening) = &mut self.desk_opening {
            if opening.host_uuid != host.uuid {
                self.toast(ToastKind::Error, "Windows of one host at a time: stop the other host's windows first", cx);
            } else if !opening.windows.iter().any(|(o, _)| o.hwnd.eq_ignore_ascii_case(&w.hwnd)) {
                opening.windows.push((w, focus));
            }
            return;
        }
        if let Some(ds) = &mut self.desk_stream {
            if ds.host_uuid != host.uuid {
                self.toast(ToastKind::Error, "Windows of one host at a time: stop the other host's windows first", cx);
                return;
            }
            // Desk already streaming: adopt where the window is on its monitor
            let (x, y) = Self::adopt_position(&w, ds.points_per_px, ds.desk.dpi, cx);
            ds.adopted.push(w.hwnd.to_ascii_lowercase());
            if focus {
                // Waits in the helper until the window shows up there
                self.focus_streamed_window(&w.hwnd);
            }
            let tx = self.windows_tx.clone();
            std::thread::spawn(move || {
                if let Err(e) = http.window_action(&w.hwnd, "adopt", x, y, 0, 0) {
                    log::warn!("adopt {}: {e}", w.hwnd);
                }
                if focus {
                    let _ = http.window_action(&w.hwnd, "activate", 0, 0, 0, 0);
                }
                let _ = tx.unbounded_send(WindowsEvent::Adopted { hwnd: w.hwnd.clone() });
            });
            cx.notify();
            return;
        }
        if host.apps.iter().all(|a| a.id == 0) {
            self.toast(ToastKind::Error, "The host lists no apps; can't start a window stream", cx);
            return;
        }
        // The desk mirrors the screen the launcher is on, at the host window's DPI
        let Some(display) = cx.primary_display() else {
            self.toast(ToastKind::Error, "No display to mirror", cx);
            return;
        };
        let b = display.bounds();
        let screen = [f32::from(b.origin.x) as i32, f32::from(b.origin.y) as i32, f32::from(b.size.width) as i32, f32::from(b.size.height) as i32];
        let plan = desk_plan(&self.settings, (screen[2], screen[3]), Self::screen_backing_scale());
        let fps = self.settings.fps.max(30);
        let options = desk_options(&self.settings, &plan);
        self.desk_opening = Some(DeskOpening { host_uuid: host.uuid.clone(), windows: vec![(w, focus)] });
        // A round trip or more to the host: not on the UI thread
        let tx = self.windows_tx.clone();
        let host_uuid = host.uuid.clone();
        std::thread::Builder::new()
            .name("open desk".into())
            .spawn(move || {
                let result = http.open_desk(plan.size.0, plan.size.1, fps, plan.dpi, &options).map_err(|e| e.to_string());
                let _ = tx.unbounded_send(WindowsEvent::DeskOpened { host_uuid, plan, screen, result });
            })
            .ok();
        cx.notify();
    }

    fn on_desk_opened(&mut self, host_uuid: String, plan: DeskPlan, screen: [i32; 4], result: Result<DeskInfo, String>, cx: &mut Context<Self>) {
        let Some(opening) = self.desk_opening.take().filter(|o| o.host_uuid == host_uuid) else {
            // Given up on meanwhile
            if result.is_ok() {
                if let Some(http) = self.host_by_uuid(&host_uuid).and_then(|h| h.http(self.identity.clone())) {
                    std::thread::spawn(move || {
                        let _ = http.close_desk();
                    });
                }
            }
            return;
        };
        let Some(host) = self.host_by_uuid(&host_uuid).cloned() else { return };
        let Some(http) = host.http(self.identity.clone()) else { return };
        let desk = match result {
            Ok(d) => d,
            Err(e) => {
                self.launch_pending = None;
                self.toast(ToastKind::Error, format!("Couldn't open the desk on the host: {e}"), cx);
                return;
            }
        };
        let mut windows = opening.windows.into_iter();
        let Some((w, focus)) = windows.next() else { return };
        log::info!("desk {} {:?} @{} {} dpi for a {}x{} pt screen ({}x{} px wanted)", desk.display, desk.rect, desk.fps, desk.dpi, screen[2], screen[3], plan.size.0, plan.size.1);
        let fail = |this: &mut Self, message: String, cx: &mut Context<Self>| {
            let http = http.clone();
            std::thread::spawn(move || {
                let _ = http.close_desk();
            });
            this.launch_pending = None;
            this.toast(ToastKind::Error, message, cx);
        };
        let helper = self.helper_path.clone().or_else(|| stream::find_helper(&self.settings));
        let Some(helper) = helper else {
            fail(self, "Streaming helper not found; set it in Settings".into(), cx);
            return;
        };
        self.helper_path = Some(helper.clone());
        let app = host.apps.iter().find(|a| a.id == host.current_game_id && a.id != 0).or_else(|| host.apps.first()).cloned();
        let Some(app) = app else {
            fail(self, "The host lists no apps; can't start a window stream".into(), cx);
            return;
        };
        let icon_png = self.windows.icons.get(&w.icon).and_then(|p| std::fs::read(p).ok()).unwrap_or_default();
        let mut settings = self.settings.clone();
        settings.bitrate_kbps = settings.app_window_bitrate_kbps.clamp(2000, 150000);
        (settings.width, settings.height) = desk.stream_size();
        settings.fps = desk.fps;
        settings.window_mode = settings::WindowMode::Windowed;
        settings.hide_host_cursor = true;
        settings.enable_vsync = false;
        settings.frame_pacing = false;
        let cfg = AppWindowConfig { icon_png_base64: stream::base64_encode(&icon_png), dpi: desk.dpi, points_per_px: plan.points_per_px as f64, desk: true, screen };
        match stream::start_with_window(&helper, &self.identity, &host, &app, &settings, None, self.stream_tx.clone(), Some((DESK_STREAM_ID.to_string(), cfg))) {
            Ok(process) => {
                let points_per_px = plan.points_per_px;
                let (x, y) = Self::adopt_position(&w, points_per_px, desk.dpi, cx);
                let adopt_http = http.clone();
                let hwnd_s = w.hwnd.clone();
                std::thread::spawn(move || {
                    if let Err(e) = adopt_http.window_action(&hwnd_s, "adopt", x, y, 0, 0) {
                        log::warn!("adopt {hwnd_s}: {e}");
                    }
                    if focus {
                        let _ = adopt_http.window_action(&hwnd_s, "activate", 0, 0, 0, 0);
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
                    focus_on_connect: focus.then(|| w.hwnd.clone()),
                });
                self.ensure_window_watch(&host);
                // The ones asked for while the desk was opening
                for (w, focus) in windows {
                    self.stream_window(&host_uuid, w, focus, cx);
                }
            }
            Err(e) => fail(self, format!("Couldn't stream {}: {e}", w.title), cx),
        }
        cx.notify();
    }

    /// Brings a streamed window to the front here (the helper takes the
    /// activation from us: macOS lets the active app hand it over), and with
    /// it the host's focus. Waits in the helper for a window not shown yet.
    pub fn focus_streamed_window(&mut self, hwnd: &str) {
        let Some(ds) = &self.desk_stream else { return };
        if !ds.connected {
            // (the helper takes commands once its session runs)
            if let Some(ds) = &mut self.desk_stream {
                ds.focus_on_connect = Some(hwnd.to_string());
            }
            return;
        }
        if let Some(pid) = ds.process.pid() {
            crate::backend::platform::yield_activation_to(pid);
        }
        ds.process.send_command(&serde_json::json!({ "cmd": "focus", "hwnd": hwnd }));
    }

    /// Where a newly adopted window goes on the desk: its place on its host
    /// monitor, so the layout comes over as it was (the host clamps it in).
    fn adopt_position(w: &HostWindow, points_per_px: f32, desk_dpi: i32, cx: &Context<Self>) -> (i32, i32) {
        let (x, y) = (w.rect[0] - w.monitor_rect[0], w.rect[1] - w.monitor_rect[1]);
        // Inside the mirrored screen (desk px), not just inside the desk
        let Some(d) = cx.primary_display() else { return (x, y) };
        let b = d.bounds();
        let (sw, sh) = ((f32::from(b.size.width) / points_per_px) as i32, (f32::from(b.size.height) / points_per_px) as i32);
        // The window rescales to the desk's DPI when it lands there
        let k = desk_dpi as f32 / w.dpi.max(48) as f32;
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
            HelperEvent::ConnectionStarted => {
                ds.connected = true;
                if let Some(hwnd) = ds.focus_on_connect.take() {
                    self.focus_streamed_window(&hwnd);
                }
            }
            HelperEvent::StageFailed { stage, error_code, .. } => ds.error_lines.push(format!("Starting {stage} failed: Error {error_code}")),
            HelperEvent::LaunchError { text } => ds.error_lines.push(text),
            HelperEvent::AppWindow { kind, hwnd, .. } => {
                // The helper talks to the host itself; this is bookkeeping.
                if kind == "shown" {
                    // The launched app's window is here: done waiting
                    if let (Some(h), Some(p)) = (&hwnd, &self.launch_pending) {
                        if p.hwnd.as_deref().is_some_and(|w| w.eq_ignore_ascii_case(h)) {
                            self.launch_pending = None;
                        }
                    }
                } else if kind == "close" {
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
                self.launch_pending = None;
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
