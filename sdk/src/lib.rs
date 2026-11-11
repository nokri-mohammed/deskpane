//! The deskpane plugin SDK.
//!
//! A data-source plugin is a program that sends contract-shaped JSON to the
//! daemon over UDP, exactly the way the reference reporter does. This crate
//! gives a plugin author the two things they need: the wire contract (the
//! types and their limits, re-exported from `deskpane-contract`) and a small
//! send helper that serialises a packet and puts it on the wire without ever
//! panicking.
//!
//! Example (a data source that sends one device report):
//!
//! ```no_run
//! use deskpane_sdk::contract::Report;
//! use deskpane_sdk::send_packet;
//!
//! let report = Report {
//!     device_id: "dp-example".to_string(),
//!     label: "Example".to_string(),
//!     hostname: "example".to_string(),
//!     ip: "192.168.1.50".to_string(),
//!     cpu_pct: 12.0,
//!     mem_used_mb: 1024.0,
//!     mem_total_mb: 8192.0,
//!     disk_used_gb: 40.0,
//!     disk_total_gb: 256.0,
//!     temp_c: 45.0,
//!     uptime_s: 3600.0,
//!     battery_pct: None,
//!     charging: None,
//! };
//! let sent = send_packet("192.168.1.50:8765", &report);
//! match sent {
//!     Ok(()) => {}
//!     Err(reason) => eprintln!("send failed: {reason}"),
//! }
//! ```

use std::net::SocketAddr;
use std::net::ToSocketAddrs;
use std::net::UdpSocket;

use serde::Serialize;

// the wire contract, re-exported so a plugin depends on one crate and gets the
// shared types, limits and validators in the language it sends from
pub use deskpane_contract as contract;

// the default daemon port, the same one the reference reporter and the daemon use
pub const DEFAULT_DAEMON_PORT: u16 = 8765;

/*
 * picks the address to send to from a resolved list: an IPv4 address if there
 * is one, else the first address. Mirrors the reference reporter so a plugin
 * reaches the daemon the same way on a mixed IPv4/IPv6 host.
 */
fn pick_address(addrs: &[SocketAddr]) -> Option<SocketAddr> {
    for addr in addrs {
        if addr.is_ipv4() {
            return Some(*addr);
        }
    }
    let first = addrs.first();
    match first {
        Some(addr) => Some(*addr),
        None => None,
    }
}

/*
 * sends one packet to target from a fresh non-blocking socket, and never
 * panics or prints, so a short-lived plugin stays silent and decides itself
 * what to log. A host name is resolved with the system resolver, which may
 * block; a plugin that must bound that wraps the call in its own deadline.
 *
 * target: address to send to, e.g. 192.168.1.50:8765 or my-server:8765
 * bytes: the packet, already serialised
 * returns: Ok(()) once the OS took the packet, or Err with the reason
 */
pub fn send_datagram(target: &str, bytes: &[u8]) -> Result<(), String> {
    // resolve the target, then pick the address to send to
    let resolved = target.to_socket_addrs();
    let found = match resolved {
        Ok(addrs) => addrs,
        Err(error) => return Err(format!("resolve {target}: {error}")),
    };
    let mut all = Vec::new();
    for addr in found {
        all.push(addr);
    }
    let picked = pick_address(&all);
    let addr = match picked {
        Some(addr) => addr,
        None => return Err(format!("resolve {target}: no address")),
    };

    // a local socket of the same family as the target, any free port
    let local;
    if addr.is_ipv6() {
        local = "[::]:0";
    } else {
        local = "0.0.0.0:0";
    }
    let bound = UdpSocket::bind(local);
    let socket = match bound {
        Ok(socket) => socket,
        Err(error) => return Err(format!("bind local udp socket: {error}")),
    };

    // never wait on the network
    let switched = socket.set_nonblocking(true);
    match switched {
        Ok(_) => {}
        Err(error) => return Err(format!("non-blocking udp socket: {error}")),
    }

    let sent = socket.send_to(bytes, addr);
    match sent {
        Ok(_) => Ok(()),
        Err(error) => Err(format!("send to {addr}: {error}")),
    }
}

/*
 * serialises a contract packet to JSON and sends it to the daemon. This is the
 * one call most data-source plugins use: build a Report (or an agent packet),
 * then send_packet(daemon, &packet).
 *
 * target: the daemon address, e.g. 192.168.1.50:8765
 * packet: any serialisable contract type
 * returns: Ok(()) once the OS took the packet, or Err with the reason
 */
pub fn send_packet<T: Serialize>(target: &str, packet: &T) -> Result<(), String> {
    let encoded = serde_json::to_vec(packet);
    let bytes = match encoded {
        Ok(bytes) => bytes,
        Err(error) => return Err(format!("serialise packet: {error}")),
    };
    let sent = send_datagram(target, &bytes);
    match sent {
        Ok(()) => Ok(()),
        Err(reason) => Err(reason),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::UdpSocket;

    // picks IPv4 when both families are present
    #[test]
    fn pick_address_prefers_ipv4() {
        let v6 = "[::1]:8765".parse::<SocketAddr>().unwrap();
        let v4 = "127.0.0.1:8765".parse::<SocketAddr>().unwrap();
        let picked = pick_address(&[v6, v4]);
        assert_eq!(picked, Some(v4));
    }

    // falls back to the first address when there is no IPv4
    #[test]
    fn pick_address_falls_back_to_first() {
        let v6 = "[::1]:8765".parse::<SocketAddr>().unwrap();
        let picked = pick_address(&[v6]);
        assert_eq!(picked, Some(v6));
    }

    // an empty list has no address, not a panic
    #[test]
    fn pick_address_of_nothing_is_none() {
        let picked = pick_address(&[]);
        assert_eq!(picked, None);
    }

    // a datagram sent to a real listening socket arrives unchanged
    #[test]
    fn send_datagram_arrives_unchanged() {
        let listener = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let sent = send_datagram(&addr, b"{\"kind\":\"test\"}");
        assert!(sent.is_ok());
        let mut buf = [0u8; 64];
        let got = listener.recv(&mut buf).unwrap();
        assert_eq!(&buf[..got], b"{\"kind\":\"test\"}");
    }

    // a bad target is an error, never a panic
    #[test]
    fn send_datagram_bad_target_is_an_error() {
        let sent = send_datagram("not-an-address", b"x");
        assert!(sent.is_err());
    }

    // send_packet serialises a Report and it arrives as JSON with its fields
    #[test]
    fn send_packet_serialises_and_sends_a_report() {
        let listener = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let report = contract::Report {
            device_id: "dp-test".to_string(),
            label: "Test".to_string(),
            hostname: "test".to_string(),
            ip: "192.168.1.50".to_string(),
            cpu_pct: 1.0,
            mem_used_mb: 2.0,
            mem_total_mb: 3.0,
            disk_used_gb: 4.0,
            disk_total_gb: 5.0,
            temp_c: 6.0,
            uptime_s: 7.0,
            battery_pct: None,
            charging: None,
        };
        let sent = send_packet(&addr, &report);
        assert!(sent.is_ok());
        let mut buf = [0u8; 512];
        let got = listener.recv(&mut buf).unwrap();
        let text = String::from_utf8(buf[..got].to_vec()).unwrap();
        assert!(text.contains("\"device_id\":\"dp-test\""));
        assert!(text.contains("\"cpu_pct\":1.0"));
    }
}
