//! deskpane client CLI: reads this device's stats and pushes them to the
//! deskpane daemon over UDP on an interval (pausing while another machine
//! holds its id), or once with --once to check the setup. See ../../docs/CONTRACT.md for the wire shape and the replies.
//! `deskpane agent` runs as a Claude Code hook and sends one agent status
//! packet per hook event (docs/plans/2026-11-08-agent-status-view.md).
//! `install`/`uninstall` (systemd unit / Windows Service) are planned
//! increments on top of this same loop.

use std::env;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use agent::{
    build_agent_packet, map_event, parse_hook_input, resolve_project, session_sid, sub_key,
};
use clap::{Args, Parser, Subcommand};
use daemon::contract::{
    check_device_id, AgentState, Reply, Report, ReportPacket, ERROR_ID_IN_USE, MAX_NAME_BYTES,
    REPLY_VERSION,
};
use daemon::services::udp::{send_datagram, Received, UdpClient};
use serde_json::Value;
use services::capped_reader::read_capped;
use services::claude_settings::{
    backup_suffix, command_handler, merge_agent_hooks, remove_agent_hooks, AGENT_HOOK_EVENTS,
};
use services::conflict::{ConflictStep, ConflictTracker};
use services::machine_id::{derive_device_id, read_machine_id};
use services::settings_file::{
    backup_file, read_json_file, write_atomically, FileStamp, LoadedJson,
};
use services::system_stats::SystemStats;
use services::watchdog::start_watchdog;

mod agent;
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
// the hook path: most stdin kept (PostToolUse carries the whole tool output),
// and the hard deadline after which the process exits 0 whatever it is doing
const MAX_HOOK_INPUT_BYTES: usize = 8 * 1024 * 1024;
const HOOK_DEADLINE: Duration = Duration::from_millis(500);
// Claude Code's own timeout for our hook, only a backstop behind HOOK_DEADLINE
const HOOK_TIMEOUT_SECS: u64 = 2;

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
    /// Claude Code hook: read the hook JSON on stdin and send one agent status packet. Always exits 0, prints nothing on stdout.
    #[command(args_conflicts_with_subcommands = true)]
    Agent {
        #[command(subcommand)]
        action: Option<AgentAction>,
        #[command(flatten)]
        hook: HookArgs,
    },
}

