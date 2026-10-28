//! App-window streaming on the launcher side: the host's window list for the
//! selected host, one stream (helper process) per streamed window, a geometry
//! poller that keeps each Mac window in sync with its host window, and the
//! forwarding of Mac-side window events (resize, focus, close…) as host
//! window actions. See DESIGN-APPWINDOWS.md.

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
use crate::backend::http::{HostWindow, NvHttp, WindowLayout, HttpError};
use crate::settings;
use crate::stream::{self, AppWindowConfig, HelperEvent, StreamCommander, StreamEvent, StreamProcess};

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

/// One streamed host window.
pub struct WindowStream {
    pub host_uuid: String,
    pub hwnd: String,
    pub title: String,
    pub process_name: String,
    pub process: StreamProcess,
    pub connected: bool,
    pub error_lines: Vec<String>,
    poller_stop: Arc<AtomicBool>,
    /// Encoder frame size this session was launched with (host pixels)
    pub canvas: (i32, i32),
    /// Mac-side resizes go through one worker thread that always sends the latest size
    resize_tx: std::sync::mpsc::Sender<(i32, i32)>,
}

impl Drop for WindowStream {
    fn drop(&mut self) {
        self.poller_stop.store(true, Ordering::Relaxed);
    }
}

pub enum WindowsEvent {
    List { host_uuid: String, result: Result<(Vec<HostWindow>, HashMap<String, PathBuf>), String> },
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
        }
        cx.notify();
    }

    pub fn window_stream(&self, hwnd: &str) -> Option<&WindowStream> {
        self.window_streams.iter().find(|w| w.hwnd.eq_ignore_ascii_case(hwnd))
    }

    /// Streams one host window as its own Mac window.
    pub fn launch_window(&mut self, uuid: &str, hwnd: &str, cx: &mut Context<Self>) {
        if self.window_stream(hwnd).is_some() {
            return;
        }
        let Some(host) = self.host_by_uuid(uuid).cloned() else { return };
        let Some(w) = self.windows.windows.iter().find(|w| w.hwnd.eq_ignore_ascii_case(hwnd)).cloned() else { return };
        let helper = self.helper_path.clone().or_else(|| stream::find_helper(&self.settings));
        let Some(helper) = helper else {
            self.toast(ToastKind::Error, "Streaming helper not found; set it in Settings", cx);
            return;
        };
        self.helper_path = Some(helper.clone());
        // The session streams the "Desktop"-style entry: any app id works for a
        // window session, but the helper needs an app to name the session.
        let app = host.apps.iter().find(|a| a.id == host.current_game_id && a.id != 0).or_else(|| host.apps.first()).cloned();
        let Some(app) = app else {
            self.toast(ToastKind::Error, "The host lists no apps; can't start a window stream", cx);
            return;
        };
        let icon_png = self.windows.icons.get(&w.icon).and_then(|p| std::fs::read(p).ok()).unwrap_or_default();
        let hit = host.http(self.identity.clone()).and_then(|h| h.get_window_hittest(&w.hwnd).ok()).unwrap_or_default();
        let mut settings = self.settings.clone();
        // Fixed canvas: the smaller of the host monitor and the Mac screen (in
        // host pixels), so maximize and free resizing never need a relaunch.
        let mac_px = cx.primary_display().map(|d| {
            let b = d.bounds();
            let k = w.dpi.max(48) as f32 / 96.0;
            ((f32::from(b.size.width) * k) as i32, (f32::from(b.size.height) * k) as i32)
        });
        let host_px = (w.monitor_rect[2] > 0 && w.monitor_rect[3] > 0).then_some((w.monitor_rect[2], w.monitor_rect[3]));
        let max = match (mac_px, host_px) {
            (Some(a), Some(b)) => Some((a.0.min(b.0), a.1.min(b.1))),
            (a, b) => a.or(b),
        };
        let (cw, ch) = stream::app_window_canvas(w.rect[2], w.rect[3], max);
        // The encoder frame is two bands: the picture and, below it, its alpha.
        settings.width = cw;
        settings.height = ch * 2;
        let layout = host.http(self.identity.clone()).and_then(|h| h.get_window_layout(&w.hwnd, (cw, ch), "0", 0).ok());
        let cfg = AppWindowConfig {
            title: w.title.clone(),
            icon_png_base64: stream::base64_encode(&icon_png),
            rect: layout.as_ref().map(|l| l.rect).unwrap_or(w.rect),
            caption: w.caption,
            dpi: w.dpi,
            hit,
            anchor: layout.as_ref().map(|l| l.anchor).unwrap_or([0, 0]),
            radius: layout.as_ref().map(|l| l.radius).unwrap_or(0),
        };
        settings.window_mode = settings::WindowMode::Windowed;
        settings.hide_host_cursor = true;  // app windows always draw the Mac cursor
        settings.enable_vsync = false;
        settings.frame_pacing = false;
        match stream::start_with_window(&helper, &self.identity, &host, &app, &settings, None, self.stream_tx.clone(), Some((w.hwnd.clone(), cfg))) {
            Ok(process) => {
                let poller_stop = Arc::new(AtomicBool::new(false));
                if let Some(http) = host.http(self.identity.clone()) {
                    spawn_geometry_poller(http, w.hwnd.clone(), (cw, ch), process.clone_commander(), poller_stop.clone());
                }
                self.window_streams.push(WindowStream {
                    host_uuid: host.uuid.clone(),
                    hwnd: w.hwnd.clone(),
                    title: w.title.clone(),
                    process_name: w.process.clone(),
                    process,
                    connected: false,
                    error_lines: Vec::new(),
                    poller_stop,
                    canvas: (cw, ch),
                    resize_tx: spawn_resize_worker(host.http(self.identity.clone()), w.hwnd.clone(), self.settings.app_window_live_resize),
                });
            }
            Err(e) => self.toast(ToastKind::Error, format!("Couldn't stream {}: {e}", w.title), cx),
        }
        cx.notify();
    }

    pub fn close_window_stream(&mut self, hwnd: &str, cx: &mut Context<Self>) {
        if let Some(w) = self.window_stream(hwnd) {
            w.process.send_command(&serde_json::json!({"cmd": "close"}));
            w.process.terminate();
        }
        cx.notify();
    }

    /// Events from a window-session helper (routed here by `on_stream_event`).
    pub fn on_window_stream_event(&mut self, hwnd: &str, event: StreamEvent, cx: &mut Context<Self>) {
        let Some(idx) = self.window_streams.iter().position(|w| w.hwnd.eq_ignore_ascii_case(hwnd)) else { return };
        let host = self.host_by_uuid(&event.host_uuid).cloned();
        let ws = &mut self.window_streams[idx];
        match event.event {
            HelperEvent::ConnectionStarted => ws.connected = true,
            HelperEvent::StageFailed { stage, error_code, .. } => ws.error_lines.push(format!("Starting {stage} failed: Error {error_code}")),
            HelperEvent::LaunchError { text } => ws.error_lines.push(text),
            HelperEvent::AppWindow { kind, w, h } => {
                // Mac-side window change -> host window action, off the UI thread.
                // Minimize/restore stay on the Mac: the host window keeps running
                // (and its capture alive) while the Mac window is in the Dock.
                let Some(http) = host.and_then(|h| h.http(self.identity.clone())) else { return };
                let hwnd = ws.hwnd.clone();
                match kind.as_str() {
                    "resize" => {
                        if w > ws.canvas.0 || h > ws.canvas.1 {
                            log::info!("app window {hwnd} grew past its {}x{} canvas", ws.canvas.0, ws.canvas.1);
                        }
                        let _ = ws.resize_tx.send((w, h));
                    }
                    "focus" | "close" => {
                        let op = if kind == "focus" { "activate" } else { "close" };
                        std::thread::spawn(move || {
                            if let Err(e) = http.window_action(&hwnd, op, 0, 0, 0, 0) {
                                log::warn!("window {op} on {hwnd}: {e}");
                            }
                        });
                    }
                    _ => {}
                }
            }
            HelperEvent::Exited { code, .. } => {
                let ws = self.window_streams.remove(idx);
                if !ws.connected && code != Some(0) {
                    let why = if ws.error_lines.is_empty() { format!("helper exited with code {code:?}") } else { ws.error_lines.join(" ") };
                    self.toast(ToastKind::Error, format!("Couldn't stream {}: {why}", ws.title), cx);
                }
            }
            _ => {}
        }
        cx.notify();
    }
}

