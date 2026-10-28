mod app;
mod appwindows;
mod assets;
mod backend;
mod display;
mod settings;
mod stream;
mod theme;
mod ui;
mod views;

use std::path::PathBuf;

use gpui::{App, Application, Bounds, KeyBinding, Menu, MenuItem, TitlebarOptions, WindowBounds, WindowOptions, point, prelude::*, px, size};

use app::{AddHost, Back, CloseWindow, FocusFilter, OpenSettings, QuickLaunch, Quit, Refresh, Root, ToggleHiddenApps, ToggleInspector, CopyHostAddress};

fn config_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("SLOPLIGHT_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    migrated_dir(dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")))
}

fn cache_dir() -> PathBuf {
    migrated_dir(dirs::cache_dir().unwrap_or_else(|| PathBuf::from(".")))
}

/// `<base>/sloplight`, moving a pre-rename `<base>/viberemote` directory into
/// place the first time so hosts, pairing identity and box art survive.
fn migrated_dir(base: PathBuf) -> PathBuf {
    let new = base.join("sloplight");
    let old = base.join("viberemote");
    if !new.exists() && old.is_dir() {
        if let Err(e) = std::fs::rename(&old, &new) {
            eprintln!("Could not migrate {} to {}: {e}", old.display(), new.display());
        }
    }
    new
}

/// `sloplight --probe <address>`: fetch serverinfo from a host and print the result.
fn probe(address: &str) -> i32 {
    let Some(addr) = backend::host::Address::parse_user_input(address) else {
        eprintln!("Invalid address: {address}");
        return 2;
    };
    let dir = std::env::temp_dir().join("sloplight-probe");
    let identity = match backend::Identity::load_or_create(&dir) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("identity: {e}");
            return 2;
        }
    };
    let mut http = backend::http::NvHttp::new(addr.clone(), 0, None, false, identity);
    let started = std::time::Instant::now();
    match http.get_server_info(false) {
        Ok(info) => {
            let host = backend::Host::from_server_info(&http, &info);
            println!("OK in {:?}: {} ({}) uuid={} https_port={} paired={:?} apollo={}", started.elapsed(), host.name, host.software().label(), host.uuid, host.active_https_port, host.pair_state, host.has_apollo_extensions);
            0
        }
        Err(e) => {
            eprintln!("FAILED after {:?}: {e} ({e:?})", started.elapsed());
            if let Some(r) = e.resolution() {
                eprintln!("Hint: {r}");
            }
            1
        }
    }
}

/// Where the app writes its log (also mirrored to stderr).
pub fn log_path() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = dirs::home_dir() {
            return home.join("Library/Logs/sloplight.log");
        }
    }
    cache_dir().join("sloplight.log")
}

/// Writes to stderr and, when it could be opened, the log file.
struct Tee {
    file: Option<std::fs::File>,
}

impl std::io::Write for Tee {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let _ = std::io::stderr().write_all(buf);
        if let Some(f) = &mut self.file {
            let _ = f.write_all(buf);
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        if let Some(f) = &mut self.file {
            let _ = f.flush();
        }
        std::io::stderr().flush()
    }
}

fn init_logging() {
    let path = log_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // Keep the file small: start over once it passes 2 MB.
    let truncate = std::fs::metadata(&path).map(|m| m.len() > 2 * 1024 * 1024).unwrap_or(false);
    let file = std::fs::OpenOptions::new().create(true).append(!truncate).write(true).truncate(truncate).open(&path).ok();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,sloplight=debug"))
        .target(env_logger::Target::Pipe(Box::new(Tee { file })))
        .init();
    log::info!("sloplight {} starting; log file: {}", env!("CARGO_PKG_VERSION"), path.display());
}

