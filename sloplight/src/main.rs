mod app;
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

fn main() {
    init_logging();
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 3 && args[1] == "--probe" {
        std::process::exit(probe(&args[2]));
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
