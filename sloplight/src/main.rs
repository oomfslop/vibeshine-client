mod app;
mod appwindows;
mod launcher;
mod assets;
mod backend;
mod display;
mod settings;
mod stream;
mod theme;
mod ui;
mod views;

use std::path::PathBuf;

use gpui::{App, Bounds, KeyBinding, Menu, MenuItem, TitlebarOptions, WindowBounds, WindowOptions, point, prelude::*, px, size};

use app::{AddHost, Back, CloseWindow, FocusFilter, LaunchOnHost, OpenSettings, QuickLaunch, Quit, Refresh, Root, ToggleHiddenApps, ToggleInspector, CopyHostAddress};

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
    // App-window test mode: SLOPLIGHT_WINDOW_HANDLE=0x... opens the host's desk,
    // streams it and adopts that window onto it (the helper follows the desk's
    // layout itself; SLOPLIGHT_ADOPT_MORE=0x..,0x.. adopts more windows).
    let window_hwnd = std::env::var("SLOPLIGHT_WINDOW_HANDLE").ok().filter(|v| !v.is_empty());
    let http = {
        let addr = host.active_address.clone().or_else(|| host.unique_addresses().into_iter().next()).expect("host address");
        backend::http::NvHttp::new(addr, host.active_https_port, host.server_cert(), !host.is_nvidia_server_software, identity.clone())
    };
    let mut settings = settings;
    // Desk mode: open the host's desk for a screen like this Mac's, stream it,
    // and adopt the window onto it.
    let screen = display::displays().first().map(|d| [0, 0, d.points.0, d.points.1]).unwrap_or([0, 0, 1728, 1117]);
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
        let icon = http.get_window_icon(&w.hwnd).unwrap_or_default();
        let plan = appwindows::desk_plan(&settings, (screen[2], screen[3]), app::Root::screen_backing_scale());
        let k = 1.0 / plan.points_per_px;
        let dpi = std::env::var("SLOPLIGHT_DESK_DPI").ok().and_then(|v| v.parse().ok()).unwrap_or(plan.dpi);
        let want = plan.size;
        let desk = match http.open_desk(want.0, want.1, settings.fps.max(30), dpi, &appwindows::desk_options(&settings, &plan)) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("desk open: {e}");
                return 1;
            }
        };
        log::info!("[stream-cli] desk {} {:?} @{} {} dpi for screen {:?} (wanted {}x{})", desk.display, desk.rect, desk.fps, desk.dpi, screen, want.0, want.1);
        settings.bitrate_kbps = settings.app_window_bitrate_kbps.clamp(2000, 150000);
        (settings.width, settings.height) = desk.stream_size();
        settings.fps = desk.fps;
        settings.window_mode = settings::WindowMode::Windowed;
        settings.hide_host_cursor = true;
        let cfg = stream::AppWindowConfig { icon_png_base64: stream::base64_encode(&icon), dpi: desk.dpi, points_per_px: plan.points_per_px as f64, desk: true, screen };
        let (sw, sh) = ((screen[2] as f32 * k) as i32, (screen[3] as f32 * k) as i32);
        let kw = dpi as f32 / w.dpi.max(48) as f32;
        let (ww, wh) = ((w.rect[2] as f32 * kw) as i32, (w.rect[3] as f32 * kw) as i32);
        let (x, y) = ((w.rect[0] - w.monitor_rect[0]).min((sw - ww).max(0)).max(0), (w.rect[1] - w.monitor_rect[1]).min((sh - wh).max(0)).max(0));
        if let Err(e) = http.window_action(&w.hwnd, "adopt", x, y, 0, 0) {
            log::warn!("[stream-cli] adopt: {e}");
        }
        for extra in std::env::var("SLOPLIGHT_ADOPT_MORE").ok().unwrap_or_default().split(',').filter(|s| !s.is_empty()) {
            if let Some(o) = windows.iter().find(|w| w.hwnd.eq_ignore_ascii_case(extra)) {
                let r = http.window_action(&o.hwnd, "adopt", o.rect[0] - o.monitor_rect[0], o.rect[1] - o.monitor_rect[1], 0, 0);
                log::info!("[stream-cli] adopt {}: {:?}", o.hwnd, r.err());
            }
        }
        log::info!("[stream-cli] app window {} {:?} adopted at {x},{y}; desk {}x{}", w.hwnd, w.title, settings.width, settings.height);
        Some((appwindows::DESK_STREAM_ID.to_string(), cfg))
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
    // Test hook: SLOPLIGHT_TEST_CMD_FILE=<path> forwards every new JSON line appended to that file
    if let Ok(path) = std::env::var("SLOPLIGHT_TEST_CMD_FILE") {
        let commander = process.clone_commander();
        std::thread::spawn(move || {
            let mut sent = 0usize;
            loop {
                std::thread::sleep(std::time::Duration::from_millis(200));
                let Ok(text) = std::fs::read_to_string(&path) else { continue };
                let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
                for line in lines.iter().skip(sent) {
                    match serde_json::from_str::<serde_json::Value>(line) {
                        Ok(v) => {
                            log::info!("[stream-cli] test cmd {v}");
                            if !commander.send_command(&v) {
                                return;
                            }
                        }
                        Err(e) => log::warn!("[stream-cli] bad test cmd {line}: {e}"),
                    }
                }
                sent = lines.len();
            }
        });
    }
    // Test hook: SLOPLIGHT_TEST_CMDS="<ms>:<json>;<ms>:<json>" sends helper commands at those delays
    if let Ok(cmds) = std::env::var("SLOPLIGHT_TEST_CMDS") {
        let commander = process.clone_commander();
        std::thread::spawn(move || {
            let start = std::time::Instant::now();
            for item in cmds.split(';').filter(|s| !s.trim().is_empty()) {
                let Some((ms, json)) = item.split_once(':') else { continue };
                let at = std::time::Duration::from_millis(ms.trim().parse().unwrap_or(0));
                if let Some(wait) = at.checked_sub(start.elapsed()) {
                    std::thread::sleep(wait);
                }
                match serde_json::from_str::<serde_json::Value>(json) {
                    Ok(v) => {
                        log::info!("[stream-cli] test cmd {v}");
                        commander.send_command(&v);
                    }
                    Err(e) => log::warn!("[stream-cli] bad test cmd {json}: {e}"),
                }
            }
        });
    }
    let mut code = 0;
    futures::executor::block_on(async {
        while let Some(ev) = rx.next().await {
            log::info!("[stream-cli] {:?}", ev.event);
            match &ev.event {
                stream::HelperEvent::Exited { code: c, .. } => {
                    code = c.unwrap_or(1);
                    break;
                }
                stream::HelperEvent::AppWindow { kind, w, h, hwnd: child, x, y } => {
                    log::info!("[stream-cli] desk event {kind} {child:?} {x},{y} {w}x{h}");
                    if kind == "close" && child.is_none() {
                        process.terminate();
                    }
                }
                _ => {}
            }
        }
    });
    drop(process);
    if window.is_some() {
        // Adopted windows go back to where they were; the desk idles out on the host
        if let Err(e) = http.close_desk() {
            log::warn!("[stream-cli] desk close: {e}");
        }
    }
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

    gpui_platform::application().with_assets(assets::Assets).run(move |cx: &mut App| {
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
            KeyBinding::new("cmd-o", LaunchOnHost, None),
            KeyBinding::new("cmd-w", CloseWindow, None),
        ]);
        cx.on_action(|_: &CloseWindow, cx| cx.hide());

        cx.set_menus(vec![
            Menu {
                name: "Moonlight".into(),
                disabled: false,
                items: vec![
                    MenuItem::action("Settings…", OpenSettings),
                    MenuItem::separator(),
                    MenuItem::action("Quit", Quit),
                ],
            },
            Menu { name: "File".into(), disabled: false, items: vec![MenuItem::action("Add Host…", AddHost), MenuItem::action("Launch App on Host…", LaunchOnHost), MenuItem::action("Refresh", Refresh)] },
            Menu {
                name: "View".into(),
                disabled: false,
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
