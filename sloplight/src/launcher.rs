//! The app launcher: what a host's signed-in user can launch (the Start
//! menu's apps, the programs on their PATH, or any command line), launched on
//! the host and its window streamed here like any other host window.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gpui::Context;

use crate::app::Root;
use crate::appwindows::WindowsEvent;
use crate::backend::http::Launchables;
use crate::ui::ToastKind;

/// The launcher's list for one host, as last fetched
#[derive(Default)]
pub struct LaunchablesState {
    pub host_uuid: String,
    pub list: Launchables,
    /// icon key -> cached PNG file
    pub icons: HashMap<String, PathBuf>,
    pub loading: bool,
    pub error: Option<String>,
    pub fetched_at: Option<Instant>,
}

/// What a launch starts
#[derive(Clone, Debug)]
pub enum LaunchTarget {
    /// A Start menu app, by its Apps-folder id
    App { id: String },
    /// A command line, as typed or a PATH program
    Command { line: String },
}

fn icon_dir(cache_dir: &Path) -> PathBuf {
    cache_dir.join("launcher-icons")
}

/// How long a launched app has to show a window (Store apps take a while)
const WINDOW_WAIT: Duration = Duration::from_secs(20);

impl Root {
    /// Fetches the host's launcher list, and the icons not cached yet (all in
    /// one request: a request per icon is slow on slow links)
    pub fn refresh_launchables(&mut self, uuid: &str, refresh: bool, cx: &mut Context<Self>) {
        let Some(host) = self.host_by_uuid(uuid).cloned() else { return };
        let Some(http) = host.http(self.identity.clone()) else { return };
        if self.launchables.host_uuid != uuid {
            self.launchables = LaunchablesState { host_uuid: uuid.to_string(), ..Default::default() };
        }
        if self.launchables.loading {
            return;
        }
        self.launchables.loading = true;
        let tx = self.windows_tx.clone();
        let dir = icon_dir(&self.cache_dir);
        let uuid = uuid.to_string();
        std::thread::Builder::new()
            .name("launcher list".into())
            .spawn(move || {
                let result = http.get_launchables(refresh).map_err(|e| e.to_string()).map(|list| {
                    let _ = std::fs::create_dir_all(&dir);
                    let path_of = |key: &str| dir.join(format!("{}.png", key.replace(['/', '\\', ':', '.'], "_")));
                    let missing: Vec<String> = list
                        .apps
                        .iter()
                        .map(|a| a.icon.clone())
                        .filter(|k| !k.is_empty() && !path_of(k).exists())
                        .collect::<HashSet<_>>()
                        .into_iter()
                        .collect();
                    for chunk in missing.chunks(64) {
                        if let Ok(icons) = http.get_launchable_icons(chunk) {
                            for (key, png) in icons {
                                let _ = std::fs::write(path_of(&key), png);
                            }
                        }
                    }
                    let icons = list
                        .apps
                        .iter()
                        .filter(|a| !a.icon.is_empty())
                        .map(|a| (a.icon.clone(), path_of(&a.icon)))
                        .filter(|(_, p)| p.exists())
                        .collect();
                    (list, icons)
                });
                let _ = tx.unbounded_send(WindowsEvent::Launchables { host_uuid: uuid, result });
            })
            .ok();
        cx.notify();
    }

    pub fn on_launchables(&mut self, host_uuid: String, result: Result<(Launchables, HashMap<String, PathBuf>), String>) {
        if self.launchables.host_uuid != host_uuid {
            return;
        }
        self.launchables.loading = false;
        self.launchables.fetched_at = Some(Instant::now());
        match result {
            Ok((list, icons)) => {
                self.launchables.list = list;
                self.launchables.icons = icons;
                self.launchables.error = None;
            }
            Err(e) => self.launchables.error = Some(e),
        }
    }

    /// Launches on the host, then streams the first window that shows up
    pub fn launch_on_host(&mut self, uuid: &str, target: LaunchTarget, label: String, cx: &mut Context<Self>) {
        let Some(host) = self.host_by_uuid(uuid).cloned() else { return };
        let Some(http) = host.http(self.identity.clone()) else { return };
        self.toast(ToastKind::Info, format!("Launching {label}…"), cx);
        let tx = self.windows_tx.clone();
        let uuid = uuid.to_string();
        std::thread::Builder::new()
            .name("launch on host".into())
            .spawn(move || {
                // The windows before, to know the new one
                let before: HashSet<String> = http.get_host_windows().unwrap_or_default().iter().map(|w| w.hwnd.to_ascii_lowercase()).collect();
                let launched = match &target {
                    LaunchTarget::App { id } => http.launch_app(id),
                    LaunchTarget::Command { line } => http.launch_command(line),
                };
                if let Err(e) = launched {
                    let _ = tx.unbounded_send(WindowsEvent::LaunchFailed { message: format!("Couldn't launch {label}: {e}") });
                    return;
                }
                let start = Instant::now();
                while start.elapsed() < WINDOW_WAIT {
                    std::thread::sleep(Duration::from_millis(400));
                    let Ok(list) = http.get_host_windows() else { continue };
                    if let Some(w) = list.into_iter().find(|w| !before.contains(&w.hwnd.to_ascii_lowercase()) && w.state != "minimized") {
                        let _ = tx.unbounded_send(WindowsEvent::Launched { host_uuid: uuid, window: w });
                        return;
                    }
                }
                let _ = tx.unbounded_send(WindowsEvent::LaunchFailed { message: format!("{label} started, but no window appeared") });
            })
            .ok();
    }
}
