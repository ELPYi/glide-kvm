//! Network priority via Windows' qWAVE API: tags glide's packets to a
//! destination as *voice* traffic. On Wi-Fi that maps to WMM's highest
//! access category, so access points send it ahead of bulk traffic
//! (downloads, video). On a quiet network it changes nothing; under load it
//! cuts queueing delay. Best-effort: if Windows declines, packets simply go
//! out untagged.

use std::collections::HashSet;
use std::mem::zeroed;
use std::net::{SocketAddr, UdpSocket};
use std::os::windows::io::AsRawSocket;
use std::sync::{Mutex, OnceLock};

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::NetworkManagement::QoS::{
    QOSAddSocketToFlow, QOSCreateHandle, QOSTrafficTypeVoice, QOS_NON_ADAPTIVE_FLOW, QOS_VERSION,
};
use windows_sys::Win32::Networking::WinSock::{AF_INET, SOCKADDR, SOCKADDR_IN};

/// One qWAVE handle for the process (stored as usize to be Sync).
fn handle() -> Option<HANDLE> {
    static H: OnceLock<Option<usize>> = OnceLock::new();
    H.get_or_init(|| unsafe {
        let version = QOS_VERSION {
            MajorVersion: 1,
            MinorVersion: 0,
        };
        let mut h: HANDLE = std::ptr::null_mut();
        (QOSCreateHandle(&version, &mut h) != 0).then_some(h as usize)
    })
    .map(|h| h as HANDLE)
}

/// Mark packets from `sock` to `dest` as voice priority. Returns whether
/// Windows accepted it. Safe to call repeatedly; each pair is added once.
pub fn prioritize(sock: &UdpSocket, dest: SocketAddr) -> bool {
    static DONE: Mutex<Option<HashSet<(u64, SocketAddr)>>> = Mutex::new(None);
    let SocketAddr::V4(v4) = dest else { return false };
    let raw = sock.as_raw_socket();
    let mut done = DONE.lock().unwrap_or_else(|e| e.into_inner());
    let done = done.get_or_insert_with(HashSet::new);
    if done.contains(&(raw, dest)) {
        return true;
    }
    let Some(h) = handle() else { return false };
    let ok = unsafe {
        let mut addr: SOCKADDR_IN = zeroed();
        addr.sin_family = AF_INET;
        addr.sin_port = v4.port().to_be();
        addr.sin_addr.S_un.S_addr = u32::from_ne_bytes(v4.ip().octets());
        let mut flow: u32 = 0;
        QOSAddSocketToFlow(
            h,
            raw as usize,
            &addr as *const SOCKADDR_IN as *const SOCKADDR,
            QOSTrafficTypeVoice,
            QOS_NON_ADAPTIVE_FLOW,
            &mut flow,
        ) != 0
    };
    if ok {
        done.insert((raw, dest));
    }
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Not an assertion: whether Windows grants priority depends on the
    /// machine. Run with --nocapture to see the answer here.
    #[test]
    fn report_whether_priority_is_granted() {
        let sock = UdpSocket::bind("0.0.0.0:0").unwrap();
        let dest: SocketAddr = "192.168.1.18:24801".parse().unwrap();
        eprintln!("QoS voice priority granted: {}", prioritize(&sock, dest));
        assert!(
            prioritize(&sock, dest) == prioritize(&sock, dest),
            "repeat calls are stable"
        );
    }
}
