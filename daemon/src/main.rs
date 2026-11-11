use std::collections::BTreeSet;
use std::collections::HashMap;
use std::env;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use daemon::agent_table::{
    agent_session, forget_agents, insert_agent, AgentLimits, AgentOutcome, AgentTable,
};
use daemon::contract::{
    build_reply, parse_packet, validate, validate_agent, validate_metric, AgentPacket,
    AgentSession, AgentsView, DeviceStatus, Envelope, MetricPacket, PacketError, ParsedPacket,
    Reply, Report, StatsView, Views, ENVELOPE_VERSION,
};
use daemon::device_table::{forget_silent, insert_report, DeviceTable, InsertOutcome};
use daemon::metric_table::{
    forget_metrics, insert_metric, metric_views, parse_plugin_list, MetricLimits, MetricOutcome,
    MetricTable,
};
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
// agent sessions: at most 16, at most 8 per source IP
const MAX_AGENTS: usize = 16;
const MAX_AGENTS_PER_SOURCE: usize = 8;
// another IP may take a session over after 2 minutes of silence from its owner
const AGENT_TAKEOVER_AFTER: Duration = Duration::from_secs(120);
// a packet older than the stored one by less than 5 s is a late arrival
const AGENT_REORDER_WINDOW: Duration = Duration::from_secs(5);
// most keys (main agent, subagents) one session remembers as waiting on you
const MAX_PENDING_PER_SESSION: usize = 8;
// a working session with no news for 10 min shows as quiet
const AGENT_QUIET_AFTER: Duration = Duration::from_secs(600);
// a session silent for 4 h is forgotten, whatever its state
const AGENT_FORGET_AFTER: Duration = Duration::from_secs(4 * 3600);
// plugin metrics: at most 16 plugins, at most 8 per source IP
const MAX_METRIC_PLUGINS: usize = 16;
const MAX_METRICS_PER_SOURCE: usize = 8;
// a plugin stays (shown as stale) for 5 min past its ttl, then it is forgotten
const METRIC_FORGET_GRACE: Duration = Duration::from_secs(300);
// an owner holds a plugin name for at most 15 min of silence, whatever its ttl
const METRIC_OWNER_HOLD: Duration = Duration::from_secs(900);

/// Everything the daemon keeps in memory: devices, agent sessions, plugin metrics.
struct Tables {
    devices: DeviceTable,
    agents: AgentTable,
    metrics: MetricTable,
    // the plugins turned on in PANELS; metric packets for any other name are dropped
    enabled_plugins: BTreeSet<String>,
}

type SharedState = Arc<RwLock<Tables>>;

// empty tables with no plugin turned on
#[cfg(test)]
fn new_state() -> SharedState {
    new_state_with(BTreeSet::new())
}

// empty tables, shared between the UDP server and the broadcast tick
fn new_state_with(enabled_plugins: BTreeSet<String>) -> SharedState {
    let tables = Tables {
        devices: HashMap::new(),
        agents: AgentTable::new(),
        metrics: MetricTable::new(),
        enabled_plugins: enabled_plugins,
    };
    Arc::new(RwLock::new(tables))
}

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
 * true when a device's owner is this machine (127.0.0.1, ::1 or the
 * IPv4-mapped form): the dashboard draws that device's battery on the
 * overview. Linux drops packets from the network with a loopback source.
 *
 * owner: the owner IP stored with the device
 */
