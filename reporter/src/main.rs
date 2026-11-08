//! deskpane client CLI: reads this device's stats and pushes them to the
//! deskpane daemon over UDP on an interval (pausing while another machine
//! holds its id), or once with --once to check the setup. See ../../docs/CONTRACT.md for the wire shape and the replies.
//! `install`/`uninstall` (systemd unit / Windows Service) are planned
//! increments on top of this same loop.

use std::net::IpAddr;
use std::thread;
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};
use daemon::contract::{
    check_device_id, Reply, Report, ReportPacket, ERROR_ID_IN_USE, MAX_NAME_BYTES, REPLY_VERSION,
};
use daemon::services::udp::{Received, UdpClient};
use services::conflict::{ConflictStep, ConflictTracker};
use services::machine_id::{derive_device_id, read_machine_id};
use services::system_stats::SystemStats;

mod services;

// exit codes (docs/CONTRACT.md): usage error (as clap); --once only: id taken, no answer
const EXIT_USAGE: i32 = 2;
const EXIT_ID_IN_USE: i32 = 10;
const EXIT_NO_ANSWER: i32 = 12;

// back off from "id in use" only when it lasts this long, with no longer gap
// than CONFLICT_MAX_GAP between replies (a reporter that changed IP is
// refused for up to 15 s, and must not pause for that)
const CONFLICT_GRACE: Duration = Duration::from_secs(16);
const CONFLICT_MAX_GAP: Duration = Duration::from_secs(25);
// a lasting conflict pauses reports this long, then they start again; the
// loop never exits, so a forged or passing conflict cannot stop it for good
const CONFLICT_BACKOFF: Duration = Duration::from_secs(300);
// most replies read per report cycle, so a flood of packets cannot stall the loop
const MAX_REPLIES_PER_CYCLE: usize = 8;
// most stale replies dropped after a pause (more than a socket buffer holds)
const MAX_STALE_REPLIES: usize = 256;
// --once waits this long for the daemon's answer, checking every 50 ms
const ONCE_WAIT: Duration = Duration::from_secs(3);
const ONCE_POLL: Duration = Duration::from_millis(50);
// replies are at most 192 bytes; a bigger buffer just cuts nothing
const REPLY_BUFFER_BYTES: usize = 512;

#[derive(Parser)]
#[command(name = "deskpane")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Read stats and push them to the daemon in a loop, forever (or once with --once).
    Report {
        /// Daemon address, e.g. 192.168.1.50:8765
        #[arg(long)]
        daemon: String,
        /// Stable id for this device, e.g. pi-zero (default: derived from the machine id)
        #[arg(long)]
        device_id: Option<String>,
        /// Short display name shown on the dashboard (default: the hostname)
        #[arg(long)]
        label: Option<String>,
        /// Seconds between pushes
        #[arg(long, default_value_t = 4)]
        interval: u64,
        /// Send one report, wait up to 3 s for the daemon's answer, and exit (0 ok, 10 id in use, 12 no answer)
        #[arg(long)]
        once: bool,
    },
}

// where the machine id comes from, for the "no machine id" error
#[cfg(target_os = "linux")]
const MACHINE_ID_SOURCE: &str = "/etc/machine-id is missing or empty";
#[cfg(windows)]
const MACHINE_ID_SOURCE: &str = "MachineGuid could not be read from the registry";
#[cfg(not(any(target_os = "linux", windows)))]
const MACHINE_ID_SOURCE: &str = "this system has no machine id source";

// true when an optional flag was left out or given as "" (an empty ${VAR} in a unit)
fn is_missing(value: &Option<String>) -> bool {
    match value {
        Some(text) => text.is_empty(),
        None => true,
    }
}

/*
 * cuts text to at most max_bytes without splitting a character
 *
 * text: the text to cut
 * max_bytes: longest allowed length in bytes (UTF-8)
 */
fn cut_to_bytes(text: &str, max_bytes: usize) -> String {
    let mut cut = String::new();
    for c in text.chars() {
        if cut.len() + c.len_utf8() > max_bytes {
            break;
        }
        cut.push(c);
    }
    cut
}

/*
 * picks the device id and label this client reports with
 *
 * device_id: --device-id; missing or "" means derive it from the machine id
 * label: --label; missing or "" means the hostname (or the id if that is empty)
 * machine_id: this machine's id, None when the OS has none
 * hostname: this machine's hostname, may be ""
 * returns: (device_id, label), or Err with the line to print before exiting 2
 */
