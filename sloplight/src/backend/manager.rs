//! Host bookkeeping: persistence, polling threads, mDNS, pairing, quitting.
//! Port of `ComputerManager`, with std threads and a futures channel towards
//! the UI instead of Qt signals.
#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::thread;
use std::time::Duration;

use futures::channel::mpsc::UnboundedSender;

use super::host::{Address, App, Host, HostState, PairState};
use super::http::{CancelToken, HttpError, NvHttp};
use super::identity::{Identity, der_to_pem};
use super::mdns::{Discovered, MdnsBrowser};
use super::pairing::{self, PairOutcome};
use super::wol;

const TRIES_BEFORE_OFFLINING: usize = 2;
const POLLS_PER_APPLIST_FETCH: u32 = 10;
const POLL_INTERVAL: Duration = Duration::from_secs(3);
/// Error text reported when the user cancelled pairing.
pub const CANCELLED: &str = "Pairing cancelled.";

#[derive(Debug, Clone)]
pub enum ManagerEvent {
    /// Some host changed state; the UI should re-read the snapshot.
    HostsChanged,
    HostAdded { uuid: String },
    HostRemoved { uuid: String },
    AddCompleted { address: String, result: Result<String, AddError> },
    PairingCompleted { uuid: String, error: Option<String> },
    QuitCompleted { uuid: String, error: Option<String> },
    BoxArtLoaded { uuid: String, app_id: i32 },
}

#[derive(Debug, Clone)]
pub struct AddError {
    pub message: String,
    pub port_blocked: bool,
    /// Platform-specific fix, e.g. the macOS Local Network permission.
    pub resolution: Option<&'static str>,
}

impl AddError {
    fn from_http(e: &HttpError) -> Self {
        AddError { message: e.to_string(), port_blocked: false, resolution: e.resolution() }
    }
}

struct PollHandle {
    stop: Arc<(Mutex<PollSignal>, Condvar)>,
    join: Option<thread::JoinHandle<()>>,
}

#[derive(Default)]
struct PollSignal {
    stop: bool,
    poll_now: bool,
}

struct Inner {
    hosts: BTreeMap<String, Arc<RwLock<Host>>>,
    pollers: HashMap<String, PollHandle>,
    mdns: Option<MdnsBrowser>,
    polling: bool,
    box_art_in_flight: HashSet<(String, i32)>,
    pairing_tokens: HashMap<String, Arc<CancelToken>>,
}

pub struct HostManager {
    inner: Mutex<Inner>,
    pub identity: Arc<Identity>,
    events: UnboundedSender<ManagerEvent>,
    data_dir: PathBuf,
    cache_dir: PathBuf,
    enable_mdns: AtomicBool,
    self_ref: Mutex<Option<std::sync::Weak<HostManager>>>,
}

impl HostManager {
    pub fn new(data_dir: PathBuf, cache_dir: PathBuf, identity: Arc<Identity>, events: UnboundedSender<ManagerEvent>, enable_mdns: bool) -> Arc<HostManager> {
        let mut hosts = BTreeMap::new();
        let path = data_dir.join("hosts.json");
        match fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<Vec<Host>>(&bytes) {
                Ok(list) => {
                    for mut h in list {
                        if h.uuid.is_empty() {
                            continue;
                        }
                        h.external_port = h.remote_address.as_ref().map(|a| a.port).unwrap_or(0);
                        h.is_supported_server_version = true;
                        let mut apps = std::mem::take(&mut h.apps);
                        apps.sort_by_cached_key(|a| a.name.to_lowercase());
                        h.apps = apps;
                        hosts.insert(h.uuid.clone(), Arc::new(RwLock::new(h)));
                    }
                }
                Err(e) => log::error!("hosts.json is unreadable: {e}"),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => log::error!("Failed to read hosts.json: {e}"),
        }
        log::info!("Loaded {} hosts", hosts.len());
        let manager = Arc::new(HostManager {
            inner: Mutex::new(Inner { hosts, pollers: HashMap::new(), mdns: None, polling: false, box_art_in_flight: HashSet::new(), pairing_tokens: HashMap::new() }),
            identity,
            events,
            data_dir,
            cache_dir,
            enable_mdns: AtomicBool::new(enable_mdns),
            self_ref: Mutex::new(None),
        });
        *manager.self_ref.lock().unwrap() = Some(Arc::downgrade(&manager));
        manager
    }

