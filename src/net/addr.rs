//! Addresses: resolving what users type, and listing this PC's own.

use std::net::{IpAddr, SocketAddr, ToSocketAddrs};

/// Accepts "host", "host:port", "1.2.3.4" or "1.2.3.4:port" (hostnames
/// include Tailscale MagicDNS names). Prefers IPv4. Blocks on DNS, so keep
/// it off latency-sensitive threads.
pub fn resolve(address: &str, default_port: u16) -> Option<SocketAddr> {
    let address = address.trim();
    if address.is_empty() {
        return None;
    }
    let addrs: Vec<SocketAddr> = match address.to_socket_addrs() {
        Ok(a) => a.collect(),
        Err(_) => (address, default_port).to_socket_addrs().ok()?.collect(),
    };
    addrs.iter().find(|a| a.is_ipv4()).or(addrs.first()).copied()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AddrKind {
    /// Home/office network: the one to type on another PC.
    Lan,
    /// Tailscale (100.64.0.0/10): works from anywhere on your tailnet.
    Tailscale,
    Other,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LocalAddr {
    pub ip: IpAddr,
    pub kind: AddrKind,
}

pub fn classify(ip: IpAddr) -> Option<AddrKind> {
    let IpAddr::V4(v4) = ip else { return None };
    let [a, b, ..] = v4.octets();
    if v4.is_loopback() || v4.is_link_local() || v4.is_unspecified() {
        None
    } else if a == 100 && (64..128).contains(&b) {
        Some(AddrKind::Tailscale)
    } else if v4.is_private() {
        Some(AddrKind::Lan)
    } else {
        Some(AddrKind::Other)
    }
}

/// This PC's useful IPv4 addresses, best first (the LAN address that the
/// default route uses comes first).
pub fn local_addresses() -> Vec<LocalAddr> {
    let primary = local_ip_address::local_ip().ok();
    let mut out: Vec<LocalAddr> = local_ip_address::list_afinet_netifas()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(_, ip)| classify(ip).map(|kind| LocalAddr { ip, kind }))
        .collect();
    out.sort_by_key(|a| (Some(a.ip) != primary, a.kind));
    out.dedup();
    out
}

/// A friendly name for this PC ("DESKTOP-PC").
pub fn machine_name() -> String {
    ["COMPUTERNAME", "HOSTNAME"]
        .iter()
        .find_map(|v| std::env::var(v).ok().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "This PC".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forms() {
        assert_eq!(resolve("127.0.0.1", 24801), Some("127.0.0.1:24801".parse().unwrap()));
        assert_eq!(resolve(" 127.0.0.1:9 ", 24801), Some("127.0.0.1:9".parse().unwrap()));
        assert_eq!(resolve("localhost", 5).map(|a| a.port()), Some(5));
        assert_eq!(resolve("", 5), None);
        assert_eq!(resolve("   ", 5), None);
    }

    #[test]
    fn classification() {
        let c = |s: &str| classify(s.parse().unwrap());
        assert_eq!(c("192.168.1.7"), Some(AddrKind::Lan));
        assert_eq!(c("10.1.2.3"), Some(AddrKind::Lan));
        assert_eq!(c("100.101.102.103"), Some(AddrKind::Tailscale));
        assert_eq!(c("100.200.0.1"), Some(AddrKind::Other));
        assert_eq!(c("127.0.0.1"), None);
        assert_eq!(c("169.254.1.1"), None);
        assert_eq!(c("::1"), None);
    }

    #[test]
    fn lists_something_sensible() {
        // Every listed address is one we'd show; no loopback.
        for a in local_addresses() {
            assert!(!a.ip.is_loopback());
        }
        assert!(!machine_name().is_empty());
    }
}
