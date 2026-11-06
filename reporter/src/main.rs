//! Reporter CLI: reads this device's stats and pushes them to the report
//! daemon over UDP on an interval. See ../../docs/CONTRACT.md for the wire
//! shape. `run` is the only subcommand for now — `report` (one-shot) and
//! `install`/`uninstall` (systemd unit / Windows Service) are planned
//! increments on top of this same loop.

use std::net::UdpSocket;
use std::path::Path;
use std::thread;
use std::time::Duration;

use clap::{Parser, Subcommand};
use daemon::contract::Report;
use sysinfo::{Components, Disks, System};

#[derive(Parser)]
#[command(name = "reporter")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Read stats and push them to the daemon in a loop, forever.
    Run {
        /// Daemon address, e.g. 192.168.1.50:8765
        #[arg(long)]
        daemon: String,
        /// Stable id for this device, e.g. my-server
        #[arg(long)]
        device_id: String,
        /// Short display name shown on the dashboard
        #[arg(long)]
        label: String,
        /// Seconds between pushes
        #[arg(long, default_value_t = 4)]
        interval: u64,
    },
}

/// Picks one representative CPU temperature out of whatever sensors are
/// available. Reliable on native Linux (`/sys/class/thermal`), usually
/// empty under WSL2 (no sensors passed through the VM) and on Windows
/// (no stable sensor API without extra drivers) — 0.0 there, which the
/// dashboard shows as "N/A".
fn read_cpu_temp(components: &mut Components) -> f64 {
    components.refresh();
    let temp = components
        .iter()
        .find(|c| {
            let label = c.label().to_ascii_lowercase();
            label.contains("cpu") || label.contains("soc") || label.contains("package")
        })
        .or_else(|| components.iter().next())
        .map(|c| c.temperature())
        .unwrap_or(0.0);
    if temp.is_nan() {
        0.0
    } else {
        temp as f64
    }
}

fn build_report(
    device_id: &str,
    label: &str,
    ip: &str,
    sys: &mut System,
    disks: &mut Disks,
    components: &mut Components,
) -> Report {
    sys.refresh_cpu_usage();
    sys.refresh_memory();
    disks.refresh();

    let hostname = System::host_name().unwrap_or_default();
    let cpu_pct = sys.global_cpu_usage() as f64;
    let mem_used_mb = sys.used_memory() as f64 / 1024.0 / 1024.0;
    let mem_total_mb = sys.total_memory() as f64 / 1024.0 / 1024.0;

    let (disk_used_gb, disk_total_gb) = disks
        .list()
        .iter()
        .find(|d| d.mount_point() == Path::new("/"))
        .or_else(|| disks.list().first())
        .map(|d| {
            let total = d.total_space() as f64 / 1e9;
            let avail = d.available_space() as f64 / 1e9;
            (total - avail, total)
        })
        .unwrap_or((0.0, 0.0));

    Report {
        device_id: device_id.to_string(),
        label: label.to_string(),
        hostname,
        ip: ip.to_string(),
        cpu_pct,
        mem_used_mb,
        mem_total_mb,
        disk_used_gb,
        disk_total_gb,
        temp_c: read_cpu_temp(components),
        uptime_s: System::uptime() as f64,
    }
}

fn run(daemon: &str, device_id: &str, label: &str, interval_secs: u64) {
    let socket = UdpSocket::bind("0.0.0.0:0").expect("bind local UDP socket");
    socket
        .connect(daemon)
        .unwrap_or_else(|e| panic!("resolve daemon address {daemon}: {e}"));
    let ip = socket
        .local_addr()
        .map(|a| a.ip().to_string())
        .unwrap_or_default();

    let mut sys = System::new_all();
    let mut disks = Disks::new_with_refreshed_list();
    let mut components = Components::new_with_refreshed_list();

    // sysinfo's CPU usage is a delta between refreshes — the very first
    // refresh has nothing to diff against, so warm it up once before the
    // real loop starts.
    sys.refresh_cpu_usage();
    thread::sleep(Duration::from_millis(250));

    println!("reporter: pushing to {daemon} as '{device_id}' every {interval_secs}s");
    loop {
        let report = build_report(device_id, label, &ip, &mut sys, &mut disks, &mut components);
        match serde_json::to_vec(&report) {
            Ok(bytes) => {
                if let Err(e) = socket.send(&bytes) {
                    eprintln!("reporter: send failed: {e}");
                }
            }
            Err(e) => eprintln!("reporter: serialize failed: {e}"),
        }
        thread::sleep(Duration::from_secs(interval_secs));
    }
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Command::Run { daemon, device_id, label, interval } => run(&daemon, &device_id, &label, interval),
    }
}
