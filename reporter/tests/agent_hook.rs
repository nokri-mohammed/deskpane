//! Runs the real `deskpane agent` binary the way Claude Code runs a hook:
//! hook JSON on stdin, then checks it printed nothing on stdout, exited 0,
//! and sent (or did not send) one agent packet to a local UDP socket.

use std::io::Write;
use std::net::UdpSocket;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use daemon::contract::{parse_packet, validate_agent, AgentState, ParsedPacket};

// the deskpane binary cargo built for this test run
const DESKPANE: &str = env!("CARGO_BIN_EXE_deskpane");

// a PreToolUse hook input as Claude Code sends it (fake ids and paths)
const PRE_TOOL_USE: &str = r#"{"session_id":"0b9f8c2e-1d4a-4f6b-9a3e-5c7d2e1f0a9b","transcript_path":"/home/user/.claude/projects/x.jsonl","cwd":"/home/user/projects/deskpane/daemon","permission_mode":"default","hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"cargo test"},"tool_use_id":"toolu_1"}"#;

// a UDP socket standing in for the daemon, with a short read timeout
fn fake_daemon() -> (UdpSocket, String) {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket
        .set_read_timeout(Some(Duration::from_millis(400)))
        .unwrap();
    let addr = socket.local_addr().unwrap().to_string();
    (socket, addr)
}

// what one run of the binary did
struct Run {
    stdout: Vec<u8>,
    code: Option<i32>,
    took: Duration,
}

/*
 * runs deskpane with args, writes input to its stdin and closes it
 *
 * args: command line after the binary name
 * input: bytes for stdin
 */
fn run(args: &[&str], input: Vec<u8>) -> Run {
    let started = Instant::now();
    let mut child = Command::new(DESKPANE)
        .args(args)
        .env("CLAUDE_PROJECT_DIR", "/home/user/projects/deskpane")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    // write from a thread: a big input must not block the wait below
    let mut stdin = child.stdin.take().unwrap();
    let writer = thread::spawn(move || {
        // the binary may stop reading early (deadline, cap); a broken pipe is fine
        let written = stdin.write_all(&input);
        match written {
            Ok(_) => {}
            Err(_) => {}
        }
    });

    let output = child.wait_with_output().unwrap();
    writer.join().unwrap();
    Run {
        stdout: output.stdout,
        code: output.status.code(),
        took: started.elapsed(),
    }
}

// the next packet the fake daemon got, or None after its read timeout
fn received(socket: &UdpSocket) -> Option<Vec<u8>> {
    let mut buf = [0u8; 2048];
    let got = socket.recv_from(&mut buf);
    match got {
        Ok((len, _)) => Some(buf[..len].to_vec()),
        Err(_) => None,
    }
}

#[test]
fn hook_never_writes_stdout() {
    let (socket, addr) = fake_daemon();

    let result = run(
        &["agent", "--daemon", &addr, "--device-id", "dp-3f9a0c41"],
        PRE_TOOL_USE.as_bytes().to_vec(),
    );

    assert_eq!(result.code, Some(0));
    assert!(result.stdout.is_empty(), "stdout: {:?}", result.stdout);

    // and it did send the packet, which passes the daemon's own checks
    let bytes = received(&socket).unwrap();
    let packet = match parse_packet(&bytes) {
        Ok(ParsedPacket::Agent(p)) => p,
        other => panic!("expected an agent packet, got {other:?}"),
    };
    assert_eq!(validate_agent(&packet), Ok(()));
    assert_eq!(packet.sid, "578eaed40e2a");
    assert_eq!(packet.device_id, "dp-3f9a0c41");
    assert_eq!(packet.project, "deskpane");
    assert_eq!(packet.state, AgentState::Working);
    assert_eq!(packet.doing, "Bash");
    assert!(packet.t > 0);

    // nothing private went on the wire
    let text = String::from_utf8(bytes).unwrap();
    assert!(!text.contains("cargo test"));
    assert!(!text.contains("/home/user"));
    assert!(!text.contains("0b9f8c2e"));
}

#[test]
fn hook_with_verbose_still_writes_nothing_on_stdout() {
    let (_socket, addr) = fake_daemon();

    let result = run(
        &[
            "agent",
            "--daemon",
            &addr,
            "--device-id",
            "my-pc",
            "--verbose",
        ],
        PRE_TOOL_USE.as_bytes().to_vec(),
    );

    assert_eq!(result.code, Some(0));
    assert!(result.stdout.is_empty());
}