/// Follows one streamed window on the host (rect, canvas anchor, popups,
/// title, state) with long-polls and forwards every change to its helper;
/// closes the helper when the window disappears.
pub fn spawn_geometry_poller(http: NvHttp, hwnd: String, canvas: (i32, i32), commander: StreamCommander, stop: Arc<AtomicBool>) {
    std::thread::Builder::new()
        .name(format!("window poller {hwnd}"))
        .spawn(move || {
            let mut etag = String::from("0");
            let mut last: Option<WindowLayout> = None;
            let mut misses = 0;
            while !stop.load(Ordering::Relaxed) {
                match http.get_window_layout(&hwnd, canvas, &etag, 1000) {
                    Ok(l) => {
                        misses = 0;
                        if l.etag == etag && last.is_some() {
                            continue;
                        }
                        etag = l.etag.clone();
                        let size_changed = last.as_ref().map(|p| p.rect[2] != l.rect[2] || p.rect[3] != l.rect[3]).unwrap_or(false);
                        let mut cmd = serde_json::json!({
                            "cmd": "geometry", "rect": l.rect, "anchor": l.anchor, "popups": l.popups, "radius": l.radius,
                            "caption": l.caption, "title": l.title, "state": l.state
                        });
                        if size_changed && l.state != "minimized" {
                            // The caption buttons move with the width: refresh the hit map.
                            if let Ok(hit) = http.get_window_hittest(&hwnd) {
                                cmd["hit"] = serde_json::to_value(hit).unwrap_or_default();
                            }
                        }
                        commander.send_command(&cmd);
                        last = Some(l);
                    }
                    Err(e) => {
                        let gone = matches!(&e, HttpError::Network(m) if m.contains("404"));
                        misses += 1;
                        if gone || misses >= 3 {
                            log::info!("window poller {hwnd}: window gone ({e}); closing");
                            commander.send_command(&serde_json::json!({"cmd": "close"}));
                            break;
                        }
                        log::debug!("window poller {hwnd}: {e}");
                        std::thread::sleep(Duration::from_millis(300));
                    }
                }
            }
        })
        .ok();
}

pub type WindowsSender = UnboundedSender<WindowsEvent>;

/// One thread per window stream sends host resizes in order, always the
/// latest size: immediately when live, or after a 120 ms pause otherwise.
fn spawn_resize_worker(http: Option<NvHttp>, hwnd: String, live: bool) -> std::sync::mpsc::Sender<(i32, i32)> {
    let (tx, rx) = std::sync::mpsc::channel::<(i32, i32)>();
    std::thread::Builder::new()
        .name(format!("window resize {hwnd}"))
        .spawn(move || {
            let Some(http) = http else { return };
            while let Ok(mut size) = rx.recv() {
                if !live {
                    std::thread::sleep(Duration::from_millis(120));
                }
                // Collapse everything queued meanwhile into the newest size.
                while let Ok(newer) = rx.try_recv() {
                    size = newer;
                }
                if let Err(e) = http.window_action(&hwnd, "resize", 0, 0, size.0, size.1) {
                    log::warn!("window resize on {hwnd}: {e}");
                }
            }
        })
        .ok();
    tx
}