/// The `deskpane agent` commands run by hand, to set up the hooks.
#[derive(Subcommand)]
enum AgentAction {
    /// Print the hooks block to merge into ~/.claude/settings.json
    HooksSnippet {
        #[command(flatten)]
        setup: SetupArgs,
    },
    /// Merge the hooks into ~/.claude/settings.json, with a backup, keeping every other entry
    InstallHooks {
        #[command(flatten)]
        setup: SetupArgs,
        /// The settings file to edit (default: ~/.claude/settings.json)
        #[arg(long)]
        settings: Option<PathBuf>,
        /// Print the merged file, write nothing
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove only the deskpane hooks from ~/.claude/settings.json, with a backup
    UninstallHooks {
        /// The settings file to edit (default: ~/.claude/settings.json)
        #[arg(long)]
        settings: Option<PathBuf>,
        /// Print the result, write nothing
        #[arg(long)]
        dry_run: bool,
    },
    /// Send a fake session through every state (about 10 s) and watch the screen
    Test {
        /// Daemon address, e.g. 192.168.1.50:8765
        #[arg(long)]
        daemon: String,
        /// Device id the fake session carries (default: derived from the machine id)
        #[arg(long)]
        device_id: Option<String>,
    },
}

/// Options the setup commands write into the hook's arguments.
#[derive(Args, Debug, Clone)]
struct SetupArgs {
    /// Daemon address, e.g. 192.168.1.50:8765
    #[arg(long)]
    daemon: String,
    /// Device id the sessions belong to, e.g. dp-3f9a0c41 (default: derived from the machine id, once)
    #[arg(long)]
    device_id: Option<String>,
    /// Show this name instead of the project folder name
    #[arg(long)]
    project: Option<String>,
    /// Send no project name (the screen shows "?")
    #[arg(long)]
    hide_project: bool,
}

/// Options of the hook path (`deskpane agent --daemon ...`).
#[derive(Args, Debug, Clone)]
struct HookArgs {
    /// Daemon address, e.g. 192.168.1.50:8765
    #[arg(long)]
    daemon: Option<String>,
    /// Device id the session belongs to, e.g. dp-3f9a0c41 (default: derived from the machine id)
    #[arg(long)]
    device_id: Option<String>,
    /// Show this name instead of the project folder name
    #[arg(long)]
    project: Option<String>,
    /// Send no project name (the screen shows "?")
    #[arg(long)]
    hide_project: bool,
    /// Print one line per hook on stderr (never on stdout)
    #[arg(long)]
    verbose: bool,
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

// the deadline action of the hook path: leave at once, successfully
fn exit_quietly() {
    std::process::exit(0);
}

// milliseconds since the Unix epoch, 0 if the clock is before it
fn now_ms() -> u64 {
    let since = SystemTime::now().duration_since(UNIX_EPOCH);
    match since {
        Ok(d) => d.as_millis() as u64,
        Err(_) => 0,
    }
}

/*
 * true when the command line is the hook path (`deskpane agent ...` without a
 * setup subcommand): a usage error there must exit 0, because exit 2 from a
 * PreToolUse or PermissionRequest hook would block the tool call
 */
fn is_hook_call(args: &[String]) -> bool {
    if args.len() < 2 || args[1] != "agent" {
        return false;
    }
    if args.len() == 2 {
        return true;
    }
    for name in AGENT_SETUP_COMMANDS {
        if args[2] == name {
            return false;
        }
    }
    true
}

// the subcommands of `deskpane agent` that are run by hand, not by a hook
const AGENT_SETUP_COMMANDS: [&str; 5] = [
    "hooks-snippet",
    "install-hooks",
    "uninstall-hooks",
    "test",
    "help",
];

/*
 * the device id an agent packet carries: --device-id when given (checked
 * against the id rule), else derived from the machine id
 *
 * returns: the id, or Err with the reason (for --verbose only)
 */
fn hook_device_id(device_id: &Option<String>) -> Result<String, String> {
    if is_missing(device_id) {
        let machine_id = read_machine_id();
        return match machine_id {
            Some(m) => Ok(derive_device_id(&m)),
            None => Err(format!(
                "no machine id ({MACHINE_ID_SOURCE}), pass --device-id"
            )),
        };
    }

    let given = match device_id {
        Some(text) => text.clone(),
        None => String::new(),
    };
    let checked = check_device_id(&given);
    match checked {
        Ok(()) => Ok(given),
        Err(reason) => Err(reason),
    }
}

/*
 * one hook call: read stdin, map the event, send one packet. Every failure
 * ends in "send nothing"; nothing is ever printed on stdout. With --verbose,
 * one line on stderr says what happened.
 *
 * hook: the hook options from the command line
 * returns: the --verbose line
 */
fn run_hook(hook: &HookArgs) -> String {
    // sender time first, so it is as close to the event as possible
    let t = now_ms();

    // drain stdin to the end (Claude Code must never see a broken pipe)
    let mut stdin = std::io::stdin().lock();
    let read = read_capped(&mut stdin, MAX_HOOK_INPUT_BYTES);
    let bytes = match read {
        Some(b) => b,
        None => return "input too big or unreadable, nothing sent".to_string(),
    };

    let parsed = parse_hook_input(&bytes);
    let input = match parsed {
        Some(i) => i,
        None => return "not a hook input, nothing sent".to_string(),
    };

    let mapped = map_event(&input);
    let (state, doing) = match mapped {
        Some(pair) => pair,
        None => return "this event sends nothing".to_string(),
    };

    if is_missing(&hook.daemon) {
        return "no --daemon given, nothing sent".to_string();
    }
    let daemon = match &hook.daemon {
        Some(d) => d.clone(),
        None => String::new(),
    };

    let resolved_id = hook_device_id(&hook.device_id);
    let device_id = match resolved_id {
        Ok(id) => id,
        Err(reason) => return format!("{reason}, nothing sent"),
    };

    // the project folder: CLAUDE_PROJECT_DIR, else the hook's cwd
    let mut project_dir: Option<String> = None;
    let project_dir_read = env::var("CLAUDE_PROJECT_DIR");
    match project_dir_read {
        Ok(dir) => project_dir = Some(dir),
        Err(_) => {}
    }
    let project = resolve_project(
        &hook.project,
        hook.hide_project,
        &project_dir,
        &input.cwd,
        &home_folder(),
    );

    let sid = session_sid(&input.session_id);
    let sub = sub_key(&input.agent_id);
    let packet = build_agent_packet(&sid, &device_id, &project, state, &doing, t, sub);
    let serialized = serde_json::to_vec(&packet);
    let packet_bytes = match serialized {
        Ok(b) => b,
        Err(e) => return format!("serialize failed: {e}"),
    };

    let state_word = state.as_str();
    let sent = send_datagram(&daemon, &packet_bytes);
    match sent {
        Ok(()) => format!("sent {state_word} for session {sid} to {daemon}"),
        Err(reason) => format!("{reason}, nothing sent"),
    }
}

/*
 * reads and drops the hook input, under the hook deadline, so Claude Code
 * never sees a broken pipe from a hook line deskpane cannot parse
 */
fn drain_hook_stdin() {
    start_watchdog(HOOK_DEADLINE, exit_quietly);
    let mut stdin = std::io::stdin().lock();
    let read = read_capped(&mut stdin, MAX_HOOK_INPUT_BYTES);
    match read {
        Some(_) => {}
        None => {}
    }
}

/*
 * the hook path, wrapped so it can never fail the hook: a hard deadline,
 * a quiet panic, exit 0 in every case
 */
fn hook_main(hook: HookArgs) {
    start_watchdog(HOOK_DEADLINE, exit_quietly);

    // a panic must not print a backtrace into the session, or exit 101
    if !hook.verbose {
        std::panic::set_hook(Box::new(|_| {}));
    }
    let outcome = std::panic::catch_unwind(|| run_hook(&hook));

    if hook.verbose {
        match outcome {
            Ok(line) => eprintln!("deskpane agent: {line}"),
            Err(_) => eprintln!("deskpane agent: panicked, nothing sent"),
        }
    }
    std::process::exit(0);
}

/*
 * the arguments of our hook handler: deskpane agent --daemon .. --device-id ..
 * plus the project options when set
 *
 * daemon: daemon address, e.g. 192.168.1.50:8765
 * device_id: resolved once at setup, so the hook never reads the machine id
 * project: --project alias, left out when missing or empty
 * hide_project: adds --hide-project
 */
fn hook_args(
    daemon: &str,
    device_id: &str,
    project: &Option<String>,
    hide_project: bool,
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "agent".to_string(),
        "--daemon".to_string(),
        daemon.to_string(),
        "--device-id".to_string(),
        device_id.to_string(),
    ];

