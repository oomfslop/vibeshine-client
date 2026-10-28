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
use crate::backend::http::{HostWindow, NvHttp};
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
        let cfg = AppWindowConfig {
            title: w.title.clone(),
            icon_png_base64: stream::base64_encode(&icon_png),
            rect: w.rect,
            caption: w.caption,
            dpi: w.dpi,
        };
        let mut settings = self.settings.clone();
        settings.width = ((w.rect[2].max(64)) + 1) & !1;
        settings.height = ((w.rect[3].max(48)) + 1) & !1;
        settings.window_mode = settings::WindowMode::Windowed;
        settings.enable_vsync = false;
        settings.frame_pacing = false;
        match stream::start_with_window(&helper, &self.identity, &host, &app, &settings, None, self.stream_tx.clone(), Some((w.hwnd.clone(), cfg))) {
            Ok(process) => {
                let poller_stop = Arc::new(AtomicBool::new(false));
                if let Some(http) = host.http(self.identity.clone()) {
                    spawn_geometry_poller(http, w.hwnd.clone(), process.clone_commander(), poller_stop.clone());
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
                let op = match kind.as_str() {
                    "resize" => Some(("resize", w, h)),
                    "focus" => Some(("activate", 0, 0)),
                    "minimize" => Some(("minimize", 0, 0)),
                    "restore" => Some(("restore", 0, 0)),
                    "close" => Some(("close", 0, 0)),
                    _ => None,
                };
                if let (Some((op, w, h)), Some(http)) = (op, host.and_then(|h| h.http(self.identity.clone()))) {
                    let hwnd = ws.hwnd.clone();
                    std::thread::spawn(move || {
                        if let Err(e) = http.window_action(&hwnd, op, 0, 0, w, h) {
                            log::warn!("window {op} on {hwnd}: {e}");
                        }
                    });
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

/// Polls the host's window list and forwards geometry/title/state changes of
/// one window to its helper; closes the helper when the window disappears.
fn spawn_geometry_poller(http: NvHttp, hwnd: String, commander: StreamCommander, stop: Arc<AtomicBool>) {
    std::thread::Builder::new()
        .name(format!("window poller {hwnd}"))
        .spawn(move || {
            let mut last: Option<HostWindow> = None;
            let mut misses = 0;
            while !stop.load(Ordering::Relaxed) {
                match http.get_host_windows() {
                    Ok(list) => match list.into_iter().find(|w| w.hwnd.eq_ignore_ascii_case(&hwnd)) {
                        Some(w) => {
                            misses = 0;
                            if last.as_ref() != Some(&w) {
                                commander.send_command(&serde_json::json!({
                                    "cmd": "geometry", "rect": w.rect, "caption": w.caption, "title": w.title, "state": w.state
                                }));
                                last = Some(w);
                            }
                        }
                        None => {
                            misses += 1;
                            if misses >= 2 {
                                commander.send_command(&serde_json::json!({"cmd": "close"}));
                                break;
                            }
                        }
                    },
                    Err(e) => log::debug!("window poller {hwnd}: {e}"),
                }
                std::thread::sleep(Duration::from_millis(300));
            }
        })
        .ok();
}

pub type WindowsSender = UnboundedSender<WindowsEvent>;