fn resolve_identity(
    device_id: Option<String>,
    label: Option<String>,
    machine_id: Option<String>,
    hostname: &str,
) -> Result<(String, String), String> {
    // the id: given and checked, or derived from the machine id
    let resolved_id;
    if is_missing(&device_id) {
        let derived = match machine_id {
            Some(m) => derive_device_id(&m),
            None => {
                return Err(format!(
                    "deskpane: error: no machine id found ({MACHINE_ID_SOURCE}). Pass a name: --device-id <name>"
                ));
            }
        };
        resolved_id = derived;
    } else {
        let given = match device_id {
            Some(text) => text,
            None => String::new(),
        };

        // same rule the daemon enforces; the reason never repeats the value
        let checked = check_device_id(&given);
        match checked {
            Ok(()) => {}
            Err(_) => {
                return Err("deskpane: error: --device-id must be 1 to 64 letters, digits, dot, dash or underscore, starting with a letter or digit".to_string());
            }
        }
        resolved_id = given;
    }

    // the label: given, else the hostname cut to the contract limit, else the id
    let resolved_label;
    if is_missing(&label) {
        let from_hostname = cut_to_bytes(hostname.trim(), MAX_NAME_BYTES);
        if from_hostname.is_empty() {
            resolved_label = resolved_id.clone();
        } else {
            resolved_label = from_hostname;
        }
    } else {
        resolved_label = match label {
            Some(text) => text,
            None => String::new(),
        };
    }

    Ok((resolved_id, resolved_label))
}

/// What a reply from the daemon means for this client.
#[derive(Debug, PartialEq)]
enum ReplyAction {
    // the report was stored (only after asking with ack)
    Ack,
    // another machine owns our id; owner is None when its IP text was not an IP
    IdInUse { owner: Option<IpAddr> },
    // something unknown but well formed: print it, keep going
    Warning(String),
    // not for us, another version, or junk
    Ignore,
}

/*
 * reads one reply from the daemon
 *
 * bytes: the packet as received
 * device_id: our own id; a reply about another id is ignored
 */
fn read_reply(bytes: &[u8], device_id: &str) -> ReplyAction {
    let parsed = serde_json::from_slice::<Reply>(bytes);
    let reply = match parsed {
        Ok(r) => r,
        Err(_) => return ReplyAction::Ignore,
    };

    if reply.v != REPLY_VERSION || reply.device_id != device_id {
        return ReplyAction::Ignore;
    }

    match reply.error {
        Some(code) => {
            if code != ERROR_ID_IN_USE {
                // the code itself is not printed: it comes from the network
                return ReplyAction::Warning(
                    "deskpane: warning: the daemon answered with an unknown error code".to_string(),
                );
            }

            // only a real IP is ever printed
            let mut owner: Option<IpAddr> = None;
            match reply.owner_ip {
                Some(text) => {
                    let parsed_ip = text.parse::<IpAddr>();
                    match parsed_ip {
                        Ok(ip) => owner = Some(ip),
                        Err(_) => {}
                    }
                }
                None => {}
            }
            ReplyAction::IdInUse { owner: owner }
        }
        None => {
            if reply.ok == Some(true) {
                return ReplyAction::Ack;
            }
            ReplyAction::Ignore
        }
    }
}

// how the owner is named in messages: its IP, "(the Pi itself)" for loopback
fn describe_owner(owner: Option<IpAddr>) -> String {
    match owner {
        Some(ip) => {
            // the Pi's own reporter talks to the daemon over loopback
            if ip.is_loopback() {
                format!("{ip} (the Pi itself)")
            } else {
                ip.to_string()
            }
        }
        None => "another machine".to_string(),
    }
}

/*
 * the line printed when another machine owns our id
 *
 * device_id: our id (already checked against the id rule, safe to print)
 * owner: the owner's IP, or None when the daemon sent something else
 */
fn id_in_use_message(device_id: &str, owner: Option<IpAddr>) -> String {
    let who = describe_owner(owner);
    format!(
        "deskpane: error: device id \"{device_id}\" is already used by {who}. Pick another one: --device-id <name>"
    )
}

/*
 * the line printed when the conflict lasted past the grace period
 *
 * device_id, owner: as for id_in_use_message
 * pause: how long reports stop before the next try
 */
