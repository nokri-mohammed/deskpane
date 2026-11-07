use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use std::time::{Duration, Instant};

use daemon::contract::{validate, DeviceStatus, Report};
use daemon::device_table::{forget_silent, insert_report, DeviceTable};
use daemon::services::broadcaster::Broadcaster;
use daemon::services::udp::UdpSocketServer;
use daemon::services::unix_socket::UnixSocketServer;
use tokio::sync::RwLock;

// a silent device shows as offline after 15 s, and is removed after 5 min
const OFFLINE_AFTER: Duration = Duration::from_secs(15);
const FORGET_AFTER: Duration = Duration::from_secs(300);
// most devices the table holds, so new device_ids cannot fill memory
const MAX_DEVICES: usize = 32;
const DEFAULT_LISTEN_ADDR: &str = "0.0.0.0:8765";
const DEFAULT_SOCKET_PATH: &str = "/tmp/pi-homelab-dashboard.sock";

type SharedState = Arc<RwLock<DeviceTable>>;

/// Transport the daemon listens on for incoming reports. UDP is the only
/// one implemented today; Tcp and TcpTls are the planned next steps; add
/// a variant + an arm in `main`'s match, reusing `state` the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Protocol {
    Udp,
}

impl Protocol {
    fn from_env() -> Self {
        let read = env::var("PROTOCOL");
        let value = match read {
            Ok(v) => v,
            Err(_) => "udp".to_string(),
        };
        match value.as_str() {
            "udp" => Protocol::Udp,
            other => panic!(
                "unsupported PROTOCOL '{other}': only 'udp' is implemented so far (tcp/tcp+tls planned)"
            ),
        }
    }
}

// receive device reports over UDP and store the latest one per device
async fn udp_server(mut udp_socket_server: UdpSocketServer, state: SharedState) {
    loop {
        // wait for the next packet from a device
        let received = udp_socket_server.receive().await;
        let (bytes, peer) = match received {
            Some(x) => x,
            None => continue,
        };

        // Parses the bytes as JSON and validates they match Report's exact
        // fields/types in one step.
        let parsed = serde_json::from_slice::<Report>(bytes);
        let report = match parsed {
            Ok(r) => r,
            Err(e) => {
                eprintln!("daemon: bad report from {peer}: {e}");
                continue;
            }
        };

        // drop reports that break the contract limits (bad text, impossible numbers)
        let checked = validate(&report);
        match checked {
            Ok(()) => {}
            Err(reason) => {
                eprintln!("daemon: rejected report from {peer}: {reason}");
                continue;
            }
        }

        // store it, unless it is a new device and the table is already full
        let mut table = state.write().await;
        let stored = insert_report(&mut table, report, Instant::now(), MAX_DEVICES);
        // release the write lock before logging
        drop(table);
        if !stored {
            eprintln!("daemon: device table full, dropping report from {peer}");
        }
    }
}

/*
 * removes devices that have been silent longer than FORGET_AFTER, and logs
 * each one. Runs once per broadcast tick (see main for why).
 */
async fn forget_silent_devices(state: &SharedState) {
    let mut table = state.write().await;
    let forgotten = forget_silent(&mut table, Instant::now(), FORGET_AFTER);
    // release the write lock before logging
    drop(table);

    let forget_secs = FORGET_AFTER.as_secs();
    for device_id in &forgotten {
        eprintln!("daemon: forgot {device_id} after {forget_secs} s of silence");
    }
}

/// Snapshot the current state (with online/offline computed) and broadcast
/// it once, as one JSON array line, to every socket subscriber.
async fn broadcast_statuses(state: &SharedState, broadcaster: &Broadcaster) {
    // read-lock the table to look at every device's latest report
    let snapshot = state.read().await;
    // get current timestamp
    let now = Instant::now();
    // build each device's status (online if seen recently) into one list
    let mut statuses: Vec<DeviceStatus> = Vec::new();
    for (report, last_seen) in snapshot.values() {
        // saturating: a report stamped after `now` counts as 0 s silent, never panics
        let silent_for = now.saturating_duration_since(*last_seen);

        let online;
        if silent_for < OFFLINE_AFTER {
            online = true;
        } else {
            online = false;
        }

        let status = DeviceStatus {
            report: report.clone(),
            online: online,
        };
        statuses.push(status);
    }
    // release the read lock early, before the JSON/broadcast work below
    drop(snapshot);

    broadcaster.send_json(&statuses);
}

/// Accepts dashboard connections and, for each one, relays every message
/// from `broadcaster` to it until the write fails or the connection drops.
async fn unix_socket_relay(socket_server: &UnixSocketServer, broadcaster: &Broadcaster) {
    loop {
        // wait for a client to connect
        let accepted = socket_server.subscribe().await;
        let mut subscriber = match accepted {
            Some(s) => s,
            None => continue,
        };
        let mut rx = broadcaster.subscribe();
        tokio::spawn(async move {
            loop {
                // wait for the next broadcast message
                let received = rx.recv().await;
                let line = match received {
                    Ok(l) => l,
                    Err(_) => break,
                };

                // push it out to this client
                let wrote_line = subscriber.broadcast(line.as_bytes()).await;
                if !wrote_line {
                    break;
                }

                let wrote_newline = subscriber.broadcast(b"\n").await;
                if !wrote_newline {
                    break;
                }
            }
        });
    }
}

#[tokio::main]
async fn main() {
    let listen_addr_read = env::var("LISTEN_ADDR");
    let listen_addr = match listen_addr_read {
        Ok(v) => v,
        Err(_) => DEFAULT_LISTEN_ADDR.to_string(),
    };

    let socket_path_read = env::var("SOCKET_PATH");
    let socket_path = match socket_path_read {
        Ok(v) => v,
        Err(_) => DEFAULT_SOCKET_PATH.to_string(),
    };

    let protocol = Protocol::from_env();

    let state: SharedState = Arc::new(RwLock::new(HashMap::new()));
    let broadcaster = Broadcaster::new(16);
    let unix_socket_server = UnixSocketServer::new(&socket_path);

    match protocol {
        Protocol::Udp => {
            let udp_socket_server = UdpSocketServer::new(&listen_addr).await;

            tokio::join!(
                // listen for incoming reports from devices.
                udp_server(udp_socket_server, state.clone()),
                /*
                 * every second, forget long-silent devices, then build a
                 * status summary and broadcast it. Forgetting here (not on
                 * insert) means a device disappears on time even when no
                 * packets arrive, and the per-packet path stays small.
                 */
                async {
                    let mut interval = tokio::time::interval(Duration::from_secs(1));
                    loop {
                        // wait for the next 1-second tick
                        interval.tick().await;
                        forget_silent_devices(&state).await;
                        broadcast_statuses(&state, &broadcaster).await;
                    }
                },
                // accepts dashboard connections and streams that summary to them.
                unix_socket_relay(&unix_socket_server, &broadcaster),
            );
        }
    }
}
