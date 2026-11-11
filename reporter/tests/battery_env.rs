//! Runs the real `deskpane report` with BATTERY in the environment, the way
//! the systemd unit does (EnvironmentFile=/etc/deskpane/env), and with the
//! older command line shapes, so a rollback to an older binary or an older
//! unit keeps the reporter running (T-064 review, MEDIUM 1).
#![cfg(unix)]

use std::io::{Read, Write};
use std::net::UdpSocket;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::Duration;

use serde_json::Value;

// the deskpane binary cargo built for this test run
const DESKPANE: &str = env!("CARGO_BIN_EXE_deskpane");
// the reporter unit the Pi bundle installs
const REPORTER_UNIT: &str = include_str!("../../deploy/systemd/deskpane-reporter.service");

// kills the reporter when the test ends, even when it fails
struct Reporter {
    child: Child,
}

impl Drop for Reporter {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// deletes the fake socket file when the test ends
struct SocketFile {
    path: PathBuf,
}

impl Drop for SocketFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/*
 * a fake pisugar-server that answers every query: battery 87, on power.
 * The thread runs until the test process exits.
 */
fn fake_pisugar(name: &str) -> SocketFile {
    let path = PathBuf::from(format!(
        "/tmp/phd-test-{}-{}.sock",
        std::process::id(),
        name
    ));
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).unwrap();
    thread::spawn(move || loop {
        let accepted = listener.accept();
        let mut stream = match accepted {
            Ok((s, _)) => s,
            Err(_) => return,
        };
        let mut received: Vec<u8> = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            let read = stream.read(&mut byte);
            match read {
                Ok(1) => {}
                _ => break,
            }
            if byte[0] == b'\n' {
                break;
            }
            received.push(byte[0]);
        }
        let command = String::from_utf8_lossy(&received).to_string();
        let reply = match command.as_str() {
            "get battery" => "battery: 87\n",
            "get battery_power_plugged" => "battery_power_plugged: true\n",
            _ => "battery_charging: true\n",
        };
        let _ = stream.write_all(reply.as_bytes());
    });
    SocketFile { path: path }
}

/*
 * starts deskpane report against a local UDP socket and returns the first
 * report it sends, as JSON
 *
 * args: what follows `report --daemon <addr>`
 * battery_env: the BATTERY variable, None to leave it unset
 * socket: the fake pisugar socket for PISUGAR_SOCKET
 */
fn first_report(args: &[&str], battery_env: Option<&str>, socket: &SocketFile) -> Value {
    let daemon = UdpSocket::bind("127.0.0.1:0").unwrap();
    daemon
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let address = daemon.local_addr().unwrap().to_string();

    let mut command = Command::new(DESKPANE);
    command
        .args(["report", "--daemon", &address])
        .args(args)
        .env("PISUGAR_SOCKET", &socket.path)
        .env_remove("BATTERY")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    match battery_env {
        Some(value) => {
            command.env("BATTERY", value);
        }
        None => {}
    }
    let _reporter = Reporter {
        child: command.spawn().unwrap(),
    };

    let mut buffer = [0u8; 4096];
    let (len, _) = daemon.recv_from(&mut buffer).unwrap();
    serde_json::from_slice(&buffer[..len]).unwrap()
}

#[test]
fn the_unit_passes_no_battery_flag() {
    /* an older binary rejects --battery, so after a rollback a unit that
     * passes it would crash-loop; BATTERY reaches the reporter from the
     * environment file instead */
    let mut exec_start = "";
    for line in REPORTER_UNIT.lines() {
        if line.starts_with("ExecStart=") {
            exec_start = line;
        }
    }
    assert!(exec_start.contains("deskpane report"), "{exec_start}");
    assert!(!exec_start.contains("--battery"), "{exec_start}");
    assert!(REPORTER_UNIT.contains("EnvironmentFile=/etc/deskpane/env"));
}

#[test]
fn battery_from_the_environment_is_honoured() {
    let socket = fake_pisugar("env-on");
    let report = first_report(&["--interval", "1"], Some("pisugar"), &socket);
    assert_eq!(report["battery_pct"], 87.0);
    assert_eq!(report["charging"], true);
}

#[test]
fn no_battery_variable_sends_no_battery() {
    let socket = fake_pisugar("env-off");
    let report = first_report(&["--interval", "1"], None, &socket);
    assert!(report.get("battery_pct").is_none(), "{report}");
    let none = first_report(&["--interval", "1"], Some("none"), &socket);
    assert!(none.get("battery_pct").is_none(), "{none}");
}

#[test]
fn the_t064_exec_start_shape_still_works() {
    // the unit of the T-064 build: empty ${VAR}s and --battery ${BATTERY}
    let socket = fake_pisugar("old-shape");
    let empty = first_report(
        &[
            "--device-id",
            "",
            "--label",
            "",
            "--battery",
            "",
            "--interval",
            "2",
        ],
        None,
        &socket,
    );
    assert!(empty.get("battery_pct").is_none(), "{empty}");

    let on = first_report(
        &[
            "--device-id",
            "",
            "--label",
            "",
            "--battery",
            "pisugar",
            "--interval",
            "2",
        ],
        Some("pisugar"),
        &socket,
    );
    assert_eq!(on["battery_pct"], 87.0);
}

#[test]
fn the_flag_wins_over_the_environment() {
    let socket = fake_pisugar("flag-wins");
    let report = first_report(
        &["--battery", "none", "--interval", "1"],
        Some("pisugar"),
        &socket,
    );
    assert!(report.get("battery_pct").is_none(), "{report}");
}

#[test]
fn a_bad_battery_variable_is_a_usage_error() {
    let output = Command::new(DESKPANE)
        .args(["report", "--daemon", "127.0.0.1:9", "--once"])
        .env("BATTERY", "acpi")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
}
