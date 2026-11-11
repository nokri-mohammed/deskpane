//! The daemon's table of plugin metrics: the latest metric packet per plugin,
//! when it arrived and which source IP owns the plugin name. Holds the table
//! policy (only turned-on plugins, caps, ownership while fresh and capped in
//! time, forget after the ttl) as plain functions, so tests can pass any time
//! and any limit.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

use crate::contract::{check_plugin_name, MetricPacket, MetricView};

/// One plugin in the table.
#[derive(Debug, Clone)]
pub struct MetricEntry {
    pub packet: MetricPacket,
    // when the latest packet arrived
    pub last_seen: Instant,
    // the source IP that created or last refreshed this entry
    pub owner: IpAddr,
}

// plugin name -> its entry
pub type MetricTable = HashMap<String, MetricEntry>;

/// The caps insert_metric applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetricLimits {
    // most plugins the table holds
    pub max_plugins: usize,
    // most plugins one source IP may own
    pub max_per_source: usize,
    // longest an owner holds a plugin name while silent, whatever ttl_s it
    // sent, so one packet cannot hold a name for a day
    pub max_owner_hold: Duration,
}

/// What happened to one metric packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricOutcome {
    // a new plugin
    Created,
    // a plugin taken over from a source that stopped holding it
    TakenOver { previous: IpAddr },
    // the owner's update to a plugin it already has
    Stored,
    // another source IP still holds this plugin
    NotOwner { owner: IpAddr },
    // the plugin is not turned on (not listed in PANELS)
    NotEnabled,
    // this source already owns max_per_source plugins
    SourceFull,
    // a new plugin, and the table already holds max_plugins
    TableFull,
}

// true while the entry's values are younger than the ttl its packet set
fn is_fresh(entry: &MetricEntry, now: Instant) -> bool {
    // saturating: a packet stamped after `now` counts as 0 s old, never panics
    let age = now.saturating_duration_since(entry.last_seen);
    let ttl = Duration::from_secs(u64::from(entry.packet.ttl_s));
    age < ttl
}

/*
 * true while the owner still holds the plugin name: its values are fresh and
 * it has not been silent for max_owner_hold
 */
fn holds_name(entry: &MetricEntry, now: Instant, max_owner_hold: Duration) -> bool {
    let age = now.saturating_duration_since(entry.last_seen);
    let ttl = Duration::from_secs(u64::from(entry.packet.ttl_s));
    let hold;
    if ttl < max_owner_hold {
        hold = ttl;
    } else {
        hold = max_owner_hold;
    }
    age < hold
}

// how many plugins one source IP owns
fn count_owned(table: &MetricTable, source: IpAddr) -> usize {
    let mut count = 0;
    for entry in table.values() {
        if entry.owner == source {
            count += 1;
        }
    }
    count
}

/*
 * stores a metric packet as its plugin's latest one, replacing all of the
 * plugin's values, unless the plugin is not turned on, another source still
 * holds it, or a cap is reached
 *
 * table: the metric table to update
 * packet: the (already validated) packet to store
 * source: the IP it came from (the port is ignored, as for reports)
 * now: the time it arrived
 * limits: the table caps and the owner hold
 * enabled: the plugin names that are turned on (PANELS); others are dropped
 * returns: Created, TakenOver, Stored, NotOwner, NotEnabled, SourceFull or
 *          TableFull
 */
