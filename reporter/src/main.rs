//! deskpane client CLI: reads this device's stats and pushes them to the
//! deskpane daemon over UDP on an interval. See ../../docs/CONTRACT.md for the
//! wire shape. `report` is the only subcommand for now; a one-shot report and
//! `install`/`uninstall` (systemd unit / Windows Service) are planned
//! increments on top of this same loop.

use std::thread;
use std::time::Duration;

use clap::{Parser, Subcommand};
use daemon::contract::Report;
use daemon::services::udp::UdpClient;
use services::system_stats::SystemStats;

mod services;

#[derive(Parser)]
#[command(name = "deskpane")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Read stats and push them to the daemon in a loop, forever.
    Report {
        /// Daemon address, e.g. 192.168.1.50:8765
        #[arg(long)]
        daemon: String,
        /// Stable id for this device, e.g. pi-zero
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

// turn the report into JSON and send it as one packet
fn send_report(client: &UdpClient, report: &Report) {
    let serialized = serde_json::to_vec(report);
    let bytes = match serialized {
        Ok(b) => b,
        Err(e) => {
            eprintln!("reporter: serialize failed: {e}");
            return;
        }
    };

    client.send(&bytes);
}

/*
 * sends this machine's stats to the daemon every few seconds, forever
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

    println!("reporter: pushing to {daemon} as '{device_id}' every {interval_secs}s");
    loop {
        let report = build_report(device_id, label, &ip, &mut stats);
        send_report(&client, &report);

        // wait before the next report
        thread::sleep(Duration::from_secs(interval_secs));
    }
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
        } => run(&daemon, &device_id, &label, interval),
    }
}
