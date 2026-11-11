// forbid unsafe in a plugin: the SECURITY.md dependency rule for data sources
#![forbid(unsafe_code)]

//! example-report: the simplest deskpane data-source plugin.
//!
//! It builds one device report with invented numbers and sends it to the
//! daemon, on an interval, exactly the way the reference reporter sends real
//! machine stats. It shows up as a device page on the dashboard with no new
//! panel, because it sends the existing Report kind. Copy this with
//! scripts/new-plugin.sh and replace build_report with your real source of
//! numbers.
//!
//! Usage:
//!   deskpane-example-report [--daemon ADDR] [--once]
//!     --daemon ADDR   where the daemon listens (default 192.168.1.50:8765)
//!     --once          send one report and exit (otherwise send every 5 s)

use std::process::ExitCode;
use std::thread::sleep;
use std::time::Duration;

use deskpane_sdk::contract::Report;
use deskpane_sdk::send_packet;

// where the daemon listens by default; a real install passes --daemon
const DEFAULT_DAEMON: &str = "192.168.1.50:8765";
// how often to send when not --once
const INTERVAL: Duration = Duration::from_secs(5);

/*
 * builds the demo device report. A real plugin reads its numbers from
 * somewhere (a sensor, an API, a file) here instead. The values are obvious
 * placeholders so the demo page is clearly synthetic.
 */
fn build_report() -> Report {
    Report {
        device_id: "dp-example".to_string(),
        label: "Example".to_string(),
        hostname: "example-plugin".to_string(),
        // a placeholder address in the documentation range, never a real host
        ip: "192.168.1.50".to_string(),
        cpu_pct: 12.0,
        mem_used_mb: 1024.0,
        mem_total_mb: 8192.0,
        disk_used_gb: 40.0,
        disk_total_gb: 256.0,
        temp_c: 45.0,
        uptime_s: 3600.0,
        battery_pct: None,
        charging: None,
    }
}

/*
 * reads the daemon address and the --once flag from the arguments. std only,
 * so the template pulls in no argument-parsing dependency.
 */
fn parse_args(args: &[String]) -> Result<(String, bool), String> {
    let mut daemon = DEFAULT_DAEMON.to_string();
    let mut once = false;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--once" {
            once = true;
        } else if arg == "--daemon" {
            let next = args.get(index + 1);
            let value = match next {
                Some(value) => value,
                None => return Err("--daemon needs an address".to_string()),
            };
            daemon = value.to_string();
            index += 1;
        } else {
            return Err(format!("unknown argument: {arg}"));
        }
        index += 1;
    }
    Ok((daemon, once))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let parsed = parse_args(&args);
    let (daemon, once) = match parsed {
        Ok(pair) => pair,
        Err(reason) => {
            eprintln!("deskpane-example-report: {reason}");
            return ExitCode::from(2);
        }
    };

    loop {
        let report = build_report();
        let sent = send_packet(&daemon, &report);
        match sent {
            Ok(()) => {}
            Err(reason) => eprintln!("deskpane-example-report: send failed: {reason}"),
        }
        if once {
            return ExitCode::SUCCESS;
        }
        sleep(INTERVAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // the demo report has a stable id and in-range numbers
    #[test]
    fn build_report_is_a_valid_demo_device() {
        let report = build_report();
        assert_eq!(report.device_id, "dp-example");
        assert!(report.cpu_pct >= 0.0 && report.cpu_pct <= 100.0);
        assert!(report.mem_used_mb <= report.mem_total_mb);
        assert_eq!(report.ip, "192.168.1.50");
    }

    // --once is recognised and the default daemon is used
    #[test]
    fn parse_args_defaults_and_once() {
        let parsed = parse_args(&["--once".to_string()]).unwrap();
        assert_eq!(parsed, (DEFAULT_DAEMON.to_string(), true));
    }

    // --daemon takes the next argument as the address
    #[test]
    fn parse_args_reads_the_daemon_address() {
        let args = vec!["--daemon".to_string(), "my-server:8765".to_string()];
        let parsed = parse_args(&args).unwrap();
        assert_eq!(parsed, ("my-server:8765".to_string(), false));
    }

    // a dangling --daemon and an unknown flag are errors, not panics
    #[test]
    fn parse_args_rejects_bad_input() {
        let dangling = parse_args(&["--daemon".to_string()]);
        assert!(dangling.is_err());
        let unknown = parse_args(&["--what".to_string()]);
        assert!(unknown.is_err());
    }
}