#[test]
fn project_alias_and_hide_project_reach_the_packet() {
    let (socket, addr) = fake_daemon();

    run(
        &[
            "agent",
            "--daemon",
            &addr,
            "--device-id",
            "my-pc",
            "--project",
            "work",
        ],
        PRE_TOOL_USE.as_bytes().to_vec(),
    );
    run(
        &[
            "agent",
            "--daemon",
            &addr,
            "--device-id",
            "my-pc",
            "--hide-project",
        ],
        PRE_TOOL_USE.as_bytes().to_vec(),
    );

    let first = received(&socket).unwrap();
    let second = received(&socket).unwrap();
    let first_text = String::from_utf8(first).unwrap();
    let second_text = String::from_utf8(second).unwrap();
    assert!(first_text.contains(r#""project":"work""#));
    assert!(second_text.contains(r#""project":"""#));
}

#[test]
fn bad_stdin_sends_nothing_and_exits_0() {
    let (socket, addr) = fake_daemon();

    let result = run(
        &["agent", "--daemon", &addr, "--device-id", "my-pc"],
        b"not json at all".to_vec(),
    );

    assert_eq!(result.code, Some(0));
    assert!(result.stdout.is_empty());
    assert_eq!(received(&socket), None);
}

#[test]
fn event_that_maps_to_nothing_sends_nothing() {
    let (socket, addr) = fake_daemon();
    let input = PRE_TOOL_USE.replace("PreToolUse", "SubagentStart");

    let result = run(
        &["agent", "--daemon", &addr, "--device-id", "my-pc"],
        input.into_bytes(),
    );

    assert_eq!(result.code, Some(0));
    assert_eq!(received(&socket), None);
}

#[test]
fn oversized_stdin_sends_nothing_and_exits_0() {
    let (socket, addr) = fake_daemon();
    // a valid hook input padded past the 8 MiB cap
    let padding = "a".repeat(8 * 1024 * 1024);
    let input = PRE_TOOL_USE.replace(
        r#""tool_use_id""#,
        &format!(r#""pad":"{padding}","tool_use_id""#),
    );

    let result = run(
        &["agent", "--daemon", &addr, "--device-id", "my-pc"],
        input.into_bytes(),
    );

    assert_eq!(result.code, Some(0));
    assert!(result.stdout.is_empty());
    assert_eq!(received(&socket), None);
}

#[test]
fn invalid_device_id_sends_nothing_and_exits_0() {
    let (socket, addr) = fake_daemon();

    let result = run(
        &["agent", "--daemon", &addr, "--device-id", "bad id"],
        PRE_TOOL_USE.as_bytes().to_vec(),
    );

    assert_eq!(result.code, Some(0));
    assert_eq!(received(&socket), None);
}

#[test]
fn missing_or_bad_daemon_exits_0() {
    let no_daemon = run(
        &["agent", "--device-id", "my-pc"],
        PRE_TOOL_USE.as_bytes().to_vec(),
    );
    let bad_daemon = run(
        &[
            "agent",
            "--daemon",
            "not-an-address",
            "--device-id",
            "my-pc",
        ],
        PRE_TOOL_USE.as_bytes().to_vec(),
    );

    assert_eq!(no_daemon.code, Some(0));
    assert!(no_daemon.stdout.is_empty());
    assert_eq!(bad_daemon.code, Some(0));
    assert!(bad_daemon.stdout.is_empty());
}

#[test]
fn unknown_hook_flag_exits_0_silently() {
    // a hook line from a newer setup must never block a tool call (exit 2 would)
    let result = run(
        &["agent", "--daemon", "127.0.0.1:9", "--some-future-flag"],
        PRE_TOOL_USE.as_bytes().to_vec(),
    );

    assert_eq!(result.code, Some(0));
    assert!(result.stdout.is_empty());
}

#[test]
fn hook_exits_0_by_the_deadline_when_stdin_stays_open() {
    let started = Instant::now();
    let mut child = Command::new(DESKPANE)
        .args(["agent", "--daemon", "127.0.0.1:9", "--device-id", "my-pc"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    // keep stdin open and never write: the read would block forever
    let _stdin = child.stdin.take().unwrap();

    let mut code: Option<i32> = None;
    while started.elapsed() < Duration::from_secs(5) {
        let status = child.try_wait().unwrap();
        match status {
            Some(s) => {
                code = s.code();
                break;
            }
            None => thread::sleep(Duration::from_millis(20)),
        }
    }
    let took = started.elapsed();
    // a still-running child is killed; either way it is reaped
    match code {
        Some(_) => {}
        None => child.kill().unwrap(),
    }
    child.wait().unwrap();

    assert_eq!(code, Some(0));
    assert!(took >= Duration::from_millis(450), "took {took:?}");
    assert!(took < Duration::from_secs(2), "took {took:?}");
}

#[test]
fn normal_hook_is_fast() {
    // far below the 500 ms deadline (T-047 measures the release binary for real)
    let (_socket, addr) = fake_daemon();

    let result = run(
        &["agent", "--daemon", &addr, "--device-id", "my-pc"],
        PRE_TOOL_USE.as_bytes().to_vec(),
    );

    assert_eq!(result.code, Some(0));
    assert!(
        result.took < Duration::from_millis(450),
        "took {:?}",
        result.took
    );
}

#[test]
fn unknown_hook_flag_still_drains_stdin() {
    // 1 MiB is far more than a pipe buffer: the write only completes if it is read
    let mut child = Command::new(DESKPANE)
        .args(["agent", "--daemon", "127.0.0.1:9", "--some-future-flag"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let writer = thread::spawn(move || {
        let big = vec![b' '; 1024 * 1024];
        let written = stdin.write_all(&big);
        written.is_ok()
    });

    let output = child.wait_with_output().unwrap();
    let wrote_everything = writer.join().unwrap();

    assert!(wrote_everything, "Claude Code would see a broken pipe");
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty());
}

#[test]
fn unknown_hook_flag_with_open_stdin_exits_0_by_the_deadline() {
    let started = Instant::now();
    let mut child = Command::new(DESKPANE)
        .args(["agent", "--daemon", "127.0.0.1:9", "--some-future-flag"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    // keep stdin open and never write
    let _stdin = child.stdin.take().unwrap();

    let mut code: Option<i32> = None;
    while started.elapsed() < Duration::from_secs(5) {
        let status = child.try_wait().unwrap();
        match status {
            Some(s) => {
                code = s.code();
                break;
            }
            None => thread::sleep(Duration::from_millis(20)),
        }
    }
    let took = started.elapsed();
    match code {
        Some(_) => {}
        None => child.kill().unwrap(),
    }
    child.wait().unwrap();

    assert_eq!(code, Some(0));
    assert!(took < Duration::from_secs(2), "took {took:?}");
}
