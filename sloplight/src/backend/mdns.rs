//! mDNS discovery of `_nvstream._tcp` hosts using mdns-sd.

use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use mdns_sd::{ServiceDaemon, ServiceEvent};

use super::host::Address;
use super::http::DEFAULT_HTTP_PORT;

pub struct MdnsBrowser {
    daemon: ServiceDaemon,
    stop: Arc<AtomicBool>,
}

pub struct Discovered {
    pub name: String,
    pub address: Address,
    /// Best global IPv6 address, if any
    pub ipv6: Option<Address>,
}

const SERVICE_TYPE: &str = "_nvstream._tcp.local.";

impl MdnsBrowser {
    /// Starts browsing; `on_found` is called from a background thread.
    pub fn start(on_found: impl Fn(Discovered) + Send + 'static) -> Result<MdnsBrowser, String> {
        let daemon = ServiceDaemon::new().map_err(|e| e.to_string())?;
        let receiver = daemon.browse(SERVICE_TYPE).map_err(|e| e.to_string())?;
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        thread::Builder::new()
            .name("mdns-browser".into())
            .spawn(move || {
                while !stop2.load(Ordering::Relaxed) {
                    match receiver.recv_timeout(Duration::from_millis(500)) {
                        Ok(ServiceEvent::ServiceResolved(info)) => {
                            let port = if info.port == 0 { DEFAULT_HTTP_PORT } else { info.port };
                            let mut v4: Option<IpAddr> = None;
                            let mut link_local_v6: Option<IpAddr> = None;
                            let mut global_v6: Option<IpAddr> = None;
                            for scoped in info.addresses.iter() {
                                let ip: IpAddr = scoped.to_ip_addr();
                                match ip {
                                    IpAddr::V4(_) => {
                                        if v4.is_none() {
                                            v4 = Some(ip);
                                        }
                                    }
                                    IpAddr::V6(v6) => {
                                        let is_link_local = (v6.segments()[0] & 0xffc0) == 0xfe80;
                                        let is_ula = (v6.segments()[0] & 0xfe00) == 0xfc00;
                                        if is_link_local {
                                            link_local_v6 = link_local_v6.or(Some(ip));
                                        } else if !is_ula && !v6.is_loopback() {
                                            global_v6 = global_v6.or(Some(ip));
                                        }
                                    }
                                }
                            }
                            let Some(primary) = v4.or(link_local_v6).or(global_v6) else {
                                log::warn!("mDNS service {} resolved without usable addresses", info.fullname);
                                continue;
                            };
                            let name = info.host.trim_end_matches('.').trim_end_matches(".local").to_string();
                            log::info!("mDNS found {} at {}:{}", name, primary, port);
                            on_found(Discovered {
                                name,
                                address: Address::new(primary.to_string(), port),
                                ipv6: global_v6.filter(|g| Some(*g) != Some(primary)).map(|g| Address::new(g.to_string(), port)),
                            });
                        }
                        Ok(_) => {}
                        Err(mdns_sd::RecvTimeoutError::Timeout) => {}
                        Err(mdns_sd::RecvTimeoutError::Disconnected) => break,
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(MdnsBrowser { daemon, stop })
    }
}

impl Drop for MdnsBrowser {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.daemon.stop_browse(SERVICE_TYPE);
        let _ = self.daemon.shutdown();
    }
}