/// `sloplight --stream <host> <app> [seconds]`: stream headlessly from the
/// command line (windowed) for end-to-end tests, printing helper events.
fn stream_cli(host_q: &str, app_q: &str, seconds: u64) -> i32 {
    use futures::StreamExt;
    let config_dir = config_dir();
    let settings = {
        let mut s = settings::Settings::load(&config_dir);
        // Windowed by default; SLOPLIGHT_STREAM_WINDOW_MODE=fullscreen|borderless
        // keeps the configured presentation for full-screen latency tests.
        s.window_mode = match std::env::var("SLOPLIGHT_STREAM_WINDOW_MODE").as_deref() {
            Ok("fullscreen") => settings::WindowMode::Fullscreen,
            Ok("borderless") => settings::WindowMode::Borderless,
            _ => settings::WindowMode::Windowed,
        };
        s
    };
    let identity = match backend::Identity::load_or_create(&config_dir) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("identity: {e}");
            return 2;
        }
    };
    let hosts: Vec<backend::Host> = match std::fs::read_to_string(config_dir.join("hosts.json")) {
        Ok(t) => match serde_json::from_str(&t) {
            Ok(h) => h,
            Err(e) => {
                log::error!("hosts.json: {e}");
                Vec::new()
            }
        },
        Err(e) => {
            log::error!("hosts.json: {e}");
            Vec::new()
        }
    };
    log::info!("[stream-cli] {} hosts loaded: {:?}", hosts.len(), hosts.iter().map(|h| format!("{} ({} apps)", h.name, h.apps.len())).collect::<Vec<_>>());
    let q = host_q.to_lowercase();
    let Some(host) = hosts.iter().find(|h| h.name.to_lowercase().contains(&q) || h.uuid.to_lowercase().starts_with(&q) || h.unique_addresses().iter().any(|a| a.host == host_q)) else {
        eprintln!("No host matching {host_q:?}; known: {:?}", hosts.iter().map(|h| &h.name).collect::<Vec<_>>());
        return 2;
    };
    // Hosts loaded from disk carry no live state; one serverinfo poll fills in
    // the HTTPS port, active address and pairing state.
    let mut host = host.clone();
    let mut reached = false;
    for addr in host.unique_addresses() {
        let mut http = backend::http::NvHttp::new(addr.clone(), 0, host.server_cert(), !host.is_nvidia_server_software, identity.clone());
        match http.get_server_info(true) {
            Ok(info) => {
                let fresh = backend::Host::from_server_info(&http, &info);
                host.update(&fresh);
                reached = true;
                break;
            }
            Err(e) => log::error!("serverinfo via {addr}: {e}"),
        }
    }
    if !reached {
        return 1;
    }
    log::info!("[stream-cli] after serverinfo: {} apps, https {}, paired {:?}", host.apps.len(), host.active_https_port, host.pair_state);
    let host = &host;
    let aq = app_q.to_lowercase();
    let Some(app) = host.apps.iter().find(|a| a.name.to_lowercase().contains(&aq)) else {
        eprintln!("No app matching {app_q:?} on {}; known: {:?}", host.name, host.apps.iter().map(|a| &a.name).collect::<Vec<_>>());
        return 2;
    };
    let Some(helper) = stream::find_helper(&settings) else {
        eprintln!("Streaming helper not found");
        return 2;
    };
    if seconds > 0 {
        // SAFETY: set before the helper is spawned, single-threaded at this point.
        unsafe { std::env::set_var("SLOPLIGHT_MAX_STREAM_MS", (seconds * 1000).to_string()) };
    }
    // App-window test mode: SLOPLIGHT_WINDOW_HANDLE=0x... streams that host
    // window as a native-looking window; the launcher polls the host window
    // list for geometry and forwards the helper's window events as actions.
    let window_hwnd = std::env::var("SLOPLIGHT_WINDOW_HANDLE").ok().filter(|v| !v.is_empty());
    let http = {
        let addr = host.active_address.clone().or_else(|| host.unique_addresses().into_iter().next()).expect("host address");
        backend::http::NvHttp::new(addr, host.active_https_port, host.server_cert(), !host.is_nvidia_server_software, identity.clone())
    };
    let mut settings = settings;
    let window = if let Some(hwnd) = &window_hwnd {
        let windows = match http.get_host_windows() {
            Ok(w) => w,
            Err(e) => {
                eprintln!("windows list: {e}");
                return 1;
            }
        };
        let Some(w) = windows.iter().find(|w| w.hwnd.eq_ignore_ascii_case(hwnd)) else {
            eprintln!("Window {hwnd} not found on the host; known: {:?}", windows.iter().map(|w| format!("{} {}", w.hwnd, w.title)).collect::<Vec<_>>());
            return 2;
        };
        let icon = match http.get_window_icon(&w.hwnd) {
            Ok(i) => i,
            Err(e) => {
                log::warn!("[stream-cli] window icon: {e}");
                Vec::new()
            }
        };
        let hit = http.get_window_hittest(&w.hwnd).unwrap_or_default();
        log::info!("[stream-cli] hit regions: {:?}", hit);
        // Fixed canvas (the helper crops to the window): host monitor capped at 2560x1600
        let host_px = (w.monitor_rect[2] > 0).then_some((w.monitor_rect[2].min(2560), w.monitor_rect[3].min(1600)));
        let (cw, ch) = stream::app_window_canvas(w.rect[2], w.rect[3], host_px.or(Some((2560, 1600))));
        settings.width = cw;
        settings.height = stream::app_window_frame_height(ch);
        let layout = http.get_window_layout(&w.hwnd, (cw, settings.height * 2 / 3), "0", 0).ok();
        log::info!("[stream-cli] layout: {:?}", layout);
        let cfg = stream::AppWindowConfig {
            title: w.title.clone(),
            icon_png_base64: stream::base64_encode(&icon),
            rect: layout.as_ref().map(|l| l.rect).unwrap_or(w.rect),
            caption: w.caption,
            dpi: w.dpi,
            hit,
            anchor: layout.as_ref().map(|l| l.anchor).unwrap_or([0, 0]),
            radius: layout.as_ref().map(|l| l.radius).unwrap_or(0),
        };
        settings.window_mode = settings::WindowMode::Windowed;
        settings.hide_host_cursor = true;
        log::info!("[stream-cli] app window {} {:?} {}x{} dpi {} caption {} icon {} bytes", w.hwnd, w.title, w.rect[2], w.rect[3], w.dpi, w.caption, icon.len());
        Some((w.hwnd.clone(), cfg))
    } else {
        None
    };
    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    let process = match stream::start_with_window(&helper, &identity, host, app, &settings, None, tx, window.clone()) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("start: {e}");
            return 1;
        }
    };
    log::info!("Streaming {} from {} for {}s (helper {})", app.name, host.name, seconds, helper.display());
    let mut code = 0;
    // Geometry poller for app windows: forwards host-side changes to the helper.
    let poller = window_hwnd.as_ref().map(|hwnd| {
        let http2 = {
            let addr = host.active_address.clone().or_else(|| host.unique_addresses().into_iter().next()).expect("host address");
            backend::http::NvHttp::new(addr, host.active_https_port, host.server_cert(), !host.is_nvidia_server_software, identity.clone())
        };
        let hwnd = hwnd.clone();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        appwindows::spawn_geometry_poller(http2, hwnd, (settings.width, settings.height * 2 / 3), process.clone_commander(), stop.clone());
        let handle = std::thread::spawn(|| {});
        (stop, handle)
    });
    futures::executor::block_on(async {
        while let Some(ev) = rx.next().await {
            log::info!("[stream-cli] {:?}", ev.event);
            match &ev.event {
                stream::HelperEvent::Exited { code: c, .. } => {
                    code = c.unwrap_or(1);
                    break;
                }
                stream::HelperEvent::AppWindow { kind, w, h, hwnd: child } => {
                    if let Some(hwnd) = child.as_ref().or(window_hwnd.as_ref()) {
                        let r = match kind.as_str() {
                            "resize" => http.window_action(hwnd, "resize", 0, 0, *w, *h),
                            "focus" => http.window_action(hwnd, "activate", 0, 0, 0, 0),
                            "minimize" => http.window_action(hwnd, "minimize", 0, 0, 0, 0),
                            "restore" => http.window_action(hwnd, "restore", 0, 0, 0, 0),
                            "close" => http.window_action(hwnd, "close", 0, 0, 0, 0),
                            _ => Ok(()),
                        };
                        if let Err(e) = r {
                            log::warn!("[stream-cli] window {kind}: {e}");
                        }
                    }
                }
                _ => {}
            }
        }
    });
    if let Some((stop, handle)) = poller {
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = handle.join();
    }
    drop(process);
    code
}