fn is_local_owner(owner: std::net::IpAddr) -> bool {
    owner.to_canonical().is_loopback()
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
 * stores an agent packet under the agent table policy; like reports, the
 * owner is the source IP only (every hook runs as a new process, new port)
 *
 * peer: the address the packet came from
 */
fn store_agent(
    table: &mut AgentTable,
    packet: AgentPacket,
    peer: SocketAddr,
    now: Instant,
) -> AgentOutcome {
    let source = reply_source(peer);
    let limits = AgentLimits {
        max_agents: MAX_AGENTS,
        max_per_source: MAX_AGENTS_PER_SOURCE,
        takeover_after: AGENT_TAKEOVER_AFTER,
        reorder_window: AGENT_REORDER_WINDOW,
        max_pending: MAX_PENDING_PER_SESSION,
    };
    insert_agent(table, packet, source, now, limits)
}

/*
 * checks and stores one agent packet. Agent packets never get a reply. Every
 * log line goes through log_limiter (one per source per window), because
 * agent packets arrive on every tool call
 *
 * packet: the parsed agent packet
 * peer: the address it came from
 */
async fn handle_agent_packet(
    state: &SharedState,
    packet: AgentPacket,
    peer: SocketAddr,
    log_limiter: &mut ReplyLimiter,
) {
    let now = Instant::now();
    let source = reply_source(peer);

    // drop packets that break the contract limits (bad sid, text, version)
    let checked = validate_agent(&packet);
    match checked {
        Ok(()) => {}
        Err(reason) => {
            if log_limiter.allow(source, now) {
                eprintln!("daemon: rejected agent packet from {peer}: {reason}");
            }
            return;
        }
    }

    // store it under the table policy, then release the lock before logging
    let sid = packet.sid.clone();
    let mut tables = state.write().await;
    let outcome = store_agent(&mut tables.agents, packet, peer, now);
    drop(tables);

    // new sessions always get a line; refusals go through the limiter
    let line = agent_outcome_line(&outcome, &sid, source);
    match line {
        AgentLogLine::Always(text) => eprintln!("{text}"),
        AgentLogLine::Limited(text) => {
            if log_limiter.allow(source, now) {
                eprintln!("{text}");
            }
        }
        AgentLogLine::Silent => {}
    }
}

/*
 * checks and stores one metric packet. Metric packets never get a reply. A
 * new or taken-over plugin always gets a log line (the table caps bound
 * them); refusals go through log_limiter
 *
 * packet: the parsed metric packet
 * peer: the address it came from
 */
async fn handle_metric_packet(
    state: &SharedState,
    packet: MetricPacket,
    peer: SocketAddr,
    log_limiter: &mut ReplyLimiter,
) {
    let now = Instant::now();
    let source = reply_source(peer);

    // drop packets that break the contract limits (bad names, numbers, ttl)
    let checked = validate_metric(&packet);
    match checked {
        Ok(()) => {}
        Err(reason) => {
            if log_limiter.allow(source, now) {
                eprintln!("daemon: rejected metric packet from {peer}: {reason}");
            }
            return;
        }
    }

    // store it under the table policy, then release the lock before logging
    // (the plugin name passed validate_metric, so it is safe to log)
    let plugin = packet.plugin.clone();
    let limits = MetricLimits {
        max_plugins: MAX_METRIC_PLUGINS,
        max_per_source: MAX_METRICS_PER_SOURCE,
        max_owner_hold: METRIC_OWNER_HOLD,
    };
    let mut guard = state.write().await;
    // split the borrow: the metric table changes, the turned-on list is only read
    let tables = &mut *guard;
    let outcome = insert_metric(
        &mut tables.metrics,
        packet,
        source,
        now,
        limits,
        &tables.enabled_plugins,
    );
    drop(guard);

    // a new plugin always gets a line (only turned-on names can be new, and
    // each only once until it is forgotten); everything else is limited
    match outcome {
        MetricOutcome::Created => eprintln!("daemon: plugin {plugin} now reporting from {source}"),
        MetricOutcome::TakenOver { previous } => {
            if log_limiter.allow(source, now) {
                eprintln!("daemon: plugin {plugin} now reporting from {source} (was {previous})");
            }
        }
        MetricOutcome::NotEnabled => {
            if log_limiter.allow(source, now) {
                eprintln!("daemon: plugin {plugin} from {source} dropped: not turned on in PANELS");
            }
        }
        MetricOutcome::Stored => {}
        MetricOutcome::NotOwner { owner } => {
            if log_limiter.allow(source, now) {
                eprintln!("daemon: plugin {plugin} from {source} dropped: owned by {owner}");
            }
        }
        MetricOutcome::SourceFull => {
            if log_limiter.allow(source, now) {
                eprintln!(
                    "daemon: {source} already owns {MAX_METRICS_PER_SOURCE} plugins, dropped {plugin}"
                );
            }
        }
        MetricOutcome::TableFull => {
            if log_limiter.allow(source, now) {
                eprintln!("daemon: metric table full, dropped plugin {plugin} from {source}");
            }
        }
    }
}

/// Whether and how one agent outcome is logged.
#[derive(Debug, Clone, PartialEq, Eq)]
enum AgentLogLine {
    // logged every time (bounded by the table caps)
    Always(String),
    // logged through the per-source log limiter
    Limited(String),
    // normal outcome, no line
    Silent,
}

/*
 * the log line for one agent outcome. The sid passed validate (12 hex
 * characters), so it is safe to log
 *
 * outcome: what insert_agent did
 * sid: the session id
 * source: the canonical source IP
 */
fn agent_outcome_line(outcome: &AgentOutcome, sid: &str, source: std::net::IpAddr) -> AgentLogLine {
    match outcome {
        AgentOutcome::Created { evicted } => match evicted {
            Some(old) => AgentLogLine::Always(format!(
                "daemon: new agent session {sid} from {source} (evicted {old})"
            )),
            None => AgentLogLine::Always(format!("daemon: new agent session {sid} from {source}")),
        },
        AgentOutcome::NotOwner => AgentLogLine::Limited(format!(
            "daemon: agent session {sid} not owned by {source}, dropped"
        )),
        AgentOutcome::SourceFull => AgentLogLine::Limited(format!(
            "daemon: {source} already owns {MAX_AGENTS_PER_SOURCE} agent sessions, dropped {sid}"
        )),
        AgentOutcome::TableFull => AgentLogLine::Limited(format!(
            "daemon: agent table full, dropped new session {sid} from {source}"
        )),
        // stored, ended, ignored and late packets are normal: no line
        AgentOutcome::Stored => AgentLogLine::Silent,
        AgentOutcome::Removed => AgentLogLine::Silent,
        AgentOutcome::Ignored => AgentLogLine::Silent,
        AgentOutcome::OutOfOrder => AgentLogLine::Silent,
    }
}

/*
 * receives device reports and agent packets over UDP on one port. Reports:
 * stores the latest one per device, and answers a duplicate id (or a report
 * that asks for an ack) with one small, rate limited reply. Agent packets:
 * stored in the agent table, never answered.
 */
async fn udp_server(mut udp_socket_server: UdpSocketServer, state: SharedState) {
    let mut limiter = ReplyLimiter::new(REPLY_EVERY, REPLIES_PER_SECOND, MAX_REPLY_SOURCES);
    // a second limiter, for bad-packet and agent refusal log lines only (same limits)
    let mut log_limiter = ReplyLimiter::new(REPLY_EVERY, REPLIES_PER_SECOND, MAX_REPLY_SOURCES);

    loop {
        // wait for the next packet from a device
        let received = udp_socket_server.receive().await;
        let (bytes, peer) = match received {
            Some(x) => x,
            None => continue,
        };
        // the reply must stay smaller than this
        let request_bytes = bytes.len();

        // look at the kind first: no kind is a report, "agent" is an agent packet
        let parsed = parse_packet(bytes);
        let packet = match parsed {
            Ok(ParsedPacket::Report(p)) => p,
            Ok(ParsedPacket::Agent(agent)) => {
                handle_agent_packet(&state, agent, peer, &mut log_limiter).await;
                continue;
            }
            Ok(ParsedPacket::Metric(metric)) => {
                handle_metric_packet(&state, metric, peer, &mut log_limiter).await;
                continue;
            }
            // the reason names only a missing field or a column, never a value
            Err(PacketError::BadReport(reason)) => {
                if log_limiter.allow(reply_source(peer), Instant::now()) {
                    eprintln!("daemon: bad report from {peer}: {reason}");
                }
                continue;
            }
            Err(PacketError::BadAgent(reason)) => {
                if log_limiter.allow(reply_source(peer), Instant::now()) {
                    eprintln!("daemon: bad agent packet from {peer}: {reason}");
                }
                continue;
            }
            Err(PacketError::BadMetric(reason)) => {
                if log_limiter.allow(reply_source(peer), Instant::now()) {
                    eprintln!("daemon: bad metric packet from {peer}: {reason}");
                }
                continue;
            }
            Err(PacketError::UnknownKind) => {
                if log_limiter.allow(reply_source(peer), Instant::now()) {
                    eprintln!("daemon: unknown packet kind from {peer}");
                }
                continue;
            }
            Err(PacketError::Malformed(reason)) => {
                if log_limiter.allow(reply_source(peer), Instant::now()) {
                    eprintln!("daemon: bad packet from {peer}: {reason}");
                }
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
        let mut tables = state.write().await;
        let outcome = store_report(&mut tables.devices, packet.report, peer, now);
        // release the write lock before logging and replying
        drop(tables);

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
    let mut tables = state.write().await;
    let forgotten = forget_silent(&mut tables.devices, Instant::now(), FORGET_AFTER);
    // release the write lock before logging
    drop(tables);

    let forget_secs = FORGET_AFTER.as_secs();
    for device_id in &forgotten {
        eprintln!("daemon: forgot {device_id} after {forget_secs} s of silence");
    }
}

/*
 * removes agent sessions silent longer than AGENT_FORGET_AFTER, and logs each
 * one. Runs on the same tick as forget_silent_devices.
 */
async fn forget_silent_agents(state: &SharedState) {
    let mut tables = state.write().await;
    let forgotten = forget_agents(&mut tables.agents, Instant::now(), AGENT_FORGET_AFTER);
    // release the write lock before logging
    drop(tables);

    let forget_secs = AGENT_FORGET_AFTER.as_secs();
    for sid in &forgotten {
        eprintln!("daemon: forgot agent session {sid} after {forget_secs} s of silence");
    }
}

/*
 * removes plugins silent longer than their ttl plus METRIC_FORGET_GRACE, and
 * logs each one. Runs on the same tick as forget_silent_devices.
 */
async fn forget_silent_metrics(state: &SharedState) {
    let mut tables = state.write().await;
    let forgotten = forget_metrics(&mut tables.metrics, Instant::now(), METRIC_FORGET_GRACE);
    // release the write lock before logging
    drop(tables);

    for plugin in &forgotten {
        eprintln!("daemon: forgot plugin {plugin} after its data expired");
    }
}

/*
 * snapshot the tables (online/offline, the agent fields and the metric ages
 * computed) and broadcast them once, as one envelope line, to every socket
 * subscriber:
 * {"v":1,"views":{"stats":{"devices":[..]},"agents":{"sessions":[..]},"metrics":{..}}}
 * metrics is left out while no plugin reports
 */
async fn broadcast_statuses(state: &SharedState, broadcaster: &Broadcaster) {
    // read-lock the tables to look at every device and session
    let snapshot = state.read().await;
    // get current timestamp
    let now = Instant::now();
    // build each device's status (online if seen recently) into one list
    let mut statuses: Vec<DeviceStatus> = Vec::new();
    for entry in snapshot.devices.values() {
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
            local: is_local_owner(entry.owner),
        };
        statuses.push(status);
    }

    // each session, with the label of the device that has the same device_id
    let mut sessions: Vec<AgentSession> = Vec::new();
    for (sid, entry) in snapshot.agents.iter() {
        let mut label = String::new();
        let device = snapshot.devices.get(&entry.packet.device_id);
        match device {
            Some(d) => label = d.report.label.clone(),
            None => {}
        }

        let session = agent_session(sid, entry, &label, now, AGENT_QUIET_AFTER);
        sessions.push(session);
    }

    // each plugin's values, with their age and staleness
    let metrics = metric_views(&snapshot.metrics, now);
    // release the read lock early, before the JSON/broadcast work below
    drop(snapshot);

    let envelope = Envelope {
        v: ENVELOPE_VERSION,
        views: Views {
            stats: StatsView { devices: statuses },
            agents: AgentsView { sessions: sessions },
            metrics: metrics,
        },
    };
    broadcaster.send_json(&envelope);
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

    // the plugins turned on in PANELS (also the dashboard's list of plugin pages)
    let panels_read = env::var("PANELS");
    let panels = match panels_read {
        Ok(v) => v,
        Err(_) => String::new(),
    };
    let (enabled_plugins, skipped) = parse_plugin_list(&panels, MAX_METRIC_PLUGINS);
    for item in &skipped {
        eprintln!("daemon: PANELS: skipped {item:?} (not a plugin name, or more than {MAX_METRIC_PLUGINS})");
    }
    if enabled_plugins.is_empty() {
        eprintln!("daemon: no plugin turned on (PANELS is empty): metric packets are dropped");
    } else {
        let mut listed: Vec<String> = Vec::new();
        for name in &enabled_plugins {
            listed.push(name.clone());
        }
        let joined = listed.join(", ");
        eprintln!("daemon: plugins turned on: {joined}");
    }

    let state = new_state_with(enabled_plugins);
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
                 * every second, forget long-silent devices and agent
                 * sessions, then build a status summary and broadcast it.
                 * Forgetting here (not on insert) means an entry disappears
                 * on time even when no packets arrive, and the per-packet
                 * path stays small.
                 */
                async {
                    let mut interval = tokio::time::interval(Duration::from_secs(1));
                    loop {
                        // wait for the next 1-second tick
                        interval.tick().await;
                        forget_silent_devices(&state).await;
                        forget_silent_agents(&state).await;
                        forget_silent_metrics(&state).await;
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
    use daemon::contract::{AgentState, ReportPacket};
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
            battery_pct: None,
            charging: None,
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
    fn loopback_owner_is_local_and_a_lan_owner_is_not() {
        let v4: std::net::IpAddr = "127.0.0.1".parse().unwrap();
        let v6: std::net::IpAddr = "::1".parse().unwrap();
        let mapped: std::net::IpAddr = "::ffff:127.0.0.1".parse().unwrap();
        let lan: std::net::IpAddr = "192.168.1.60".parse().unwrap();
        let lan_mapped: std::net::IpAddr = "::ffff:192.168.1.60".parse().unwrap();
        assert!(is_local_owner(v4));
        assert!(is_local_owner(v6));
        assert!(is_local_owner(mapped));
        assert!(!is_local_owner(lan));
        assert!(!is_local_owner(lan_mapped));
    }

    #[test]
    fn stored_battery_reaches_the_table_unchanged() {
        let mut table: DeviceTable = HashMap::new();
        let mut report = report("pi-zero");
        report.battery_pct = Some(15.0);
        report.charging = Some(false);
        let peer: SocketAddr = "127.0.0.1:40000".parse().unwrap();
        store_report(&mut table, report, peer, Instant::now());
        let entry = table.get("pi-zero").unwrap();
        assert_eq!(entry.report.battery_pct, Some(15.0));
        assert_eq!(entry.report.charging, Some(false));
        assert!(is_local_owner(entry.owner));
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
        let (addr, task, _state) = start_daemon_with_state().await;
        (addr, task)
    }

    // the same, and also hands back the shared tables for envelope checks
    async fn start_daemon_with_state() -> (SocketAddr, tokio::task::JoinHandle<()>, SharedState) {
        start_daemon_with_plugins(&[]).await
    }

    // the same, with the given plugins turned on (PANELS)
    async fn start_daemon_with_plugins(
        plugins: &[&str],
    ) -> (SocketAddr, tokio::task::JoinHandle<()>, SharedState) {
        let server = UdpSocketServer::try_new("127.0.0.1:0").await.unwrap();
        let addr = server.local_addr().unwrap();
        let mut enabled = BTreeSet::new();
        for name in plugins {
            enabled.insert(name.to_string());
        }
        let state = new_state_with(enabled);
        let task = tokio::spawn(udp_server(server, state.clone()));
        (addr, task, state)
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

    // ---- agent packets (T-041) ----

    // an agent packet as JSON bytes, for the given session, device and state word
    fn agent_bytes(sid: &str, device_id: &str, state: &str, t: u64) -> Vec<u8> {
        format!(
            r#"{{"kind":"agent","v":1,"sid":"{sid}","device_id":"{device_id}","project":"deskpane","state":"{state}","doing":"allow Bash?","t":{t}}}"#
        )
        .into_bytes()
    }

    // broadcasts once and returns the line the dashboard would read, parsed
    async fn envelope_line(state: &SharedState) -> serde_json::Value {
        let broadcaster = Broadcaster::new(4);
        let mut rx = broadcaster.subscribe();
        broadcast_statuses(state, &broadcaster).await;
        let line = rx.recv().await.unwrap();
        serde_json::from_str(&line).unwrap()
    }

    #[tokio::test]
    async fn empty_tables_broadcast_an_empty_envelope() {
        let state = new_state();
        let broadcaster = Broadcaster::new(4);
        let mut rx = broadcaster.subscribe();

        broadcast_statuses(&state, &broadcaster).await;

        // the raw line, exactly as the dashboard reads it
        let line = rx.recv().await.unwrap();
        assert_eq!(
            line,
            r#"{"v":1,"views":{"stats":{"devices":[]},"agents":{"sessions":[]}}}"#
        );
    }

    #[tokio::test]
    async fn agent_packet_appears_in_the_envelope() {
        let (daemon, task, state) = start_daemon_with_state().await;
        let socket = reporter_socket("127.0.0.1");

        tokio::task::spawn_blocking(move || {
            let bytes = agent_bytes("3f9a0c41b2d7", "dp-3f9a0c41", "needs_you", 1000);
            socket.send_to(&bytes, daemon).unwrap();
            std::thread::sleep(Duration::from_millis(150));
        })
        .await
        .unwrap();
        task.abort();

        let line = envelope_line(&state).await;
        let sessions = line["views"]["agents"]["sessions"].as_array().unwrap();
        assert_eq!(sessions.len(), 1);
        let session = &sessions[0];
        assert_eq!(session["sid"], "3f9a0c41b2d7");
        assert_eq!(session["device_id"], "dp-3f9a0c41");
        assert_eq!(session["project"], "deskpane");
        assert_eq!(session["state"], "needs_you");
        assert_eq!(session["doing"], "allow Bash?");
        assert_eq!(session["quiet"], false);
        assert_eq!(session["running_s"], 0);
        assert_eq!(session["state_s"], 0);
        assert_eq!(session["silent_s"], 0);
        assert!(session["ctx"].is_null());
        assert_eq!(session["label"], "");
    }

    // a valid metric packet's bytes for the given plugin and temperature
    fn metric_bytes(plugin: &str, temp_c: f64) -> Vec<u8> {
        format!(
            r#"{{"kind":"metric","v":1,"plugin":"{plugin}","ttl_s":1800,"values":{{"temp_c":{temp_c},"rain_pct":40}}}}"#
        )
        .into_bytes()
    }

    #[tokio::test]
    async fn metric_packet_appears_in_the_envelope() {
        let (daemon, task, state) = start_daemon_with_plugins(&["weather"]).await;
        let socket = reporter_socket("127.0.0.1");

        tokio::task::spawn_blocking(move || {
            socket
                .send_to(&metric_bytes("weather", 12.4), daemon)
                .unwrap();
            std::thread::sleep(Duration::from_millis(150));
        })
        .await
        .unwrap();
        task.abort();

        let line = envelope_line(&state).await;
        let weather = &line["views"]["metrics"]["weather"];
        assert_eq!(weather["values"]["temp_c"], 12.4);
        assert_eq!(weather["values"]["rain_pct"], 40.0);
        assert_eq!(weather["age_s"], 0);
        assert_eq!(weather["stale"], false);
    }

    #[tokio::test]
    async fn a_second_source_cannot_overwrite_a_fresh_plugin() {
        let (daemon, task, state) = start_daemon_with_plugins(&["weather"]).await;
        let first = reporter_socket("127.0.0.1");
        let second = reporter_socket("127.0.0.2");

        tokio::task::spawn_blocking(move || {
            first
                .send_to(&metric_bytes("weather", 12.4), daemon)
                .unwrap();
            std::thread::sleep(Duration::from_millis(100));
            second
                .send_to(&metric_bytes("weather", 99.0), daemon)
                .unwrap();
            std::thread::sleep(Duration::from_millis(150));
        })
        .await
        .unwrap();
        task.abort();

        let line = envelope_line(&state).await;
        assert_eq!(
            line["views"]["metrics"]["weather"]["values"]["temp_c"],
            12.4
        );
    }

    #[tokio::test]
    async fn a_plugin_not_turned_on_is_not_stored() {
        let (daemon, task, state) = start_daemon_with_plugins(&["weather"]).await;
        let socket = reporter_socket("127.0.0.1");

        tokio::task::spawn_blocking(move || {
            socket
                .send_to(&metric_bytes("squatter", 1.0), daemon)
                .unwrap();
            std::thread::sleep(Duration::from_millis(150));
        })
        .await
        .unwrap();
        task.abort();

        // nothing stored, so the line has no metrics view at all
        let line = envelope_line(&state).await;
        assert!(line["views"].get("metrics").is_none());
    }

    #[tokio::test]
    async fn metric_packets_get_no_reply_and_bad_ones_are_not_stored() {
        let (daemon, task, state) = start_daemon_with_plugins(&["weather"]).await;
        let socket = reporter_socket("127.0.0.1");

        let reply = tokio::task::spawn_blocking(move || {
            // a valid one, one that fails validate_metric, one with a bad shape
            socket
                .send_to(&metric_bytes("weather", 12.4), daemon)
                .unwrap();
            socket
                .send_to(&metric_bytes("Bad-Name", 1.0), daemon)
                .unwrap();
            socket
                .send_to(
                    br#"{"kind":"metric","v":1,"plugin":"ups","values":{}}"#,
                    daemon,
                )
                .unwrap();
            read_reply(&socket)
        })
        .await
        .unwrap();
        task.abort();

        assert!(reply.is_none());
        let line = envelope_line(&state).await;
        let metrics = line["views"]["metrics"].as_object().unwrap();
        assert_eq!(metrics.len(), 1);
        assert!(metrics.contains_key("weather"));
    }

    #[tokio::test]
    async fn agent_packet_gets_no_reply() {
        let (daemon, task) = start_daemon().await;
        let socket = reporter_socket("127.0.0.1");

        let reply = tokio::task::spawn_blocking(move || {
            // a valid one, an invalid one, an unknown kind, and an ended for an unknown sid
            socket
                .send_to(&agent_bytes("3f9a0c41b2d7", "dp-1", "working", 1), daemon)
                .unwrap();
            socket
                .send_to(&agent_bytes("BAD", "dp-1", "working", 1), daemon)
                .unwrap();
            socket
                .send_to(br#"{"kind":"tasks","v":1}"#, daemon)
                .unwrap();
            socket
                .send_to(&agent_bytes("000000000009", "dp-1", "ended", 1), daemon)
                .unwrap();
            read_reply(&socket)
        })
        .await
        .unwrap();
        task.abort();

        assert_eq!(reply, None);
    }

    #[tokio::test]
    async fn ended_packet_removes_the_session_from_the_envelope() {
        let (daemon, task, state) = start_daemon_with_state().await;
        let socket = reporter_socket("127.0.0.1");

        tokio::task::spawn_blocking(move || {
            socket
                .send_to(&agent_bytes("3f9a0c41b2d7", "dp-1", "done", 1000), daemon)
                .unwrap();
            socket
                .send_to(&agent_bytes("3f9a0c41b2d7", "dp-1", "ended", 2000), daemon)
                .unwrap();
            std::thread::sleep(Duration::from_millis(150));
        })
        .await
        .unwrap();
        task.abort();

        let line = envelope_line(&state).await;
        assert!(line["views"]["agents"]["sessions"]
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn invalid_agent_packet_is_not_stored() {
        let (daemon, task, state) = start_daemon_with_state().await;
        let socket = reporter_socket("127.0.0.1");

        tokio::task::spawn_blocking(move || {
            // an upper-case sid breaks the sid rule
            socket
                .send_to(&agent_bytes("3F9A0C41B2D7", "dp-1", "working", 1), daemon)
                .unwrap();
            std::thread::sleep(Duration::from_millis(150));
        })
        .await
        .unwrap();
        task.abort();

        let line = envelope_line(&state).await;
        assert!(line["views"]["agents"]["sessions"]
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn report_and_agent_packets_both_work_on_one_port() {
        let (daemon, task, state) = start_daemon_with_state().await;
        let socket = reporter_socket("127.0.0.1");

        let reply = tokio::task::spawn_blocking(move || {
            send_packet(&socket, daemon, "my-pc", true);
            let ack = read_reply(&socket);
            socket
                .send_to(&agent_bytes("3f9a0c41b2d7", "dp-1", "working", 1), daemon)
                .unwrap();
            std::thread::sleep(Duration::from_millis(150));
            ack
        })
        .await
        .unwrap();
        task.abort();

        assert_eq!(reply, Some(Reply::ack("my-pc")));
        let line = envelope_line(&state).await;
        let devices = line["views"]["stats"]["devices"].as_array().unwrap();
        let sessions = line["views"]["agents"]["sessions"].as_array().unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0]["device_id"], "my-pc");
        assert_eq!(devices[0]["online"], true);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0]["state"], "working");
    }

    #[tokio::test]
    async fn agent_label_comes_from_the_device_table() {
        let (daemon, task, state) = start_daemon_with_state().await;
        let socket = reporter_socket("127.0.0.1");

        tokio::task::spawn_blocking(move || {
            // the reporter for my-pc has the label "Test"; dp-other has no reporter
            send_packet(&socket, daemon, "my-pc", false);
            socket
                .send_to(&agent_bytes("000000000001", "my-pc", "working", 1), daemon)
                .unwrap();
            socket
                .send_to(&agent_bytes("000000000002", "dp-other", "done", 1), daemon)
                .unwrap();
            std::thread::sleep(Duration::from_millis(150));
        })
        .await
        .unwrap();
        task.abort();

        let line = envelope_line(&state).await;
        let sessions = line["views"]["agents"]["sessions"].as_array().unwrap();
        assert_eq!(sessions.len(), 2);
        for session in sessions {
            if session["sid"] == "000000000001" {
                assert_eq!(session["label"], "Test");
            } else {
                assert_eq!(session["label"], "");
            }
        }
    }

    #[test]
    fn agent_from_same_ip_different_port_is_the_same_owner() {
        let mut table = AgentTable::new();
        let now = Instant::now();
        let first: SocketAddr = "[::ffff:192.168.1.60]:40000".parse().unwrap();
        let next_hook: SocketAddr = "192.168.1.60:51234".parse().unwrap();
        let packet = AgentPacket {
            kind: "agent".to_string(),
            v: 1,
            sid: "3f9a0c41b2d7".to_string(),
            device_id: "dp-1".to_string(),
            project: String::new(),
            state: AgentState::Working,
            doing: String::new(),
            t: 1,
            ctx: None,
            sub: None,
        };

        store_agent(&mut table, packet.clone(), first, now);
        let mut later = packet;
        later.state = AgentState::Done;
        later.t = 2;
        let outcome = store_agent(&mut table, later, next_hook, now);

        assert_eq!(outcome, AgentOutcome::Stored);
    }

    #[test]
    fn agent_limits_match_the_plan() {
        assert_eq!(MAX_AGENTS, 16);
        assert_eq!(MAX_AGENTS_PER_SOURCE, 8);
        assert_eq!(AGENT_TAKEOVER_AFTER, Duration::from_secs(120));
        assert_eq!(AGENT_REORDER_WINDOW, Duration::from_secs(5));
        assert_eq!(AGENT_QUIET_AFTER, Duration::from_secs(600));
        assert_eq!(AGENT_FORGET_AFTER, Duration::from_secs(4 * 3600));
    }

    // ---- agent log lines (T-045) ----

    fn lan_source() -> std::net::IpAddr {
        "192.168.1.60".parse().unwrap()
    }

    #[test]
    fn new_session_line_is_always_logged() {
        // the caps already bound how many new sessions there can be
        let line = agent_outcome_line(
            &AgentOutcome::Created { evicted: None },
            "3f9a0c41b2d7",
            lan_source(),
        );
        assert_eq!(
            line,
            AgentLogLine::Always(
                "daemon: new agent session 3f9a0c41b2d7 from 192.168.1.60".to_string()
            )
        );
        let evicting = agent_outcome_line(
            &AgentOutcome::Created {
                evicted: Some("000000000001".to_string()),
            },
            "3f9a0c41b2d7",
            lan_source(),
        );
        match evicting {
            AgentLogLine::Always(text) => assert!(text.contains("evicted 000000000001")),
            other => panic!("expected Always, got {other:?}"),
        }
    }

    #[test]
    fn refusals_are_limited_and_normal_outcomes_are_silent() {
        for outcome in [
            AgentOutcome::NotOwner,
            AgentOutcome::SourceFull,
            AgentOutcome::TableFull,
        ] {
            match agent_outcome_line(&outcome, "3f9a0c41b2d7", lan_source()) {
                AgentLogLine::Limited(_) => {}
                other => panic!("expected Limited for {outcome:?}, got {other:?}"),
            }
        }
        for outcome in [
            AgentOutcome::Stored,
            AgentOutcome::Removed,
            AgentOutcome::Ignored,
            AgentOutcome::OutOfOrder,
        ] {
            assert_eq!(
                agent_outcome_line(&outcome, "3f9a0c41b2d7", lan_source()),
                AgentLogLine::Silent
            );
        }
    }

    #[tokio::test]
    async fn bad_report_still_gets_no_reply_and_is_not_stored() {
        let (daemon, task, state) = start_daemon_with_state().await;
        let socket = reporter_socket("127.0.0.1");

        let reply = tokio::task::spawn_blocking(move || {
            socket
                .send_to(br#"{"device_id":"my-pc","cpu_pct":"evil"}"#, daemon)
                .unwrap();
            read_reply(&socket)
        })
        .await
        .unwrap();
        task.abort();

        assert_eq!(reply, None);
        let line = envelope_line(&state).await;
        assert!(line["views"]["stats"]["devices"]
            .as_array()
            .unwrap()
            .is_empty());
    }
}
