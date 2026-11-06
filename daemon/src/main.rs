//! Report daemon: accepts one JSON report per datagram from any reporting
//! device (see ../../docs/CONTRACT.md), keeps the latest report per device
//! in memory, and relays the live set (with an `online` flag) to any local
//! subscriber over a Unix domain socket.
//!
//! v1 transport is plain UDP with no authentication — fine on a trusted
//! LAN, not fine beyond it. PROTOCOL below is the single switch point for
//! adding TCP and TCP+TLS later without restructuring the rest of the
//! daemon: each transport just needs to land a `Report` into `state`.

use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use std::time::{Duration, Instant};

use daemon::contract::{DeviceStatus, Report};
use tokio::io::AsyncWriteExt;
use tokio::net::{UdpSocket, UnixListener};
use tokio::sync::{broadcast, RwLock};

const OFFLINE_AFTER: Duration = Duration::from_secs(15);

type SharedState = Arc<RwLock<HashMap<String, (Report, Instant)>>>;

/// Transport the daemon listens on for incoming reports. UDP is the only
/// one implemented today; Tcp and TcpTls are the planned next steps — add
/// a variant + an arm in `main`'s match, reusing `state` the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Protocol {
    Udp,
}

impl Protocol {
    fn from_env() -> Self {
        match env::var("PROTOCOL").unwrap_or_else(|_| "udp".into()).as_str() {
            "udp" => Protocol::Udp,
            other => panic!(
                "unsupported PROTOCOL '{other}' — only 'udp' is implemented so far (tcp/tcp+tls planned)"
            ),
        }
    }
}

async fn udp_server(addr: &str, state: SharedState) {
    let socket = UdpSocket::bind(addr)
        .await
        .unwrap_or_else(|e| panic!("bind udp {addr}: {e}"));
    println!("daemon: UDP listening on {addr} (no auth — trusted LAN only for now)");

    let mut buf = vec![0u8; 8192];
    loop {
        match socket.recv_from(&mut buf).await {
            Ok((len, peer)) => match serde_json::from_slice::<Report>(&buf[..len]) {
                Ok(report) => {
                    let device_id = report.device_id.clone();
                    state.write().await.insert(device_id, (report, Instant::now()));
                }
                Err(e) => eprintln!("daemon: bad report from {peer}: {e}"),
            },
            Err(e) => eprintln!("daemon: recv error: {e}"),
        }
    }
}

/// Every second, snapshot the current state (with online/offline computed)
/// and broadcast it as one JSON array line to every socket subscriber.
async fn tick_loop(state: SharedState, tx: broadcast::Sender<String>) {
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    loop {
        interval.tick().await;
        let snapshot = state.read().await;
        let now = Instant::now();
        let statuses: Vec<DeviceStatus> = snapshot
            .values()
            .map(|(report, last_seen)| DeviceStatus {
                report: report.clone(),
                online: now.duration_since(*last_seen) < OFFLINE_AFTER,
            })
            .collect();
        drop(snapshot);
        if let Ok(line) = serde_json::to_string(&statuses) {
            let _ = tx.send(line); // no subscribers yet is fine
        }
    }
}

async fn unix_socket_server(socket_path: &str, tx: broadcast::Sender<String>) {
    let _ = std::fs::remove_file(socket_path); // stale socket from a previous run
    let listener = UnixListener::bind(socket_path)
        .unwrap_or_else(|e| panic!("bind unix socket {socket_path}: {e}"));
    println!("daemon: dashboard socket at {socket_path}");
    loop {
        let (mut stream, _) = match listener.accept().await {
            Ok(x) => x,
            Err(e) => {
                eprintln!("daemon: unix accept error: {e}");
                continue;
            }
        };
        let mut rx = tx.subscribe();
        tokio::spawn(async move {
            while let Ok(line) = rx.recv().await {
                if stream.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
                if stream.write_all(b"\n").await.is_err() {
                    break;
                }
            }
        });
    }
}

#[tokio::main]
async fn main() {
    let listen_addr = env::var("LISTEN_ADDR").unwrap_or_else(|_| "0.0.0.0:8765".into());
    let socket_path =
        env::var("SOCKET_PATH").unwrap_or_else(|_| "/tmp/pi-homelab-dashboard.sock".into());
    let protocol = Protocol::from_env();

    let state: SharedState = Arc::new(RwLock::new(HashMap::new()));
    let (tx, _rx) = broadcast::channel::<String>(16);

    match protocol {
        Protocol::Udp => {
            tokio::join!(
                udp_server(&listen_addr, state.clone()),
                tick_loop(state.clone(), tx.clone()),
                unix_socket_server(&socket_path, tx.clone()),
            );
        }
    }
}
