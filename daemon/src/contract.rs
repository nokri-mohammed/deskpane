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

// limits a report must respect before the daemon stores it (see docs/CONTRACT.md)
pub const MAX_NAME_BYTES: usize = 64;
pub const MAX_IP_BYTES: usize = 45;
pub const MIN_TEMP_C: f64 = -50.0;
pub const MAX_TEMP_C: f64 = 150.0;
// used may be up to 1% above total, to allow for rounding on the device
pub const USED_OVER_TOTAL_TOLERANCE: f64 = 1.01;

/*
 * checks a report against the contract limits, so one broken or hostile
 * packet cannot reach the dashboard
 *
 * report: the parsed report to check
 * returns: Ok(()) when every field is sane, or Err with a reason that starts
 *          with the field name; the reason never repeats a bad text value,
 *          so it is safe to log; numbers are included
 */
pub fn validate(report: &Report) -> Result<(), String> {
    // text fields: length in bytes, empty or not, no control characters
    let device_id_checked = check_text("device_id", &report.device_id, MAX_NAME_BYTES, false);
    match device_id_checked {
        Ok(()) => {}
        Err(reason) => return Err(reason),
    }

    let label_checked = check_text("label", &report.label, MAX_NAME_BYTES, false);
    match label_checked {
        Ok(()) => {}
        Err(reason) => return Err(reason),
    }

    // hostname may be empty: the reporter sends "" when the OS gives none
    let hostname_checked = check_text("hostname", &report.hostname, MAX_NAME_BYTES, true);
    match hostname_checked {
        Ok(()) => {}
        Err(reason) => return Err(reason),
    }

    let ip_checked = check_text("ip", &report.ip, MAX_IP_BYTES, true);
    match ip_checked {
        Ok(()) => {}
        Err(reason) => return Err(reason),
    }

    // cpu is a percentage
    let cpu_checked = check_range("cpu_pct", report.cpu_pct, 0.0, 100.0);
    match cpu_checked {
        Ok(()) => {}
        Err(reason) => return Err(reason),
    }

    // memory: both sides non-negative, used not above total
    let mem_checked = check_used_total(
        "mem_used_mb",
        report.mem_used_mb,
        "mem_total_mb",
        report.mem_total_mb,
    );
    match mem_checked {
        Ok(()) => {}
        Err(reason) => return Err(reason),
    }

    // disk: same rule as memory
    let disk_checked = check_used_total(
        "disk_used_gb",
        report.disk_used_gb,
        "disk_total_gb",
        report.disk_total_gb,
    );
    match disk_checked {
        Ok(()) => {}
        Err(reason) => return Err(reason),
    }

    let temp_checked = check_range("temp_c", report.temp_c, MIN_TEMP_C, MAX_TEMP_C);
    match temp_checked {
        Ok(()) => {}
        Err(reason) => return Err(reason),
    }

    // uptime has no upper limit, only a lower one
    let uptime_checked = check_range("uptime_s", report.uptime_s, 0.0, f64::MAX);
    match uptime_checked {
        Ok(()) => {}
        Err(reason) => return Err(reason),
    }

    Ok(())
}

/*
 * checks one text field
 *
 * name: field name, used at the start of the reason
 * value: the text to check
 * max_bytes: longest allowed length in bytes (UTF-8), not characters
 * allow_empty: true if "" is a valid value
 */
fn check_text(name: &str, value: &str, max_bytes: usize, allow_empty: bool) -> Result<(), String> {
    if value.is_empty() && !allow_empty {
        return Err(format!("{name}: must not be empty"));
    }

    let length = value.len();
    if length > max_bytes {
        return Err(format!(
            "{name}: {length} bytes, longer than the limit of {max_bytes}"
        ));
    }

    // control characters (NUL, newline, escape, DEL, ...) break drawing and logs
    for c in value.chars() {
        if c.is_control() {
            return Err(format!("{name}: contains a control character"));
        }
    }

    Ok(())
}