    fn me(&self) -> Arc<HostManager> {
        self.self_ref.lock().unwrap().as_ref().and_then(|w| w.upgrade()).expect("manager alive")
    }

    fn emit(&self, event: ManagerEvent) {
        let _ = self.events.unbounded_send(event);
    }

    pub fn set_mdns_enabled(&self, enabled: bool) {
        if self.enable_mdns.swap(enabled, Ordering::Relaxed) == enabled {
            return;
        }
        let polling = self.inner.lock().unwrap().polling;
        if !polling {
            return;
        }
        if enabled {
            let me = self.me();
            match MdnsBrowser::start(move |d: Discovered| {
                me.add_host(d.address, true, Some(d.name), d.ipv6);
            }) {
                Ok(browser) => self.inner.lock().unwrap().mdns = Some(browser),
                Err(e) => log::error!("mDNS browsing failed to start: {e}"),
            }
        } else {
            let browser = self.inner.lock().unwrap().mdns.take();
            drop(browser);
        }
    }

    pub fn mdns_enabled(&self) -> bool {
        self.enable_mdns.load(Ordering::Relaxed)
    }

    pub fn is_polling(&self) -> bool {
        self.inner.lock().unwrap().polling
    }

    // ---------------------------------------------------------------- snapshot

    /// Clones of all hosts, sorted by name.
    pub fn snapshot(&self) -> Vec<Host> {
        let inner = self.inner.lock().unwrap();
        let mut list: Vec<Host> = inner.hosts.values().map(|h| h.read().unwrap().clone()).collect();
        list.sort_by_cached_key(|h| h.name.to_lowercase());
        list
    }

    pub fn host(&self, uuid: &str) -> Option<Host> {
        let inner = self.inner.lock().unwrap();
        inner.hosts.get(uuid).map(|h| h.read().unwrap().clone())
    }

    fn host_arc(&self, uuid: &str) -> Option<Arc<RwLock<Host>>> {
        self.inner.lock().unwrap().hosts.get(uuid).cloned()
    }

    // ------------------------------------------------------------- persistence

    fn save_hosts(&self) {
        let list: Vec<Host> = {
            let inner = self.inner.lock().unwrap();
            inner.hosts.values().map(|h| h.read().unwrap().clone()).collect()
        };
        if let Err(e) = fs::create_dir_all(&self.data_dir) {
            log::error!("create data dir: {e}");
            return;
        }
        let path = self.data_dir.join("hosts.json");
        let tmp = self.data_dir.join("hosts.json.tmp");
        match serde_json::to_vec_pretty(&list) {
            Ok(bytes) => {
                if let Err(e) = fs::write(&tmp, bytes).and_then(|_| fs::rename(&tmp, &path)) {
                    log::error!("Failed to save hosts: {e}");
                }
            }
            Err(e) => log::error!("Failed to serialize hosts: {e}"),
        }
    }

    // ----------------------------------------------------------------- polling

    pub fn start_polling(&self) {
        {
            let mut inner = self.inner.lock().unwrap();
            if inner.polling {
                return;
            }
            inner.polling = true;
            let uuids: Vec<String> = inner.hosts.keys().cloned().collect();
            for uuid in uuids {
                Self::start_poller(&self.me(), &mut inner, &uuid);
            }
        }
        if self.enable_mdns.load(Ordering::Relaxed) {
            let me = self.me();
            match MdnsBrowser::start(move |d: Discovered| {
                me.add_host(d.address, true, Some(d.name), d.ipv6);
            }) {
                Ok(browser) => self.inner.lock().unwrap().mdns = Some(browser),
                Err(e) => log::error!("mDNS browsing failed to start: {e}"),
            }
        }
        self.emit(ManagerEvent::HostsChanged);
    }

