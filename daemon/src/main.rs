use std::collections::HashMap;
use std::env;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use daemon::contract::{build_reply, validate, DeviceStatus, Reply, Report, ReportPacket};
use daemon::device_table::{forget_silent, insert_report, DeviceTable, InsertOutcome};
use daemon::services::broadcaster::Broadcaster;
use daemon::services::reply_limiter::ReplyLimiter;
use daemon::services::udp::UdpSocketServer;
use daemon::services::unix_socket::UnixSocketServer;
use tokio::sync::RwLock;

// a silent device shows as offline after 15 s, and is removed after 5 min
const OFFLINE_AFTER: Duration = Duration::from_secs(15);
const FORGET_AFTER: Duration = Duration::from_secs(300);
// most devices the table holds, so new device_ids cannot fill memory
const MAX_DEVICES: usize = 32;
const DEFAULT_LISTEN_ADDR: &str = "0.0.0.0:8765";
const DEFAULT_SOCKET_PATH: &str = "/run/deskpane/daemon.sock";
// replies (and duplicate log lines): one per source IP per 10 s, 20 per second
// in total, and at most 256 source IPs remembered; 256 >= 10 * 20, so the
// limiter never forgets a source that is still inside its window (tested)
const REPLY_EVERY: Duration = Duration::from_secs(10);
const REPLIES_PER_SECOND: u32 = 20;
const MAX_REPLY_SOURCES: usize = 256;
// exit code when the UDP port is taken; the unit has RestartPreventExitStatus=11
const EXIT_PORT_IN_USE: i32 = 11;

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

/*
 * turns a failed UDP bind into one line for the log and an exit code
 *
 * addr: the LISTEN_ADDR that failed, e.g. 0.0.0.0:8765
 * error: what the bind returned
 * returns: (message, exit code); 11 for "port in use", 1 for anything else
 */
fn bind_error_message(addr: &str, error: &std::io::Error) -> (String, i32) {
    if error.kind() == std::io::ErrorKind::AddrInUse {
        // the port is whatever follows the last ':' (works for [::]:8765 too)
        let mut port = addr;
        let split = addr.rsplit_once(':');
        match split {
            Some((_host, p)) => port = p,
            None => {}
        }

        let message = format!(
            "deskpane-daemon: error: UDP port {port} is already in use by another program. Find it with: sudo ss -ulpn 'sport = :{port}'"
        );
        return (message, EXIT_PORT_IN_USE);
    }

    let message = format!("deskpane-daemon: error: cannot listen on {addr}: {error}");
    (message, 1)
}

/*
 * the IP a packet counts as coming from, for id ownership and the reply
 * limiter alike. to_canonical: ::ffff:192.168.1.60 (daemon on [::]) and
 * 192.168.1.60 are one source
 */
fn reply_source(peer: SocketAddr) -> std::net::IpAddr {
    peer.ip().to_canonical()
}

/*
 * stores a report under the table policy; the owner is the source IP only,
 * so a restarted reporter (new random port) keeps its id
 *
 * peer: the address the report came from
 */
fn store_report(
    table: &mut DeviceTable,
    report: Report,
    peer: SocketAddr,
    now: Instant,
) -> InsertOutcome {
    let source = reply_source(peer);
    insert_report(table, report, source, now, MAX_DEVICES, OFFLINE_AFTER)
}

/*
 * receives device reports over UDP, stores the latest one per device, and
 * answers a duplicate id (or a report that asks for an ack) with one small,
 * rate limited reply
 */