    if !is_missing(project) {
        let alias = match project {
            Some(text) => text.clone(),
            None => String::new(),
        };
        args.push("--project".to_string());
        args.push(alias);
    }
    if hide_project {
        args.push("--hide-project".to_string());
    }
    args
}

/*
 * where Claude Code keeps the user settings: <home>/.claude/settings.json
 *
 * home: HOME (Linux, macOS) or USERPROFILE (Windows); None or "" means unknown
 */
fn default_settings_path(home: Option<String>) -> Option<PathBuf> {
    let folder = match home {
        Some(h) => h,
        None => return None,
    };
    if folder.is_empty() {
        return None;
    }
    Some(Path::new(&folder).join(".claude").join("settings.json"))
}

// the home folder from the environment, the one Claude Code uses
fn home_folder() -> Option<String> {
    #[cfg(windows)]
    let read = env::var("USERPROFILE");
    #[cfg(not(windows))]
    let read = env::var("HOME");
    match read {
        Ok(h) => Some(h),
        Err(_) => None,
    }
}

// seconds since the Unix epoch, 0 if the clock is before it
fn now_secs() -> u64 {
    let since = SystemTime::now().duration_since(UNIX_EPOCH);
    match since {
        Ok(d) => d.as_secs(),
        Err(_) => 0,
    }
}

// a settings document as written: pretty JSON with a final newline
fn settings_bytes(value: &Value) -> Result<Vec<u8>, String> {
    let serialized = serde_json::to_string_pretty(value);
    match serialized {
        Ok(text) => Ok(format!("{text}\n").into_bytes()),
        Err(e) => Err(format!("cannot serialize the settings: {e}")),
    }
}

/*
 * writes a changed settings document: a backup of the old file first (no
 * backup, no write), then the atomic write
 *
 * path: the settings file
 * loaded: what was read, for the stamp and whether there was a file
 * new_value: the document to write
 * now_secs: for the backup name
 * returns: the backup line ("deskpane: backup: <path>" or "deskpane: created <path>")
 */
fn write_settings(
    path: &Path,
    loaded: &LoadedJson,
    new_value: &Value,
    now_secs: u64,
) -> Result<String, String> {
    let encoded = settings_bytes(new_value);
    let bytes = match encoded {
        Ok(b) => b,
        Err(reason) => return Err(reason),
    };

    let shown = path.display();
    let backup_line;
    match loaded.stamp {
        FileStamp::Present(_) => {
            let backed_up = backup_file(path, &backup_suffix(now_secs));
            let backup = match backed_up {
                Ok(b) => b,
                Err(reason) => return Err(reason),
            };
            let backup_shown = backup.display();
            backup_line = format!("deskpane: backup: {backup_shown}");
        }
        FileStamp::Missing => backup_line = format!("deskpane: created {shown}"),
    }

    let written = write_atomically(path, &bytes, loaded.stamp);
    match written {
        Ok(()) => Ok(backup_line),
        Err(reason) => Err(reason),
    }
}

// the last line after a change: running sessions may keep the old hooks
const RESTART_LINE: &str =
    "deskpane: Restart open Claude Code sessions so they pick up the change.";

/*
 * merges our handler into the settings file (install-hooks)
 *
 * path: the settings file; a missing one is created with only our block
 * handler: our hook handler
 * dry_run: print the merged file instead of writing it
 * now_secs: for the backup name
 * returns: the lines to print, or Err when the file cannot be read, is not
 *          valid JSON or has a shape we do not edit; then nothing changed
 */
fn install_hooks(
    path: &Path,
    handler: Value,
    dry_run: bool,
    now_secs: u64,
) -> Result<Vec<String>, String> {
    let read = read_json_file(path);
    let loaded = match read {
        Ok(l) => l,
        Err(reason) => return Err(reason),
    };
    let original = match &loaded.value {
        Some(v) => v.clone(),
        None => serde_json::json!({}),
    };

    let merged_result = merge_agent_hooks(original.clone(), handler);
    let merged = match merged_result {
        Ok(m) => m,
        Err(reason) => {
            let shown = path.display();
            return Err(format!("{shown}: {reason}"));
        }
    };

    let shown = path.display();
    let mut lines: Vec<String> = Vec::new();

    if dry_run {
        let encoded = settings_bytes(&merged);
        let bytes = match encoded {
            Ok(b) => b,
            Err(reason) => return Err(reason),
        };
        lines.push(format!(
            "deskpane: dry run, nothing written. {shown} would become:"
        ));
        lines.push(String::from_utf8_lossy(&bytes).trim_end().to_string());
        return Ok(lines);
    }

    // an existing file that already holds exactly our hooks is left alone
    let has_file = loaded.value.is_some();
    if has_file && merged == original {
        lines.push(format!(
            "deskpane: the deskpane hooks in {shown} are already up to date, nothing changed"
        ));
        return Ok(lines);
    }

    let written = write_settings(path, &loaded, &merged, now_secs);
    let backup_line = match written {
        Ok(line) => line,
        Err(reason) => return Err(reason),
    };

    let events = AGENT_HOOK_EVENTS.join(", ");
    lines.push(format!(
        "deskpane: added the deskpane hook to {shown} for: {events}"
    ));
    lines.push(backup_line);
    lines.push(RESTART_LINE.to_string());
    Ok(lines)
}

/*
 * removes our handlers from the settings file (uninstall-hooks)
 *
 * path, dry_run, now_secs: as for install_hooks
 * returns: the lines to print, or Err; then nothing changed
 */
fn uninstall_hooks(path: &Path, dry_run: bool, now_secs: u64) -> Result<Vec<String>, String> {
    let read = read_json_file(path);
    let loaded = match read {
        Ok(l) => l,
        Err(reason) => return Err(reason),
    };

    let shown = path.display();
    let mut lines: Vec<String> = Vec::new();
    let original = match &loaded.value {
        Some(v) => v.clone(),
        None => {
            lines.push(format!(
                "deskpane: no settings file at {shown}, nothing to remove"
            ));
            return Ok(lines);
        }
    };

    let removed_result = remove_agent_hooks(original.clone());
    let removed = match removed_result {
        Ok(r) => r,
        Err(reason) => return Err(format!("{shown}: {reason}")),
    };

    if removed == original {
        lines.push(format!(
            "deskpane: no deskpane hooks in {shown}, nothing to remove"
        ));
        return Ok(lines);
    }

    if dry_run {
        let encoded = settings_bytes(&removed);
        let bytes = match encoded {
            Ok(b) => b,
            Err(reason) => return Err(reason),
        };
        lines.push(format!(
            "deskpane: dry run, nothing written. {shown} would become:"
        ));
        lines.push(String::from_utf8_lossy(&bytes).trim_end().to_string());
        return Ok(lines);
    }

    let written = write_settings(path, &loaded, &removed, now_secs);
    let backup_line = match written {
        Ok(line) => line,
        Err(reason) => return Err(reason),
    };

    lines.push(format!("deskpane: removed the deskpane hooks from {shown}"));
    lines.push(backup_line);
    lines.push(RESTART_LINE.to_string());
    Ok(lines)
}

/*
 * the states `deskpane agent test` walks through, with their "doing" text:
 * every state the screen can show, then ended (the row walks off)
 */
fn test_sequence() -> Vec<(AgentState, &'static str)> {
    vec![
        (AgentState::Idle, ""),
        (AgentState::Working, "thinking"),
        (AgentState::NeedsYou, "allow Bash?"),
        (AgentState::Done, ""),
        (AgentState::Error, "API error: rate_limit"),
        (AgentState::Ended, ""),
    ]
}

// the project name of the fake session `deskpane agent test` sends
const TEST_PROJECT: &str = "deskpane test";
// time between two steps of `deskpane agent test` (6 steps: about 10 s)
const TEST_STEP: Duration = Duration::from_secs(2);

/*
 * sends a fake session through every state so the user can watch the screen
 * (deskpane agent test). There is no reply: the proof is visual.
 *
 * daemon: daemon address
 * device_id: the device id the fake session carries
 */
fn run_agent_test(daemon: &str, device_id: &str) {
    // a new fake session each run, so a second run is a new row
    let pid = std::process::id();
    let started = now_ms();
    let sid = session_sid(&format!("deskpane-test-{pid}-{started}"));
    println!("deskpane: sending a test session ({sid}) to {daemon}; watch the screen");

    let sequence = test_sequence();
    let mut first = true;
    for (state, doing) in sequence {
        if !first {
            thread::sleep(TEST_STEP);
        }
        first = false;

        let packet =
            build_agent_packet(&sid, device_id, TEST_PROJECT, state, doing, now_ms(), None);
        let serialized = serde_json::to_vec(&packet);
        let bytes = match serialized {
            Ok(b) => b,
            Err(e) => {
                eprintln!("deskpane: serialize failed: {e}");
                continue;
            }
        };

        let word = state.as_str();
        let sent = send_datagram(daemon, &bytes);
        match sent {
            Ok(()) => println!("deskpane: {word}"),
            Err(reason) => eprintln!("deskpane: warning: {word} not sent: {reason}"),
        }
    }
}

/*
 * checks the setup options and resolves the device id once
 *
 * setup: the options of hooks-snippet, install-hooks and test
 * returns: the device id, or exits 2 with a usage line
 */
fn resolve_setup_device_id(daemon: &str, device_id: &Option<String>) -> String {
    // the hook needs host:port; catch a typo here, not silently in every hook
    let has_port = daemon.rsplit_once(':');
    match has_port {
        Some((host, port)) => {
            if host.is_empty() || port.is_empty() {
                eprintln!("deskpane: error: --daemon must be host:port, e.g. 192.168.1.50:8765");
                std::process::exit(EXIT_USAGE);
            }
        }
        None => {
            eprintln!("deskpane: error: --daemon must be host:port, e.g. 192.168.1.50:8765");
            std::process::exit(EXIT_USAGE);
        }
    }

    let resolved = resolve_identity(device_id.clone(), None, read_machine_id(), "");
    match resolved {
        Ok((id, _label)) => id,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(EXIT_USAGE);
        }
    }
}

