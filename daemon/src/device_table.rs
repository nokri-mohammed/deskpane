//! The daemon's table of devices: the latest report per device, when it
//! arrived and which source IP owns the id. Holds the table policy (cap on
//! size, id ownership, forget silent devices) as plain functions, so tests
//! can pass any time and any limit.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

use crate::contract::Report;

/// One device in the table.
#[derive(Debug, Clone)]
pub struct DeviceEntry {
    pub report: Report,
    // when the latest report arrived
    pub last_seen: Instant,
    // the source IP that created or last refreshed this entry
    pub owner: IpAddr,
}

// device_id -> its entry
pub type DeviceTable = HashMap<String, DeviceEntry>;

/// What happened to one report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertOutcome {
    Stored,
    // a new device, and the table already holds max_devices
    TableFull,
    // another source IP owns this id and is still online
    IdInUse { owner: IpAddr },
}

/*
 * stores a report as its device's latest one, unless another online source
 * owns the id, or the device is new and the table is full
 *
 * table: the device table to update
 * report: the (already validated) report to store
 * source: the IP the report came from (the port is ignored on purpose, a
 *         restarted reporter gets a new port)
 * now: the time the report arrived
 * max_devices: most devices the table may hold
 * offline_after: an owner silent this long or longer no longer holds its id
 * returns: Stored, TableFull or IdInUse with the owner's IP. The duplicate
 *          check comes first, so a duplicate is reported even when full.
 */