    pub fn stop_polling(&self) {
        let (mdns, pollers) = {
            let mut inner = self.inner.lock().unwrap();
            if !inner.polling {
                return;
            }
            inner.polling = false;
            (inner.mdns.take(), std::mem::take(&mut inner.pollers))
        };
        drop(mdns);
        for (_, mut p) in pollers {
            {
                let (lock, cv) = &*p.stop;
                lock.lock().unwrap().stop = true;
                cv.notify_all();
            }
            // Don't block the UI thread on in-flight requests
            if let Some(join) = p.join.take() {
                thread::spawn(move || {
                    let _ = join.join();
                });
            }
        }
        self.emit(ManagerEvent::HostsChanged);
    }

    /// Re-polls all hosts right away.
    pub fn refresh(&self) {
        let inner = self.inner.lock().unwrap();
        for p in inner.pollers.values() {
            let (lock, cv) = &*p.stop;
            lock.lock().unwrap().poll_now = true;
            cv.notify_all();
        }
    }

    fn start_poller(me: &Arc<HostManager>, inner: &mut Inner, uuid: &str) {
        if inner.pollers.contains_key(uuid) {
            return;
        }
        let Some(host) = inner.hosts.get(uuid).cloned() else { return };
        let stop = Arc::new((Mutex::new(PollSignal::default()), Condvar::new()));
        let stop2 = stop.clone();
        let me2 = me.clone();
        let name = host.read().unwrap().name.clone();
        let join = thread::Builder::new()
            .name(format!("poll {name}"))
            .spawn(move || me2.poll_loop(host, stop2))
            .ok();
        inner.pollers.insert(uuid.to_string(), PollHandle { stop, join });
    }

