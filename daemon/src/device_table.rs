//! The daemon's table of devices: the latest report per device and when it
//! arrived. Holds the table policy (cap on size, forget silent devices) as
//! plain functions, so tests can pass any time and any limit.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::contract::Report;

// device_id -> (latest report, when it arrived)
pub type DeviceTable = HashMap<String, (Report, Instant)>;

/*
 * stores a report as its device's latest one, unless the table is full
 *
 * table: the device table to update
 * report: the (already validated) report to store
 * now: the time the report arrived
 * max_devices: most devices the table may hold
 * returns: true if stored; false if the device is new and the table is full.
 *          A device already in the table is always updated.
 */
pub fn insert_report(
    table: &mut DeviceTable,
    report: Report,
    now: Instant,
    max_devices: usize,
) -> bool {
    let is_known = table.contains_key(&report.device_id);
    if !is_known && table.len() >= max_devices {
        return false;
    }

    let device_id = report.device_id.clone();
    table.insert(device_id, (report, now));
    true
}

/*
 * removes every device that has been silent for longer than forget_after
 *
 * table: the device table to clean up
 * now: the current time
 * forget_after: how long a device may stay silent before it is removed
 * returns: the device_ids that were removed, so the caller can log them
 */
pub fn forget_silent(table: &mut DeviceTable, now: Instant, forget_after: Duration) -> Vec<String> {
    // first collect the ids, since a map cannot be changed while looping over it
    let mut expired: Vec<String> = Vec::new();
    for (device_id, (_report, last_seen)) in table.iter() {
        // saturating: a report stamped after `now` counts as 0 s silent, never panics
        let silent_for = now.saturating_duration_since(*last_seen);
        if silent_for > forget_after {
            expired.push(device_id.clone());
        }
    }

    // then remove them
    for device_id in &expired {
        table.remove(device_id);
    }

    expired
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAP: usize = 2;
    const FORGET: Duration = Duration::from_secs(300);

    // a valid report for the given device
    fn report(device_id: &str, cpu_pct: f64) -> Report {
        Report {
            device_id: device_id.to_string(),
            label: "Test".to_string(),
            hostname: "test-host".to_string(),
            ip: String::new(),
            cpu_pct: cpu_pct,
            mem_used_mb: 1.0,
            mem_total_mb: 2.0,
            disk_used_gb: 1.0,
            disk_total_gb: 2.0,
            temp_c: 40.0,
            uptime_s: 10.0,
        }
    }

    // ---- insert ----

    #[test]
    fn new_device_is_stored_under_cap() {
        let mut table: DeviceTable = HashMap::new();
        let now = Instant::now();

        let stored = insert_report(&mut table, report("a", 1.0), now, CAP);

        assert!(stored);
        assert_eq!(table.len(), 1);
        assert!(table.contains_key("a"));
    }

    #[test]
    fn known_device_is_updated_at_cap() {
        let mut table: DeviceTable = HashMap::new();
        let start = Instant::now();
        insert_report(&mut table, report("a", 1.0), start, CAP);
        insert_report(&mut table, report("b", 1.0), start, CAP);

        let later = start + Duration::from_secs(5);
        let stored = insert_report(&mut table, report("a", 99.0), later, CAP);

        assert!(stored);
        assert_eq!(table.len(), 2);
        let (latest, seen_at) = table.get("a").unwrap();
        assert_eq!(latest.cpu_pct, 99.0);
        assert_eq!(*seen_at, later);
    }

    #[test]
    fn new_device_is_rejected_at_cap() {
        let mut table: DeviceTable = HashMap::new();
        let now = Instant::now();
        insert_report(&mut table, report("a", 1.0), now, CAP);
        insert_report(&mut table, report("b", 1.0), now, CAP);

        let stored = insert_report(&mut table, report("c", 1.0), now, CAP);

        assert!(!stored);
        assert_eq!(table.len(), 2);
        assert!(!table.contains_key("c"));
    }

    #[test]
    fn zero_max_devices_rejects_every_new_device() {
        let mut table: DeviceTable = HashMap::new();
        let now = Instant::now();

        let first = insert_report(&mut table, report("a", 1.0), now, 0);
        let second = insert_report(&mut table, report("b", 1.0), now, 0);

        assert!(!first);
        assert!(!second);
        assert!(table.is_empty());
    }

    // ---- forget ----

    #[test]
    fn forget_returns_every_expired_id() {
        let mut table: DeviceTable = HashMap::new();
        let start = Instant::now();
        let three = 3;
        insert_report(&mut table, report("old-1", 1.0), start, three);
        insert_report(&mut table, report("old-2", 1.0), start, three);
        insert_report(
            &mut table,
            report("fresh", 1.0),
            start + Duration::from_secs(200),
            three,
        );

        let now = start + Duration::from_secs(301);
        let mut forgotten = forget_silent(&mut table, now, FORGET);

        // a HashMap has no fixed order, so sort before comparing
        forgotten.sort();
        assert_eq!(forgotten, vec!["old-1".to_string(), "old-2".to_string()]);
        assert_eq!(table.len(), 1);
        assert!(table.contains_key("fresh"));
    }

    #[test]
    fn forget_on_empty_table_removes_nothing() {
        let mut table: DeviceTable = HashMap::new();

        let forgotten = forget_silent(&mut table, Instant::now(), FORGET);

        assert!(forgotten.is_empty());
    }

    #[test]
    fn forget_removes_only_devices_past_forget_after() {
        let mut table: DeviceTable = HashMap::new();
        let start = Instant::now();
        insert_report(&mut table, report("old", 1.0), start, CAP);
        insert_report(
            &mut table,
            report("fresh", 1.0),
            start + Duration::from_secs(200),
            CAP,
        );

        // "old" has been silent 301 s, "fresh" only 101 s
        let now = start + Duration::from_secs(301);
        let forgotten = forget_silent(&mut table, now, FORGET);

        assert_eq!(forgotten, vec!["old".to_string()]);
        assert_eq!(table.len(), 1);
        assert!(table.contains_key("fresh"));
    }

    #[test]
    fn device_silent_exactly_forget_after_is_kept() {
        let mut table: DeviceTable = HashMap::new();
        let start = Instant::now();
        insert_report(&mut table, report("a", 1.0), start, CAP);

        let forgotten = forget_silent(&mut table, start + FORGET, FORGET);

        assert!(forgotten.is_empty());
        assert!(table.contains_key("a"));
    }

    #[test]
    fn expired_slot_frees_room_for_a_new_device() {
        let mut table: DeviceTable = HashMap::new();
        let start = Instant::now();
        insert_report(&mut table, report("a", 1.0), start, CAP);
        insert_report(
            &mut table,
            report("b", 1.0),
            start + Duration::from_secs(100),
            CAP,
        );

        // full: a new device is turned away
        let now = start + Duration::from_secs(301);
        let before_forget = insert_report(&mut table, report("c", 1.0), now, CAP);
        assert!(!before_forget);

        // "a" expires, which makes room for "c"
        forget_silent(&mut table, now, FORGET);
        let after_forget = insert_report(&mut table, report("c", 1.0), now, CAP);

        assert!(after_forget);
        assert!(table.contains_key("b"));
        assert!(table.contains_key("c"));
        assert!(!table.contains_key("a"));
    }
}
