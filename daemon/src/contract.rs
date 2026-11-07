//! The JSON shape every reporting device sends, and the shape the daemon
//! relays to the dashboard. See docs/CONTRACT.md for the full spec.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub device_id: String,
    pub label: String,
    pub hostname: String,
    pub ip: String,
    pub cpu_pct: f64,
    pub mem_used_mb: f64,
    pub mem_total_mb: f64,
    pub disk_used_gb: f64,
    pub disk_total_gb: f64,
    pub temp_c: f64,
    pub uptime_s: f64,
}

/// What the daemon pushes to the dashboard over the Unix socket
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceStatus {
    #[serde(flatten)]
    pub report: Report,
    pub online: bool,
}
