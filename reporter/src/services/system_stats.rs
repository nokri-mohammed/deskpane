use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use sysinfo::{Components, Disks, System};

/// Reads this machine's stats (CPU, memory, disk, temperature) via sysinfo.
/// Knows nothing about reports or where the numbers are sent.
pub struct SystemStats {
    sys: System,
    disks: Disks,
    components: Components,
}

impl SystemStats {
    pub fn new() -> Self {
        let mut sys = System::new_all();

        // CPU usage is a difference between two readings, so take a first one now
        sys.refresh_cpu_usage();
        thread::sleep(Duration::from_millis(250));

        SystemStats {
            sys: sys,
            disks: Disks::new_with_refreshed_list(),
            components: Components::new_with_refreshed_list(),
        }
    }

    // overall CPU usage in percent since the last call
    pub fn cpu_pct(&mut self) -> f64 {
        self.sys.refresh_cpu_usage();
        self.sys.global_cpu_usage() as f64
    }

    // (used, total) memory in MB
    pub fn memory_mb(&mut self) -> (f64, f64) {
        self.sys.refresh_memory();
        let used = self.sys.used_memory() as f64 / 1024.0 / 1024.0;
        let total = self.sys.total_memory() as f64 / 1024.0 / 1024.0;
        (used, total)
    }

    // (used, total) disk space in GB for the root disk
    pub fn root_disk_gb(&mut self) -> (f64, f64) {
        self.disks.refresh();

        let mut readings: Vec<DiskReading> = Vec::new();
        for disk in self.disks.list() {
            let reading = DiskReading {
                mount: disk.mount_point().to_path_buf(),
                total_bytes: disk.total_space(),
                available_bytes: disk.available_space(),
            };
            readings.push(reading);
        }

        pick_disk_gb(&readings)
    }

    // CPU temperature in °C, or 0.0 when no sensor is available
    pub fn cpu_temp_c(&mut self) -> f64 {
        self.components.refresh();

        let mut readings: Vec<TempReading> = Vec::new();
        for component in self.components.iter() {
            let reading = TempReading {
                label: component.label().to_string(),
                celsius: component.temperature(),
            };
            readings.push(reading);
        }

        pick_cpu_temp(&readings)
    }

    // this machine's hostname, or empty if the OS doesn't report one
    pub fn hostname() -> String {
        let name = System::host_name();
        match name {
            Some(n) => n,
            None => String::new(),
        }
    }

    // seconds since boot
    pub fn uptime_s() -> f64 {
        System::uptime() as f64
    }
}

struct DiskReading {
    mount: PathBuf,
    total_bytes: u64,
    available_bytes: u64,
}

struct TempReading {
    label: String,
    celsius: f32,
}

// prefers the disk mounted at "/", else the first disk, else (0, 0)
fn pick_disk_gb(disks: &[DiskReading]) -> (f64, f64) {
    let mut chosen: Option<&DiskReading> = None;
    for disk in disks {
        if disk.mount == Path::new("/") {
            chosen = Some(disk);
            break;
        }
    }

    let disk = match chosen {
        Some(d) => d,
        None => {
            let first = disks.first();
            match first {
                Some(d) => d,
                None => return (0.0, 0.0),
            }
        }
    };

    let total = disk.total_bytes as f64 / 1e9;
    let available = disk.available_bytes as f64 / 1e9;
    (total - available, total)
}

// prefers a sensor that looks like the CPU, else the first sensor, else 0.0
fn pick_cpu_temp(readings: &[TempReading]) -> f64 {
    let mut chosen: Option<f32> = None;
    for reading in readings {
        let label = reading.label.to_ascii_lowercase();
        if label.contains("cpu") || label.contains("soc") || label.contains("package") {
            chosen = Some(reading.celsius);
            break;
        }
    }

    let celsius = match chosen {
        Some(c) => c,
        None => {
            let first = readings.first();
            match first {
                Some(r) => r.celsius,
                None => return 0.0,
            }
        }
    };

    // some sensors report "not a number" when they have no reading
    if celsius.is_nan() {
        0.0
    } else {
        celsius as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disk(mount: &str, total_bytes: u64, available_bytes: u64) -> DiskReading {
        DiskReading {
            mount: PathBuf::from(mount),
            total_bytes: total_bytes,
            available_bytes: available_bytes,
        }
    }

    fn temp(label: &str, celsius: f32) -> TempReading {
        TempReading {
            label: label.to_string(),
            celsius: celsius,
        }
    }

    #[test]
    fn root_disk_is_preferred_even_when_not_first() {
        let disks = vec![disk("/boot", 1_000_000_000, 0), disk("/", 10_000_000_000, 4_000_000_000)];

        assert_eq!(pick_disk_gb(&disks), (6.0, 10.0));
    }

    #[test]
    fn first_disk_is_used_when_there_is_no_root() {
        let disks = vec![disk("C:\\", 2_000_000_000, 500_000_000), disk("D:\\", 9_000_000_000, 0)];

        assert_eq!(pick_disk_gb(&disks), (1.5, 2.0));
    }

    #[test]
    fn no_disks_gives_zero() {
        assert_eq!(pick_disk_gb(&[]), (0.0, 0.0));
    }

    #[test]
    fn empty_disk_gives_zero_used() {
        let disks = vec![disk("/", 0, 0)];

        assert_eq!(pick_disk_gb(&disks), (0.0, 0.0));
    }

    #[test]
    fn cpu_sensor_is_preferred_over_earlier_sensors() {
        let readings = vec![temp("nvme Composite", 40.0), temp("cpu_thermal", 55.0)];

        assert_eq!(pick_cpu_temp(&readings), 55.0);
    }

    #[test]
    fn sensor_label_match_ignores_case() {
        let readings = vec![temp("acpitz", 30.0), temp("Package id 0", 61.0)];

        assert_eq!(pick_cpu_temp(&readings), 61.0);
    }

    #[test]
    fn soc_sensor_counts_as_cpu() {
        let readings = vec![temp("gpu", 45.0), temp("soc_thermal", 50.0)];

        assert_eq!(pick_cpu_temp(&readings), 50.0);
    }

    #[test]
    fn first_sensor_is_used_when_none_look_like_cpu() {
        let readings = vec![temp("acpitz", 33.0), temp("nvme", 41.0)];

        assert_eq!(pick_cpu_temp(&readings), 33.0);
    }

    #[test]
    fn no_sensors_gives_zero() {
        assert_eq!(pick_cpu_temp(&[]), 0.0);
    }

    #[test]
    fn nan_reading_gives_zero() {
        let readings = vec![temp("cpu", f32::NAN)];

        assert_eq!(pick_cpu_temp(&readings), 0.0);
    }

    // the tests below read this real machine, so they only check the numbers are sane

    #[test]
    fn live_cpu_is_a_percentage() {
        let mut stats = SystemStats::new();
        let cpu = stats.cpu_pct();

        assert!(cpu >= 0.0 && cpu <= 100.0, "cpu was {cpu}");
    }

    #[test]
    fn live_memory_used_is_within_total() {
        let mut stats = SystemStats::new();
        let (used, total) = stats.memory_mb();

        assert!(total > 0.0);
        assert!(used >= 0.0 && used <= total);
    }

    #[test]
    fn live_disk_used_is_within_total() {
        let mut stats = SystemStats::new();
        let (used, total) = stats.root_disk_gb();

        assert!(used >= 0.0 && used <= total);
    }

    #[test]
    fn live_uptime_is_positive() {
        assert!(SystemStats::uptime_s() > 0.0);
    }
}