fn back_off_message(device_id: &str, owner: Option<IpAddr>, pause: Duration) -> String {
    let who = describe_owner(owner);
    let secs = pause.as_secs();
    format!(
        "deskpane: error: device id \"{device_id}\" is still used by {who}. Pausing reports for {secs} s, then trying again. Pick another one: --device-id <name>"
    )
}

// the line printed when --once was acknowledged
fn once_ok_message(daemon: &str, device_id: &str, label: &str) -> String {
    format!("deskpane: reported to {daemon} as {device_id} ({label})")
}

// the line printed when --once heard nothing back
fn no_answer_message(daemon: &str) -> String {
    format!(
        "deskpane: warning: no answer from {daemon} (Pi off, firewall, a daemon older than 0.1.0-beta.3, or another check from this machine in the last 10 s)"
    )
}

/*
 * longest quiet time between two "id in use" replies that still counts as
 * one conflict: 25 s, or one report interval plus 5 s when that is longer,
 * because replies cannot come faster than our reports
 */
fn conflict_max_gap(interval_secs: u64) -> Duration {
    let one_interval = Duration::from_secs(interval_secs + 5);
    if one_interval > CONFLICT_MAX_GAP {
        return one_interval;
    }
    CONFLICT_MAX_GAP
}

// fill one Report from the current stats
fn build_report(device_id: &str, label: &str, ip: &str, stats: &mut SystemStats) -> Report {
    let (mem_used_mb, mem_total_mb) = stats.memory_mb();
    let (disk_used_gb, disk_total_gb) = stats.root_disk_gb();

    Report {
        device_id: device_id.to_string(),
        label: label.to_string(),
        hostname: SystemStats::hostname(),
        ip: ip.to_string(),
        cpu_pct: stats.cpu_pct(),
        mem_used_mb: mem_used_mb,
        mem_total_mb: mem_total_mb,
        disk_used_gb: disk_used_gb,
        disk_total_gb: disk_total_gb,
        temp_c: stats.cpu_temp_c(),
        uptime_s: SystemStats::uptime_s(),
    }
}

// turn the report into JSON (with the ack flag) and send it as one packet
fn send_report(client: &UdpClient, report: Report, ack: bool) {
    let packet = ReportPacket {
        report: report,
        ack: ack,
    };
    let serialized = serde_json::to_vec(&packet);
    let bytes = match serialized {
        Ok(b) => b,
        Err(e) => {
            eprintln!("deskpane: serialize failed: {e}");
            return;
        }
    };

    client.send(&bytes);
}

/*
 * reads the replies waiting on the socket, without blocking
 *
 * max: most packets to read now; the rest wait for the next call
 * returns: each packet's bytes, oldest first
 */
fn read_waiting_replies(client: &UdpClient, max: usize) -> Vec<Vec<u8>> {
    let mut replies: Vec<Vec<u8>> = Vec::new();
    let mut buf = [0u8; REPLY_BUFFER_BYTES];
    while replies.len() < max {
        let received = client.try_receive(&mut buf);
        let len = match received {
            Received::Packet(n) => n,
            Received::Nothing => break,
            // the daemon is not up (yet); keep reporting as before
            Received::Refused => break,
        };
        replies.push(buf[..len].to_vec());
    }
    replies
}

/// What one cycle's replies said, at most one line of each kind.
#[derive(Debug, PartialEq)]
struct ReplyReview {
    // Some(owner) when at least one "id in use" reply came (the first owner)
    conflict: Option<Option<IpAddr>>,
    // the first warning line, if any
    warning: Option<String>,
}

/*
 * reads one cycle's replies
 *
 * replies: the packets read this cycle
 * device_id: our id; replies about other ids are ignored
 */
fn review_replies(replies: &[Vec<u8>], device_id: &str) -> ReplyReview {
    let mut review = ReplyReview {
        conflict: None,
        warning: None,
    };

    for bytes in replies {
        let action = read_reply(bytes, device_id);
        match action {
            ReplyAction::IdInUse { owner } => {
                if review.conflict.is_none() {
                    review.conflict = Some(owner);
                }
            }
            ReplyAction::Warning(text) => {
                if review.warning.is_none() {
                    review.warning = Some(text);
                }
            }
            ReplyAction::Ack => {}
            ReplyAction::Ignore => {}
        }
    }
    review
}