fn main() {
    init_logging();
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 3 && args[1] == "--probe" {
        std::process::exit(probe(&args[2]));
    }
    if args.len() >= 4 && args[1] == "--stream" {
        let secs = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(30);
        std::process::exit(stream_cli(&args[2], &args[3], secs));
    }

    let config_dir = config_dir();
    let cache_dir = cache_dir();
    let settings = settings::Settings::load(&config_dir);
    let identity = match backend::Identity::load_or_create(&config_dir) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("Fatal: could not create a client identity: {e}");
            std::process::exit(1);
        }
    };

    Application::new().with_assets(assets::Assets).run(move |cx: &mut App| {
        let mut theme = theme::Theme::load_from_zed();
        theme.resolve_fonts(cx);
        cx.set_global(theme);
        ui::text_input::bind_keys(cx);

        cx.bind_keys([
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("cmd-,", OpenSettings, None),
            KeyBinding::new("cmd-n", AddHost, None),
            KeyBinding::new("cmd-r", Refresh, None),
            KeyBinding::new("cmd-f", FocusFilter, None),
            KeyBinding::new("cmd-i", ToggleInspector, None),
            KeyBinding::new("cmd-shift-c", CopyHostAddress, None),
            KeyBinding::new("cmd-shift-h", ToggleHiddenApps, None),
            KeyBinding::new("cmd-k", QuickLaunch, None),
            KeyBinding::new("cmd-w", CloseWindow, None),
        ]);
        cx.on_action(|_: &CloseWindow, cx| cx.hide());

        cx.set_menus(vec![
            Menu {
                name: "Moonlight".into(),
                items: vec![
                    MenuItem::action("Settings…", OpenSettings),
                    MenuItem::separator(),
                    MenuItem::action("Quit", Quit),
                ],
            },
            Menu { name: "File".into(), items: vec![MenuItem::action("Add Host…", AddHost), MenuItem::action("Refresh", Refresh)] },
            Menu {
                name: "View".into(),
                items: vec![
                    MenuItem::action("Quick Launch", QuickLaunch),
                    MenuItem::action("Show Hidden Apps", ToggleHiddenApps),
                    MenuItem::action("Host Info", ToggleInspector),
                    MenuItem::separator(),
                    MenuItem::action("Back", Back),
                ],
            },
        ]);

        let bounds = Bounds::centered(None, size(px(settings.window_width.max(680.)), px(settings.window_height.max(440.))), cx);
        let config_dir = config_dir.clone();
        let cache_dir = cache_dir.clone();
        let settings = settings.clone();
        let identity = identity.clone();
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions { title: Some("Moonlight".into()), appears_transparent: true, traffic_light_position: Some(point(px(9.), px(11.))) }),
                window_min_size: Some(size(px(680.), px(440.))),
                app_id: Some("com.sloplight.moonlight".into()),
                ..Default::default()
            },
            move |window, cx| cx.new(|cx| Root::new(window, cx, config_dir, cache_dir, settings, identity)),
        )
        .unwrap();
        cx.activate(true);
    });
}