/*
 * a warning when the hook would run a binary from a build or temp folder:
 * cargo clean or a reboot removes it, and every hook then fails quietly
 *
 * exe: this binary's path (current_exe)
 * temp_dir: the system temp folder
 * returns: the warning, or None for an installed binary
 */
fn exe_location_warning(exe: &Path, temp_dir: &Path) -> Option<String> {
    let shown = exe.display();
    let text = shown.to_string();
    let normal = text.replace('\\', "/");

    // a cargo build folder: a path part named exactly "target"
    let mut in_target = false;
    for part in normal.split('/') {
        if part == "target" {
            in_target = true;
        }
    }

    let in_temp = !temp_dir.as_os_str().is_empty() && exe.starts_with(temp_dir);

    if in_target || in_temp {
        return Some(format!(
            "deskpane: warning: the hooks will run {shown}, which is in a build or temp folder and may disappear. Install deskpane (for example cargo install --path reporter) and run install-hooks from there"
        ));
    }
    None
}

// prints exe_location_warning for this binary on stderr, if there is one
fn warn_about_exe_location() {
    let found = env::current_exe();
    let exe = match found {
        Ok(p) => p,
        Err(_) => return,
    };
    let warning = exe_location_warning(&exe, &env::temp_dir());
    match warning {
        Some(text) => eprintln!("{text}"),
        None => {}
    }
}

