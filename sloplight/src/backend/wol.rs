//! Wake-on-LAN, port of `NvComputer::wake`.

use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs, UdpSocket};

use super::host::Host;

const STATIC_WOL_PORTS: [u16; 2] = [9, 47009];
const DYNAMIC_WOL_PORTS: [u16; 5] = [47998, 47999, 48000, 48002, 48010];

/// Sends magic packets to every address we know for the host, plus broadcast.
pub fn wake(host: &Host) -> Result<(), String> {
    if host.mac.is_empty() {
        return Err(format!("{} has no MAC address stored", host.name));
    }
    let mut payload = vec![0xFFu8; 6];
    for _ in 0..16 {
        payload.extend_from_slice(&host.mac);
    }

    // address -> known base HTTP port (0 = unknown)
    let mut targets: BTreeMap<String, u16> = BTreeMap::new();
    let mut base_ports: BTreeSet<u16> = BTreeSet::new();
    for addr in host.unique_addresses() {
        targets.insert(addr.host.clone(), addr.port);
        base_ports.insert(addr.port);
    }
    targets.insert("255.255.255.255".into(), 0);
    if let Ok(ifaces) = if_addrs::get_if_addrs() {
        for iface in ifaces {
            if iface.is_loopback() {
                continue;
            }
            if let if_addrs::IfAddr::V4(v4) = &iface.addr
                && let Some(bcast) = v4.broadcast {
                    targets.insert(bcast.to_string(), 0);
                }
        }
    }
    targets.insert("ff02::1".into(), 0);

    let mut success = false;
    for (target, base_port) in targets {
        let ips: Vec<IpAddr> = match target.parse::<IpAddr>() {
            Ok(ip) => vec![ip],
            Err(_) => match (target.as_str(), 0u16).to_socket_addrs() {
                Ok(addrs) => addrs.map(|a| a.ip()).collect(),
                Err(e) => {
                    log::warn!("Error resolving {target}: {e}");
                    continue;
                }
            },
        };
        for ip in ips {
            let bind: SocketAddr = match ip {
                IpAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
                IpAddr::V6(_) => (std::net::Ipv6Addr::UNSPECIFIED, 0).into(),
            };
            let Ok(sock) = UdpSocket::bind(bind) else { continue };
            let _ = sock.set_broadcast(true);
            let mut ports: Vec<u16> = STATIC_WOL_PORTS.to_vec();
            let bases: Vec<u16> = if base_port != 0 { vec![base_port] } else { base_ports.iter().copied().collect() };
            for base in bases {
                for p in DYNAMIC_WOL_PORTS {
                    ports.push((p as i32 - 47989 + base as i32).clamp(1, 65535) as u16);
                }
            }
            for port in ports {
                match sock.send_to(&payload, SocketAddr::new(ip, port)) {
                    Ok(_) => {
                        log::info!("Sent WoL packet to {} via {ip}:{port}", host.name);
                        success = true;
                    }
                    Err(e) => log::debug!("WoL send to {ip}:{port} failed: {e}"),
                }
            }
        }
    }
    if success { Ok(()) } else { Err("Failed to send any Wake-on-LAN packets".into()) }
}