pub fn insert_metric(
    table: &mut MetricTable,
    packet: MetricPacket,
    source: IpAddr,
    now: Instant,
    limits: MetricLimits,
    enabled: &BTreeSet<String>,
) -> MetricOutcome {
    // only turned-on plugins: nobody can squat a name or fill the table with others
    if !enabled.contains(&packet.plugin) {
        return MetricOutcome::NotEnabled;
    }

    let outcome;
    let existing = table.get(&packet.plugin);
    match existing {
        Some(entry) => {
            if entry.owner == source {
                outcome = MetricOutcome::Stored;
            } else {
                // another IP holds the plugin: refuse while it still holds the name
                if holds_name(entry, now, limits.max_owner_hold) {
                    return MetricOutcome::NotOwner { owner: entry.owner };
                }
                // a takeover counts against the new owner's cap
                if count_owned(table, source) >= limits.max_per_source {
                    return MetricOutcome::SourceFull;
                }
                outcome = MetricOutcome::TakenOver {
                    previous: entry.owner,
                };
            }
        }
        None => {
            if table.len() >= limits.max_plugins {
                return MetricOutcome::TableFull;
            }
            if count_owned(table, source) >= limits.max_per_source {
                return MetricOutcome::SourceFull;
            }
            outcome = MetricOutcome::Created;
        }
    }

    let plugin = packet.plugin.clone();
    let entry = MetricEntry {
        packet: packet,
        last_seen: now,
        owner: source,
    };
    table.insert(plugin, entry);
    outcome
}

/*
 * reads the turned-on plugin names from a comma-separated setting (PANELS),
 * e.g. "weather, ups-2". Spaces and empty items are ignored, a name listed
 * twice counts once.
 *
 * text: the setting's value
 * max: most names kept
 * returns: (the names, the items skipped because they break the plugin name
 *          rule or come after max). The setting is the owner's own file, so
 *          the caller may log the skipped items.
 */
pub fn parse_plugin_list(text: &str, max: usize) -> (BTreeSet<String>, Vec<String>) {
    let mut names = BTreeSet::new();
    let mut skipped: Vec<String> = Vec::new();
    for item in text.split(',') {
        let name = item.trim();
        if name.is_empty() || names.contains(name) {
            continue;
        }
        let checked = check_plugin_name(name);
        match checked {
            Ok(()) => {}
            Err(_) => {
                skipped.push(name.to_string());
                continue;
            }
        }
        if names.len() >= max {
            skipped.push(name.to_string());
            continue;
        }
        names.insert(name.to_string());
    }
    (names, skipped)
}

/*
 * removes every plugin that has been silent for longer than its ttl plus
 * grace
 *
 * table: the metric table to clean up
 * now: the current time
 * grace: how long past its ttl a plugin is kept (shown as stale)
 * returns: the plugin names that were removed, so the caller can log them
 */
pub fn forget_metrics(table: &mut MetricTable, now: Instant, grace: Duration) -> Vec<String> {
    // first collect the names, since a map cannot be changed while looping over it
    let mut expired: Vec<String> = Vec::new();
    for (plugin, entry) in table.iter() {
        let silent_for = now.saturating_duration_since(entry.last_seen);
        let keep_for = Duration::from_secs(u64::from(entry.packet.ttl_s)) + grace;
        if silent_for > keep_for {
            expired.push(plugin.clone());
        }
    }

    // then remove them
    for plugin in &expired {
        table.remove(plugin);
    }

    expired
}

/*
 * the metrics view for the socket line: every plugin's values, how old they
 * are and whether they are stale
 *
 * table: the metric table
 * now: the current time
 * returns: plugin name -> its view, sorted by name
 */