/*
 * sends this machine's stats to the daemon every few seconds, forever. When
 * another machine keeps our id past the grace period, it pauses for
 * CONFLICT_BACKOFF and tries again; it never exits for that.
 *
 * daemon: daemon address, e.g. 192.168.1.50:8765
 * device_id: stable id the dashboard tracks this device by, e.g. pi-zero
 * label: short name shown on the dashboard, e.g. Pi
 * interval_secs: seconds to wait between reports
 */
fn run(daemon: &str, device_id: &str, label: &str, interval_secs: u64) {
    let client = UdpClient::new(daemon);
    let ip = client.local_ip();
    let mut stats = SystemStats::new();
    let mut tracker = ConflictTracker::new(CONFLICT_GRACE, conflict_max_gap(interval_secs));

    loop {
        let report = build_report(device_id, label, &ip, &mut stats);
        send_report(&client, report, false);

        // replies to earlier reports; never waits on the network
        let replies = read_waiting_replies(&client, MAX_REPLIES_PER_CYCLE);
        let review = review_replies(&replies, device_id);
        match review.warning {
            Some(text) => eprintln!("{text}"),
            None => {}
        }

        // at most one conflict line per cycle, and one per back-off
        let mut owner: Option<IpAddr> = None;
        match review.conflict {
            Some(o) => owner = o,
            None => {}
        }
        let step = tracker.on_cycle(review.conflict.is_some(), Instant::now());
        match step {
            ConflictStep::Warn => eprintln!("{}", id_in_use_message(device_id, owner)),
            ConflictStep::BackOff => {
                eprintln!("{}", back_off_message(device_id, owner, CONFLICT_BACKOFF));
                thread::sleep(CONFLICT_BACKOFF);

                // replies from before the pause are stale: drop them
                read_waiting_replies(&client, MAX_STALE_REPLIES);
                tracker.after_back_off(Instant::now());
                continue;
            }
            ConflictStep::Silent => {}
            ConflictStep::Quiet => {}
        }

        // wait before the next report
        thread::sleep(Duration::from_secs(interval_secs));
    }
}

/*
 * sends one report that asks for an answer, waits up to ONCE_WAIT, and
 * exits: 0 acknowledged, 10 id in use, 12 no answer
 *
 * daemon, device_id, label: as for run
 */
fn report_once(daemon: &str, device_id: &str, label: &str) {
    let client = UdpClient::new(daemon);
    let ip = client.local_ip();
    let mut stats = SystemStats::new();

    let report = build_report(device_id, label, &ip, &mut stats);
    send_report(&client, report, true);

    // check for the answer every ONCE_POLL until ONCE_WAIT is over
    let deadline = Instant::now() + ONCE_WAIT;
    let mut buf = [0u8; REPLY_BUFFER_BYTES];
    while Instant::now() < deadline {
        let received = client.try_receive(&mut buf);
        let len = match received {
            Received::Packet(n) => n,
            Received::Nothing => {
                thread::sleep(ONCE_POLL);
                continue;
            }
            Received::Refused => break,
        };

        let action = read_reply(&buf[..len], device_id);
        match action {
            ReplyAction::Ack => {
                println!("{}", once_ok_message(daemon, device_id, label));
                std::process::exit(0);
            }
            // at install time a clash is real: no grace window
            ReplyAction::IdInUse { owner } => {
                eprintln!("{}", id_in_use_message(device_id, owner));
                std::process::exit(EXIT_ID_IN_USE);
            }
            ReplyAction::Warning(text) => eprintln!("{text}"),
            ReplyAction::Ignore => {}
        }
    }

    eprintln!("{}", no_answer_message(daemon));
    std::process::exit(EXIT_NO_ANSWER);
}

