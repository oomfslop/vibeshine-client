//! Host (`NvComputer`) and app (`NvApp`) models plus serverinfo/applist parsing.
#![allow(dead_code)]

use std::fmt;
use std::net::{IpAddr, Ipv4Addr};

use rustls_pki_types::CertificateDer;
use serde::{Deserialize, Serialize};

use super::http::{DEFAULT_HTTP_PORT, DEFAULT_HTTPS_PORT, NvHttp};
use super::identity::{der_to_pem, pem_to_der};
use super::xml;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Address {
    pub host: String,
    pub port: u16,
}

impl Address {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Address { host: host.into(), port }
    }

    /// Parses user input: `host`, `host:port`, `[v6]:port`, bare IPv6 literal.
    pub fn parse_user_input(input: &str) -> Option<Address> {
        let s = input.trim();
        if s.is_empty() {
            return None;
        }
        let s = s.strip_prefix("moonlight://").unwrap_or(s).trim_end_matches('/');
        if let Some(rest) = s.strip_prefix('[') {
            let (host, tail) = rest.split_once(']')?;
            let port = match tail.strip_prefix(':') {
                Some(p) if !p.is_empty() => p.parse().ok()?,
                _ => DEFAULT_HTTP_PORT,
            };
            return host.parse::<IpAddr>().ok().map(|ip| Address::new(ip.to_string(), port));
        }
        if s.parse::<IpAddr>().is_ok() {
            // IPv4 or IPv6 literal without a port
            return Some(Address::new(s, DEFAULT_HTTP_PORT));
        }
        if let Some((host, port)) = s.rsplit_once(':')
            && !host.is_empty() && !host.contains(':') {
                return port.parse::<u16>().ok().map(|p| Address::new(host, p));
            }
        if s.contains(char::is_whitespace) || s.contains('/') {
            return None;
        }
        Some(Address::new(s, DEFAULT_HTTP_PORT))
    }

    pub fn ip(&self) -> Option<IpAddr> {
        self.host.parse().ok()
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.host.contains(':') {
            write!(f, "[{}]:{}", self.host, self.port)
        } else {
            write!(f, "{}:{}", self.host, self.port)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct App {
    pub id: i32,
    pub name: String,
    /// Apollo app UUID (empty for other hosts)
    #[serde(default)]
    pub uuid: String,
    #[serde(default)]
    pub hdr_supported: bool,
    #[serde(default)]
    pub is_app_collector_game: bool,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub direct_launch: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayMode {
    pub width: i32,
    pub height: i32,
    pub refresh_rate: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum HostState {
    #[default]
    Unknown,
    Online,
    Offline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PairState {
    #[default]
    Unknown,
    Paired,
    NotPaired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Software {
    Apollo,
    Sunshine,
    GeForceExperience,
}

impl Software {
    pub fn label(self) -> &'static str {
        match self {
            Software::Apollo => "Apollo",
            Software::Sunshine => "Sunshine",
            Software::GeForceExperience => "GeForce Experience",
        }
    }
}

/// Server codec mode support bits (SCM_* in Limelight.h).
pub mod scm {
    pub const H264: i32 = 0x0001;
    pub const H264_HIGH8_444: i32 = 0x0002;
    pub const HEVC: i32 = 0x0100;
    pub const HEVC_MAIN10: i32 = 0x0200;
    pub const HEVC_REXT8_444: i32 = 0x0400;
    pub const HEVC_REXT10_444: i32 = 0x0800;
    pub const AV1_MAIN8: i32 = 0x1_0000;
    pub const AV1_MAIN10: i32 = 0x2_0000;
    pub const AV1_HIGH8_444: i32 = 0x4_0000;
    pub const AV1_HIGH10_444: i32 = 0x8_0000;
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Host {
    // ---- persisted ----
    pub name: String,
    #[serde(default)]
    pub has_custom_name: bool,
    pub uuid: String,
    #[serde(default)]
    pub mac: Vec<u8>,
    #[serde(default)]
    pub local_address: Option<Address>,
    #[serde(default)]
    pub remote_address: Option<Address>,
    #[serde(default)]
    pub ipv6_address: Option<Address>,
    #[serde(default)]
    pub manual_address: Option<Address>,
    #[serde(default)]
    pub server_cert_pem: Option<String>,
    #[serde(default)]
    pub is_nvidia_server_software: bool,
    #[serde(default)]
    pub apps: Vec<App>,
    /// Client-side: show hidden apps for this host
    #[serde(default)]
    pub show_hidden_apps: bool,

    // ---- ephemeral ----
    #[serde(skip)]
    pub state: HostState,
    #[serde(skip)]
    pub pair_state: PairState,
    #[serde(skip)]
    pub active_address: Option<Address>,
    #[serde(skip)]
    pub active_https_port: u16,
    #[serde(skip)]
    pub current_game_id: i32,
    #[serde(skip)]
    pub gfe_version: String,
    #[serde(skip)]
    pub app_version: String,
    #[serde(skip)]
    pub display_modes: Vec<DisplayMode>,
    #[serde(skip)]
    pub max_luma_pixels_hevc: i32,
    #[serde(skip)]
    pub server_codec_mode_support: i32,
    #[serde(skip)]
    pub gpu_model: String,
    #[serde(skip)]
    pub is_supported_server_version: bool,
    #[serde(skip)]
    pub external_port: u16,
    #[serde(skip)]
    pub pending_quit: bool,
    #[serde(skip)]
    pub pairing_in_progress: bool,
    /// Every known address failed with "no route to host" on the last poll.
    /// On macOS that means the Local Network permission is missing.
    #[serde(skip)]
    pub unreachable: bool,
    /// Unix seconds of the last successful poll (persisted for "last seen")
    #[serde(default)]
    pub last_seen: Option<u64>,
    /// Set when applist fetching failed with a protocol error (e.g. Apollo 403)
    #[serde(skip)]
    pub app_list_error: Option<(i32, String)>,

    // Apollo extensions (ephemeral)
    #[serde(skip)]
    pub has_apollo_extensions: bool,
    #[serde(skip)]
    pub apollo_permissions: u32,
    #[serde(skip)]
    pub server_commands: Vec<String>,
    #[serde(skip)]
    pub virtual_display_capable: bool,
    #[serde(skip)]
    pub virtual_display_driver_ready: bool,
}

pub fn current_game(server_info: &str) -> i32 {
    // GFE 2.8+ keeps currentgame set to the last game; only trust it while busy
    let state = xml::get_string(server_info, "state").unwrap_or_default();
    if state.ends_with("_SERVER_BUSY") {
        xml::get_string(server_info, "currentgame").and_then(|g| g.parse().ok()).unwrap_or(0)
    } else {
        0
    }
}

pub fn parse_display_modes(server_info: &str) -> Vec<DisplayMode> {
    let Ok(doc) = roxmltree::Document::parse(server_info) else { return Vec::new() };
    let mut modes = Vec::new();
    for node in doc.descendants().filter(|n| n.is_element() && n.tag_name().name() == "DisplayMode") {
        let get = |tag: &str| {
            node.children()
                .find(|c| c.is_element() && c.tag_name().name() == tag)
                .and_then(|c| c.text())
                .and_then(|t| t.trim().parse::<i32>().ok())
                .unwrap_or(0)
        };
        modes.push(DisplayMode { width: get("Width"), height: get("Height"), refresh_rate: get("RefreshRate") });
    }
    modes.sort_by_key(|m| (m.width as u64) * (m.height as u64) * (m.refresh_rate as u64));
    modes
}

pub fn parse_app_list(app_xml: &str) -> Result<Vec<App>, String> {
    let doc = roxmltree::Document::parse(app_xml).map_err(|e| format!("Invalid applist XML: {e}"))?;
    let mut apps = Vec::new();
    for node in doc.descendants().filter(|n| n.is_element() && n.tag_name().name() == "App") {
        let text = |tag: &str| {
            node.children()
                .find(|c| c.is_element() && c.tag_name().name() == tag)
                .map(|c| c.text().unwrap_or("").to_string())
        };
        let id = text("ID").and_then(|s| s.trim().parse::<i32>().ok()).unwrap_or(0);
        let Some(name) = text("AppTitle") else {
            return Err("Invalid applist XML: app without title".into());
        };
        if id == 0 {
            return Err("Invalid applist XML: app without id".into());
        }
        apps.push(App {
            id,
            name,
            uuid: text("UUID").unwrap_or_default(),
            hdr_supported: text("IsHdrSupported").map(|v| v == "1").unwrap_or(false),
            is_app_collector_game: text("IsAppCollectorGame").map(|v| v == "1").unwrap_or(false),
            hidden: false,
            direct_launch: false,
        });
    }
    Ok(apps)
}

fn sort_apps(apps: &mut [App]) {
    apps.sort_by_cached_key(|a| a.name.to_lowercase());
}

impl Host {
    /// Builds a host from a `serverinfo` document, as `NvComputer(NvHTTP&, QString)` does.
    pub fn from_server_info(http: &NvHttp, info: &str) -> Host {
        let mut host = Host::default();
        host.server_cert_pem = http.server_cert().map(|c| der_to_pem(c.as_ref()));
        host.name = xml::get_string(info, "hostname").filter(|n| !n.is_empty()).unwrap_or_else(|| "UNKNOWN".into());
        host.uuid = xml::get_string(info, "uniqueid").unwrap_or_default();

        if let Some(mac) = xml::get_string(info, "mac")
            && mac != "00:00:00:00:00:00" {
                host.mac = mac.split(':').filter_map(|o| u8::from_str_radix(o.trim(), 16).ok()).collect();
            }

        // Apollo advertises the client's permission mask; its presence identifies Apollo
        let permissions = xml::get_string(info, "Permission");
        host.has_apollo_extensions = permissions.is_some();
        host.apollo_permissions = permissions.and_then(|p| p.trim().parse().ok()).unwrap_or(0);
        host.server_commands = xml::get_strings(info, "ServerCommand");
        host.virtual_display_capable = xml::get_string(info, "VirtualDisplayCapable").as_deref() == Some("true");
        host.virtual_display_driver_ready = xml::get_string(info, "VirtualDisplayDriverReady").as_deref() == Some("true");

        host.server_codec_mode_support = xml::get_string(info, "ServerCodecModeSupport")
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(scm::H264);
        host.max_luma_pixels_hevc = xml::get_string(info, "MaxLumaPixelsHEVC").and_then(|s| s.trim().parse().ok()).unwrap_or(0);
        host.display_modes = parse_display_modes(info);

        let local_ip = xml::get_string(info, "LocalIP").unwrap_or_default();
        host.local_address = if local_ip.is_empty() || local_ip.starts_with("127.") { None } else { Some(Address::new(local_ip, http.address().port)) };

        host.active_https_port = xml::get_string(info, "HttpsPort").and_then(|p| p.trim().parse::<u16>().ok()).filter(|p| *p != 0).unwrap_or(DEFAULT_HTTPS_PORT);
        host.external_port = xml::get_string(info, "ExternalPort").and_then(|p| p.trim().parse::<u16>().ok()).filter(|p| *p != 0).unwrap_or(http.address().port);
        host.remote_address = xml::get_string(info, "ExternalIP").filter(|a| !a.is_empty()).map(|a| Address::new(a, host.external_port));

        host.is_nvidia_server_software = xml::get_string(info, "state").map(|s| s.contains("MJOLNIR")).unwrap_or(false);
        host.pair_state = if xml::get_string(info, "PairStatus").as_deref() == Some("1") { PairState::Paired } else { PairState::NotPaired };
        host.current_game_id = current_game(info);
        host.app_version = xml::get_string(info, "appversion").unwrap_or_default();
        host.gfe_version = xml::get_string(info, "GfeVersion").unwrap_or_default();
        host.gpu_model = xml::get_string(info, "gputype").unwrap_or_default();
        host.active_address = Some(http.address().clone());
        host.state = HostState::Online;
        host.is_supported_server_version = true;
        host.last_seen = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs());
        host
    }

    pub fn server_cert(&self) -> Option<CertificateDer<'static>> {
        self.server_cert_pem.as_deref().and_then(pem_to_der)
    }

    pub fn software(&self) -> Software {
        if self.has_apollo_extensions {
            Software::Apollo
        } else if self.is_nvidia_server_software {
            Software::GeForceExperience
        } else {
            Software::Sunshine
        }
    }

    pub fn is_online(&self) -> bool {
        self.state == HostState::Online
    }

    pub fn is_paired(&self) -> bool {
        self.pair_state == PairState::Paired
    }

    pub fn running_app(&self) -> Option<&App> {
        if self.current_game_id == 0 {
            return None;
        }
        self.apps.iter().find(|a| a.id == self.current_game_id)
    }

    pub fn direct_launch_app(&self) -> Option<&App> {
        self.apps.iter().find(|a| a.direct_launch && !a.hidden)
    }

    pub fn mac_string(&self) -> String {
        self.mac.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(":")
    }

    /// Addresses to try, in priority order, without duplicates.
    pub fn unique_addresses(&self) -> Vec<Address> {
        let mut out: Vec<Address> = Vec::new();
        for a in [&self.active_address, &self.local_address, &self.remote_address, &self.ipv6_address, &self.manual_address].into_iter().flatten() {
            if !out.contains(a) {
                out.push(a.clone());
            }
        }
        out
    }

    pub fn is_site_local_v4(addr: &Address) -> bool {
        match addr.ip() {
            Some(IpAddr::V4(ip)) => ip.is_private() || ip == Ipv4Addr::LOCALHOST,
            _ => false,
        }
    }

    /// Merges a fresh poll result into this host. Returns true if anything changed.
    pub fn update(&mut self, that: &Host) -> bool {
        let mut changed = false;
        macro_rules! assign {
            ($field:ident) => {
                if self.$field != that.$field {
                    self.$field = that.$field.clone();
                    changed = true;
                }
            };
        }
        macro_rules! assign_if_some {
            ($field:ident) => {
                if that.$field.is_some() && self.$field != that.$field {
                    self.$field = that.$field.clone();
                    changed = true;
                }
            };
        }
        if !self.has_custom_name {
            assign!(name);
        }
        if !that.mac.is_empty() && self.mac != that.mac {
            self.mac = that.mac.clone();
            changed = true;
        }
        assign_if_some!(local_address);
        assign_if_some!(remote_address);
        assign_if_some!(ipv6_address);
        assign_if_some!(manual_address);
        assign!(active_https_port);
        assign!(external_port);
        assign!(pair_state);
        assign!(server_codec_mode_support);
        assign!(current_game_id);
        assign!(active_address);
        assign!(state);
        assign!(gfe_version);
        assign!(app_version);
        assign!(is_supported_server_version);
        assign!(is_nvidia_server_software);
        assign!(max_luma_pixels_hevc);
        assign!(gpu_model);
        assign!(has_apollo_extensions);
        assign!(apollo_permissions);
        assign!(server_commands);
        assign!(virtual_display_capable);
        assign!(virtual_display_driver_ready);
        assign_if_some!(server_cert_pem);
        if !that.display_modes.is_empty() && self.display_modes != that.display_modes {
            self.display_modes = that.display_modes.clone();
            changed = true;
        }
        if that.last_seen.is_some() {
            self.last_seen = that.last_seen;
        }
        if !that.apps.is_empty() && self.update_app_list(that.apps.clone()) {
            changed = true;
        }
        changed
    }

    /// Replaces the app list, carrying over client-side attributes.
    pub fn update_app_list(&mut self, mut new_apps: Vec<App>) -> bool {
        for new_app in new_apps.iter_mut() {
            if let Some(existing) = self.apps.iter().find(|a| a.id == new_app.id) {
                new_app.hidden = existing.hidden;
                new_app.direct_launch = existing.direct_launch;
            }
        }
        sort_apps(&mut new_apps);
        if self.apps == new_apps {
            return false;
        }
        self.apps = new_apps;
        true
    }

    /// Fields that are persisted to disk, for change detection.
    pub fn persisted_equal(&self, that: &Host) -> bool {
        self.name == that.name
            && self.has_custom_name == that.has_custom_name
            && self.uuid == that.uuid
            && self.mac == that.mac
            && self.local_address == that.local_address
            && self.remote_address == that.remote_address
            && self.ipv6_address == that.ipv6_address
            && self.manual_address == that.manual_address
            && self.server_cert_pem == that.server_cert_pem
            && self.is_nvidia_server_software == that.is_nvidia_server_software
            && self.apps == that.apps
            && self.show_hidden_apps == that.show_hidden_apps
            && self.last_seen == that.last_seen
    }

    pub fn http(&self, identity: std::sync::Arc<super::Identity>) -> Option<NvHttp> {
        let address = self.active_address.clone().or_else(|| self.unique_addresses().into_iter().next())?;
        Some(NvHttp::new(address, self.active_https_port, self.server_cert(), !self.is_nvidia_server_software, identity))
    }
}