pub fn metric_views(table: &MetricTable, now: Instant) -> BTreeMap<String, MetricView> {
    let mut views = BTreeMap::new();
    for (plugin, entry) in table.iter() {
        let age = now.saturating_duration_since(entry.last_seen);
        let view = MetricView {
            values: entry.packet.values.clone(),
            age_s: age.as_secs(),
            stale: !is_fresh(entry, now),
        };
        views.insert(plugin.clone(), view);
    }
    views
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{METRIC_PACKET_VERSION, PACKET_KIND_METRIC};
    use std::net::Ipv4Addr;

    const LIMITS: MetricLimits = MetricLimits {
        max_plugins: 3,
        max_per_source: 2,
        max_owner_hold: Duration::from_secs(120),
    };
    const GRACE: Duration = Duration::from_secs(300);
    const TTL_S: u32 = 60;

    // three machines on the LAN
    fn ip_a() -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(192, 168, 1, 60))
    }
    fn ip_b() -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(192, 168, 1, 61))
    }
    fn ip_c() -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(192, 168, 1, 62))
    }

    // a valid packet for the given plugin with one value
    fn packet(plugin: &str, value: f64) -> MetricPacket {
        let mut values = BTreeMap::new();
        values.insert("x".to_string(), value);
        MetricPacket {
            kind: PACKET_KIND_METRIC.to_string(),
            v: METRIC_PACKET_VERSION,
            plugin: plugin.to_string(),
            ttl_s: TTL_S,
            values: values,
        }
    }

    // the plugin names the tests turn on (PANELS)
    fn enabled() -> BTreeSet<String> {
        let mut names = BTreeSet::new();
        for name in ["weather", "p1", "p2", "p3", "p4"] {
            names.insert(name.to_string());
        }
        names
    }

    // a packet for the given plugin with a chosen ttl
    fn packet_with_ttl(plugin: &str, ttl_s: u32) -> MetricPacket {
        let mut built = packet(plugin, 1.0);
        built.ttl_s = ttl_s;
        built
    }

    #[test]
    fn a_plugin_that_is_not_turned_on_is_dropped() {
        let mut table = MetricTable::new();
        let now = Instant::now();
        let outcome = insert_metric(
            &mut table,
            packet("squatter", 1.0),
            ip_a(),
            now,
            LIMITS,
            &enabled(),
        );
        assert_eq!(outcome, MetricOutcome::NotEnabled);
        assert!(table.is_empty());

        // with nothing turned on, nothing is stored at all
        let none = BTreeSet::new();
        let outcome = insert_metric(
            &mut table,
            packet("weather", 1.0),
            ip_a(),
            now,
            LIMITS,
            &none,
        );
        assert_eq!(outcome, MetricOutcome::NotEnabled);
        assert!(table.is_empty());
    }

    #[test]
    fn a_long_ttl_holds_the_name_only_up_to_the_owner_cap() {
        // a day-long ttl must not let one packet hold the name for a day
        let mut table = MetricTable::new();
        let t0 = Instant::now();
        insert_metric(
            &mut table,
            packet_with_ttl("weather", 86_400),
            ip_a(),
            t0,
            LIMITS,
            &enabled(),
        );

        let just_held = t0 + LIMITS.max_owner_hold - secs(1);
        let outcome = insert_metric(
            &mut table,
            packet("weather", 9.0),
            ip_b(),
            just_held,
            LIMITS,
            &enabled(),
        );
        assert_eq!(outcome, MetricOutcome::NotOwner { owner: ip_a() });

        let released = t0 + LIMITS.max_owner_hold;
        let outcome = insert_metric(
            &mut table,
            packet("weather", 9.0),
            ip_b(),
            released,
            LIMITS,
            &enabled(),
        );
        assert_eq!(outcome, MetricOutcome::TakenOver { previous: ip_a() });
        assert_eq!(table["weather"].owner, ip_b());
    }

    #[test]
    fn the_hold_cap_does_not_make_values_stale_early() {
        let mut table = MetricTable::new();
        let t0 = Instant::now();
        insert_metric(
            &mut table,
            packet_with_ttl("weather", 86_400),
            ip_a(),
            t0,
            LIMITS,
            &enabled(),
        );
        let views = metric_views(&table, t0 + LIMITS.max_owner_hold + secs(60));
        assert!(!views["weather"].stale);
    }

    #[test]
    fn plugin_list_reads_names_and_skips_the_rest() {
        let (names, skipped) = parse_plugin_list(" weather, ups-2,,weather ,  ", 16);
        let mut want = BTreeSet::new();
        want.insert("weather".to_string());
        want.insert("ups-2".to_string());
        assert_eq!(names, want);
        assert!(skipped.is_empty());

        let (names, skipped) = parse_plugin_list("Weather,1x,ok,a_b", 16);
        assert_eq!(names.len(), 1);
        assert!(names.contains("ok"));
        assert_eq!(
            skipped,
            vec!["Weather".to_string(), "1x".to_string(), "a_b".to_string()]
        );
    }

    #[test]
    fn plugin_list_keeps_at_most_max_names() {
        let mut text = String::new();
        for index in 0..5 {
            text.push_str(&format!("p{index},"));
        }
        let (names, skipped) = parse_plugin_list(&text, 3);
        assert_eq!(names.len(), 3);
        assert_eq!(skipped, vec!["p3".to_string(), "p4".to_string()]);
    }

    #[test]
    fn an_empty_plugin_list_turns_nothing_on() {
        let (names, skipped) = parse_plugin_list("", 16);
        assert!(names.is_empty());
        assert!(skipped.is_empty());
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn a_new_plugin_is_created() {
        let mut table = MetricTable::new();
        let now = Instant::now();
        let outcome = insert_metric(
            &mut table,
            packet("weather", 1.0),
            ip_a(),
            now,
            LIMITS,
            &enabled(),
        );
        assert_eq!(outcome, MetricOutcome::Created);
        assert_eq!(table["weather"].owner, ip_a());
    }

    #[test]
    fn the_owner_replaces_all_values() {
        let mut table = MetricTable::new();
        let t0 = Instant::now();
        insert_metric(
            &mut table,
            packet("weather", 1.0),
            ip_a(),
            t0,
            LIMITS,
            &enabled(),
        );

        let mut update = packet("weather", 2.0);
        update.values = BTreeMap::new();
        update.values.insert("y".to_string(), 3.0);
        let outcome = insert_metric(&mut table, update, ip_a(), t0 + secs(5), LIMITS, &enabled());

        assert_eq!(outcome, MetricOutcome::Stored);
        let values = &table["weather"].packet.values;
        assert_eq!(values.len(), 1);
        assert_eq!(values["y"], 3.0);
        assert_eq!(table["weather"].last_seen, t0 + secs(5));
    }

    #[test]
    fn another_source_is_refused_while_the_data_is_fresh() {
        let mut table = MetricTable::new();
        let t0 = Instant::now();
        insert_metric(
            &mut table,
            packet("weather", 1.0),
            ip_a(),
            t0,
            LIMITS,
            &enabled(),
        );

        let just_fresh = t0 + secs(u64::from(TTL_S) - 1);
        let outcome = insert_metric(
            &mut table,
            packet("weather", 9.0),
            ip_b(),
            just_fresh,
            LIMITS,
            &enabled(),
        );
        assert_eq!(outcome, MetricOutcome::NotOwner { owner: ip_a() });
        assert_eq!(table["weather"].packet.values["x"], 1.0);
    }

    #[test]
    fn another_source_takes_over_once_the_data_is_stale() {
        let mut table = MetricTable::new();
        let t0 = Instant::now();
        insert_metric(
            &mut table,
            packet("weather", 1.0),
            ip_a(),
            t0,
            LIMITS,
            &enabled(),
        );

        let stale = t0 + secs(u64::from(TTL_S));
        let outcome = insert_metric(
            &mut table,
            packet("weather", 9.0),
            ip_b(),
            stale,
            LIMITS,
            &enabled(),
        );
        assert_eq!(outcome, MetricOutcome::TakenOver { previous: ip_a() });
        assert_eq!(table["weather"].owner, ip_b());
    }

    #[test]
    fn a_full_table_refuses_new_plugins_but_updates_existing_ones() {
        let mut table = MetricTable::new();
        let now = Instant::now();
        insert_metric(
            &mut table,
            packet("p1", 1.0),
            ip_a(),
            now,
            LIMITS,
            &enabled(),
        );
        insert_metric(
            &mut table,
            packet("p2", 1.0),
            ip_b(),
            now,
            LIMITS,
            &enabled(),
        );
        insert_metric(
            &mut table,
            packet("p3", 1.0),
            ip_c(),
            now,
            LIMITS,
            &enabled(),
        );

        let outcome = insert_metric(
            &mut table,
            packet("p4", 1.0),
            ip_c(),
            now,
            LIMITS,
            &enabled(),
        );
        assert_eq!(outcome, MetricOutcome::TableFull);
        assert_eq!(table.len(), 3);

        let outcome = insert_metric(
            &mut table,
            packet("p1", 2.0),
            ip_a(),
            now,
            LIMITS,
            &enabled(),
        );
        assert_eq!(outcome, MetricOutcome::Stored);
    }

    #[test]
    fn one_source_cannot_own_more_than_its_cap() {
        let mut table = MetricTable::new();
        let now = Instant::now();
        insert_metric(
            &mut table,
            packet("p1", 1.0),
            ip_a(),
            now,
            LIMITS,
            &enabled(),
        );
        insert_metric(
            &mut table,
            packet("p2", 1.0),
            ip_a(),
            now,
            LIMITS,
            &enabled(),
        );

        let outcome = insert_metric(
            &mut table,
            packet("p3", 1.0),
            ip_a(),
            now,
            LIMITS,
            &enabled(),
        );
        assert_eq!(outcome, MetricOutcome::SourceFull);
        // its own plugins still update
        let outcome = insert_metric(
            &mut table,
            packet("p2", 2.0),
            ip_a(),
            now,
            LIMITS,
            &enabled(),
        );
        assert_eq!(outcome, MetricOutcome::Stored);
    }

    #[test]
    fn a_takeover_counts_against_the_new_owners_cap() {
        let mut table = MetricTable::new();
        let t0 = Instant::now();
        insert_metric(
            &mut table,
            packet("p1", 1.0),
            ip_b(),
            t0,
            LIMITS,
            &enabled(),
        );
        insert_metric(
            &mut table,
            packet("p2", 1.0),
            ip_a(),
            t0,
            LIMITS,
            &enabled(),
        );
        insert_metric(
            &mut table,
            packet("p3", 1.0),
            ip_a(),
            t0,
            LIMITS,
            &enabled(),
        );

        let stale = t0 + secs(u64::from(TTL_S));
        let outcome = insert_metric(
            &mut table,
            packet("p1", 9.0),
            ip_a(),
            stale,
            LIMITS,
            &enabled(),
        );
        assert_eq!(outcome, MetricOutcome::SourceFull);
        assert_eq!(table["p1"].owner, ip_b());
    }

    #[test]
    fn a_plugin_is_forgotten_only_after_its_ttl_plus_grace() {
        let mut table = MetricTable::new();
        let t0 = Instant::now();
        insert_metric(
            &mut table,
            packet("weather", 1.0),
            ip_a(),
            t0,
            LIMITS,
            &enabled(),
        );
        let keep_for = secs(u64::from(TTL_S)) + GRACE;

        let forgotten = forget_metrics(&mut table, t0 + keep_for, GRACE);
        assert!(forgotten.is_empty());
        assert!(table.contains_key("weather"));

        let forgotten = forget_metrics(&mut table, t0 + keep_for + secs(1), GRACE);
        assert_eq!(forgotten, vec!["weather".to_string()]);
        assert!(table.is_empty());
    }

    #[test]
    fn views_carry_values_age_and_staleness() {
        let mut table = MetricTable::new();
        let t0 = Instant::now();
        insert_metric(
            &mut table,
            packet("weather", 12.5),
            ip_a(),
            t0,
            LIMITS,
            &enabled(),
        );

        let views = metric_views(&table, t0 + secs(42));
        let view = &views["weather"];
        assert_eq!(view.values["x"], 12.5);
        assert_eq!(view.age_s, 42);
        assert!(!view.stale);

        let views = metric_views(&table, t0 + secs(u64::from(TTL_S)));
        assert!(views["weather"].stale);
    }

    #[test]
    fn a_packet_stamped_in_the_future_counts_as_fresh() {
        let mut table = MetricTable::new();
        let t0 = Instant::now();
        insert_metric(
            &mut table,
            packet("weather", 1.0),
            ip_a(),
            t0 + secs(10),
            LIMITS,
            &enabled(),
        );

        let views = metric_views(&table, t0);
        assert_eq!(views["weather"].age_s, 0);
        assert!(!views["weather"].stale);
        assert!(forget_metrics(&mut table, t0, GRACE).is_empty());
    }

    #[test]
    fn an_empty_table_has_no_views() {
        let table = MetricTable::new();
        assert!(metric_views(&table, Instant::now()).is_empty());
    }
}