async fn udp_server(mut udp_socket_server: UdpSocketServer, state: SharedState) {
    let mut limiter = ReplyLimiter::new(REPLY_EVERY, REPLIES_PER_SECOND, MAX_REPLY_SOURCES);

    loop {
        // wait for the next packet from a device
        let received = udp_socket_server.receive().await;
        let (bytes, peer) = match received {
            Some(x) => x,
            None => continue,
        };
        // the reply must stay smaller than this
        let request_bytes = bytes.len();

        // Parses the bytes as JSON and checks they match the report's exact
        // fields/types in one step (plus the optional ack flag).
        let parsed = serde_json::from_slice::<ReportPacket>(bytes);
        let packet = match parsed {
            Ok(p) => p,
            Err(e) => {
                eprintln!("daemon: bad report from {peer}: {e}");
                continue;
            }
        };

        // drop reports that break the contract limits (bad text, impossible numbers)
        let checked = validate(&packet.report);
        match checked {
            Ok(()) => {}
            Err(reason) => {
                eprintln!("daemon: rejected report from {peer}: {reason}");
                continue;
            }
        }

        // store it, unless another online machine owns the id or the table is full
        let device_id = packet.report.device_id.clone();
        let now = Instant::now();
        let mut table = state.write().await;
        let outcome = store_report(&mut table, packet.report, peer, now);
        // release the write lock before logging and replying
        drop(table);

        // decide whether this report gets a reply
        let reply;
        match outcome {
            InsertOutcome::Stored => {
                if !packet.ack {
                    continue;
                }
                reply = Reply::ack(&device_id);
            }
            InsertOutcome::TableFull => {
                eprintln!("daemon: device table full, dropping report from {peer}");
                continue;
            }
            InsertOutcome::IdInUse { owner } => {
                reply = Reply::id_in_use(&device_id, owner);
            }
        }

        // the limiter also keeps duplicate log lines from flooding the journal
        let source = reply_source(peer);
        let allowed = limiter.allow(source, now);
        if !allowed {
            continue;
        }
        match outcome {
            InsertOutcome::IdInUse { owner } => {
                eprintln!("daemon: refused device id {device_id} from {source}: in use by {owner}");
            }
            _ => {}
        }

        // only send when the reply is small enough (never bigger than the request)
        let built = build_reply(&reply, request_bytes);
        let reply_bytes = match built {
            Some(b) => b,
            None => continue,
        };
        udp_socket_server.send_to(&reply_bytes, peer).await;
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
    for entry in snapshot.values() {
        // saturating: a report stamped after `now` counts as 0 s silent, never panics
        let silent_for = now.saturating_duration_since(entry.last_seen);

        let online;
        if silent_for < OFFLINE_AFTER {
            online = true;
        } else {
            online = false;
        }

        let status = DeviceStatus {
            report: entry.report.clone(),
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

    match protocol {
        Protocol::Udp => {
            // bind the port; a clear line and a fixed exit code instead of a panic loop
            let bound = UdpSocketServer::try_new(&listen_addr).await;
            let udp_socket_server = match bound {
                Ok(s) => s,
                Err(e) => {
                    let (message, code) = bind_error_message(&listen_addr, &e);
                    eprintln!("{message}");
                    std::process::exit(code);
                }
            };

            // the dashboard socket only after the port is ours, so a failed start leaves no socket file
            let unix_socket_server = UnixSocketServer::new(&socket_path);

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Error, ErrorKind};
    use std::net::{SocketAddr, UdpSocket};

    // a valid report for the given device
    fn report(device_id: &str) -> Report {
        Report {
            device_id: device_id.to_string(),
            label: "Test".to_string(),
            hostname: "test-host".to_string(),
            ip: String::new(),
            cpu_pct: 1.0,
            mem_used_mb: 1.0,
            mem_total_mb: 2.0,
            disk_used_gb: 1.0,
            disk_total_gb: 2.0,
            temp_c: 40.0,
            uptime_s: 10.0,
        }
    }

    // ---- ownership by IP (T-033) ----

    #[test]
    fn same_ip_different_port_is_the_same_owner() {
        let mut table: DeviceTable = HashMap::new();
        let now = Instant::now();
        let first: SocketAddr = "192.168.1.60:40000".parse().unwrap();
        let restarted: SocketAddr = "192.168.1.60:51234".parse().unwrap();

        store_report(&mut table, report("my-pc"), first, now);
        let outcome = store_report(&mut table, report("my-pc"), restarted, now);

        assert_eq!(outcome, InsertOutcome::Stored);
    }

    #[test]
    fn different_ip_is_a_different_owner() {
        let mut table: DeviceTable = HashMap::new();
        let now = Instant::now();
        let first: SocketAddr = "192.168.1.60:40000".parse().unwrap();
        let other: SocketAddr = "192.168.1.61:40000".parse().unwrap();

        store_report(&mut table, report("my-pc"), first, now);
        let outcome = store_report(&mut table, report("my-pc"), other, now);

        let owner: std::net::IpAddr = "192.168.1.60".parse().unwrap();
        assert_eq!(outcome, InsertOutcome::IdInUse { owner: owner });
    }

    #[test]
    fn ipv4_mapped_ipv6_source_is_the_same_owner_as_plain_ipv4() {
        // a daemon on [::] sees IPv4 peers as ::ffff:a.b.c.d
        let mut table: DeviceTable = HashMap::new();
        let now = Instant::now();
        let plain: SocketAddr = "192.168.1.60:40000".parse().unwrap();
        let mapped: SocketAddr = "[::ffff:192.168.1.60]:40001".parse().unwrap();

        store_report(&mut table, report("my-pc"), plain, now);
        let outcome = store_report(&mut table, report("my-pc"), mapped, now);

        assert_eq!(outcome, InsertOutcome::Stored);
    }

    #[test]
    fn reply_source_is_the_canonical_ip() {
        // the same IP is the owner key and the limiter key, so a daemon on [::]
        // cannot give an IPv4 peer two reply budgets
        let mapped: SocketAddr = "[::ffff:192.168.1.60]:40001".parse().unwrap();
        let plain: SocketAddr = "192.168.1.60:40000".parse().unwrap();
        let expected: std::net::IpAddr = "192.168.1.60".parse().unwrap();

        assert_eq!(reply_source(mapped), expected);
        assert_eq!(reply_source(plain), expected);
    }

    #[test]
    fn limiter_remembers_every_source_it_can_answer_in_one_window() {
        /*
         * the limiter forgets the oldest source when full. If it could hold
         * fewer sources than it may answer in one REPLY_EVERY window, a flood
         * of new sources would push out a source still inside its window and
         * that source would get a second reply early
         */
        let answerable = REPLY_EVERY.as_secs() * REPLIES_PER_SECOND as u64;
        assert!(MAX_REPLY_SOURCES as u64 >= answerable);
    }

    // ---- end to end over real sockets (T-033) ----

    // starts udp_server on a free loopback port; returns its address and the task
    async fn start_daemon() -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let server = UdpSocketServer::try_new("127.0.0.1:0").await.unwrap();
        let addr = server.local_addr().unwrap();
        let state: SharedState = Arc::new(RwLock::new(HashMap::new()));
        let task = tokio::spawn(udp_server(server, state));
        (addr, task)
    }

    // a blocking socket on the given local IP, with a short read timeout
    fn reporter_socket(local_ip: &str) -> UdpSocket {
        let socket = UdpSocket::bind(format!("{local_ip}:0")).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        socket
    }

    // sends one packet of the given report, asking for an ack or not
    fn send_packet(socket: &UdpSocket, daemon: SocketAddr, device_id: &str, ack: bool) {
        let packet = ReportPacket {
            report: report(device_id),
            ack: ack,
        };
        let bytes = serde_json::to_vec(&packet).unwrap();
        socket.send_to(&bytes, daemon).unwrap();
    }

    // the next reply on the socket, or None after the read timeout
    fn read_reply(socket: &UdpSocket) -> Option<Reply> {
        let mut buf = [0u8; 512];
        let received = socket.recv_from(&mut buf);
        match received {
            Ok((len, _)) => Some(serde_json::from_slice(&buf[..len]).unwrap()),
            Err(_) => None,
        }
    }

    #[tokio::test]
    async fn second_source_with_same_id_gets_id_in_use_reply() {
        let (daemon, task) = start_daemon().await;
        let first = reporter_socket("127.0.0.1");
        let second = reporter_socket("127.0.0.2");

        let replies = tokio::task::spawn_blocking(move || {
            send_packet(&first, daemon, "dev-server", false);
            // let the daemon store the first report before the second arrives
            std::thread::sleep(Duration::from_millis(100));
            send_packet(&second, daemon, "dev-server", false);
            (read_reply(&second), read_reply(&first))
        })
        .await
        .unwrap();
        task.abort();

        let (to_second, to_first) = replies;
        let expected = Reply::id_in_use("dev-server", "127.0.0.1".parse().unwrap());
        assert_eq!(to_second, Some(expected));
        assert_eq!(to_first, None, "the owner gets no reply");
    }

    #[tokio::test]
    async fn report_asking_for_ack_gets_ok_reply() {
        let (daemon, task) = start_daemon().await;
        let socket = reporter_socket("127.0.0.1");

        let reply = tokio::task::spawn_blocking(move || {
            send_packet(&socket, daemon, "my-pc", true);
            read_reply(&socket)
        })
        .await
        .unwrap();
        task.abort();

        assert_eq!(reply, Some(Reply::ack("my-pc")));
    }

    #[tokio::test]
    async fn invalid_report_gets_no_reply() {
        let (daemon, task) = start_daemon().await;
        let socket = reporter_socket("127.0.0.1");

        let reply = tokio::task::spawn_blocking(move || {
            // junk, then a report with a bad id that asks for an ack
            socket.send_to(b"not json", daemon).unwrap();
            send_packet(&socket, daemon, "bad id", true);
            read_reply(&socket)
        })
        .await
        .unwrap();
        task.abort();

        assert_eq!(reply, None);
    }

    #[tokio::test]
    async fn repeated_duplicates_get_one_reply_per_window() {
        let (daemon, task) = start_daemon().await;
        let first = reporter_socket("127.0.0.1");
        let second = reporter_socket("127.0.0.2");

        let replies = tokio::task::spawn_blocking(move || {
            send_packet(&first, daemon, "dev-server", false);
            std::thread::sleep(Duration::from_millis(100));
            send_packet(&second, daemon, "dev-server", false);
            send_packet(&second, daemon, "dev-server", false);
            send_packet(&second, daemon, "dev-server", false);
            (read_reply(&second), read_reply(&second))
        })
        .await
        .unwrap();
        task.abort();

        let (one, two) = replies;
        assert!(one.is_some());
        assert_eq!(two, None, "REPLY_EVERY limits replies to one per source");
    }

    // ---- bind errors (T-027) ----

    #[test]
    fn addr_in_use_gives_exit_11_and_names_the_port() {
        let error = Error::new(ErrorKind::AddrInUse, "Address already in use");

        let (message, code) = bind_error_message("0.0.0.0:8765", &error);

        assert_eq!(code, 11);
        assert_eq!(code, EXIT_PORT_IN_USE);
        assert_eq!(
            message,
            "deskpane-daemon: error: UDP port 8765 is already in use by another program. Find it with: sudo ss -ulpn 'sport = :8765'"
        );
    }

    #[test]
    fn addr_in_use_takes_the_port_from_listen_addr() {
        let error = Error::new(ErrorKind::AddrInUse, "Address already in use");

        let (message, _code) = bind_error_message("127.0.0.1:9999", &error);

        assert!(message.contains("UDP port 9999"));
        assert!(message.contains("'sport = :9999'"));
    }

    #[test]
    fn other_bind_error_gives_exit_1() {
        let error = Error::new(ErrorKind::InvalidInput, "invalid socket address");

        let (message, code) = bind_error_message("not-an-address", &error);

        assert_eq!(code, 1);
        assert!(message.starts_with("deskpane-daemon: error: "));
        assert!(message.contains("not-an-address"));
        assert!(!message.contains('\n'));
    }
}