pub fn insert_report(
    table: &mut DeviceTable,
    report: Report,
    source: IpAddr,
    now: Instant,
    max_devices: usize,
    offline_after: Duration,
) -> InsertOutcome {
    let existing = table.get(&report.device_id);
    match existing {
        Some(entry) => {
            // another IP holds the id: refuse while that owner is online
            if entry.owner != source {
                // saturating: a report stamped before last_seen counts as 0 s silent
                let silent_for = now.saturating_duration_since(entry.last_seen);
                if silent_for < offline_after {
                    return InsertOutcome::IdInUse { owner: entry.owner };
                }
            }
        }
        None => {
            if table.len() >= max_devices {
                return InsertOutcome::TableFull;
            }
        }
    }

    let device_id = report.device_id.clone();
    let entry = DeviceEntry {
        report: report,
        last_seen: now,
        owner: source,
    };
    table.insert(device_id, entry);
    InsertOutcome::Stored
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
    for (device_id, entry) in table.iter() {
        // saturating: a report stamped after `now` counts as 0 s silent, never panics
        let silent_for = now.saturating_duration_since(entry.last_seen);
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
    use std::net::Ipv4Addr;

    const CAP: usize = 2;
    const FORGET: Duration = Duration::from_secs(300);
    const OFFLINE: Duration = Duration::from_secs(15);

    // two machines on the LAN
    fn ip_a() -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(192, 168, 1, 60))
    }
    fn ip_b() -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(192, 168, 1, 61))
    }

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

    // insert from machine A with the test cap and offline time
    fn insert_a(
        table: &mut DeviceTable,
        device_id: &str,
        now: Instant,
        cap: usize,
    ) -> InsertOutcome {
        insert_report(table, report(device_id, 1.0), ip_a(), now, cap, OFFLINE)
    }

    // ---- insert ----

    #[test]
    fn new_device_is_stored_under_cap() {
        let mut table: DeviceTable = HashMap::new();
        let now = Instant::now();

        let outcome = insert_a(&mut table, "a", now, CAP);

        assert_eq!(outcome, InsertOutcome::Stored);
        assert_eq!(table.len(), 1);
        assert!(table.contains_key("a"));
    }

    #[test]
    fn known_device_is_updated_at_cap() {
        let mut table: DeviceTable = HashMap::new();
        let start = Instant::now();
        insert_a(&mut table, "a", start, CAP);
        insert_a(&mut table, "b", start, CAP);

        let later = start + Duration::from_secs(5);
        let outcome = insert_report(&mut table, report("a", 99.0), ip_a(), later, CAP, OFFLINE);

        assert_eq!(outcome, InsertOutcome::Stored);
        assert_eq!(table.len(), 2);
        let entry = table.get("a").unwrap();
        assert_eq!(entry.report.cpu_pct, 99.0);
        assert_eq!(entry.last_seen, later);
    }

    #[test]
    fn new_device_is_rejected_at_cap() {
        let mut table: DeviceTable = HashMap::new();
        let now = Instant::now();
        insert_a(&mut table, "a", now, CAP);
        insert_a(&mut table, "b", now, CAP);

        let outcome = insert_a(&mut table, "c", now, CAP);

        assert_eq!(outcome, InsertOutcome::TableFull);
        assert_eq!(table.len(), 2);
        assert!(!table.contains_key("c"));
    }

    #[test]
    fn zero_max_devices_rejects_every_new_device() {
        let mut table: DeviceTable = HashMap::new();
        let now = Instant::now();

        let first = insert_a(&mut table, "a", now, 0);
        let second = insert_a(&mut table, "b", now, 0);

        assert_eq!(first, InsertOutcome::TableFull);
        assert_eq!(second, InsertOutcome::TableFull);
        assert!(table.is_empty());
    }

    // ---- ownership (T-033) ----

    #[test]
    fn new_device_is_owned_by_its_source() {
        let mut table: DeviceTable = HashMap::new();

        insert_a(&mut table, "a", Instant::now(), CAP);

        assert_eq!(table.get("a").unwrap().owner, ip_a());
    }

    #[test]
    fn owner_can_update_its_device() {
        let mut table: DeviceTable = HashMap::new();
        let start = Instant::now();
        insert_a(&mut table, "a", start, CAP);

        let later = start + Duration::from_secs(4);
        let outcome = insert_report(&mut table, report("a", 50.0), ip_a(), later, CAP, OFFLINE);

        assert_eq!(outcome, InsertOutcome::Stored);
        assert_eq!(table.get("a").unwrap().report.cpu_pct, 50.0);
    }

    #[test]
    fn other_source_is_refused_while_owner_is_online() {
        let mut table: DeviceTable = HashMap::new();
        let start = Instant::now();
        insert_a(&mut table, "a", start, CAP);

        let later = start + Duration::from_secs(14);
        let outcome = insert_report(&mut table, report("a", 77.0), ip_b(), later, CAP, OFFLINE);

        assert_ne!(outcome, InsertOutcome::Stored);
        // the owner's report and time are untouched
        let entry = table.get("a").unwrap();
        assert_eq!(entry.report.cpu_pct, 1.0);
        assert_eq!(entry.last_seen, start);
        assert_eq!(entry.owner, ip_a());
    }

    #[test]
    fn refusal_reports_the_owner_ip() {
        let mut table: DeviceTable = HashMap::new();
        let start = Instant::now();
        insert_a(&mut table, "a", start, CAP);

        let outcome = insert_report(&mut table, report("a", 1.0), ip_b(), start, CAP, OFFLINE);

        assert_eq!(outcome, InsertOutcome::IdInUse { owner: ip_a() });
    }

    #[test]
    fn other_source_takes_over_once_owner_is_offline() {
        let mut table: DeviceTable = HashMap::new();
        let start = Instant::now();
        insert_a(&mut table, "a", start, CAP);

        let later = start + Duration::from_secs(20);
        let outcome = insert_report(&mut table, report("a", 33.0), ip_b(), later, CAP, OFFLINE);

        assert_eq!(outcome, InsertOutcome::Stored);
        let entry = table.get("a").unwrap();
        assert_eq!(entry.owner, ip_b());
        assert_eq!(entry.report.cpu_pct, 33.0);
        assert_eq!(entry.last_seen, later);
    }

    #[test]
    fn owner_silent_exactly_offline_after_counts_as_offline() {
        // broadcast_statuses shows online only while silent_for < OFFLINE_AFTER
        let mut table: DeviceTable = HashMap::new();
        let start = Instant::now();
        insert_a(&mut table, "a", start, CAP);

        let outcome = insert_report(
            &mut table,
            report("a", 1.0),
            ip_b(),
            start + OFFLINE,
            CAP,
            OFFLINE,
        );

        assert_eq!(outcome, InsertOutcome::Stored);
        assert_eq!(table.get("a").unwrap().owner, ip_b());
    }

    #[test]
    fn duplicate_is_reported_before_table_full() {
        let mut table: DeviceTable = HashMap::new();
        let now = Instant::now();
        insert_a(&mut table, "a", now, CAP);
        insert_a(&mut table, "b", now, CAP);

        let outcome = insert_report(&mut table, report("a", 1.0), ip_b(), now, CAP, OFFLINE);

        assert_eq!(outcome, InsertOutcome::IdInUse { owner: ip_a() });
    }

    #[test]
    fn report_stamped_before_last_seen_does_not_panic() {
        // time going backwards counts as 0 s silent: the owner is still online
        let mut table: DeviceTable = HashMap::new();
        let start = Instant::now() + Duration::from_secs(60);
        insert_a(&mut table, "a", start, CAP);

        let earlier = start - Duration::from_secs(30);
        let outcome = insert_report(&mut table, report("a", 1.0), ip_b(), earlier, CAP, OFFLINE);

        assert_eq!(outcome, InsertOutcome::IdInUse { owner: ip_a() });
    }

    // ---- forget ----

    #[test]
    fn forget_returns_every_expired_id() {
        let mut table: DeviceTable = HashMap::new();
        let start = Instant::now();
        let three = 3;
        insert_a(&mut table, "old-1", start, three);
        insert_a(&mut table, "old-2", start, three);
        insert_a(&mut table, "fresh", start + Duration::from_secs(200), three);

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
        insert_a(&mut table, "old", start, CAP);
        insert_a(&mut table, "fresh", start + Duration::from_secs(200), CAP);

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
        insert_a(&mut table, "a", start, CAP);

        let forgotten = forget_silent(&mut table, start + FORGET, FORGET);

        assert!(forgotten.is_empty());
        assert!(table.contains_key("a"));
    }

    #[test]
    fn expired_slot_frees_room_for_a_new_device() {
        let mut table: DeviceTable = HashMap::new();
        let start = Instant::now();
        insert_a(&mut table, "a", start, CAP);
        insert_a(&mut table, "b", start + Duration::from_secs(100), CAP);

        // full: a new device is turned away
        let now = start + Duration::from_secs(301);
        let before_forget = insert_a(&mut table, "c", now, CAP);
        assert_eq!(before_forget, InsertOutcome::TableFull);

        // "a" expires, which makes room for "c"
        forget_silent(&mut table, now, FORGET);
        let after_forget = insert_a(&mut table, "c", now, CAP);

        assert_eq!(after_forget, InsertOutcome::Stored);
        assert!(table.contains_key("b"));
        assert!(table.contains_key("c"));
        assert!(!table.contains_key("a"));
    }
}