/*
 * checks one number is finite and within min..=max
 *
 * name: field name, used at the start of the reason
 * value: the number to check
 * min, max: inclusive bounds
 */
fn check_range(name: &str, value: f64, min: f64, max: f64) -> Result<(), String> {
    // NaN and infinity are never valid readings
    if !value.is_finite() {
        return Err(format!("{name}: must be a finite number"));
    }

    if value < min || value > max {
        return Err(format!("{name}: {value} is outside {min}..={max}"));
    }

    Ok(())
}

/*
 * checks a used/total pair, like memory or disk
 *
 * used_name, used: the "used" field name and value
 * total_name, total: the "total" field name and value
 */
fn check_used_total(
    used_name: &str,
    used: f64,
    total_name: &str,
    total: f64,
) -> Result<(), String> {
    let used_checked = check_range(used_name, used, 0.0, f64::MAX);
    match used_checked {
        Ok(()) => {}
        Err(reason) => return Err(reason),
    }

    let total_checked = check_range(total_name, total, 0.0, f64::MAX);
    match total_checked {
        Ok(()) => {}
        Err(reason) => return Err(reason),
    }

    // allow a small tolerance for rounding on the device
    let limit = total * USED_OVER_TOTAL_TOLERANCE;
    if used > limit {
        return Err(format!(
            "{used_name}: {used} is more than {total_name} {total}"
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // a report that respects every limit, used as the base for each test
    fn valid_report() -> Report {
        Report {
            device_id: "my-server".to_string(),
            label: "Server".to_string(),
            hostname: "my-server".to_string(),
            ip: "192.168.1.60".to_string(),
            cpu_pct: 12.3,
            mem_used_mb: 1234.0,
            mem_total_mb: 8000.0,
            disk_used_gb: 45.2,
            disk_total_gb: 500.0,
            temp_c: 45.0,
            uptime_s: 123456.0,
        }
    }

    // fails the test unless validate rejects the report with a reason naming `field`
    fn assert_rejected(report: &Report, field: &str) {
        let checked = validate(report);
        match checked {
            Ok(()) => panic!("expected {field} to be rejected, but the report passed"),
            Err(reason) => {
                let prefix = format!("{field}:");
                let names_field = reason.starts_with(&prefix);
                assert!(
                    names_field,
                    "reason '{reason}' should start with '{prefix}'"
                );
            }
        }
    }

    // fails the test unless validate accepts the report
    fn assert_accepted(report: &Report) {
        let checked = validate(report);
        match checked {
            Ok(()) => {}
            Err(reason) => panic!("expected the report to pass, got: {reason}"),
        }
    }

    // ---- valid reports ----

    #[test]
    fn valid_report_passes() {
        assert_accepted(&valid_report());
    }

    #[test]
    fn empty_ip_passes() {
        let mut report = valid_report();
        report.ip = String::new();
        assert_accepted(&report);
    }

    #[test]
    fn strings_at_max_length_pass() {
        let mut report = valid_report();
        report.device_id = "a".repeat(64);
        report.label = "b".repeat(64);
        report.hostname = "c".repeat(64);
        report.ip = "d".repeat(45);
        assert_accepted(&report);
    }

    #[test]
    fn non_ascii_text_without_control_characters_passes() {
        let mut report = valid_report();
        report.label = "Küche Pi".to_string();
        assert_accepted(&report);
    }

    #[test]
    fn number_boundaries_pass() {
        let mut report = valid_report();
        report.cpu_pct = 0.0;
        report.temp_c = -50.0;
        report.uptime_s = 0.0;
        report.mem_used_mb = 0.0;
        report.mem_total_mb = 0.0;
        report.disk_used_gb = 0.0;
        report.disk_total_gb = 0.0;
        assert_accepted(&report);

        report.cpu_pct = 100.0;
        report.temp_c = 150.0;
        assert_accepted(&report);
    }

    #[test]
    fn used_within_one_percent_over_total_passes() {
        let mut report = valid_report();
        report.mem_total_mb = 1000.0;
        report.mem_used_mb = 1010.0;
        report.disk_total_gb = 100.0;
        report.disk_used_gb = 101.0;
        assert_accepted(&report);
    }

    // ---- empty strings ----

    #[test]
    fn empty_device_id_is_rejected() {
        let mut report = valid_report();
        report.device_id = String::new();
        assert_rejected(&report, "device_id");
    }

    #[test]
    fn empty_label_is_rejected() {
        let mut report = valid_report();
        report.label = String::new();
        assert_rejected(&report, "label");
    }

    #[test]
    fn empty_hostname_passes() {
        // the reporter can send "" when the OS gives no hostname
        let mut report = valid_report();
        report.hostname = String::new();
        assert_accepted(&report);
    }

    // ---- strings too long ----

    #[test]
    fn device_id_over_64_bytes_is_rejected() {
        let mut report = valid_report();
        report.device_id = "a".repeat(65);
        assert_rejected(&report, "device_id");
    }

    #[test]
    fn label_over_64_bytes_is_rejected() {
        let mut report = valid_report();
        report.label = "a".repeat(65);
        assert_rejected(&report, "label");
    }

    #[test]
    fn hostname_over_64_bytes_is_rejected() {
        let mut report = valid_report();
        report.hostname = "a".repeat(65);
        assert_rejected(&report, "hostname");
    }

    #[test]
    fn ip_over_45_bytes_is_rejected() {
        let mut report = valid_report();
        report.ip = "1".repeat(46);
        assert_rejected(&report, "ip");
    }

    #[test]
    fn length_is_counted_in_bytes_not_characters() {
        // 17 emoji are 17 characters but 68 bytes
        let mut report = valid_report();
        report.label = "\u{1F600}".repeat(17);
        assert_rejected(&report, "label");
    }

    // ---- control characters ----

    #[test]
    fn nul_in_hostname_is_rejected() {
        let mut report = valid_report();
        report.hostname = "host\u{0000}name".to_string();
        assert_rejected(&report, "hostname");
    }

    #[test]
    fn newline_in_device_id_is_rejected() {
        let mut report = valid_report();
        report.device_id = "dev\nice".to_string();
        assert_rejected(&report, "device_id");
    }

    #[test]
    fn escape_in_label_is_rejected() {
        let mut report = valid_report();
        report.label = "\u{001B}[31mred".to_string();
        assert_rejected(&report, "label");
    }

    #[test]
    fn tab_in_label_is_rejected() {
        let mut report = valid_report();
        report.label = "a\tb".to_string();
        assert_rejected(&report, "label");
    }

    #[test]
    fn c1_control_in_hostname_is_rejected() {
        // U+009B is the one-character form of the terminal escape "ESC ["
        let mut report = valid_report();
        report.hostname = "host\u{009B}2J".to_string();
        assert_rejected(&report, "hostname");
    }

    #[test]
    fn delete_char_in_ip_is_rejected() {
        let mut report = valid_report();
        report.ip = "10.0.0.1\u{007F}".to_string();
        assert_rejected(&report, "ip");
    }

    // ---- non-finite floats ----

    #[test]
    fn nan_cpu_pct_is_rejected() {
        let mut report = valid_report();
        report.cpu_pct = f64::NAN;
        assert_rejected(&report, "cpu_pct");
    }

    #[test]
    fn infinite_mem_used_is_rejected() {
        let mut report = valid_report();
        report.mem_used_mb = f64::INFINITY;
        assert_rejected(&report, "mem_used_mb");
    }

    #[test]
    fn infinite_mem_total_is_rejected() {
        let mut report = valid_report();
        report.mem_total_mb = f64::INFINITY;
        assert_rejected(&report, "mem_total_mb");
    }

    #[test]
    fn nan_mem_total_is_rejected() {
        let mut report = valid_report();
        report.mem_total_mb = f64::NAN;
        assert_rejected(&report, "mem_total_mb");
    }

    #[test]
    fn nan_disk_used_is_rejected() {
        let mut report = valid_report();
        report.disk_used_gb = f64::NAN;
        assert_rejected(&report, "disk_used_gb");
    }

    #[test]
    fn infinite_disk_total_is_rejected() {
        let mut report = valid_report();
        report.disk_total_gb = f64::INFINITY;
        assert_rejected(&report, "disk_total_gb");
    }

    #[test]
    fn negative_infinite_temp_is_rejected() {
        let mut report = valid_report();
        report.temp_c = f64::NEG_INFINITY;
        assert_rejected(&report, "temp_c");
    }

    #[test]
    fn nan_uptime_is_rejected() {
        let mut report = valid_report();
        report.uptime_s = f64::NAN;
        assert_rejected(&report, "uptime_s");
    }

    // ---- out of range numbers ----

    #[test]
    fn cpu_pct_below_zero_is_rejected() {
        let mut report = valid_report();
        report.cpu_pct = -0.1;
        assert_rejected(&report, "cpu_pct");
    }

    #[test]
    fn cpu_pct_above_100_is_rejected() {
        let mut report = valid_report();
        report.cpu_pct = 100.1;
        assert_rejected(&report, "cpu_pct");
    }

    #[test]
    fn negative_mem_used_is_rejected() {
        let mut report = valid_report();
        report.mem_used_mb = -1.0;
        assert_rejected(&report, "mem_used_mb");
    }

    #[test]
    fn negative_mem_total_is_rejected() {
        let mut report = valid_report();
        report.mem_total_mb = -1.0;
        assert_rejected(&report, "mem_total_mb");
    }

    #[test]
    fn mem_used_over_total_is_rejected() {
        let mut report = valid_report();
        report.mem_total_mb = 1000.0;
        report.mem_used_mb = 1011.0;
        assert_rejected(&report, "mem_used_mb");
    }

    #[test]
    fn mem_used_just_past_tolerance_is_rejected() {
        // the limit is 1000.0 * 1.01 = 1010.0, so 1010.5 is just past it
        let mut report = valid_report();
        report.mem_total_mb = 1000.0;
        report.mem_used_mb = 1010.5;
        assert_rejected(&report, "mem_used_mb");
    }

    #[test]
    fn negative_disk_used_is_rejected() {
        let mut report = valid_report();
        report.disk_used_gb = -1.0;
        assert_rejected(&report, "disk_used_gb");
    }

    #[test]
    fn negative_disk_total_is_rejected() {
        let mut report = valid_report();
        report.disk_total_gb = -1.0;
        assert_rejected(&report, "disk_total_gb");
    }

    #[test]
    fn disk_used_over_total_is_rejected() {
        let mut report = valid_report();
        report.disk_total_gb = 100.0;
        report.disk_used_gb = 101.1;
        assert_rejected(&report, "disk_used_gb");
    }

    #[test]
    fn temp_below_minus_50_is_rejected() {
        let mut report = valid_report();
        report.temp_c = -50.1;
        assert_rejected(&report, "temp_c");
    }

    #[test]
    fn temp_above_150_is_rejected() {
        let mut report = valid_report();
        report.temp_c = 150.1;
        assert_rejected(&report, "temp_c");
    }

    #[test]
    fn negative_uptime_is_rejected() {
        let mut report = valid_report();
        report.uptime_s = -1.0;
        assert_rejected(&report, "uptime_s");
    }

    #[test]
    fn reason_does_not_echo_the_bad_value() {
        // the reason is logged, so it must not carry the control characters back out
        let mut report = valid_report();
        report.hostname = "evil\u{001B}[2J".to_string();
        let checked = validate(&report);
        match checked {
            Ok(()) => panic!("expected hostname to be rejected"),
            Err(reason) => {
                let echoes = reason.contains("evil");
                assert!(!echoes, "reason '{reason}' should not contain the value");
            }
        }
    }
}
