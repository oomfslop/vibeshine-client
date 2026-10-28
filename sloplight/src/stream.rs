//! Launches the C++ streaming helper (`moonlight stream-helper`) and relays
//! its JSON-lines events back to the UI.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use futures::channel::mpsc::UnboundedSender;
use serde::{Deserialize, Serialize};

use crate::backend::{App, Host, Identity};
use crate::settings::Settings;

/// Config handed to the helper on stdin.
#[derive(Serialize)]
struct HelperConfig<'a> {
    identity: HelperIdentity<'a>,
    host: HelperHost<'a>,
    app: HelperApp<'a>,
    settings: &'a Settings,
    /// Screen origin of the launcher window so the stream opens on the same display
    display_origin: Option<(i32, i32)>,
    /// End the stream after this long (test runs; `SLOPLIGHT_MAX_STREAM_MS`).
    #[serde(skip_serializing_if = "Option::is_none")]
    max_duration_ms: Option<u64>,
    /// Send a test mouse move this often (test runs; `SLOPLIGHT_TEST_INPUT_MS`).
    #[serde(skip_serializing_if = "Option::is_none")]
    test_input_period_ms: Option<u64>,
    /// Fixed always-on-top window `[x, y, w, h]` (test runs; `SLOPLIGHT_TEST_WINDOW=x,y,w,h`).
    #[serde(skip_serializing_if = "Option::is_none")]
    test_window: Option<[i32; 4]>,
    /// App-window streaming: host window handle as a "0x..." string
    window_handle: Option<String>,
    /// App-window streaming: how the helper should present the window
    #[serde(skip_serializing_if = "Option::is_none")]
    app_window: Option<AppWindowConfig>,
}

/// App-window presentation passed to the helper (`app_window` in the config).
#[derive(Serialize, Clone, Debug)]
pub struct AppWindowConfig {
    pub title: String,
    pub icon_png_base64: String,
    /// Host window frame rect x, y, w, h in host pixels
    pub rect: [i32; 4],
    pub caption: i32,
    pub dpi: i32,
    /// Host hit-test map: where the caption (drag) and the caption buttons are
    pub hit: Vec<crate::backend::http::HitRegion>,
    /// Canvas position of the window's top-left (host layout rule)
    pub anchor: [i32; 2],
    /// Corner radius (host px) the OS gives the window and its popups
    pub radius: i32,
}

/// Encoder frame for an app window. The helper copies the window into the
/// top-left of this fixed canvas and the client crops, so the canvas is the
/// largest size the window can take without a relaunch: the smaller of the
/// host monitor and the Mac screen (in host pixels), rounded to 64, and never
/// smaller than the window itself.
/// Encoder frame height for a window band of `ch`: the colour canvas is the
/// window band plus a popup band (a third of the canvas, where menus and
/// dialogs get slots of their own); below it sits the canvas' alpha at half
/// resolution. Frame = 3/2 * canvas. Must match the host's `frame_bands`.
pub fn app_window_frame_height(ch: i32) -> i32 {
    let round64 = |v: i32| ((v.max(64) + 63) / 64) * 64;
    // canvas = window band + popup band with popup band = (canvas / 3) & !63
    let mut canvas = round64(ch + ch / 2);
    while canvas - ((canvas / 3) & !63) < ch {
        canvas += 64;
    }
    canvas * 3 / 2
}

pub fn app_window_canvas(w: i32, h: i32, max: Option<(i32, i32)>) -> (i32, i32) {
    let round64 = |v: i32| ((v.max(64) + 63) / 64) * 64;
    let (mut cw, mut ch) = match max {
        Some((mw, mh)) if mw > 0 && mh > 0 => (round64(mw), round64(mh)),
        _ => (round64(w + 512), round64(h + 512)),
    };
    cw = cw.max((w + 1) & !1).min(4096);
    ch = ch.max((h + 1) & !1).min(2048);  // the alpha band below doubles the frame height
    (cw, ch)
}

#[derive(Serialize)]
struct HelperIdentity<'a> {
    unique_id: &'a str,
    certificate_pem: &'a str,
    private_key_pem: &'a str,
}

