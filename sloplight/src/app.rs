//! Application state: one `Root` entity owns everything the window shows.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use gpui::{App, ClipboardItem, Context, Entity, FocusHandle, Focusable, KeyDownEvent, Pixels, Point, Timer, Window, actions, prelude::*};

use crate::backend::{App as NvApp, Host, HostManager, Identity, ManagerEvent};
use crate::settings::Settings;
use crate::stream::{self, HelperEvent, StreamEvent, StreamProcess};
use crate::ui::{InputKind, TextInput, TextInputEvent, ToastKind};

actions!(
    sloplight,
    [Quit, OpenSettings, AddHost, Refresh, FocusFilter, ToggleInspector, CopyHostAddress, ToggleHiddenApps, QuickLaunch, Back, CloseWindow, SetupGuide, Troubleshooting, Discord, About]
);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Region {
    Sidebar,
    Grid,
}


/// Pairing in progress for one host, shown inline on its Details tab.
pub struct InlinePairing {
    pub uuid: String,
    pub pin: String,
    pub otp: bool,
    pub in_progress: bool,
    pub error: Option<String>,
    pub otp_pin: Entity<TextInput>,
    pub otp_pass: Entity<TextInput>,
    pub reveal: bool,
}

/// What an input dialog does with its text.
#[derive(Clone, Debug)]
pub enum InputPurpose {
    AddHost,
    Rename { uuid: String },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ViewMode {
    Main,
    Settings,
}

#[derive(Clone, Debug)]
pub enum ConfirmAction {
    QuitApp { uuid: String },
    QuitAndStart { uuid: String, next_app_id: i32 },
    RemoveHost { uuid: String },
    QuitWhileStreaming,
}

pub enum Dialog {
    Confirm {
        title: String,
        body: String,
        primary: String,
        danger: bool,
        action: ConfirmAction,
    },
    Error {
        title: String,
        body: Vec<String>,
        details: String,
        permission: bool,
        host_uuid: Option<String>,
    },
    /// A single text field with Cancel / primary action (add host, rename).
    Input {
        purpose: InputPurpose,
        title: String,
        help: Option<String>,
        primary: String,
        input: Entity<TextInput>,
        in_progress: bool,
        error: Option<String>,
        /// The OS refused the connection (macOS Local Network permission).
        blocked_by_os: bool,
    },
}

#[derive(Clone, Debug)]
pub enum PopupKind {
    HostMenu(String),
    /// The ⌘I info popover for a host.
    HostInfo(String),
    AppMenu(String, i32),
    Dropdown(crate::views::settings::Dropdown),
}

pub struct Popup {
    pub kind: PopupKind,
    pub position: Point<Pixels>,
    pub highlighted: Option<usize>,
}

pub struct Toast {
    pub id: u64,
    pub kind: ToastKind,
    pub text: String,
}

#[allow(dead_code)]
pub struct StreamState {
    pub process: StreamProcess,
    pub host_uuid: String,
    pub host_name: String,
    pub app_id: i32,
    pub app_name: String,
    pub is_resume: bool,
    pub stage: String,
    pub warnings: Vec<String>,
    pub connected: bool,
    pub quitting: bool,
    pub error_lines: Vec<String>,
    pub finished: bool,
}

pub struct PendingLaunch {
    pub uuid: String,
    pub app_id: i32,
}

pub struct Root {
    pub focus_handle: FocusHandle,
    pub manager: Arc<HostManager>,
    pub identity: Arc<Identity>,
    pub settings: Settings,
    pub config_dir: PathBuf,
    pub hosts: Vec<Host>,
    pub selected_host: Option<String>,
    pub region: Region,
    pub grid_selected: Option<usize>,
    pub filter: Entity<TextInput>,
    pub details_more: bool,
    /// Content pane size from the last render, for popups anchored to the header.
    pub last_content_w: f32,
    pub last_body_h: f32,
    pub pairings: Vec<InlinePairing>,
    pub waking: std::collections::HashMap<String, Instant>,
    pub view: ViewMode,
    pub settings_view: crate::views::settings::SettingsViewState,
    pub dialog: Option<Dialog>,
    pub popup: Option<Popup>,
    pub toasts: Vec<Toast>,
    pub next_toast_id: u64,
    pub stream: Option<StreamState>,
    pub stream_tx: UnboundedSender<StreamEvent>,
    /// App-window streaming: window list of the selected host and the streamed windows
    pub windows: crate::appwindows::WindowsState,
    pub window_streams: Vec<crate::appwindows::WindowStream>,
    pub windows_tx: crate::appwindows::WindowsSender,
    /// Stops the "new host windows" watcher that runs while windows are streamed
    pub window_watch_stop: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    pub cache_dir: PathBuf,
    pub pending_launch: Option<PendingLaunch>,
    pub box_art_requested: HashSet<(String, i32)>,
    pub helper_path: Option<PathBuf>,
    pub window_active: bool,
    pub polling_paused: bool,
    pub inactivity_deadline: Option<Instant>,
    pub direct_launch_done: HashSet<String>,
    pub palette: Option<crate::views::palette::PaletteState>,
    pub startup: bool,
    pub filter_text: String,
    pub needs_focus: bool,
    pub pending_focus: Option<FocusHandle>,
    pub grid_columns: usize,
    pub pending_add_host: bool,
    /// `SLOPLIGHT_DEBUG_STATE`: open a UI state right after the first poll (for screenshots)
    pub debug_state: Option<String>,
    pub debug_state_applied: bool,
}

impl Root {
    pub fn new(window: &mut Window, cx: &mut Context<Self>, config_dir: PathBuf, cache_dir: PathBuf, settings: Settings, identity: Arc<Identity>) -> Self {
        let (tx, rx) = unbounded::<ManagerEvent>();
        let manager = HostManager::new(config_dir.clone(), cache_dir.clone(), identity.clone(), tx, settings.enable_mdns);
        let (stream_tx, stream_rx) = unbounded::<StreamEvent>();
        let (windows_tx, windows_rx) = unbounded::<crate::appwindows::WindowsEvent>();
        let cache_dir_copy = cache_dir.clone();

        let filter = cx.new(|cx| TextInput::new(cx, "Filter apps", InputKind::Text));
        cx.subscribe(&filter, Self::on_filter_event).detach();

        let hosts = manager.snapshot();
        let selected_host = if hosts.iter().any(|h| h.uuid == settings.last_host_uuid) {
            Some(settings.last_host_uuid.clone())
        } else {
            hosts.first().map(|h| h.uuid.clone())
        };

        let settings_view = crate::views::settings::SettingsViewState::new(cx, &settings);
        let helper_path = stream::find_helper(&settings);
        if helper_path.is_none() {
            log::warn!("Streaming helper (moonlight binary) not found");
        }

        let mut root = Root {
            focus_handle: cx.focus_handle(),
            manager,
            identity,
            details_more: false,
            last_content_w: 776.,
            last_body_h: 584.,
            pairings: Vec::new(),
            waking: std::collections::HashMap::new(),
            settings,
            config_dir,
            hosts,
            selected_host,
            region: Region::Grid,
            grid_selected: None,
            filter,
            view: ViewMode::Main,
            settings_view,
            dialog: None,
            popup: None,
            toasts: Vec::new(),
            next_toast_id: 1,
            stream: None,
            stream_tx,
            pending_launch: None,
            box_art_requested: HashSet::new(),
            windows: Default::default(),
            window_streams: Vec::new(),
            windows_tx,
            window_watch_stop: None,
            cache_dir: cache_dir_copy,
            helper_path,
            window_active: true,
            polling_paused: false,
            inactivity_deadline: None,
            direct_launch_done: HashSet::new(),
            palette: None,
            startup: true,
            filter_text: String::new(),
            needs_focus: true,
            pending_focus: None,
            grid_columns: 5,
            pending_add_host: false,
            debug_state: None,
            debug_state_applied: false,
        };
        root.spawn_event_pumps(rx, stream_rx, cx);
        cx.spawn(async move |this, cx| {
            let mut windows_rx = windows_rx;
            while let Some(event) = windows_rx.next().await {
                if this.update(cx, |root, cx| root.on_windows_event(event, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        root.manager.start_polling();

        cx.observe_window_activation(window, |this, window, cx| {
            this.window_active = window.is_window_active();
            this.update_polling(cx);
        })
        .detach();
        cx.observe_window_bounds(window, |this, window, cx| {
            let b = window.bounds();
            this.settings.window_width = f32::from(b.size.width);
            this.settings.window_height = f32::from(b.size.height);
            let _ = cx;
        })
        .detach();
        cx.on_app_quit(|this, _cx| {
            this.settings.save(&this.config_dir);
            if let Some(stream) = &this.stream {
                stream.process.terminate();
            }
            futures::future::ready(())
        })
        .detach();
        root
    }

    fn spawn_event_pumps(&mut self, mut rx: UnboundedReceiver<ManagerEvent>, mut stream_rx: UnboundedReceiver<StreamEvent>, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            while let Some(event) = rx.next().await {
                if this.update(cx, |root, cx| root.on_manager_event(event, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            while let Some(event) = stream_rx.next().await {
                if this.update(cx, |root, cx| root.on_stream_event(event, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        // Tick for toasts and the polling inactivity timer
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_secs(1)).await;
                if this.update(cx, |root, cx| root.tick(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        let before = self.waking.len();
        self.waking.retain(|_, t| t.elapsed() < Duration::from_secs(60));
        if self.waking.len() != before {
            cx.notify();
        }
        if let Some(deadline) = self.inactivity_deadline
            && Instant::now() >= deadline && !self.window_active && !self.polling_paused {
                self.manager.stop_polling();
                self.polling_paused = true;
                self.inactivity_deadline = None;
                cx.notify();
            }
    }

    fn update_polling(&mut self, cx: &mut Context<Self>) {
        if self.window_active {
            self.inactivity_deadline = None;
            if self.polling_paused {
                self.polling_paused = false;
                self.manager.start_polling();
            }
        } else if self.stream.is_some() {
            // Streaming: stop polling right away like the Qt app does when hidden
            if !self.polling_paused {
                self.manager.stop_polling();
                self.polling_paused = true;
            }
        } else if self.inactivity_deadline.is_none() {
            self.inactivity_deadline = Some(Instant::now() + Duration::from_secs(5 * 60));
        }
        cx.notify();
    }

    // ------------------------------------------------------------ helpers

    pub fn selected(&self) -> Option<&Host> {
        let uuid = self.selected_host.as_ref()?;
        self.hosts.iter().find(|h| &h.uuid == uuid)
    }

    pub fn host_by_uuid(&self, uuid: &str) -> Option<&Host> {
        self.hosts.iter().find(|h| h.uuid == uuid)
    }

    /// Apps of the selected host after the hidden filter and the text filter.
    /// Apps in grid order: visible ones first, then (when shown) the hidden
    /// ones, keeping the host's order within each group.
    pub fn visible_apps(&self) -> Vec<NvApp> {
        let Some(host) = self.selected() else { return Vec::new() };
        let filter = self.filter_text.clone();
        let matches = |a: &NvApp| filter.is_empty() || fuzzy_match(&a.name, &filter);
        let mut out: Vec<NvApp> = host.apps.iter().filter(|a| !a.hidden && matches(a)).cloned().collect();
        if host.show_hidden_apps {
            out.extend(host.apps.iter().filter(|a| a.hidden && matches(a)).cloned());
        }
        out
    }

    /// How many of `visible_apps()` are not hidden (the hidden section starts there).
    pub fn visible_count(&self) -> usize {
        self.visible_apps().iter().filter(|a| !a.hidden).count()
    }

    /// Hidden apps on the selected host that match the filter, shown or not.
    pub fn hidden_matching(&self) -> usize {
        let Some(host) = self.selected() else { return 0 };
        let filter = self.filter_text.clone();
        host.apps.iter().filter(|a| a.hidden && (filter.is_empty() || fuzzy_match(&a.name, &filter))).count()
    }

    pub fn clamp_grid_selection(&mut self) {
        let n = self.visible_apps().len();
        self.grid_selected = match self.grid_selected {
            Some(_) if n == 0 => None,
            Some(i) => Some(i.min(n - 1)),
            None => None,
        };
    }

    pub fn save_settings(&mut self) {
        self.settings.last_host_uuid = self.selected_host.clone().unwrap_or_default();
        self.settings.save(&self.config_dir);
    }

    /// Whether the selected host has a browsable app grid.
    pub fn host_browsable(&self) -> bool {
        self.selected().map(|h| h.is_online() && h.is_paired() && h.is_supported_server_version && !(h.apps.is_empty() && h.app_list_error.is_some())).unwrap_or(false)
    }

    pub fn pairing_for(&self, uuid: &str) -> Option<&InlinePairing> {
        self.pairings.iter().find(|p| p.uuid == uuid)
    }

    pub fn toast(&mut self, kind: ToastKind, text: impl Into<String>, cx: &mut Context<Self>) {
        let id = self.next_toast_id;
        self.next_toast_id += 1;
        self.toasts.push(Toast { id, kind, text: text.into() });
        if self.toasts.len() > 2 {
            self.toasts.remove(0);
        }
        // Errors stay until dismissed
        if matches!(kind, ToastKind::Error | ToastKind::Warning) {
            cx.notify();
            return;
        }
        let ttl = 3;
        cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_secs(ttl)).await;
            this.update(cx, |root, cx| {
                root.toasts.retain(|t| t.id != id);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub fn copy_to_clipboard(&mut self, label: &str, value: &str, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(value.to_string()));
        self.toast(ToastKind::Success, format!("Copied {label}"), cx);
    }

    pub fn select_host(&mut self, uuid: Option<String>, cx: &mut Context<Self>) {
        if self.selected_host != uuid {
            self.selected_host = uuid;
            self.grid_selected = None;
            self.details_more = false;
            if matches!(self.popup, Some(Popup { kind: PopupKind::HostInfo(_), .. })) {
                self.popup = None;
            }
            self.set_filter(String::new(), cx);
            self.save_settings();
            cx.notify();
        }
    }

    pub fn select_host_index(&mut self, index: usize, cx: &mut Context<Self>) {
        let order = self.sidebar_order();
        if let Some(uuid) = order.get(index) {
            self.select_host(Some(uuid.clone()), cx);
        }
    }

    /// Hosts in sidebar order: paired first (by name), then unpaired.
    pub fn sidebar_order(&self) -> Vec<String> {
        let mut paired: Vec<&Host> = self.hosts.iter().filter(|h| h.is_paired() || h.pair_state == crate::backend::PairState::Unknown).collect();
        let mut unpaired: Vec<&Host> = self.hosts.iter().filter(|h| h.pair_state == crate::backend::PairState::NotPaired).collect();
        paired.sort_by_cached_key(|h| h.name.to_lowercase());
        unpaired.sort_by_cached_key(|h| h.name.to_lowercase());
        paired.into_iter().chain(unpaired).map(|h| h.uuid.clone()).collect()
    }

    // ------------------------------------------------------------ manager events

    fn on_manager_event(&mut self, event: ManagerEvent, cx: &mut Context<Self>) {
        match event {
            ManagerEvent::HostsChanged | ManagerEvent::HostAdded { .. } | ManagerEvent::HostRemoved { .. } => {
                self.hosts = self.manager.snapshot();
                if self.debug_state.is_none() && !self.debug_state_applied
                    && let Ok(state) = std::env::var("SLOPLIGHT_DEBUG_STATE")
                        && self.selected().map(|h| h.state != crate::backend::HostState::Unknown).unwrap_or(true) {
                            self.debug_state = Some(state);
                            self.debug_state_applied = true;
                        }
                if let ManagerEvent::HostAdded { uuid } = &event
                    && self.selected_host.is_none() {
                        self.selected_host = Some(uuid.clone());
                    }
                if let Some(sel) = &self.selected_host {
                    if !self.hosts.iter().any(|h| &h.uuid == sel) {
                        self.selected_host = self.sidebar_order().first().cloned();
                        self.grid_selected = None;
                    }
                } else if !self.hosts.is_empty() {
                    self.selected_host = self.sidebar_order().first().cloned();
                }
                let online: Vec<String> = self.hosts.iter().filter(|h| h.is_online()).map(|h| h.uuid.clone()).collect();
                self.waking.retain(|u, _| !online.contains(u));
                self.clamp_grid_selection();
                self.maybe_direct_launch(cx);
                self.try_pending_launch(cx);
                cx.notify();
            }
            ManagerEvent::AddCompleted { address, result } => {
                if let Some(Dialog::Input { purpose: InputPurpose::AddHost, in_progress, error, blocked_by_os, input, .. }) = &mut self.dialog {
                    *in_progress = false;
                    match result {
                        Ok(uuid) => {
                            self.dialog = None;
                            self.hosts = self.manager.snapshot();
                            self.select_host(Some(uuid), cx);
                            self.region = Region::Sidebar;
                            self.focus_main(cx);
                        }
                        Err(e) => {
                            log::warn!("Adding {address} failed: {}", e.message);
                            let mut msg = if e.port_blocked {
                                format!("Couldn't reach {address}. This network is blocking Moonlight.")
                            } else {
                                format!("Couldn't reach {address}: {}", e.message)
                            };
                            if e.resolution.is_some() {
                                msg.push('.');
                            }
                            *error = Some(msg);
                            *blocked_by_os = e.resolution.is_some();
                            input.update(cx, |t, cx| {
                                t.invalid = true;
                                t.disabled = false;
                                t.select_all_text(cx);
                            });
                        }
                    }
                }
                cx.notify();
            }
            ManagerEvent::PairingCompleted { uuid, error } => {
                self.hosts = self.manager.snapshot();
                match error {
                    None => {
                        self.pairings.retain(|p| p.uuid != uuid);
                        self.focus_main(cx);
                    }
                    Some(e) if e == crate::backend::manager::CANCELLED => self.pairings.retain(|p| p.uuid != uuid),
                    Some(e) => {
                        if let Some(p) = self.pairings.iter_mut().find(|p| p.uuid == uuid) {
                            p.in_progress = false;
                            p.error = Some(e);
                        } else {
                            self.toast(ToastKind::Warning, e, cx);
                        }
                    }
                }
                cx.notify();
            }
            ManagerEvent::QuitCompleted { uuid, error } => {
                self.hosts = self.manager.snapshot();
                if let Some(e) = error {
                    let name = self.host_by_uuid(&uuid).and_then(|h| h.running_app().map(|a| a.name.clone())).unwrap_or_else(|| "the app".into());
                    self.pending_launch = None;
                    self.dialog = Some(Dialog::Error { title: format!("Couldn't quit {name}"), body: vec![e.clone()], details: e, permission: false, host_uuid: Some(uuid) });
                } else {
                    self.try_pending_launch(cx);
                }
                cx.notify();
            }
            ManagerEvent::BoxArtLoaded { .. } => cx.notify(),
        }
    }

    /// Direct launch applies once, to the host that was selected when the app
    /// was last closed, as soon as its first poll has completed.
    fn maybe_direct_launch(&mut self, cx: &mut Context<Self>) {
        if !self.startup || self.stream.is_some() {
            return;
        }
        if std::env::var_os("SLOPLIGHT_DEBUG_STATE").is_some() {
            // Screenshot runs must not start a stream.
            self.startup = false;
            return;
        }
        let last = self.settings.last_host_uuid.clone();
        let Some(host) = self.host_by_uuid(&last).cloned() else {
            self.startup = false;
            return;
        };
        if host.state == crate::backend::HostState::Unknown {
            return;
        }
        self.startup = false;
        if !host.is_online() || !host.is_paired() || host.apps.is_empty() || self.direct_launch_done.contains(&host.uuid) {
            return;
        }
        if host.current_game_id == 0
            && let Some(app) = host.direct_launch_app().cloned() {
                self.direct_launch_done.insert(host.uuid.clone());
                self.launch(&host.uuid, app.id, cx);
            }
    }

    // ------------------------------------------------------------ launching

    /// Starts or resumes an app, asking to quit a different running app first.
    pub fn launch(&mut self, uuid: &str, app_id: i32, cx: &mut Context<Self>) {
        let Some(host) = self.host_by_uuid(uuid).cloned() else { return };
        if self.stream.is_some() {
            return;
        }
        if host.current_game_id != 0 && host.current_game_id != app_id {
            let running = host.running_app().map(|a| a.name.clone()).unwrap_or_else(|| "the running app".into());
            let next = host.apps.iter().find(|a| a.id == app_id).map(|a| a.name.clone()).unwrap_or_default();
            self.dialog = Some(Dialog::Confirm {
                title: format!("Quit {running} and start {next}?"),
                body: format!("Unsaved progress in {running} will be lost."),
                primary: "Quit and start".into(),
                danger: true,
                action: ConfirmAction::QuitAndStart { uuid: uuid.to_string(), next_app_id: app_id },
            });
            cx.notify();
            return;
        }
        self.start_stream(&host, app_id, cx);
    }

    fn start_stream(&mut self, host: &Host, app_id: i32, cx: &mut Context<Self>) {
        let Some(app) = host.apps.iter().find(|a| a.id == app_id).cloned() else { return };
        let helper = self.helper_path.clone().or_else(|| stream::find_helper(&self.settings));
        let Some(helper) = helper else {
            self.dialog = Some(Dialog::Error {
                title: format!("Couldn't start {}", app.name),
                body: vec![
                    "The streaming helper (the Moonlight binary) wasn't found.".into(),
                    "Build it with `nix build` in the moonlight-qt directory, install Moonlight.app, or set its path in Settings > Advanced > Streaming helper.".into(),
                ],
                details: String::new(),
                permission: false,
                host_uuid: None,
            });
            cx.notify();
            return;
        };
        self.helper_path = Some(helper.clone());
        let is_resume = host.current_game_id == app.id;
        match stream::start(&helper, &self.identity, host, &app, &self.settings, None, self.stream_tx.clone()) {
            Ok(process) => {
                self.stream = Some(StreamState {
                    process,
                    host_uuid: host.uuid.clone(),
                    host_name: host.name.clone(),
                    app_id: app.id,
                    app_name: app.name.clone(),
                    is_resume,
                    stage: String::new(),
                    warnings: Vec::new(),
                    connected: false,
                    quitting: false,
                    error_lines: Vec::new(),
                    finished: false,
                });
                self.popup = None;
                self.dialog = None;
            }
            Err(e) => {
                self.dialog = Some(Dialog::Error { title: format!("Couldn't start {}", app.name), body: vec![e.clone()], details: e, permission: false, host_uuid: None });
            }
        }
        cx.notify();
    }

    fn try_pending_launch(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = &self.pending_launch else { return };
        let Some(host) = self.host_by_uuid(&pending.uuid).cloned() else {
            self.pending_launch = None;
            return;
        };
        if host.current_game_id == 0 && !host.pending_quit {
            let app_id = pending.app_id;
            self.pending_launch = None;
            self.start_stream(&host, app_id, cx);
        }
    }

    pub fn cancel_stream(&mut self, cx: &mut Context<Self>) {
        if let Some(stream) = &self.stream {
            stream.process.terminate();
        }
        cx.notify();
    }

    fn on_stream_event(&mut self, event: StreamEvent, cx: &mut Context<Self>) {
        if let Some(hwnd) = event.window.clone() {
            self.on_window_stream_event(&hwnd, event, cx);
            return;
        }
        let Some(stream) = &mut self.stream else { return };
        if stream.host_uuid != event.host_uuid || stream.app_id != event.app_id {
            return;
        }
        match event.event {
            HelperEvent::Warnings { warnings } => stream.warnings = warnings,
            HelperEvent::StageStarting { stage } => stream.stage = format!("Starting {stage}…"),
            HelperEvent::StageFailed { stage, error_code, failing_ports } => {
                stream.error_lines.push(format!("Starting {stage} failed: Error {error_code}"));
                if !failing_ports.is_empty() {
                    stream.error_lines.push(format!("Check your firewall and port forwarding rules for port(s): {failing_ports}"));
                }
            }
            HelperEvent::ConnectionStarted => {
                stream.connected = true;
                let uuid = stream.host_uuid.clone();
                let app_id = stream.app_id;
                self.manager.note_app_running(&uuid, app_id);
                // Get out of the way of the stream window
                cx.hide();
            }
            HelperEvent::LaunchError { text } => stream.error_lines.push(text),
            HelperEvent::QuitStarting => {
                stream.quitting = true;
                stream.stage = format!("Quitting {}…", stream.app_name);
            }
            HelperEvent::SessionFinished { port_test_result } => {
                stream.finished = true;
                if port_test_result != 0 && port_test_result != -1 && !stream.error_lines.is_empty() {
                    stream.error_lines.push("This network is blocking Moonlight. Streaming over the Internet may not work here.".into());
                }
            }
            HelperEvent::TestInput { .. } => {}
            HelperEvent::AppWindow { .. } => {}
            HelperEvent::Exited { code, stderr_tail } => {
                let stream = self.stream.take().unwrap();
                if !stream.finished && stream.error_lines.is_empty() && code != Some(0) {
                    // Crashed or killed before reporting
                    if code.is_some() {
                        self.dialog = Some(Dialog::Error {
                            title: if stream.connected { "Stream ended".into() } else { format!("Couldn't start {}", stream.app_name) },
                            body: vec![format!("The streaming helper exited unexpectedly (code {}).", code.unwrap())],
                            details: stderr_tail,
                            permission: false,
                            host_uuid: Some(stream.host_uuid.clone()),
                        });
                    }
                } else if !stream.error_lines.is_empty() {
                    let permission = stream.error_lines.iter().any(|l| l.contains("403"));
                    let (title, body) = if permission {
                        (
                            format!("{} denied this request", stream.host_name),
                            vec!["This client doesn't have the \"Launch apps\" permission. Grant it in Apollo's web UI under the client list, then try again.".to_string()],
                        )
                    } else {
                        (if stream.connected { "Stream ended".to_string() } else { format!("Couldn't start {}", stream.app_name) }, stream.error_lines.clone())
                    };
                    let details = format!("{}\n\nHost: {}\nProfile: {}\n\n{}", stream.error_lines.join("\n"), stream.host_name, self.settings.profile_summary(true), stderr_tail);
                    self.dialog = Some(Dialog::Error { title, body, details, permission, host_uuid: Some(stream.host_uuid.clone()) });
                }
                // Select the app we just streamed
                if self.selected_host.as_deref() == Some(&stream.host_uuid) {
                    let apps = self.visible_apps();
                    self.grid_selected = apps.iter().position(|a| a.id == stream.app_id);
                    self.region = Region::Grid;
                }
                cx.activate(true);
                self.focus_main(cx);
                self.manager.refresh();
                if self.polling_paused {
                    self.polling_paused = false;
                    self.manager.start_polling();
                }
            }
        }
        cx.notify();
    }

    // ------------------------------------------------------------ dialogs

    pub fn close_dialog(&mut self, cx: &mut Context<Self>) {
        self.dialog = None;
        self.focus_main(cx);
        cx.notify();
    }

    /// Returns keyboard focus to the main view on the next render.
    pub fn focus_main(&mut self, cx: &mut Context<Self>) {
        self.needs_focus = true;
        cx.notify();
    }

    pub fn confirm(&mut self, action: ConfirmAction, cx: &mut Context<Self>) {
        self.dialog = None;
        match action {
            ConfirmAction::QuitApp { uuid } => {
                self.manager.quit_running_app(&uuid);
            }
            ConfirmAction::QuitAndStart { uuid, next_app_id } => {
                self.pending_launch = Some(PendingLaunch { uuid: uuid.clone(), app_id: next_app_id });
                self.manager.quit_running_app(&uuid);
            }
            ConfirmAction::RemoveHost { uuid } => {
                let name = self.host_by_uuid(&uuid).map(|h| h.name.clone()).unwrap_or_default();
                self.manager.delete_host(&uuid);
                self.toast(ToastKind::Info, format!("Removed {name}"), cx);
            }
            ConfirmAction::QuitWhileStreaming => {
                if let Some(s) = &self.stream {
                    s.process.terminate();
                }
                cx.quit();
            }
        }
        self.focus_main(cx);
        cx.notify();
    }

    fn new_otp_inputs(&self, cx: &mut Context<Self>) -> (Entity<TextInput>, Entity<TextInput>) {
        let otp_pin = cx.new(|cx| {
            let mut t = TextInput::new(cx, "PIN", InputKind::Numeric { decimal: false, negative: false });
            t.max_len = Some(4);
            t
        });
        let otp_pass = cx.new(|cx| TextInput::new(cx, "Passphrase (optional)", InputKind::Password));
        cx.subscribe(&otp_pin, Self::on_otp_pin_event).detach();
        cx.subscribe(&otp_pass, Self::on_otp_pass_event).detach();
        (otp_pin, otp_pass)
    }

    /// Starts standard PIN pairing; the PIN is shown inline on the Details tab.
    pub fn start_pin_pairing(&mut self, uuid: &str, cx: &mut Context<Self>) {
        if self.host_by_uuid(uuid).is_none() {
            return;
        }
        self.pairings.retain(|p| p.uuid != uuid);
        let pin = HostManager::generate_pin();
        self.manager.pair_host(uuid, pin.clone());
        let (otp_pin, otp_pass) = self.new_otp_inputs(cx);
        self.pairings.push(InlinePairing { uuid: uuid.to_string(), pin, otp: false, in_progress: true, error: None, otp_pin, otp_pass, reveal: false });
        self.select_host(Some(uuid.to_string()), cx);
        self.popup = None;
        cx.notify();
    }

    /// Shows the one-time-PIN form in the host's state panel.
    pub fn open_otp_pairing(&mut self, uuid: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.host_by_uuid(uuid).is_none() {
            return;
        }
        self.pairings.retain(|p| p.uuid != uuid);
        let (otp_pin, otp_pass) = self.new_otp_inputs(cx);
        window.focus(&otp_pin.read(cx).focus_handle);
        self.pairings.push(InlinePairing { uuid: uuid.to_string(), pin: String::new(), otp: true, in_progress: false, error: None, otp_pin, otp_pass, reveal: false });
        self.select_host(Some(uuid.to_string()), cx);
        self.popup = None;
        cx.notify();
    }

    pub fn cancel_pairing(&mut self, uuid: &str, cx: &mut Context<Self>) {
        if self.pairing_for(uuid).map(|p| p.in_progress).unwrap_or(false) {
            self.manager.cancel_pairing(uuid);
        }
        self.pairings.retain(|p| p.uuid != uuid);
        self.focus_main(cx);
        cx.notify();
    }

    pub fn submit_otp(&mut self, uuid: &str, cx: &mut Context<Self>) {
        let Some(p) = self.pairings.iter_mut().find(|p| p.uuid == uuid && p.otp) else { return };
        let pin = p.otp_pin.read(cx).text().to_string();
        if pin.len() != 4 || !pin.chars().all(|c| c.is_ascii_digit()) || p.in_progress {
            return;
        }
        let pass = p.otp_pass.read(cx).text().to_string();
        p.in_progress = true;
        p.error = None;
        self.manager.pair_host_with_otp(uuid, pin, pass);
        cx.notify();
    }

    fn pairing_uuid_for_input(&self, input: &Entity<TextInput>) -> Option<String> {
        self.pairings.iter().find(|p| &p.otp_pin == input || &p.otp_pass == input).map(|p| p.uuid.clone())
    }

    fn on_otp_pin_event(&mut self, input: Entity<TextInput>, event: &TextInputEvent, cx: &mut Context<Self>) {
        let Some(uuid) = self.pairing_uuid_for_input(&input) else { return };
        match event {
            TextInputEvent::Submit => {
                if let Some(p) = self.pairing_for(&uuid) {
                    self.pending_focus = Some(p.otp_pass.read(cx).focus_handle.clone());
                    cx.notify();
                }
            }
            TextInputEvent::Escape => self.cancel_pairing(&uuid, cx),
            TextInputEvent::Changed => cx.notify(),
            _ => {}
        }
    }

    fn on_otp_pass_event(&mut self, input: Entity<TextInput>, event: &TextInputEvent, cx: &mut Context<Self>) {
        let Some(uuid) = self.pairing_uuid_for_input(&input) else { return };
        match event {
            TextInputEvent::Submit => self.submit_otp(&uuid, cx),
            TextInputEvent::Escape => self.cancel_pairing(&uuid, cx),
            _ => {}
        }
    }

    /// Opens the "Add host" dialog.
    pub fn open_add_host(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.view != ViewMode::Main {
            return;
        }
        if let Some(Dialog::Input { purpose: InputPurpose::AddHost, input, .. }) = &self.dialog {
            self.pending_focus = Some(input.read(cx).focus_handle.clone());
            return;
        }
        let input = cx.new(|cx| TextInput::new(cx, "192.168.1.20 or host:port", InputKind::Mono));
        cx.subscribe(&input, Self::on_input_dialog_event).detach();
        self.pending_focus = Some(input.read(cx).focus_handle.clone());
        self.dialog = Some(Dialog::Input {
            purpose: InputPurpose::AddHost,
            title: "Add host".into(),
            help: Some("IP address or host name. The port is only needed when it isn't 47989.".into()),
            primary: "Add".into(),
            input,
            in_progress: false,
            error: None,
            blocked_by_os: false,
        });
        self.popup = None;
        cx.notify();
    }

    fn on_input_dialog_event(&mut self, _input: Entity<TextInput>, event: &TextInputEvent, cx: &mut Context<Self>) {
        match event {
            TextInputEvent::Submit => self.submit_input_dialog(cx),
            TextInputEvent::Escape => self.close_dialog(cx),
            TextInputEvent::Changed => {
                if let Some(Dialog::Input { error, blocked_by_os, input, .. }) = &mut self.dialog {
                    *error = None;
                    *blocked_by_os = false;
                    input.update(cx, |t, _| t.invalid = false);
                }
                cx.notify();
            }
            _ => {}
        }
    }

    /// Runs the primary action of the open input dialog.
    pub fn submit_input_dialog(&mut self, cx: &mut Context<Self>) {
        let Some(Dialog::Input { purpose, input, in_progress, error, blocked_by_os, .. }) = &mut self.dialog else { return };
        let text = input.read(cx).text().trim().to_string();
        if text.is_empty() || *in_progress {
            return;
        }
        match purpose.clone() {
            InputPurpose::AddHost => {
                *in_progress = true;
                *error = None;
                *blocked_by_os = false;
                input.update(cx, |t, _| t.disabled = true);
                self.manager.add_host_manually(&text);
            }
            InputPurpose::Rename { uuid } => {
                self.manager.rename_host(&uuid, text.clone());
                self.hosts = self.manager.snapshot();
                self.close_dialog(cx);
                self.toast(ToastKind::Success, format!("Renamed to {text}"), cx);
            }
        }
        cx.notify();
    }

    pub fn confirm_remove_host(&mut self, uuid: &str, cx: &mut Context<Self>) {
        let Some(host) = self.host_by_uuid(uuid) else { return };
        self.dialog = Some(Dialog::Confirm {
            title: format!("Remove {}?", host.name),
            body: "You'll need to pair again to use it.".into(),
            primary: "Remove".into(),
            danger: true,
            action: ConfirmAction::RemoveHost { uuid: uuid.to_string() },
        });
        self.popup = None;
        cx.notify();
    }

    pub fn confirm_quit_app(&mut self, uuid: &str, cx: &mut Context<Self>) {
        let Some(host) = self.host_by_uuid(uuid) else { return };
        let app = host.running_app().map(|a| a.name.clone()).unwrap_or_else(|| "the running app".into());
        self.dialog = Some(Dialog::Confirm {
            title: format!("Quit {app} on {}?", host.name),
            body: "Unsaved progress will be lost.".into(),
            primary: "Quit".into(),
            danger: true,
            action: ConfirmAction::QuitApp { uuid: uuid.to_string() },
        });
        self.popup = None;
        cx.notify();
    }

    pub fn wake(&mut self, uuid: &str, cx: &mut Context<Self>) {
        let manager = self.manager.clone();
        let uuid_s = uuid.to_string();
        self.popup = None;
        self.waking.insert(uuid.to_string(), Instant::now());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let u = uuid_s.clone();
            let result = cx.background_executor().spawn(async move { manager.wake_host(&u) }).await;
            this.update(cx, |root, cx| {
                if let Err(e) = result {
                    root.waking.remove(&uuid_s);
                    root.toast(ToastKind::Error, e, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn start_rename(&mut self, uuid: &str, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(host) = self.host_by_uuid(uuid).cloned() else { return };
        let input = cx.new(|cx| {
            let mut t = TextInput::new(cx, host.name.clone(), InputKind::Text);
            t.set_text(host.name.clone(), cx);
            t.select_all_text(cx);
            t
        });
        cx.subscribe(&input, Self::on_input_dialog_event).detach();
        self.pending_focus = Some(input.read(cx).focus_handle.clone());
        self.dialog = Some(Dialog::Input {
            purpose: InputPurpose::Rename { uuid: uuid.to_string() },
            title: format!("Rename {}", host.name),
            help: None,
            primary: "Rename".into(),
            input,
            in_progress: false,
            error: None,
            blocked_by_os: false,
        });
        self.popup = None;
        cx.notify();
    }

    /// Primary action for a host: open, pair or wake.
    pub fn activate_host(&mut self, uuid: &str, cx: &mut Context<Self>) {
        let Some(host) = self.host_by_uuid(uuid).cloned() else { return };
        self.select_host(Some(uuid.to_string()), cx);
        if host.is_online() {
            if !host.is_supported_server_version {
                return;
            }
            if host.is_paired() {
                if host.current_game_id == 0
                    && let Some(app) = host.direct_launch_app().cloned() {
                        self.launch(uuid, app.id, cx);
                        return;
                    }
                self.region = Region::Grid;
                if self.grid_selected.is_none() && !self.visible_apps().is_empty() {
                    self.grid_selected = Some(0);
                }
            } else if self.pairing_for(uuid).is_none() {
                self.start_pin_pairing(uuid, cx);
            }
        } else if !host.mac.is_empty() && !self.waking.contains_key(uuid) {
            self.wake(uuid, cx);
        }
        cx.notify();
    }

    // ------------------------------------------------------------ filter

    fn on_filter_event(&mut self, input: Entity<TextInput>, event: &TextInputEvent, cx: &mut Context<Self>) {
        match event {
            TextInputEvent::Changed => {
                self.filter_text = input.read(cx).text().to_string();
                let apps = self.visible_apps();
                self.grid_selected = if apps.is_empty() { None } else { Some(0) };
                cx.notify();
            }
            TextInputEvent::Submit => {
                let apps = self.visible_apps();
                if apps.len() == 1 {
                    if let Some(uuid) = self.selected_host.clone() {
                        self.launch(&uuid, apps[0].id, cx);
                    }
                } else if !apps.is_empty() {
                    self.grid_selected = Some(self.grid_selected.unwrap_or(0).min(apps.len() - 1));
                    self.region = Region::Grid;
                    self.focus_main(cx);
                }
            }
            TextInputEvent::Arrow(1) => {
                let apps = self.visible_apps();
                if !apps.is_empty() {
                    self.grid_selected = Some(self.grid_selected.unwrap_or(0).min(apps.len() - 1));
                    self.region = Region::Grid;
                    self.focus_main(cx);
                }
            }
            TextInputEvent::Escape => {
                self.set_filter(String::new(), cx);
                self.focus_main(cx);
            }
            _ => {}
        }
    }

    pub fn set_filter(&mut self, text: String, cx: &mut Context<Self>) {
        self.filter_text = text.clone();
        self.filter.update(cx, |f, cx| f.set_text(text, cx));
        let apps = self.visible_apps();
        self.grid_selected = if apps.is_empty() { None } else { Some(0) };
        cx.notify();
    }

    // ------------------------------------------------------------ keyboard

    pub fn handle_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        let key = ks.key.as_str();
        let cmd = ks.modifiers.platform;

        // Text inputs handle their own keys
        if let Some(focused) = window.focused(cx)
            && focused != self.focus_handle {
                return;
            }

        if self.popup.is_some() {
            match key {
                "escape" => {
                    self.popup = None;
                    cx.notify();
                }
                "up" | "down" => {
                    let count = self.popup_entry_count(cx);
                    if let Some(p) = &mut self.popup {
                        let delta: isize = if key == "up" { -1 } else { 1 };
                        let cur = p.highlighted.map(|h| h as isize).unwrap_or(-1);
                        if count > 0 {
                            p.highlighted = Some(((cur + delta).rem_euclid(count as isize)) as usize);
                        }
                    }
                    cx.notify();
                }
                "enter" => self.activate_popup_entry(window, cx),
                _ => {}
            }
            cx.stop_propagation();
            return;
        }

        if self.palette.is_some() {
            crate::views::palette::handle_key(self, event, window, cx);
            cx.stop_propagation();
            return;
        }

        if self.dialog.is_some() {
            match key {
                "escape" => self.close_dialog(cx),
                "enter" => self.dialog_primary(window, cx),
                _ => {}
            }
            cx.stop_propagation();
            return;
        }

        // Inline pairing: copy the PIN, cancel with Esc
        if let Some(p) = self.selected_host.as_ref().and_then(|u| self.pairing_for(u)) {
            let uuid = p.uuid.clone();
            let pin = p.pin.clone();
            if cmd && key == "c" && !p.otp {
                self.copy_to_clipboard("PIN", &pin, cx);
                return;
            }
            if key == "escape" {
                self.cancel_pairing(&uuid, cx);
                return;
            }
        }

        if self.view == ViewMode::Settings {
            crate::views::settings::handle_key(self, event, window, cx);
            return;
        }

        // Cmd+1..9 select host
        if cmd && key.len() == 1 && key.as_bytes()[0].is_ascii_digit() && key != "0" {
            let n = (key.as_bytes()[0] - b'1') as usize;
            self.select_host_index(n, cx);
            self.region = Region::Grid;
            return;
        }
        if cmd && (key == "[" || key == "]") {
            let order = self.sidebar_order();
            if let Some(cur) = self.selected_host.as_ref().and_then(|s| order.iter().position(|u| u == s)) {
                let next = if key == "[" { (cur + order.len() - 1) % order.len() } else { (cur + 1) % order.len() };
                self.select_host_index(next, cx);
            }
            return;
        }
        if cmd && key == "backspace" {
            match self.region {
                Region::Sidebar => {
                    if let Some(uuid) = self.selected_host.clone() {
                        self.confirm_remove_host(&uuid, cx);
                    }
                }
                _ => {
                    if let Some(host) = self.selected().cloned()
                        && host.current_game_id != 0 {
                            self.confirm_quit_app(&host.uuid, cx);
                        }
                }
            }
            return;
        }
        if cmd {
            return;
        }

        // State panel: Esc cancels a pairing in progress, ⏎ runs the primary action
        if !self.host_browsable() {
            if let Some(uuid) = self.selected_host.clone() {
                if key == "escape" && self.pairing_for(&uuid).is_some() {
                    self.cancel_pairing(&uuid, cx);
                    return;
                }
                if key == "enter" && !ks.modifiers.shift {
                    self.run_primary_action(window, cx);
                    cx.notify();
                    return;
                }
            }
        }
        match self.region {
            Region::Sidebar => self.sidebar_key(key, ks.modifiers.shift, window, cx),
            Region::Grid => self.grid_key(event, window, cx),
        }
    }

    fn sidebar_key(&mut self, key: &str, _shift: bool, window: &mut Window, cx: &mut Context<Self>) {
        let order = self.sidebar_order();
        let cur = self.selected_host.as_ref().and_then(|s| order.iter().position(|u| u == s));
        match key {
            "up" | "down" | "home" | "end" => {
                if order.is_empty() {
                    return;
                }
                let next = match key {
                    "up" => cur.map(|c| c.saturating_sub(1)).unwrap_or(0),
                    "down" => cur.map(|c| (c + 1).min(order.len() - 1)).unwrap_or(0),
                    "home" => 0,
                    _ => order.len() - 1,
                };
                self.select_host_index(next, cx);
            }
            "enter" => {
                if let Some(uuid) = self.selected_host.clone() {
                    self.activate_host(&uuid, cx);
                }
            }
            "right" | "tab" => {
                self.region = Region::Grid;
                cx.notify();
            }
            "f2" => {
                if let Some(uuid) = self.selected_host.clone() {
                    self.start_rename(&uuid, window, cx);
                }
            }
            "delete" => {
                if let Some(uuid) = self.selected_host.clone() {
                    self.confirm_remove_host(&uuid, cx);
                }
            }
            "menu" | "f10" => {
                if let Some(uuid) = self.selected_host.clone() {
                    self.popup = Some(Popup { kind: PopupKind::HostMenu(uuid), position: window.mouse_position(), highlighted: Some(0) });
                    cx.notify();
                }
            }
            _ => {}
        }
    }

    fn grid_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        let key = ks.key.as_str();
        let apps = self.visible_apps();
        let cols = self.grid_columns.max(1);
        let n = apps.len();
        let sel = self.grid_selected;
        let set = |this: &mut Self, idx: usize, cx: &mut Context<Self>| {
            if n > 0 {
                this.grid_selected = Some(idx.min(n - 1));
                cx.notify();
            }
        };
        match key {
            "left" => match sel {
                Some(i) if i % cols > 0 => set(self, i - 1, cx),
                Some(_) | None => {
                    if sel.is_none() && n > 0 {
                        set(self, 0, cx);
                    } else {
                        self.region = Region::Sidebar;
                        cx.notify();
                    }
                }
            },
            "right" => match sel {
                Some(i) if i + 1 < n => set(self, i + 1, cx),
                None => set(self, 0, cx),
                _ => {}
            },
            "up" | "down" => {
                // Two wraps (visible, hidden) with their own rows; ↑/↓ cross the
                // section break keeping the column.
                let split = self.visible_count().min(n);
                let section = |i: usize| if i < split { (0usize, split) } else { (split, n - split) };
                match sel {
                    None => set(self, 0, cx),
                    Some(i) => {
                        let (st, len) = section(i);
                        let local = i - st;
                        let (row, col) = (local / cols, local % cols);
                        let rows = len.div_ceil(cols);
                        let target = if key == "up" {
                            if row > 0 {
                                Some(i - cols)
                            } else if st > 0 {
                                // top of hidden section → last row of visible, same column
                                let last_row = (split - 1) / cols;
                                Some((last_row * cols + col).min(split - 1))
                            } else {
                                None
                            }
                        } else if row + 1 < rows {
                            Some((i + cols).min(st + len - 1))
                        } else if st == 0 && split < n {
                            // bottom of visible section → first row of hidden, same column
                            Some((split + col).min(n - 1))
                        } else {
                            None
                        };
                        if let Some(t) = target {
                            set(self, t, cx);
                        }
                    }
                }
            }
            "home" => set(self, 0, cx),
            "end" => set(self, n.saturating_sub(1), cx),
            "pageup" => set(self, sel.unwrap_or(0).saturating_sub(cols * 3), cx),
            "pagedown" => set(self, sel.unwrap_or(0) + cols * 3, cx),
            "enter" => {
                if let (Some(i), Some(uuid)) = (sel, self.selected_host.clone()) {
                    if let Some(app) = apps.get(i) {
                        self.launch(&uuid, app.id, cx);
                    }
                } else if let Some(uuid) = self.selected_host.clone() {
                    self.activate_host(&uuid, cx);
                }
            }
            "tab" => {
                self.region = Region::Sidebar;
                cx.notify();
            }
            "escape" => {
                if self.stream.as_ref().map(|s| !s.connected).unwrap_or(false) {
                    self.cancel_stream(cx);
                } else if !self.filter_text.is_empty() {
                    self.set_filter(String::new(), cx);
                    self.grid_selected = None;
                } else {
                    self.grid_selected = None;
                }
                cx.notify();
            }
            "backspace" => {
                let text = self.filter_text.clone();
                if !text.is_empty() {
                    let mut chars = text.chars();
                    chars.next_back();
                    let t = chars.as_str().to_string();
                    self.set_filter(t, cx);
                }
            }
            "menu" | "f10" => {
                if let (Some(i), Some(uuid)) = (sel, self.selected_host.clone())
                    && let Some(app) = apps.get(i) {
                        self.popup = Some(Popup { kind: PopupKind::AppMenu(uuid, app.id), position: window.mouse_position(), highlighted: Some(0) });
                        cx.notify();
                    }
            }
            _ => {
                // Type-to-filter
                if let Some(ch) = ks.key_char.as_ref().filter(|c| !c.is_empty() && !ks.modifiers.control && !ks.modifiers.alt)
                    && ch.chars().all(|c| !c.is_control()) {
                        let text = format!("{}{}", self.filter_text, ch);
                        self.set_filter(text, cx);
                    }
            }
        }
    }

    fn dialog_primary(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let _ = window;
        match &self.dialog {
            Some(Dialog::Confirm { action, .. }) => {
                let action = action.clone();
                self.confirm(action, cx);
            }
            Some(Dialog::Error { .. }) => self.close_dialog(cx),
            Some(Dialog::Input { .. }) => self.submit_input_dialog(cx),
            _ => {}
        }
    }

    /// Development aid: `SLOPLIGHT_DEBUG_STATE=settings|settings:N|dropdown:res|dropdown:fps|inspector|pairing|otp|add-host|error|confirm|palette|host-menu|app-menu|toast`.
    pub fn apply_debug_state(&mut self, state: &str, window: &mut Window, cx: &mut Context<Self>) {
        let uuid = self.selected_host.clone().unwrap_or_default();
        match state.split(':').next().unwrap_or("") {
            "settings" => {
                self.view = ViewMode::Settings;
                self.settings_view.sync_from(&self.settings, cx);
                if let Some(n) = state.split(':').nth(1).and_then(|n| n.parse::<usize>().ok()) {
                    self.settings_view.section = n;
                }
            }
            "dropdown" => {
                self.view = ViewMode::Settings;
                self.settings_view.sync_from(&self.settings, cx);
                let kind = match state.split(':').nth(1).unwrap_or("res") {
                    "fps" => crate::views::settings::Dropdown::Fps,
                    _ => crate::views::settings::Dropdown::Resolution,
                };
                self.popup = Some(Popup { kind: PopupKind::Dropdown(kind), position: gpui::point(gpui::px(0.), gpui::px(0.)), highlighted: None });
            }
            "inspector" | "details" | "info" => self.open_host_info(cx),
            "pairing" => {
                let (otp_pin, otp_pass) = self.new_otp_inputs(cx);
                self.pairings.push(InlinePairing { uuid: uuid.clone(), pin: "4817".into(), otp: false, in_progress: true, error: None, otp_pin, otp_pass, reveal: false });
            }
            "otp" => self.open_otp_pairing(&uuid, window, cx),
            "add-host" => self.open_add_host(window, cx),
            "rename" => self.start_rename(&uuid, window, cx),
            "grid" => {
                self.region = Region::Grid;
                self.grid_selected = state.split(':').nth(1).and_then(|n| n.parse().ok()).or(Some(0));
                if state.ends_with(":none") {
                    self.grid_selected = None;
                    self.region = Region::Sidebar;
                }
            }
            "error" => {
                self.dialog = Some(Dialog::Error {
                    title: "Couldn't start Cyberpunk 2077".into(),
                    body: vec!["Starting RTSP handshake failed: Error 61".into(), "Check your firewall and port forwarding rules for port(s): TCP 48010, UDP 48000, UDP 48010".into()],
                    details: "details".into(),
                    permission: false,
                    host_uuid: Some(uuid.clone()),
                })
            }
            "confirm" => self.confirm_quit_app(&uuid, cx),
            "palette" => crate::views::palette::open(self, window, cx),
            "host-menu" => self.popup = Some(Popup { kind: PopupKind::HostMenu(uuid.clone()), position: gpui::point(gpui::px(120.), gpui::px(120.)), highlighted: Some(0) }),
            "app-menu" => {
                let app_id = self.visible_apps().first().map(|a| a.id).unwrap_or(0);
                self.popup = Some(Popup { kind: PopupKind::AppMenu(uuid.clone(), app_id), position: gpui::point(gpui::px(400.), gpui::px(200.)), highlighted: Some(0) });
            }
            "toast" => {
                self.toast(ToastKind::Success, "Paired with desk-pc", cx);
                self.toast(ToastKind::Warning, "Wake-on-LAN packet sent to laptop", cx);
            }
            _ => {}
        }
        cx.notify();
    }

    // ------------------------------------------------------------ actions

    pub fn action_settings(&mut self, _: &OpenSettings, _window: &mut Window, cx: &mut Context<Self>) {
        self.view = if self.view == ViewMode::Settings { ViewMode::Main } else { ViewMode::Settings };
        self.popup = None;
        self.settings_view.sync_from(&self.settings, cx);
        self.focus_main(cx);
        cx.notify();
    }

    pub fn action_add_host(&mut self, _: &AddHost, window: &mut Window, cx: &mut Context<Self>) {
        if self.view == ViewMode::Main && self.dialog.is_none() {
            self.open_add_host(window, cx);
        }
    }

    pub fn action_refresh(&mut self, _: &Refresh, _window: &mut Window, cx: &mut Context<Self>) {
        if self.polling_paused {
            self.polling_paused = false;
            self.manager.start_polling();
        }
        self.manager.refresh();
        cx.notify();
    }

    pub fn action_focus_filter(&mut self, _: &FocusFilter, window: &mut Window, cx: &mut Context<Self>) {
        let handle = match self.view {
            ViewMode::Main => self.filter.read(cx).focus_handle.clone(),
            ViewMode::Settings => self.settings_view.filter.read(cx).focus_handle.clone(),
        };
        window.focus(&handle);
        cx.notify();
    }

    /// Anchor for popups opened from the host title button (under its left edge).
    pub fn host_title_anchor(&self) -> gpui::Point<gpui::Pixels> {
        let pad = crate::theme::layout(self.last_content_w, self.last_body_h).pad;
        gpui::point(gpui::px(crate::theme::SIDEBAR_W + pad - 6.), gpui::px(crate::theme::HEADER_H))
    }

    /// Opens the ⌘I info popover for the selected host.
    pub fn open_host_info(&mut self, cx: &mut Context<Self>) {
        if self.view != ViewMode::Main {
            return;
        }
        let Some(uuid) = self.selected_host.clone() else { return };
        self.details_more = false;
        self.popup = Some(Popup { kind: PopupKind::HostInfo(uuid), position: self.host_title_anchor(), highlighted: None });
        cx.notify();
    }

    pub fn action_toggle_host_info(&mut self, _: &ToggleInspector, _window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.popup, Some(Popup { kind: PopupKind::HostInfo(_), .. })) {
            self.popup = None;
            cx.notify();
        } else {
            self.open_host_info(cx);
        }
    }

    pub fn action_copy_host_address(&mut self, _: &CopyHostAddress, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(host) = self.selected().cloned() else { return };
        if let Some(a) = host.active_address.clone().or_else(|| host.unique_addresses().into_iter().next()) {
            self.copy_to_clipboard("address", &a.host, cx);
        }
    }

    pub fn action_toggle_hidden(&mut self, _: &ToggleHiddenApps, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(host) = self.selected().cloned() else { return };
        if !host.apps.iter().any(|a| a.hidden) {
            return;
        }
        let show = !host.show_hidden_apps;
        self.manager.set_show_hidden_apps(&host.uuid, show);
        if !show {
            // Collapsing with a hidden tile selected: move to the last visible tile.
            let visible = self.visible_count();
            if let Some(i) = self.grid_selected
                && i >= visible {
                    self.grid_selected = if visible == 0 { None } else { Some(visible - 1) };
                }
        }
        cx.notify();
    }

    /// Hides or unhides an app; unhiding the last hidden app collapses the section.
    pub fn set_app_hidden(&mut self, uuid: &str, app_id: i32, hidden: bool) {
        self.manager.set_app_hidden(uuid, app_id, hidden);
        if !hidden
            && let Some(host) = self.host_by_uuid(uuid)
                && !host.apps.iter().any(|a| a.hidden && a.id != app_id) {
                    self.manager.set_show_hidden_apps(uuid, false);
                }
    }

    pub fn action_back(&mut self, _: &Back, _window: &mut Window, cx: &mut Context<Self>) {
        if self.popup.is_some() {
            self.popup = None;
        } else if self.palette.is_some() {
            self.palette = None;
        } else if self.dialog.is_some() {
            self.close_dialog(cx);
        } else if self.view == ViewMode::Settings {
            if self.settings_view.filter.read(cx).is_empty() {
                self.view = ViewMode::Main;
                self.focus_main(cx);
            } else {
                self.settings_view.filter.update(cx, |f, cx| f.clear(cx));
            }
        }
        cx.notify();
    }

    pub fn action_quit(&mut self, _: &Quit, _window: &mut Window, cx: &mut Context<Self>) {
        if self.stream.is_some() {
            self.dialog = Some(Dialog::Confirm {
                title: "Quit Moonlight?".into(),
                body: "The stream will end.".into(),
                primary: "Quit".into(),
                danger: true,
                action: ConfirmAction::QuitWhileStreaming,
            });
            cx.notify();
        } else {
            cx.quit();
        }
    }

    pub fn action_quick_launch(&mut self, _: &QuickLaunch, window: &mut Window, cx: &mut Context<Self>) {
        crate::views::palette::open(self, window, cx);
    }

}

/// Simple subsequence match, case-insensitive, with a word-prefix bonus.
pub fn fuzzy_match(haystack: &str, needle: &str) -> bool {
    fuzzy_score(haystack, needle).is_some()
}

pub fn fuzzy_score(haystack: &str, needle: &str) -> Option<i32> {
    let h: Vec<char> = haystack.to_lowercase().chars().collect();
    let n: Vec<char> = needle.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    if n.is_empty() {
        return Some(0);
    }
    let mut score = 0;
    let mut hi = 0;
    let mut prev_match: Option<usize> = None;
    for &c in &n {
        let mut found = None;
        while hi < h.len() {
            if h[hi] == c {
                found = Some(hi);
                hi += 1;
                break;
            }
            hi += 1;
        }
        let pos = found?;
        score += 10;
        if pos == 0 || !h[pos - 1].is_alphanumeric() {
            score += 15;
        }
        if let Some(p) = prev_match
            && p + 1 == pos {
                score += 8;
            }
        prev_match = Some(pos);
    }
    if haystack.to_lowercase().starts_with(&needle.to_lowercase()) {
        score += 30;
    }
    Some(score)
}

impl Focusable for Root {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