fn main() {
    // e.g. deskpane report --daemon 192.168.1.50:8765 --device-id pi-zero --label Pi --interval 4
    let cli = Cli::parse();
    match cli.command {
        Command::Report {
            daemon,
            device_id,
            label,
            interval,
            once,
        } => {
            // an id typed by hand vs one derived from the machine id, for the startup line
            let automatic = is_missing(&device_id);

            // work out the id and label before anything is sent
            let resolved = resolve_identity(
                device_id,
                label,
                read_machine_id(),
                &SystemStats::hostname(),
            );
            let (resolved_id, resolved_label) = match resolved {
                Ok(pair) => pair,
                Err(message) => {
                    eprintln!("{message}");
                    std::process::exit(EXIT_USAGE);
                }
            };

            if once {
                report_once(&daemon, &resolved_id, &resolved_label);
                return;
            }

            let how;
            if automatic {
                how = "automatic id";
            } else {
                how = "id from --device-id";
            }
            println!(
                "deskpane: reporting to {daemon} as {resolved_id} ({how}, label \"{resolved_label}\") every {interval}s"
            );
            run(&daemon, &resolved_id, &resolved_label, interval);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MACHINE: &str = "0123456789abcdef0123456789abcdef";

    // shorthand for the tests: an owned Option from a str
    fn some(text: &str) -> Option<String> {
        Some(text.to_string())
    }

    // ---- identity (T-032) ----

    #[test]
    fn explicit_device_id_is_used_as_given() {
        let resolved = resolve_identity(some("my-pc"), some("My PC"), some(MACHINE), "host");
        let (device_id, label) = resolved.unwrap();
        assert_eq!(device_id, "my-pc");
        assert_eq!(label, "My PC");
    }

    #[test]
    fn missing_device_id_means_automatic() {
        let resolved = resolve_identity(None, some("My PC"), some(MACHINE), "host");
        let (device_id, _label) = resolved.unwrap();
        assert_eq!(device_id, "dp-e2f21a97");
    }

    #[test]
    fn empty_device_id_means_automatic() {
        // the units pass --device-id ${DEVICE_ID}, which is "" when the env file leaves it empty
        let resolved = resolve_identity(some(""), some("My PC"), some(MACHINE), "host");
        let (device_id, _label) = resolved.unwrap();
        assert_eq!(device_id, "dp-e2f21a97");
    }

    #[test]
    fn missing_device_id_and_no_machine_id_is_an_error() {
        let resolved = resolve_identity(None, None, None, "host");
        match resolved {
            Ok(_) => panic!("expected an error without a machine id"),
            Err(message) => {
                assert!(message.starts_with("deskpane: error: no machine id found"));
                assert!(message.contains("--device-id <name>"));
            }
        }
    }

    #[test]
    fn explicit_device_id_works_without_machine_id() {
        let resolved = resolve_identity(some("my-pc"), None, None, "host");
        assert!(resolved.is_ok());
    }

    #[test]
    fn invalid_explicit_device_id_is_an_error() {
        let resolved = resolve_identity(some("my pc"), None, some(MACHINE), "host");
        match resolved {
            Ok(_) => panic!("expected the id with a space to be refused"),
            Err(message) => {
                assert!(message.starts_with("deskpane: error: --device-id"));
                assert!(!message.contains("my pc"));
            }
        }
    }

    #[test]
    fn empty_label_defaults_to_hostname() {
        let resolved = resolve_identity(some("my-pc"), some(""), None, "workstation");
        let (_device_id, label) = resolved.unwrap();
        assert_eq!(label, "workstation");
    }

    #[test]
    fn missing_label_defaults_to_hostname() {
        let resolved = resolve_identity(some("my-pc"), None, None, "workstation");
        let (_device_id, label) = resolved.unwrap();
        assert_eq!(label, "workstation");
    }

    #[test]
    fn long_hostname_label_is_cut_to_64_bytes_on_a_char_boundary() {
        // 63 ASCII bytes, then a 2-byte char that would cross the 64-byte line
        let hostname = format!("{}é-rest", "a".repeat(63));
        let resolved = resolve_identity(some("my-pc"), None, None, &hostname);
        let (_device_id, label) = resolved.unwrap();
        assert_eq!(label, "a".repeat(63));
        assert!(label.len() <= 64);
    }

    #[test]
    fn hostname_of_exactly_64_bytes_is_kept_whole() {
        let hostname = "b".repeat(64);
        let resolved = resolve_identity(some("my-pc"), None, None, &hostname);
        let (_device_id, label) = resolved.unwrap();
        assert_eq!(label, hostname);
    }

    // ---- replies (T-033) ----

    // the IP in a test reply
    fn lan_ip() -> std::net::IpAddr {
        "192.168.1.60".parse().unwrap()
    }

    #[test]
    fn id_in_use_message_names_the_id_and_owner() {
        let message = id_in_use_message("dev-server", Some(lan_ip()));
        assert_eq!(
            message,
            "deskpane: error: device id \"dev-server\" is already used by 192.168.1.60. Pick another one: --device-id <name>"
        );
    }

    #[test]
    fn loopback_owner_is_called_the_pi_itself() {
        let v4 = id_in_use_message("pi-zero", Some("127.0.0.1".parse().unwrap()));
        assert!(v4.contains("used by 127.0.0.1 (the Pi itself). Pick"));

        let v6 = id_in_use_message("pi-zero", Some("::1".parse().unwrap()));
        assert!(v6.contains("(the Pi itself)"));
    }

    #[test]
    fn unknown_owner_is_called_another_machine() {
        let message = id_in_use_message("dev-server", None);
        assert!(message.contains("already used by another machine. Pick"));
    }

    #[test]
    fn id_in_use_reply_is_read() {
        let bytes =
            br#"{"v":1,"error":"id_in_use","device_id":"dev-server","owner_ip":"192.168.1.60"}"#;
        let action = read_reply(bytes, "dev-server");
        assert_eq!(
            action,
            ReplyAction::IdInUse {
                owner: Some(lan_ip())
            }
        );
    }

    #[test]
    fn id_in_use_reply_with_a_bad_owner_ip_hides_it() {
        // the owner text is never printed unless it parses as an IP
        let bytes = b"{\"v\":1,\"error\":\"id_in_use\",\"device_id\":\"dev-server\",\"owner_ip\":\"\\u001b[2J\"}";
        let action = read_reply(bytes, "dev-server");
        assert_eq!(action, ReplyAction::IdInUse { owner: None });
    }

    #[test]
    fn ack_reply_is_read() {
        let bytes = br#"{"v":1,"ok":true,"device_id":"my-pc"}"#;
        assert_eq!(read_reply(bytes, "my-pc"), ReplyAction::Ack);
    }

    #[test]
    fn unknown_reply_version_is_ignored() {
        let bytes =
            br#"{"v":2,"error":"id_in_use","device_id":"dev-server","owner_ip":"192.168.1.60"}"#;
        assert_eq!(read_reply(bytes, "dev-server"), ReplyAction::Ignore);
    }

    #[test]
    fn unknown_error_code_is_a_warning_not_an_exit() {
        let bytes = br#"{"v":1,"error":"too_fast","device_id":"my-pc"}"#;
        let action = read_reply(bytes, "my-pc");
        match action {
            ReplyAction::Warning(text) => {
                assert!(text.starts_with("deskpane: warning: "));
                // the unknown code is not echoed back to the terminal
                assert!(!text.contains("too_fast"));
            }
            other => panic!("expected a warning, got {other:?}"),
        }
    }

    #[test]
    fn reply_for_another_id_is_ignored() {
        let bytes =
            br#"{"v":1,"error":"id_in_use","device_id":"someone-else","owner_ip":"192.168.1.60"}"#;
        assert_eq!(read_reply(bytes, "my-pc"), ReplyAction::Ignore);
    }

    #[test]
    fn junk_reply_is_ignored() {
        assert_eq!(read_reply(b"", "my-pc"), ReplyAction::Ignore);
        assert_eq!(read_reply(b"not json", "my-pc"), ReplyAction::Ignore);
        assert_eq!(read_reply(br#"{"v":1}"#, "my-pc"), ReplyAction::Ignore);
    }

    #[test]
    fn once_messages_name_the_daemon_and_id() {
        assert_eq!(
            once_ok_message("192.168.1.50:8765", "dp-3f9a0c41", "My PC"),
            "deskpane: reported to 192.168.1.50:8765 as dp-3f9a0c41 (My PC)"
        );
        assert_eq!(
            no_answer_message("192.168.1.50:8765"),
            "deskpane: warning: no answer from 192.168.1.50:8765 (Pi off, firewall, a daemon older than 0.1.0-beta.3, or another check from this machine in the last 10 s)"
        );
    }

    #[test]
    fn back_off_message_names_the_id_owner_and_pause() {
        let message = back_off_message("dev-server", Some(lan_ip()), Duration::from_secs(300));
        assert_eq!(
            message,
            "deskpane: error: device id \"dev-server\" is still used by 192.168.1.60. Pausing reports for 300 s, then trying again. Pick another one: --device-id <name>"
        );
        let loopback = back_off_message(
            "pi-zero",
            Some("127.0.0.1".parse().unwrap()),
            Duration::from_secs(300),
        );
        assert!(loopback.contains("127.0.0.1 (the Pi itself)"));
    }

    // a reply packet as the daemon sends it
    fn id_in_use_bytes(device_id: &str, owner: &str) -> Vec<u8> {
        format!(r#"{{"v":1,"error":"id_in_use","device_id":"{device_id}","owner_ip":"{owner}"}}"#)
            .into_bytes()
    }

    #[test]
    fn review_with_no_replies_has_no_conflict() {
        let review = review_replies(&[], "my-pc");
        assert_eq!(review.conflict, None);
        assert_eq!(review.warning, None);
    }

    #[test]
    fn review_reports_one_conflict_for_many_id_in_use_replies() {
        let replies = vec![
            id_in_use_bytes("my-pc", "192.168.1.60"),
            id_in_use_bytes("my-pc", "192.168.1.61"),
            id_in_use_bytes("my-pc", "192.168.1.62"),
        ];
        let review = review_replies(&replies, "my-pc");
        // the first owner is the one printed, once
        assert_eq!(review.conflict, Some(Some(lan_ip())));
    }

    #[test]
    fn review_ignores_replies_for_other_ids_and_junk() {
        let replies = vec![
            id_in_use_bytes("someone-else", "192.168.1.60"),
            b"junk".to_vec(),
        ];
        let review = review_replies(&replies, "my-pc");
        assert_eq!(review.conflict, None);
        assert_eq!(review.warning, None);
    }

    #[test]
    fn review_keeps_one_warning_per_cycle() {
        let unknown = br#"{"v":1,"error":"too_fast","device_id":"my-pc"}"#.to_vec();
        let replies = vec![unknown.clone(), unknown];
        let review = review_replies(&replies, "my-pc");
        assert!(review.warning.is_some());
        assert_eq!(review.conflict, None);
    }

    // a plain socket standing in for the daemon, and a client connected to it
    fn daemon_and_client() -> (std::net::UdpSocket, UdpClient, std::net::SocketAddr) {
        let daemon = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        daemon
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let client = UdpClient::new(&daemon.local_addr().unwrap().to_string());
        client.send(b"hello");
        let mut buf = [0u8; 64];
        let (_, client_addr) = daemon.recv_from(&mut buf).unwrap();
        (daemon, client, client_addr)
    }

    #[test]
    fn read_waiting_replies_takes_at_most_the_limit_per_cycle() {
        let (daemon, client, client_addr) = daemon_and_client();
        for _ in 0..10 {
            daemon
                .send_to(&id_in_use_bytes("my-pc", "192.168.1.60"), client_addr)
                .unwrap();
        }
        thread::sleep(Duration::from_millis(100));

        let first = read_waiting_replies(&client, MAX_REPLIES_PER_CYCLE);
        let second = read_waiting_replies(&client, MAX_REPLIES_PER_CYCLE);

        assert_eq!(MAX_REPLIES_PER_CYCLE, 8);
        assert_eq!(first.len(), 8);
        assert_eq!(second.len(), 2, "the rest waits for the next cycle");
    }

    #[test]
    fn read_waiting_replies_with_nothing_waiting_is_empty() {
        let (_daemon, client, _client_addr) = daemon_and_client();
        assert!(read_waiting_replies(&client, MAX_REPLIES_PER_CYCLE).is_empty());
    }

    #[test]
    fn exit_codes_match_the_contract() {
        assert_eq!(EXIT_USAGE, 2);
        assert_eq!(EXIT_ID_IN_USE, 10);
        assert_eq!(EXIT_NO_ANSWER, 12);
    }

    #[test]
    fn conflict_gap_is_25_s_for_normal_intervals() {
        assert_eq!(conflict_max_gap(4), Duration::from_secs(25));
        assert_eq!(conflict_max_gap(20), Duration::from_secs(25));
    }

    #[test]
    fn conflict_gap_grows_with_long_intervals() {
        // replies cannot come faster than the reports, so allow one interval plus slack
        assert_eq!(conflict_max_gap(60), Duration::from_secs(65));
    }

    #[test]
    fn empty_hostname_label_defaults_to_the_id() {
        let resolved = resolve_identity(None, None, some(MACHINE), "");
        let (device_id, label) = resolved.unwrap();
        assert_eq!(label, device_id);
        assert_eq!(label, "dp-e2f21a97");
    }
}