#[derive(Serialize)]
struct HelperHost<'a> {
    name: &'a str,
    uuid: &'a str,
    address: String,
    port: u16,
    https_port: u16,
    server_cert_pem: &'a str,
    app_version: &'a str,
    gfe_version: &'a str,
    gpu_model: &'a str,
    is_nvidia_server_software: bool,
    server_codec_mode_support: i32,
    max_luma_pixels_hevc: i32,
    current_game_id: i32,
    has_apollo_extensions: bool,
    apollo_permissions: u32,
    server_commands: &'a [String],
    mac: String,
}

#[derive(Serialize)]
struct HelperApp<'a> {
    id: i32,
    name: &'a str,
    uuid: &'a str,
    hdr_supported: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum HelperEvent {
    /// Launch warnings collected during initialization
    Warnings { warnings: Vec<String> },
    StageStarting { stage: String },
    StageFailed { stage: String, error_code: i32, failing_ports: String },
    ConnectionStarted,
    LaunchError { text: String },
    QuitStarting,
    SessionFinished { port_test_result: i32 },
    /// Synthesized by the Rust side when the process exits
    Exited { code: Option<i32>, stderr_tail: String },
    /// A synthetic test mouse move was sent at this wall-clock time (ms).
    TestInput { t_ms: f64 },
    /// App-window mode: the Mac window changed (kind = resize|focus|blur|minimize|restore|close;
    /// w/h in host pixels for resize)
    AppWindow {
        kind: String,
        #[serde(default)]
        w: i32,
        #[serde(default)]
        h: i32,
        /// Set when the event is about one of the window's child dialogs
        #[serde(default)]
        hwnd: Option<String>,
    },
}

/// Command-only handle to a helper process (see `StreamProcess::clone_commander`).
#[derive(Clone)]
pub struct StreamCommander {
    stdin: Option<Arc<Mutex<std::process::ChildStdin>>>,
}

impl StreamCommander {
    pub fn send_command(&self, cmd: &serde_json::Value) -> bool {
        let Some(stdin) = &self.stdin else { return false };
        let mut line = cmd.to_string();
        line.push('\n');
        let mut stdin = stdin.lock().unwrap();
        stdin.write_all(line.as_bytes()).and_then(|_| stdin.flush()).is_ok()
    }
}

#[derive(Debug, Clone)]
pub struct StreamEvent {
    pub host_uuid: String,
    pub app_id: i32,
    /// App-window sessions: the host window handle ("0x..."), else None
    pub window: Option<String>,
    pub event: HelperEvent,
}

#[cfg(unix)]
unsafe extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, sig: i32) -> i32;
}

pub struct StreamProcess {
    child: Arc<Mutex<Option<Child>>>,
    pub host_uuid: String,
    pub app_id: i32,
    /// App-window mode: the helper's stdin, kept open for JSON command lines
    stdin: Option<Arc<Mutex<std::process::ChildStdin>>>,
}

impl StreamProcess {
    /// A handle that can only send commands (for a poller thread).
    pub fn clone_commander(&self) -> StreamCommander {
        StreamCommander { stdin: self.stdin.clone() }
    }

    /// App-window mode: send a command line to the helper
    /// (`{"cmd":"geometry",...}`, `{"cmd":"activate"}`, `{"cmd":"close"}`).
    pub fn send_command(&self, cmd: &serde_json::Value) -> bool {
        let Some(stdin) = &self.stdin else { return false };
        let mut line = cmd.to_string();
        line.push('\n');
        let mut stdin = stdin.lock().unwrap();
        stdin.write_all(line.as_bytes()).and_then(|_| stdin.flush()).is_ok()
    }

    /// Asks the helper to end the stream cleanly (it handles SIGTERM by
    /// closing the session like the window's close button would).
    pub fn terminate(&self) {
        if let Some(child) = self.child.lock().unwrap().as_mut() {
            #[cfg(unix)]
            {
                let pid = child.id() as i32;
                // SAFETY: plain libc call with our own child's pid.
                if unsafe { libc_kill(pid, 15) } == 0 {
                    return;
                }
            }
            let _ = child.kill();
        }
    }
}