/*
 * our hook handler for this machine: the absolute path of this binary, in
 * exec form, with the setup options
 *
 * setup: the options given to hooks-snippet or install-hooks
 * returns: the handler, or exits 1 when the binary's path is unknown
 */
fn setup_handler(setup: &SetupArgs) -> Value {
    let device_id = resolve_setup_device_id(&setup.daemon, &setup.device_id);
    let args = hook_args(
        &setup.daemon,
        &device_id,
        &setup.project,
        setup.hide_project,
    );

    let exe = env::current_exe();
    let command = match exe {
        Ok(p) => p.to_string_lossy().to_string(),
        Err(e) => {
            eprintln!("deskpane: error: cannot find the path of this program: {e}");
            std::process::exit(1);
        }
    };
    command_handler(&command, &args, HOOK_TIMEOUT_SECS)
}

/*
 * the settings file a setup command edits: --settings, else the user
 * settings in the home folder
 *
 * returns: the path, or exits 2 when there is no home folder
 */
fn settings_path(settings: &Option<PathBuf>) -> PathBuf {
    match settings {
        Some(path) => path.clone(),
        None => {
            let default = default_settings_path(home_folder());
            match default {
                Some(path) => path,
                None => {
                    eprintln!(
                        "deskpane: error: no home folder found. Pass the file: --settings <path>"
                    );
                    std::process::exit(EXIT_USAGE);
                }
            }
        }
    }
}