    fn poll_loop(&self, host: Arc<RwLock<Host>>, signal: Arc<(Mutex<PollSignal>, Condvar)>) {
        let mut polls_since_applist = POLLS_PER_APPLIST_FETCH;
        let should_stop = || signal.0.lock().unwrap().stop;
        loop {
            if should_stop() {
                return;
            }
            let mut changed = false;
            let (was_online, addresses, server_cert, is_gfe, uuid) = {
                let h = host.read().unwrap();
                (h.state == HostState::Online, h.unique_addresses(), h.server_cert(), h.is_nvidia_server_software, h.uuid.clone())
            };
            let mut online = false;
            let mut all_unreachable = !addresses.is_empty();
            let tries = if was_online { TRIES_BEFORE_OFFLINING } else { 1 };
            'outer: for _ in 0..tries {
                for address in &addresses {
                    if should_stop() {
                        return;
                    }
                    let mut http = NvHttp::new(address.clone(), 0, server_cert.clone(), !is_gfe, self.identity.clone());
                    let info = match http.get_server_info(true) {
                        Ok(info) => info,
                        Err(e) => {
                            all_unreachable &= matches!(e, HttpError::HostUnreachable);
                            continue;
                        }
                    };
                    let fresh = Host::from_server_info(&http, &info);
                    // Something answered, so the route works even if it's the wrong host.
                    all_unreachable = false;
                    if fresh.uuid != uuid {
                        log::info!("Found unexpected host {} looking for {}", fresh.name, uuid);
                        continue;
                    }
                    let mut h = host.write().unwrap();
                    if h.update(&fresh) {
                        changed = true;
                    }
                    if !was_online {
                        log::info!("{} is now online at {}", h.name, address);
                    }
                    online = true;
                    break 'outer;
                }
            }
            if !online {
                let mut h = host.write().unwrap();
                if h.state != HostState::Offline {
                    log::info!("{} is now offline", h.name);
                    h.state = HostState::Offline;
                    changed = true;
                }
                if h.unreachable != all_unreachable {
                    h.unreachable = all_unreachable;
                    changed = true;
                }
            } else {
                let mut h = host.write().unwrap();
                if h.unreachable {
                    h.unreachable = false;
                    changed = true;
                }
            }

            polls_since_applist += 1;
            let (online_paired, apps_empty) = {
                let h = host.read().unwrap();
                (h.state == HostState::Online && h.pair_state == PairState::Paired, h.apps.is_empty())
            };
            if online_paired && (apps_empty || polls_since_applist >= POLLS_PER_APPLIST_FETCH || host.read().unwrap().app_list_error.is_some()) {
                if changed {
                    self.emit(ManagerEvent::HostsChanged);
                    changed = false;
                }
                let http = host.read().unwrap().http(self.identity.clone());
                if let Some(http) = http {
                    match http.get_app_list() {
                        Ok(apps) if !apps.is_empty() => {
                            let mut h = host.write().unwrap();
                            if h.update_app_list(apps) {
                                changed = true;
                            }
                            if h.app_list_error.take().is_some() {
                                changed = true;
                            }
                            polls_since_applist = 0;
                        }
                        Ok(_) => {}
                        Err(HttpError::Status { code, message }) => {
                            let mut h = host.write().unwrap();
                            let err = Some((code, message));
                            if h.app_list_error != err {
                                h.app_list_error = err;
                                changed = true;
                            }
                            polls_since_applist = 0;
                        }
                        Err(e) => log::debug!("applist failed: {e}"),
                    }
                }
            }

            if changed {
                self.save_hosts();
                self.emit(ManagerEvent::HostsChanged);
            }

            // Sleep, interruptible
            let (lock, cv) = &*signal;
            let mut guard = lock.lock().unwrap();
            let deadline = std::time::Instant::now() + POLL_INTERVAL;
            while !guard.stop && !guard.poll_now {
                let now = std::time::Instant::now();
                if now >= deadline {
                    break;
                }
                let (g, _) = cv.wait_timeout(guard, deadline - now).unwrap();
                guard = g;
            }
            if guard.poll_now {
                guard.poll_now = false;
                polls_since_applist = POLLS_PER_APPLIST_FETCH;
            }
            if guard.stop {
                return;
            }
        }
    }

    // -------------------------------------------------------------- add hosts

    /// Adds a host from user input. Completion is reported via `AddCompleted`.
    pub fn add_host_manually(&self, input: &str) {
        match Address::parse_user_input(input) {
            Some(address) => self.add_host(address, false, None, None),
            None => self.emit(ManagerEvent::AddCompleted {
                address: input.to_string(),
                result: Err(AddError { message: "That doesn't look like a valid address.".into(), port_blocked: false, resolution: None }),
            }),
        }
    }

    pub fn add_host(&self, address: Address, mdns: bool, name: Option<String>, mdns_ipv6: Option<Address>) {
        let me = self.me();
        thread::Builder::new()
            .name("add host".into())
            .spawn(move || me.add_host_task(address, mdns, name, mdns_ipv6))
            .ok();
    }

    fn fetch_server_info(http: &mut NvHttp) -> Result<String, HttpError> {
        match http.get_server_info(false) {
            Err(e) if e.is_service_unavailable() => {
                // mDNS can announce before the HTTPS server is ready
                log::warn!("Retrying serverinfo in 5 seconds after 503");
                thread::sleep(Duration::from_secs(5));
                http.get_server_info(false)
            }
            r => r,
        }
    }

    fn add_host_task(&self, address: Address, mdns: bool, name: Option<String>, mdns_ipv6: Option<Address>) {
        let input = address.to_string();
        log::info!("Processing new host at {address} ({})", if mdns { "mDNS" } else { "manual" });
        let mut http = NvHttp::new(address.clone(), 0, None, false, self.identity.clone());
        let mut info = Self::fetch_server_info(&mut http);
        if info.is_err()
            && let Some(v6) = &mdns_ipv6 {
                http.set_address(v6.clone());
                info = Self::fetch_server_info(&mut http);
            }
        let info = match info {
            Ok(i) => i,
            Err(e) => {
                if !mdns {
                    self.emit(ManagerEvent::AddCompleted { address: input, result: Err(AddError::from_http(&e)) });
                } else {
                    log::info!("mDNS host {} unreachable: {e}", name.unwrap_or_default());
                }
                return;
            }
        };

        let mut new_host = Host::from_server_info(&http, &info);
        http.set_true_uid(!new_host.is_nvidia_server_software);

        // Pull the pinned cert if we know this UUID already and re-fetch over HTTPS
        let existing = self.host_arc(&new_host.uuid);
        if let Some(existing) = &existing {
            let cert = existing.read().unwrap().server_cert();
            if cert.is_some() {
                http.set_server_cert(cert);
                http.set_https_port(new_host.active_https_port);
                match Self::fetch_server_info(&mut http) {
                    Ok(info) => {
                        let https_host = Host::from_server_info(&http, &info);
                        new_host.update(&https_host);
                    }
                    Err(e) => {
                        if !mdns {
                            self.emit(ManagerEvent::AddCompleted { address: input, result: Err(AddError::from_http(&e)) });
                        }
                        return;
                    }
                }
            }
        }

        if mdns {
            if http.address() == &address {
                new_host.local_address = Some(address.clone());
            }
            if let Some(v6) = mdns_ipv6 {
                new_host.ipv6_address = Some(v6);
            }
        } else {
            new_host.manual_address = Some(address.clone());
        }

        let uuid = new_host.uuid.clone();
        let display_name = new_host.name.clone();
        let changed;
        let added;
        {
            let mut inner = self.inner.lock().unwrap();
            if let Some(existing) = inner.hosts.get(&uuid).cloned() {
                changed = existing.write().unwrap().update(&new_host);
                added = false;
            } else {
                inner.hosts.insert(uuid.clone(), Arc::new(RwLock::new(new_host)));
                if inner.polling {
                    Self::start_poller(&self.me(), &mut inner, &uuid);
                }
                changed = true;
                added = true;
            }
        }
        if changed {
            self.save_hosts();
        }
        if added {
            self.emit(ManagerEvent::HostAdded { uuid: uuid.clone() });
        }
        if changed {
            self.emit(ManagerEvent::HostsChanged);
        }
        if !mdns {
            self.emit(ManagerEvent::AddCompleted { address: input, result: Ok(uuid) });
        }
        let _ = display_name;
    }

    // ------------------------------------------------------------------ pairing

    pub fn generate_pin() -> String {
        pairing::generate_pin()
    }

    /// Standard PIN pairing (client-generated PIN). Completion via `PairingCompleted`.
    pub fn pair_host(&self, uuid: &str, pin: String) {
        self.start_pairing(uuid, pin, None);
    }

    /// Apollo one-time-password pairing (host-generated PIN plus optional passphrase).
    pub fn pair_host_with_otp(&self, uuid: &str, pin: String, passphrase: String) {
        self.start_pairing(uuid, pin, Some(passphrase));
    }

    fn start_pairing(&self, uuid: &str, pin: String, otp_passphrase: Option<String>) {
        let Some(host) = self.host_arc(uuid) else { return };
        {
            let mut h = host.write().unwrap();
            if h.pairing_in_progress {
                self.emit(ManagerEvent::PairingCompleted { uuid: uuid.to_string(), error: Some("Another pairing attempt is already in progress.".into()) });
                return;
            }
            h.pairing_in_progress = true;
        }
        let token = Arc::new(CancelToken::default());
        self.inner.lock().unwrap().pairing_tokens.insert(uuid.to_string(), token.clone());
        self.emit(ManagerEvent::HostsChanged);
        let me = self.me();
        let uuid = uuid.to_string();
        thread::Builder::new()
            .name("pairing".into())
            .spawn(move || {
                let (mut http, app_version, is_gfe, current_game) = {
                    let h = host.read().unwrap();
                    (h.http(me.identity.clone()), h.app_version.clone(), h.is_nvidia_server_software, h.current_game_id)
                };
                let Some(http) = http.as_mut() else {
                    host.write().unwrap().pairing_in_progress = false;
                    me.inner.lock().unwrap().pairing_tokens.remove(&uuid);
                    me.emit(ManagerEvent::PairingCompleted { uuid, error: Some("The host has no known address.".into()) });
                    return;
                };
                http.set_cancel_token(token.clone());
                let is_otp = otp_passphrase.is_some();
                let result = pairing::pair(http, &me.identity, &app_version, is_gfe, &pin, otp_passphrase.as_deref());
                let error = match result {
                    Ok(PairOutcome::Paired(cert)) => {
                        let mut h = host.write().unwrap();
                        h.server_cert_pem = Some(der_to_pem(cert.as_ref()));
                        h.pair_state = PairState::Paired;
                        None
                    }
                    Ok(PairOutcome::PinWrong) => Some(if is_otp {
                        "The PIN or passphrase didn't match the one-time PIN on the host. Try again.".to_string()
                    } else {
                        "The PIN didn't match. Try again.".to_string()
                    }),
                    Ok(PairOutcome::Failed) => Some(if current_game != 0 {
                        "A session is still running on the host. Quit any running games or restart the host, then pair again.".to_string()
                    } else {
                        "Pairing failed. Try again.".to_string()
                    }),
                    Ok(PairOutcome::AlreadyInProgress) => Some("Another pairing attempt is already in progress.".to_string()),
                    Err(HttpError::Status { code: 503, .. }) if is_otp => {
                        Some("No one-time PIN is active on the host. Generate a new one in Apollo's web UI and try again.".to_string())
                    }
                    Err(HttpError::Cancelled) => Some(CANCELLED.to_string()),
                    Err(e @ HttpError::Status { .. }) => Some(if is_gfe {
                        format!("GeForce Experience returned an error: {e}")
                    } else {
                        format!("The host returned an error: {e}")
                    }),
                    Err(e) => Some(e.to_string()),
                };
                host.write().unwrap().pairing_in_progress = false;
                me.inner.lock().unwrap().pairing_tokens.remove(&uuid);
                if error.is_none() {
                    me.save_hosts();
                    me.refresh();
                }
                me.emit(ManagerEvent::HostsChanged);
                me.emit(ManagerEvent::PairingCompleted { uuid, error });
            })
            .ok();
    }

    /// Aborts a pairing attempt started with `pair_host`/`pair_host_with_otp`.
    pub fn cancel_pairing(&self, uuid: &str) {
        let token = self.inner.lock().unwrap().pairing_tokens.get(uuid).cloned();
        if let Some(t) = token {
            t.cancel();
        }
    }

    // --------------------------------------------------------------- quit app

    pub fn quit_running_app(&self, uuid: &str) {
        let Some(host) = self.host_arc(uuid) else { return };
        host.write().unwrap().pending_quit = true;
        self.emit(ManagerEvent::HostsChanged);
        let me = self.me();
        let uuid = uuid.to_string();
        thread::Builder::new()
            .name("quit app".into())
            .spawn(move || {
                let (http, current_game) = {
                    let h = host.read().unwrap();
                    (h.http(me.identity.clone()), h.current_game_id)
                };
                let mut error = None;
                if current_game != 0 {
                    match http {
                        Some(mut http) => {
                            if let Err(e) = http.quit_app() {
                                error = Some(match e {
                                    HttpError::Status { code: 599, .. } => {
                                        "This app wasn't started by this PC. Quit it on the host or from the device that started it.".to_string()
                                    }
                                    e => e.to_string(),
                                });
                            }
                        }
                        None => error = Some("The host has no known address.".into()),
                    }
                }
                {
                    let mut h = host.write().unwrap();
                    h.pending_quit = false;
                    if error.is_none() {
                        h.current_game_id = 0;
                    }
                }
                me.refresh();
                me.emit(ManagerEvent::HostsChanged);
                me.emit(ManagerEvent::QuitCompleted { uuid, error });
            })
            .ok();
    }

    // -------------------------------------------------------------- mutations

    pub fn delete_host(&self, uuid: &str) {
        let poller = {
            let mut inner = self.inner.lock().unwrap();
            inner.hosts.remove(uuid);
            inner.pollers.remove(uuid)
        };
        if let Some(mut p) = poller {
            let (lock, cv) = &*p.stop;
            lock.lock().unwrap().stop = true;
            cv.notify_all();
            if let Some(join) = p.join.take() {
                thread::spawn(move || {
                    let _ = join.join();
                });
            }
        }
        let _ = fs::remove_dir_all(self.box_art_dir(uuid));
        self.save_hosts();
        self.emit(ManagerEvent::HostRemoved { uuid: uuid.to_string() });
        self.emit(ManagerEvent::HostsChanged);
    }

    pub fn rename_host(&self, uuid: &str, name: String) {
        if let Some(host) = self.host_arc(uuid) {
            let mut h = host.write().unwrap();
            h.name = name;
            h.has_custom_name = true;
        }
        self.save_hosts();
        self.emit(ManagerEvent::HostsChanged);
    }

    pub fn set_app_hidden(&self, uuid: &str, app_id: i32, hidden: bool) {
        if let Some(host) = self.host_arc(uuid) {
            let mut h = host.write().unwrap();
            if let Some(app) = h.apps.iter_mut().find(|a| a.id == app_id) {
                app.hidden = hidden;
            }
        }
        self.save_hosts();
        self.emit(ManagerEvent::HostsChanged);
    }

    pub fn set_app_direct_launch(&self, uuid: &str, app_id: i32, direct: bool) {
        if let Some(host) = self.host_arc(uuid) {
            let mut h = host.write().unwrap();
            for app in h.apps.iter_mut() {
                if direct {
                    app.direct_launch = app.id == app_id;
                } else if app.id == app_id {
                    app.direct_launch = false;
                }
            }
        }
        self.save_hosts();
        self.emit(ManagerEvent::HostsChanged);
    }

    pub fn set_show_hidden_apps(&self, uuid: &str, show: bool) {
        if let Some(host) = self.host_arc(uuid) {
            host.write().unwrap().show_hidden_apps = show;
        }
        self.save_hosts();
        self.emit(ManagerEvent::HostsChanged);
    }

    /// Sends Wake-on-LAN packets. Returns an error message on failure.
    pub fn wake_host(&self, uuid: &str) -> Result<(), String> {
        let Some(host) = self.host(uuid) else { return Err("Unknown host".into()) };
        if host.state == HostState::Online {
            return Err(format!("{} is already online", host.name));
        }
        wol::wake(&host)
    }

    /// Marks a host as the one we are currently streaming from (UI hint only).
    pub fn note_app_running(&self, uuid: &str, app_id: i32) {
        if let Some(host) = self.host_arc(uuid) {
            host.write().unwrap().current_game_id = app_id;
        }
        self.emit(ManagerEvent::HostsChanged);
    }

    // ---------------------------------------------------------------- box art

    fn box_art_dir(&self, uuid: &str) -> PathBuf {
        self.cache_dir.join("boxart").join(uuid)
    }

    /// Path of the cached box art if present on disk.
    pub fn box_art_path(&self, uuid: &str, app_id: i32) -> Option<PathBuf> {
        let path = self.box_art_dir(uuid).join(format!("{app_id}.img"));
        path.is_file().then_some(path)
    }

    /// Fetches box art in the background when not cached; emits `BoxArtLoaded`.
    pub fn request_box_art(&self, uuid: &str, app: &App) {
        let path = self.box_art_dir(uuid).join(format!("{}.img", app.id));
        if path.exists() {
            return;
        }
        let Some(host) = self.host_arc(uuid) else { return };
        {
            let mut inner = self.inner.lock().unwrap();
            if !inner.box_art_in_flight.insert((uuid.to_string(), app.id)) {
                return;
            }
        }
        let me = self.me();
        let uuid = uuid.to_string();
        let app_id = app.id;
        thread::Builder::new()
            .name("boxart".into())
            .spawn(move || {
                let http = host.read().unwrap().http(me.identity.clone());
                if let Some(http) = http {
                    match http.get_box_art(app_id) {
                        Ok(bytes) if !bytes.is_empty() && looks_like_image(&bytes) => {
                            if let Some(parent) = path.parent() {
                                let _ = fs::create_dir_all(parent);
                            }
                            if fs::write(&path, &bytes).is_ok() {
                                me.emit(ManagerEvent::BoxArtLoaded { uuid: uuid.clone(), app_id });
                            }
                        }
                        Ok(_) => {
                            // Remember that there is no art so we don't retry every render
                            if let Some(parent) = path.parent() {
                                let _ = fs::create_dir_all(parent);
                            }
                            let _ = fs::write(path.with_extension("none"), b"");
                        }
                        Err(e) => log::debug!("box art for app {app_id} failed: {e}"),
                    }
                }
                me.inner.lock().unwrap().box_art_in_flight.remove(&(uuid, app_id));
            })
            .ok();
    }

    pub fn box_art_known_missing(&self, uuid: &str, app_id: i32) -> bool {
        self.box_art_dir(uuid).join(format!("{app_id}.none")).exists()
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }
}

fn looks_like_image(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0x89, b'P', b'N', b'G']) || bytes.starts_with(&[0xFF, 0xD8]) || bytes.starts_with(b"RIFF") || bytes.starts_with(b"GIF8") || bytes.starts_with(b"BM")
}