/// Finds the moonlight binary to use as the streaming helper.
pub fn find_helper(settings: &Settings) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if !settings.helper_path.trim().is_empty() {
        candidates.push(PathBuf::from(settings.helper_path.trim()));
    }
    if let Ok(env) = std::env::var("SLOPLIGHT_HELPER") {
        candidates.push(PathBuf::from(env));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent() {
            candidates.push(dir.join("moonlight"));
            candidates.push(dir.join("Moonlight"));
            // App bundle: Contents/MacOS/sloplight -> Contents/Helpers/Moonlight.app
            candidates.push(dir.join("../Helpers/Moonlight.app/Contents/MacOS/Moonlight"));
            candidates.push(dir.join("../Resources/Moonlight.app/Contents/MacOS/Moonlight"));
            // Dev tree: sloplight/target/{debug,release}/sloplight -> moonlight-qt/result
            candidates.push(dir.join("../../../result/bin/moonlight"));
            candidates.push(dir.join("../../../result/Applications/Moonlight.app/Contents/MacOS/Moonlight"));
            candidates.push(dir.join("../../../app/Moonlight.app/Contents/MacOS/Moonlight"));
            candidates.push(dir.join("../../../build/app/Moonlight.app/Contents/MacOS/Moonlight"));
        }
    candidates.push(PathBuf::from("/Applications/Moonlight.app/Contents/MacOS/Moonlight"));
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join("Applications/Moonlight.app/Contents/MacOS/Moonlight"));
    }
    for c in candidates {
        if c.is_file() {
            return Some(c);
        }
    }
    // PATH lookup
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(':') {
            let p = Path::new(dir).join("moonlight");
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// Spawns the helper for `app` on `host`. Events arrive on `events`.
pub fn start(
    helper: &Path,
    identity: &Identity,
    host: &Host,
    app: &App,
    settings: &Settings,
    display_origin: Option<(i32, i32)>,
    events: UnboundedSender<StreamEvent>,
) -> Result<StreamProcess, String> {
    start_with_window(helper, identity, host, app, settings, display_origin, events, None)
}

/// Like `start`, for an app-window session: `window` carries the host window
/// handle and presentation; the helper's stdin stays open for commands.
pub fn start_with_window(
    helper: &Path,
    identity: &Identity,
    host: &Host,
    app: &App,
    settings: &Settings,
    display_origin: Option<(i32, i32)>,
    events: UnboundedSender<StreamEvent>,
    window: Option<(String, AppWindowConfig)>,
) -> Result<StreamProcess, String> {
    let address = host.active_address.clone().or_else(|| host.unique_addresses().into_iter().next()).ok_or("The host has no known address.")?;
    let cert = host.server_cert_pem.clone().ok_or("The host isn't paired.")?;
    let config = HelperConfig {
        identity: HelperIdentity { unique_id: &identity.unique_id, certificate_pem: &identity.cert_pem, private_key_pem: &identity.key_pem },
        host: HelperHost {
            name: &host.name,
            uuid: &host.uuid,
            address: address.host.clone(),
            port: address.port,
            https_port: host.active_https_port,
            server_cert_pem: &cert,
            app_version: &host.app_version,
            gfe_version: &host.gfe_version,
            gpu_model: &host.gpu_model,
            is_nvidia_server_software: host.is_nvidia_server_software,
            server_codec_mode_support: host.server_codec_mode_support,
            max_luma_pixels_hevc: host.max_luma_pixels_hevc,
            current_game_id: host.current_game_id,
            has_apollo_extensions: host.has_apollo_extensions,
            apollo_permissions: host.apollo_permissions,
            server_commands: &host.server_commands,
            mac: host.mac_string(),
        },
        app: HelperApp { id: app.id, name: &app.name, uuid: &app.uuid, hdr_supported: app.hdr_supported },
        settings,
        display_origin,
        max_duration_ms: std::env::var("SLOPLIGHT_MAX_STREAM_MS").ok().and_then(|v| v.parse().ok()),
        test_input_period_ms: std::env::var("SLOPLIGHT_TEST_INPUT_MS").ok().and_then(|v| v.parse().ok()),
        window_handle: window.as_ref().map(|(h, _)| h.clone()).or_else(|| std::env::var("SLOPLIGHT_WINDOW_HANDLE").ok().filter(|v| !v.is_empty())),
        app_window: window.as_ref().map(|(_, c)| c.clone()),
        test_window: std::env::var("SLOPLIGHT_TEST_WINDOW").ok().and_then(|v| {
            let n: Vec<i32> = v.split(',').filter_map(|p| p.trim().parse().ok()).collect();
            (n.len() == 4).then(|| [n[0], n[1], n[2], n[3]])
        }),
    };
    let json = serde_json::to_vec(&config).map_err(|e| e.to_string())?;

    let mut cmd = Command::new(helper);
    cmd.arg("stream-helper");
    if cfg!(target_os = "macos") && settings.exclusive_fullscreen && settings.window_mode == crate::settings::WindowMode::Fullscreen {
        // Opt into real modesetting fullscreen; see Session::initialize in session.cpp.
        cmd.env("I_WANT_BUGGY_FULLSCREEN", "1");
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Couldn't start the streaming helper at {}: {e}", helper.display()))?;
    let mut stdin = child.stdin.take().ok_or("helper stdin")?;
    stdin.write_all(&json).map_err(|e| e.to_string())?;
    stdin.write_all(b"\n").map_err(|e| e.to_string())?;
    stdin.flush().map_err(|e| e.to_string())?;
    // Normal streams: closing stdin signals the end of the config. App windows
    // keep it open for commands.
    let kept_stdin = if window.is_some() { Some(Arc::new(Mutex::new(stdin))) } else { drop(stdin); None };
    let stdout = child.stdout.take().ok_or("helper stdout")?;
    let stderr = child.stderr.take().ok_or("helper stderr")?;
    let child = Arc::new(Mutex::new(Some(child)));

    let stderr_tail: Arc<Mutex<std::collections::VecDeque<String>>> = Arc::new(Mutex::new(Default::default()));
    {
        let tail = stderr_tail.clone();
        thread::Builder::new()
            .name("helper stderr".into())
            .spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    log::debug!("[helper] {line}");
                    let mut t = tail.lock().unwrap();
                    if t.len() >= 40 {
                        t.pop_front();
                    }
                    t.push_back(line);
                }
            })
            .ok();
    }
    {
        let events = events.clone();
        let host_uuid = host.uuid.clone();
        let app_id = app.id;
        let window_id = window.as_ref().map(|(h, _)| h.clone());
        let child = child.clone();
        thread::Builder::new()
            .name("helper stdout".into())
            .spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<HelperEvent>(line) {
                        Ok(event) => {
                            let _ = events.unbounded_send(StreamEvent { host_uuid: host_uuid.clone(), app_id, window: window_id.clone(), event });
                        }
                        Err(_) => log::debug!("[helper] {line}"),
                    }
                }
                let code = loop {
                    let status = child.lock().unwrap().as_mut().and_then(|c| c.try_wait().ok().flatten());
                    match status {
                        Some(s) => break s.code(),
                        None => {
                            if child.lock().unwrap().is_none() {
                                break None;
                            }
                            thread::sleep(std::time::Duration::from_millis(100));
                        }
                    }
                };
                let tail = stderr_tail.lock().unwrap().iter().cloned().collect::<Vec<_>>().join("\n");
                let _ = events.unbounded_send(StreamEvent { host_uuid, app_id, window: window_id, event: HelperEvent::Exited { code, stderr_tail: tail } });
            })
            .ok();
    }
    Ok(StreamProcess { child, host_uuid: host.uuid.clone(), app_id: app.id, stdin: kept_stdin })
}

/// Standard base64 (no dependency needed for the few icons we ship to the helper).
pub fn base64_encode(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}