// prints a setup command's result, or the reason it changed nothing and exits 1
fn finish_setup(result: Result<Vec<String>, String>) {
    match result {
        Ok(lines) => {
            for line in lines {
                println!("{line}");
            }
        }
        Err(reason) => {
            eprintln!("deskpane: error: {reason}");
            eprintln!(
                "deskpane: nothing was changed. Merge the output of `deskpane agent hooks-snippet` into the file by hand."
            );
            std::process::exit(1);
        }
    }
}

// runs one `deskpane agent <subcommand>` (the setup commands, run by hand)
fn agent_setup_main(action: AgentAction) {
    match action {
        AgentAction::HooksSnippet { setup } => {
            // the snippet is our block merged into an empty settings file
            let handler = setup_handler(&setup);
            let merged = merge_agent_hooks(serde_json::json!({}), handler);
            let snippet = match merged {
                Ok(m) => m,
                Err(reason) => {
                    eprintln!("deskpane: error: {reason}");
                    std::process::exit(1);
                }
            };
            let encoded = settings_bytes(&snippet);
            match encoded {
                Ok(bytes) => print!("{}", String::from_utf8_lossy(&bytes)),
                Err(reason) => {
                    eprintln!("deskpane: error: {reason}");
                    std::process::exit(1);
                }
            }
        }
        AgentAction::InstallHooks {
            setup,
            settings,
            dry_run,
        } => {
            warn_about_exe_location();
            let handler = setup_handler(&setup);
            let path = settings_path(&settings);
            finish_setup(install_hooks(&path, handler, dry_run, now_secs()));
        }
        AgentAction::UninstallHooks { settings, dry_run } => {
            let path = settings_path(&settings);
            finish_setup(uninstall_hooks(&path, dry_run, now_secs()));
        }
        AgentAction::Test { daemon, device_id } => {
            let resolved_id = resolve_setup_device_id(&daemon, &device_id);
            run_agent_test(&daemon, &resolved_id);
        }
    }
}

fn main() {
    // e.g. deskpane report --daemon 192.168.1.50:8765 --device-id pi-zero --label Pi --interval 4
    let mut args: Vec<String> = Vec::new();
    for arg in env::args() {
        args.push(arg);
    }
    let parsed = Cli::try_parse_from(&args);
    let cli = match parsed {
        Ok(c) => c,
        Err(e) => {
            // help and version print as usual; a bad hook line exits 0 silently
            let kind = e.kind();
            let is_info = kind == clap::error::ErrorKind::DisplayHelp
                || kind == clap::error::ErrorKind::DisplayVersion;
            if !is_info && is_hook_call(&args) {
                drain_hook_stdin();
                std::process::exit(0);
            }
            e.exit();
        }
    };
    match cli.command {
        Command::Agent { action, hook } => match action {
            Some(setup) => agent_setup_main(setup),
            None => hook_main(hook),
        },
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

    // ---- agent setup (T-043) ----

    #[test]
    fn hook_args_carry_daemon_and_id() {
        let args = hook_args("192.168.1.50:8765", "dp-3f9a0c41", &None, false);
        assert_eq!(
            args,
            vec![
                "agent",
                "--daemon",
                "192.168.1.50:8765",
                "--device-id",
                "dp-3f9a0c41"
            ]
        );
    }

    #[test]
    fn hook_args_carry_project_options() {
        let alias = hook_args("192.168.1.50:8765", "my-pc", &some("work"), false);
        assert_eq!(&alias[5..], &["--project", "work"]);
        let hidden = hook_args("192.168.1.50:8765", "my-pc", &None, true);
        assert_eq!(&hidden[5..], &["--hide-project"]);
        let empty_alias = hook_args("192.168.1.50:8765", "my-pc", &some(""), false);
        assert_eq!(empty_alias.len(), 5);
    }

    #[test]
    fn default_settings_path_is_in_the_home_folder() {
        let path = default_settings_path(some("/home/user")).unwrap();
        assert_eq!(
            path,
            Path::new("/home/user")
                .join(".claude")
                .join("settings.json")
        );
        assert_eq!(default_settings_path(None), None);
        assert_eq!(default_settings_path(some("")), None);
    }

    #[test]
    fn test_sequence_walks_every_state_and_ends_with_ended() {
        let sequence = test_sequence();
        let mut states: Vec<AgentState> = Vec::new();
        for (state, doing) in &sequence {
            states.push(*state);
            assert!(doing.len() <= daemon::contract::MAX_DOING_BYTES);
        }
        assert_eq!(
            states,
            vec![
                AgentState::Idle,
                AgentState::Working,
                AgentState::NeedsYou,
                AgentState::Done,
                AgentState::Error,
                AgentState::Ended
            ]
        );
    }

    #[test]
    fn hook_calls_are_told_apart_from_setup_commands() {
        // owned args, as env::args gives them
        fn args(list: &[&str]) -> Vec<String> {
            let mut owned: Vec<String> = Vec::new();
            for item in list {
                owned.push(item.to_string());
            }
            owned
        }
        assert!(is_hook_call(&args(&["deskpane", "agent"])));
        assert!(is_hook_call(&args(&[
            "deskpane", "agent", "--daemon", "x:1"
        ])));
        assert!(is_hook_call(&args(&["deskpane", "agent", "--bogus"])));
        assert!(!is_hook_call(&args(&[
            "deskpane",
            "agent",
            "install-hooks",
            "--bogus"
        ])));
        assert!(!is_hook_call(&args(&[
            "deskpane",
            "agent",
            "hooks-snippet"
        ])));
        assert!(!is_hook_call(&args(&["deskpane", "report", "--bogus"])));
        assert!(!is_hook_call(&args(&["deskpane"])));
    }

    // a fresh, empty folder for one test (never the real home folder)
    fn test_dir(name: &str) -> PathBuf {
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!("dp-main-{pid}-{name}"));
        let removed = std::fs::remove_dir_all(&dir);
        match removed {
            Ok(_) => {}
            Err(_) => {}
        }
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // the names of the files in a folder, sorted
    fn files_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            names.push(entry.unwrap().file_name().to_string_lossy().to_string());
        }
        names.sort();
        names
    }

    // our handler for the tests
    fn test_handler(daemon: &str) -> Value {
        let args = hook_args(daemon, "dp-3f9a0c41", &None, false);
        command_handler("/usr/local/bin/deskpane", &args, HOOK_TIMEOUT_SECS)
    }

    // a settings file with someone else's hook in it
    const THEIR_SETTINGS: &str = r#"{
  "model": "opus",
  "hooks": {
    "Stop": [{"hooks": [{"type": "command", "command": "/home/user/bin/notify.sh"}]}]
  }
}
"#;

    const NOW: u64 = 1794152400;

    #[test]
    fn install_into_a_missing_file_creates_it_without_backup() {
        let dir = test_dir("install-new");
        let path = dir.join(".claude").join("settings.json");

        let lines = install_hooks(&path, test_handler("192.168.1.50:8765"), false, NOW).unwrap();

        let written: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            written["hooks"]["PreToolUse"][0]["hooks"][0],
            test_handler("192.168.1.50:8765")
        );
        assert_eq!(files_in(&dir.join(".claude")), vec!["settings.json"]);
        let all = lines.join("\n");
        assert!(all.contains("Restart open Claude Code sessions"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn install_backs_up_then_merges() {
        let dir = test_dir("install-merge");
        let path = dir.join("settings.json");
        std::fs::write(&path, THEIR_SETTINGS).unwrap();

        let lines = install_hooks(&path, test_handler("192.168.1.50:8765"), false, NOW).unwrap();

        let backup = dir.join("settings.json.deskpane-backup-20261108-154000");
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), THEIR_SETTINGS);
        let written: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(written["model"], "opus");
        let stop = written["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop[0]["hooks"][0]["command"], "/home/user/bin/notify.sh");
        assert_eq!(stop[1]["hooks"][0], test_handler("192.168.1.50:8765"));
        let all = lines.join("\n");
        assert!(all.contains(&backup.display().to_string()));
        for event in AGENT_HOOK_EVENTS {
            assert!(all.contains(event));
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn install_twice_changes_nothing_and_makes_no_second_backup() {
        let dir = test_dir("install-twice");
        let path = dir.join("settings.json");
        std::fs::write(&path, THEIR_SETTINGS).unwrap();
        install_hooks(&path, test_handler("192.168.1.50:8765"), false, NOW).unwrap();
        let after_first = std::fs::read(&path).unwrap();

        let lines =
            install_hooks(&path, test_handler("192.168.1.50:8765"), false, NOW + 60).unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), after_first);
        assert_eq!(files_in(&dir).len(), 2, "settings plus one backup");
        assert!(lines.join("\n").contains("nothing changed"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn install_with_a_new_daemon_replaces_our_handler() {
        let dir = test_dir("install-update");
        let path = dir.join("settings.json");
        install_hooks(&path, test_handler("192.168.1.40:8765"), false, NOW).unwrap();

        install_hooks(&path, test_handler("192.168.1.50:8765"), false, NOW + 1).unwrap();

        let written: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let groups = written["hooks"]["SessionEnd"].as_array().unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0]["hooks"][0], test_handler("192.168.1.50:8765"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn install_refuses_invalid_json_and_changes_nothing() {
        let dir = test_dir("install-invalid");
        let path = dir.join("settings.json");
        std::fs::write(&path, "{\"hooks\": {,}").unwrap();

        let refused = install_hooks(&path, test_handler("192.168.1.50:8765"), false, NOW);

        match refused {
            Ok(_) => panic!("expected a refusal"),
            Err(reason) => assert!(reason.contains("not valid JSON")),
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"hooks\": {,}");
        assert_eq!(files_in(&dir), vec!["settings.json"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn install_refuses_hooks_that_are_not_an_object() {
        let dir = test_dir("install-badhooks");
        let path = dir.join("settings.json");
        std::fs::write(&path, "{\"hooks\": []}").unwrap();

        let refused = install_hooks(&path, test_handler("192.168.1.50:8765"), false, NOW);

        assert!(refused.is_err());
        assert_eq!(files_in(&dir), vec!["settings.json"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn install_dry_run_prints_the_merge_and_writes_nothing() {
        let dir = test_dir("install-dry");
        let path = dir.join("settings.json");
        std::fs::write(&path, THEIR_SETTINGS).unwrap();

        let lines = install_hooks(&path, test_handler("192.168.1.50:8765"), true, NOW).unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), THEIR_SETTINGS);
        assert_eq!(files_in(&dir), vec!["settings.json"]);
        let all = lines.join("\n");
        assert!(all.contains("\"PermissionRequest\""));
        assert!(all.contains("notify.sh"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn uninstall_removes_ours_with_a_backup() {
        let dir = test_dir("uninstall");
        let path = dir.join("settings.json");
        std::fs::write(&path, THEIR_SETTINGS).unwrap();
        install_hooks(&path, test_handler("192.168.1.50:8765"), false, NOW).unwrap();

        let lines = uninstall_hooks(&path, false, NOW + 60).unwrap();

        let written: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let original: Value = serde_json::from_str(THEIR_SETTINGS).unwrap();
        assert_eq!(written, original);
        assert!(dir
            .join("settings.json.deskpane-backup-20261108-154100")
            .exists());
        assert!(lines.join("\n").contains("removed"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn uninstall_without_our_hooks_or_file_changes_nothing() {
        let dir = test_dir("uninstall-none");
        let path = dir.join("settings.json");

        let no_file = uninstall_hooks(&path, false, NOW).unwrap();
        std::fs::write(&path, THEIR_SETTINGS).unwrap();
        let not_ours = uninstall_hooks(&path, false, NOW).unwrap();

        assert!(no_file.join("\n").contains("nothing"));
        assert!(not_ours.join("\n").contains("nothing"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), THEIR_SETTINGS);
        assert_eq!(files_in(&dir), vec!["settings.json"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // ---- install-hooks warning (T-045) ----

    #[test]
    fn exe_under_target_folder_gets_a_warning() {
        let temp = Path::new("/tmp");
        let exe = Path::new("/home/user/deskpane/target/release/deskpane");
        let warning = exe_location_warning(exe, temp);
        assert!(warning.unwrap().contains("target"));
        let windows = Path::new("C:\\code\\deskpane\\target\\debug\\deskpane.exe");
        assert!(exe_location_warning(windows, temp).is_some());
    }

    #[test]
    fn exe_under_temp_folder_gets_a_warning() {
        let temp = Path::new("/tmp");
        let exe = Path::new("/tmp/unpacked/deskpane");
        assert!(exe_location_warning(exe, temp).is_some());
    }

    #[test]
    fn installed_exe_gets_no_warning() {
        let temp = Path::new("/tmp");
        for exe in [
            "/usr/local/bin/deskpane",
            "/home/user/.cargo/bin/deskpane",
            "/opt/targets/deskpane",
        ] {
            assert_eq!(exe_location_warning(Path::new(exe), temp), None, "{exe}");
        }
    }
}
