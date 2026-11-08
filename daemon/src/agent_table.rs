//! The daemon's table of agent sessions: the latest agent packet per session
//! id, which source IP owns it, and when it was first seen, changed state and
//! was last heard from. Holds the table policy (caps, ownership, reorder
//! guard, eviction, forget) as plain functions, so tests can pass any time
//! and any limit. Same pattern as device_table.rs.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

use crate::contract::{AgentPacket, AgentSession, AgentState, DOING_THINKING};

/// One agent session in the table.
#[derive(Debug, Clone)]
pub struct AgentEntry {
    // latest valid packet (state, project, doing, device_id, t)
    pub packet: AgentPacket,
    // source IP of the packet that created or last refreshed it
    pub owner: IpAddr,
    // when the daemon first saw the session, for "running for"
    pub first_seen: Instant,
    // when the state last changed
    pub state_since: Instant,
    // when the latest packet arrived, for quiet and forget
    pub last_seen: Instant,
    // keys ("main" or a sub) waiting on the user, with their question, oldest first
    pub pending: Vec<(String, String)>,
    // the key of the latest stored packet (the reorder guard compares only within one key)
    pub last_key: String,
}

// most recently ended sessions remembered for the reorder window
pub const RECENTLY_ENDED_CAP: usize = 32;

/// A session that ended a moment ago, so late packets for it can be dropped.
#[derive(Debug, Clone)]
pub struct EndedSession {
    pub sid: String,
    // sender time of the ended packet
    pub t: u64,
    // when it arrived
    pub at: Instant,
}

/// Every agent session, plus the sessions that ended a moment ago.
#[derive(Debug, Clone, Default)]
pub struct AgentTable {
    // sid -> its entry
    pub sessions: HashMap<String, AgentEntry>,
    // at most RECENTLY_ENDED_CAP, each kept for the reorder window
    pub ended: Vec<EndedSession>,
}

impl AgentTable {
    // an empty table
    pub fn new() -> Self {
        AgentTable {
            sessions: HashMap::new(),
            ended: Vec::new(),
        }
    }

    // the entry of one session
    pub fn get(&self, sid: &str) -> Option<&AgentEntry> {
        self.sessions.get(sid)
    }

    // true when the session is in the table
    pub fn contains_key(&self, sid: &str) -> bool {
        self.sessions.contains_key(sid)
    }

    // how many sessions the table holds
    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    // true when the table holds no session
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    // every (sid, entry)
    pub fn iter(&self) -> std::collections::hash_map::Iter<'_, String, AgentEntry> {
        self.sessions.iter()
    }
}

/// The limits insert_agent works with (the daemon's constants, or test values).
#[derive(Debug, Clone, Copy)]
pub struct AgentLimits {
    // most sessions in the table
    pub max_agents: usize,
    // most sessions one source IP may own
    pub max_per_source: usize,
    // an owner silent this long or longer no longer holds its sessions
    pub takeover_after: Duration,
    // a packet older than the stored one by less than this is a late arrival
    pub reorder_window: Duration,
    // most keys one session remembers as waiting on the user
    pub max_pending: usize,
}

/// What happened to one agent packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentOutcome {
    // a new session was stored; evicted names the session removed to make room
    Created { evicted: Option<String> },
    // a known session was updated
    Stored,
    // an ended packet removed the session
    Removed,
    // an ended packet for a session the table does not hold
    Ignored,
    // another source IP owns the session and sent something recently
    NotOwner,
    // older than the stored packet, inside the reorder window
    OutOfOrder,
    // a new session, and its source already owns max_per_source
    SourceFull,
    // a new session, the table is full and nothing may be evicted
    TableFull,
}

// the key of packets without a sub: the main agent
pub const MAIN_KEY: &str = "main";

/*
 * stores an agent packet as its session's latest one, under the table policy
 *
 * table: the agent table to update
 * packet: the (already validated) agent packet
 * source: the IP it came from (the port is ignored: every hook is a new process)
 * now: the time it arrived
 * limits: caps and windows, see AgentLimits
 * returns: what happened, see AgentOutcome. Order of the checks for a known
 *          session: ownership (and room for a new owner), ended, reorder
 *          guard within one key. For a new one: ended, a late packet of a
 *          session that just ended, per-source cap, table cap (with eviction).
 */
pub fn insert_agent(
    table: &mut AgentTable,
    packet: AgentPacket,
    source: IpAddr,
    now: Instant,
    limits: AgentLimits,
) -> AgentOutcome {
    let sid = packet.sid.clone();
    let key = packet_key(&packet);
    forget_old_endings(table, now, limits.reorder_window);

    // counted before the entry is borrowed below
    let owned_by_source = count_owned_by(table, source);

    let existing = table.sessions.get_mut(&sid);
    match existing {
        Some(entry) => {
            // another IP holds the session: refuse while that owner is recent
            if entry.owner != source {
                // saturating: a packet stamped before last_seen counts as 0 s silent
                let silent_for = now.saturating_duration_since(entry.last_seen);
                if silent_for < limits.takeover_after {
                    return AgentOutcome::NotOwner;
                }

                // a takeover must not lift the new owner over its cap
                if owned_by_source >= limits.max_per_source {
                    return AgentOutcome::SourceFull;
                }
            }

            // ended: the row goes at once (the reorder guard does not apply)
            if packet.state == AgentState::Ended {
                table.sessions.remove(&sid);
                remember_ending(table, &sid, packet.t, now);
                return AgentOutcome::Removed;
            }

            /*
             * a late arrival is dropped; a much older one means the clock
             * jumped back. Only within one key: parallel subagents run in
             * separate processes, so their packets interleave in any order
             */
            if key == entry.last_key && packet.t < entry.packet.t {
                let older_by = Duration::from_millis(entry.packet.t - packet.t);
                if older_by < limits.reorder_window {
                    return AgentOutcome::OutOfOrder;
                }
            }

            update_pending(&mut entry.pending, &key, &packet, limits.max_pending);
            let shown = shown_packet(packet, &entry.pending);

            if shown.state != entry.packet.state {
                entry.state_since = now;
            }
            entry.packet = shown;
            entry.owner = source;
            entry.last_seen = now;
            entry.last_key = key;
            AgentOutcome::Stored
        }
        None => {
            // nothing to end
            if packet.state == AgentState::Ended {
                return AgentOutcome::Ignored;
            }

            // a packet sent just before its session's ended packet, arriving after it
            let late = is_late_for_ended(table, &sid, packet.t, limits.reorder_window);
            if late {
                return AgentOutcome::OutOfOrder;
            }

            // one source may not take the whole table
            if owned_by_source >= limits.max_per_source {
                return AgentOutcome::SourceFull;
            }

            // a full table makes room only by evicting a finished session
            let mut evicted: Option<String> = None;
            if table.len() >= limits.max_agents {
                let oldest = oldest_evictable(table);
                let victim = match oldest {
                    Some(v) => v,
                    None => return AgentOutcome::TableFull,
                };
                table.sessions.remove(&victim);
                evicted = Some(victim);
            }

            let mut pending: Vec<(String, String)> = Vec::new();
            update_pending(&mut pending, &key, &packet, limits.max_pending);
            let entry = AgentEntry {
                packet: packet,
                owner: source,
                first_seen: now,
                state_since: now,
                last_seen: now,
                pending: pending,
                last_key: key,
            };
            table.sessions.insert(sid, entry);
            AgentOutcome::Created { evicted: evicted }
        }
    }
}

// the key a packet counts under: its sub, or MAIN_KEY for the main agent
fn packet_key(packet: &AgentPacket) -> String {
    match &packet.sub {
        Some(sub) => sub.clone(),
        None => MAIN_KEY.to_string(),
    }
}

/*
 * updates the keys waiting on the user after one packet from key
 *
 * pending: (key, question) pairs, oldest first
 * packet: the new packet
 * max_pending: most keys kept; a needs_you from one more key is not added
 */
fn update_pending(
    pending: &mut Vec<(String, String)>,
    key: &str,
    packet: &AgentPacket,
    max_pending: usize,
) {
    // a new turn of the main agent (a prompt) or its end clears every key
    if key == MAIN_KEY {
        let turn_ended = packet.state == AgentState::Done;
        let new_prompt = packet.state == AgentState::Working && packet.doing == DOING_THINKING;
        if turn_ended || new_prompt {
            pending.clear();
            return;
        }
    }

    // where this key sits in the list, if it is there
    let mut position: Option<usize> = None;
    for (i, (pending_key, _)) in pending.iter().enumerate() {
        if pending_key == key {
            position = Some(i);
        }
    }

    if packet.state == AgentState::NeedsYou {
        match position {
            // the same key asks again: keep its place, take the new question
            Some(i) => pending[i].1 = packet.doing.clone(),
            None => {
                if pending.len() < max_pending {
                    pending.push((key.to_string(), packet.doing.clone()));
                }
            }
        }
        return;
    }

    // any other state from this key means it is no longer waiting
    match position {
        Some(i) => {
            pending.remove(i);
        }
        None => {}
    }
}

/*
 * what the row shows after a packet: the packet itself, or needs_you with the
 * oldest open question while any key is still waiting
 */
fn shown_packet(packet: AgentPacket, pending: &[(String, String)]) -> AgentPacket {
    if pending.is_empty() {
        return packet;
    }
    let mut shown = packet;
    shown.state = AgentState::NeedsYou;
    shown.doing = pending[0].1.clone();
    shown
}

// drops endings older than the reorder window
fn forget_old_endings(table: &mut AgentTable, now: Instant, window: Duration) {
    let old = std::mem::take(&mut table.ended);
    for ending in old {
        let since = now.saturating_duration_since(ending.at);
        if since < window {
            table.ended.push(ending);
        }
    }
}

// remembers an ending, dropping the oldest past RECENTLY_ENDED_CAP
fn remember_ending(table: &mut AgentTable, sid: &str, t: u64, now: Instant) {
    if table.ended.len() >= RECENTLY_ENDED_CAP {
        table.ended.remove(0);
    }
    let ending = EndedSession {
        sid: sid.to_string(),
        t: t,
        at: now,
    };
    table.ended.push(ending);
}

/*
 * true when sid ended a moment ago and a packet sent up to the reorder window
 * before that ended packet arrives now
 */
fn is_late_for_ended(table: &AgentTable, sid: &str, t: u64, window: Duration) -> bool {
    for ending in &table.ended {
        if ending.sid != sid {
            continue;
        }
        if t > ending.t {
            return false;
        }
        let older_by = Duration::from_millis(ending.t - t);
        return older_by < window;
    }
    false
}

// how many sessions the source IP owns
fn count_owned_by(table: &AgentTable, source: IpAddr) -> usize {
    let mut owned = 0;
    for entry in table.sessions.values() {
        if entry.owner == source {
            owned += 1;
        }
    }
    owned
}

/*
 * the idle, done or error session with the oldest last_seen; working and
 * needs_you sessions are never evicted
 *
 * returns: its sid, or None when every session is working or needs_you
 */
fn oldest_evictable(table: &AgentTable) -> Option<String> {
    let mut oldest: Option<(String, Instant)> = None;
    for (sid, entry) in table.iter() {
        let evictable;
        match entry.packet.state {
            AgentState::Idle => evictable = true,
            AgentState::Done => evictable = true,
            AgentState::Error => evictable = true,
            AgentState::Working => evictable = false,
            AgentState::NeedsYou => evictable = false,
            AgentState::Ended => evictable = false,
        }
        if !evictable {
            continue;
        }

        match &oldest {
            Some((_, oldest_seen)) => {
                if entry.last_seen < *oldest_seen {
                    oldest = Some((sid.clone(), entry.last_seen));
                }
            }
            None => oldest = Some((sid.clone(), entry.last_seen)),
        }
    }

    match oldest {
        Some((sid, _)) => Some(sid),
        None => None,
    }
}

/*
 * removes every session that has been silent for longer than forget_after,
 * whatever its state
 *
 * table: the agent table to clean up
 * now: the current time
 * forget_after: how long a session may stay silent before it is removed
 * returns: the sids that were removed, so the caller can log them
 */
pub fn forget_agents(table: &mut AgentTable, now: Instant, forget_after: Duration) -> Vec<String> {
    // first collect the sids, since a map cannot be changed while looping over it
    let mut expired: Vec<String> = Vec::new();
    for (sid, entry) in table.iter() {
        // saturating: a packet stamped after `now` counts as 0 s silent, never panics
        let silent_for = now.saturating_duration_since(entry.last_seen);
        if silent_for > forget_after {
            expired.push(sid.clone());
        }
    }

    // then remove them
    for sid in &expired {
        table.sessions.remove(sid);
    }

    expired
}

/*
 * one session as the dashboard sees it, with the derived fields
 *
 * sid: the session id (the table key)
 * entry: its entry
 * label: the label of the device with the same device_id, or ""
 * now: the current time
 * quiet_after: a working session silent this long or longer is quiet
 */
pub fn agent_session(
    sid: &str,
    entry: &AgentEntry,
    label: &str,
    now: Instant,
    quiet_after: Duration,
) -> AgentSession {
    // saturating: a time before the entry's times counts as 0 s, never panics
    let running_for = now.saturating_duration_since(entry.first_seen);
    let in_state_for = now.saturating_duration_since(entry.state_since);
    let silent_for = now.saturating_duration_since(entry.last_seen);

    // no news is normal for every state except working
    let quiet;
    if entry.packet.state == AgentState::Working && silent_for >= quiet_after {
        quiet = true;
    } else {
        quiet = false;
    }

    AgentSession {
        sid: sid.to_string(),
        device_id: entry.packet.device_id.clone(),
        label: label.to_string(),
        project: entry.packet.project.clone(),
        state: entry.packet.state,
        doing: entry.packet.doing.clone(),
        running_s: running_for.as_secs(),
        state_s: in_state_for.as_secs(),
        silent_s: silent_for.as_secs(),
        quiet: quiet,
        ctx: entry.packet.ctx,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    // small limits so tests can fill the table quickly
    const LIMITS: AgentLimits = AgentLimits {
        max_agents: 4,
        max_per_source: 3,
        takeover_after: Duration::from_secs(120),
        reorder_window: Duration::from_secs(5),
        max_pending: 2,
    };
    const QUIET: Duration = Duration::from_secs(600);
    const FORGET: Duration = Duration::from_secs(4 * 3600);

    // machines on the LAN
    fn ip(last: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(192, 168, 1, last))
    }
    fn ip_a() -> IpAddr {
        ip(60)
    }
    fn ip_b() -> IpAddr {
        ip(61)
    }

    // a 12-hex sid made from a small number, e.g. 000000000001
    fn sid(n: u32) -> String {
        format!("{n:012x}")
    }

    // a valid agent packet for a session, in a state, at sender time t (ms)
    fn packet(session: &str, state: AgentState, t: u64) -> AgentPacket {
        AgentPacket {
            kind: "agent".to_string(),
            v: 1,
            sid: session.to_string(),
            device_id: "dp-3f9a0c41".to_string(),
            project: "deskpane".to_string(),
            state: state,
            doing: String::new(),
            t: t,
            ctx: None,
            sub: None,
        }
    }

    // insert from machine A
    fn insert_a(table: &mut AgentTable, p: AgentPacket, now: Instant) -> AgentOutcome {
        insert_agent(table, p, ip_a(), now, LIMITS)
    }

    // ---- insert ----

    #[test]
    fn new_session_is_stored_and_owned_by_source() {
        let mut table = AgentTable::new();
        let now = Instant::now();

        let outcome = insert_a(&mut table, packet(&sid(1), AgentState::Working, 1000), now);

        assert_eq!(outcome, AgentOutcome::Created { evicted: None });
        let entry = table.get(&sid(1)).unwrap();
        assert_eq!(entry.owner, ip_a());
        assert_eq!(entry.first_seen, now);
        assert_eq!(entry.state_since, now);
        assert_eq!(entry.last_seen, now);
        assert_eq!(entry.packet.state, AgentState::Working);
    }

    #[test]
    fn known_session_is_updated() {
        let mut table = AgentTable::new();
        let start = Instant::now();
        insert_a(
            &mut table,
            packet(&sid(1), AgentState::Working, 1000),
            start,
        );

        let later = start + Duration::from_secs(3);
        let mut next = packet(&sid(1), AgentState::Working, 4000);
        next.doing = "Bash".to_string();
        let outcome = insert_a(&mut table, next, later);

        assert_eq!(outcome, AgentOutcome::Stored);
        let entry = table.get(&sid(1)).unwrap();
        assert_eq!(entry.packet.doing, "Bash");
        assert_eq!(entry.last_seen, later);
        assert_eq!(entry.first_seen, start);
    }

    #[test]
    fn state_change_resets_state_since() {
        let mut table = AgentTable::new();
        let start = Instant::now();
        insert_a(
            &mut table,
            packet(&sid(1), AgentState::Working, 1000),
            start,
        );

        let later = start + Duration::from_secs(30);
        insert_a(
            &mut table,
            packet(&sid(1), AgentState::NeedsYou, 31000),
            later,
        );

        let entry = table.get(&sid(1)).unwrap();
        assert_eq!(entry.state_since, later);
        assert_eq!(entry.packet.state, AgentState::NeedsYou);
    }

    #[test]
    fn same_state_keeps_state_since() {
        let mut table = AgentTable::new();
        let start = Instant::now();
        insert_a(
            &mut table,
            packet(&sid(1), AgentState::Working, 1000),
            start,
        );

        let later = start + Duration::from_secs(30);
        insert_a(
            &mut table,
            packet(&sid(1), AgentState::Working, 31000),
            later,
        );

        let entry = table.get(&sid(1)).unwrap();
        assert_eq!(entry.state_since, start);
        assert_eq!(entry.last_seen, later);
    }

    #[test]
    fn ended_removes_the_session() {
        let mut table = AgentTable::new();
        let start = Instant::now();
        insert_a(&mut table, packet(&sid(1), AgentState::Done, 1000), start);

        let outcome = insert_a(&mut table, packet(&sid(1), AgentState::Ended, 2000), start);

        assert_eq!(outcome, AgentOutcome::Removed);
        assert!(table.is_empty());
    }

    #[test]
    fn ended_for_unknown_sid_is_ignored() {
        let mut table = AgentTable::new();

        let outcome = insert_a(
            &mut table,
            packet(&sid(1), AgentState::Ended, 1000),
            Instant::now(),
        );

        assert_eq!(outcome, AgentOutcome::Ignored);
        assert!(table.is_empty());
    }

    #[test]
    fn ended_from_another_recent_ip_is_refused() {
        let mut table = AgentTable::new();
        let start = Instant::now();
        insert_a(
            &mut table,
            packet(&sid(1), AgentState::Working, 1000),
            start,
        );

        let outcome = insert_agent(
            &mut table,
            packet(&sid(1), AgentState::Ended, 2000),
            ip_b(),
            start + Duration::from_secs(1),
            LIMITS,
        );

        assert_eq!(outcome, AgentOutcome::NotOwner);
        assert!(table.contains_key(&sid(1)));
    }

    #[test]
    fn other_ip_is_refused_while_owner_is_recent() {
        let mut table = AgentTable::new();
        let start = Instant::now();
        insert_a(
            &mut table,
            packet(&sid(1), AgentState::Working, 1000),
            start,
        );

        let later = start + Duration::from_secs(119);
        let outcome = insert_agent(
            &mut table,
            packet(&sid(1), AgentState::NeedsYou, 120000),
            ip_b(),
            later,
            LIMITS,
        );

        assert_eq!(outcome, AgentOutcome::NotOwner);
        let entry = table.get(&sid(1)).unwrap();
        assert_eq!(entry.owner, ip_a());
        assert_eq!(entry.packet.state, AgentState::Working);
        assert_eq!(entry.last_seen, start);
    }

    #[test]
    fn other_ip_takes_over_after_takeover_window() {
        let mut table = AgentTable::new();
        let start = Instant::now();
        insert_a(
            &mut table,
            packet(&sid(1), AgentState::Working, 1000),
            start,
        );

        // exactly the takeover window counts as silent long enough
        let later = start + LIMITS.takeover_after;
        let outcome = insert_agent(
            &mut table,
            packet(&sid(1), AgentState::NeedsYou, 121000),
            ip_b(),
            later,
            LIMITS,
        );

        assert_eq!(outcome, AgentOutcome::Stored);
        let entry = table.get(&sid(1)).unwrap();
        assert_eq!(entry.owner, ip_b());
        assert_eq!(entry.packet.state, AgentState::NeedsYou);
        assert_eq!(entry.first_seen, start);
    }

    #[test]
    fn older_packet_inside_reorder_window_is_dropped() {
        // PermissionRequest arrived first, its PreToolUse 100 ms later
        let mut table = AgentTable::new();
        let start = Instant::now();
        insert_a(
            &mut table,
            packet(&sid(1), AgentState::NeedsYou, 10_100),
            start,
        );

        let outcome = insert_a(
            &mut table,
            packet(&sid(1), AgentState::Working, 10_000),
            start,
        );

        assert_eq!(outcome, AgentOutcome::OutOfOrder);
        let entry = table.get(&sid(1)).unwrap();
        assert_eq!(entry.packet.state, AgentState::NeedsYou);
    }

    #[test]
    fn older_packet_just_inside_the_window_is_dropped() {
        let mut table = AgentTable::new();
        let start = Instant::now();
        insert_a(
            &mut table,
            packet(&sid(1), AgentState::NeedsYou, 20_000),
            start,
        );

        let outcome = insert_a(
            &mut table,
            packet(&sid(1), AgentState::Working, 15_001),
            start,
        );

        assert_eq!(outcome, AgentOutcome::OutOfOrder);
    }

    #[test]
    fn much_older_packet_counts_as_clock_change_and_is_stored() {
        // the sender's clock jumped back by 5 s or more (NTP): accept, never freeze the row
        let mut table = AgentTable::new();
        let start = Instant::now();
        insert_a(
            &mut table,
            packet(&sid(1), AgentState::NeedsYou, 20_000),
            start,
        );

        let outcome = insert_a(
            &mut table,
            packet(&sid(1), AgentState::Working, 15_000),
            start,
        );

        assert_eq!(outcome, AgentOutcome::Stored);
        assert_eq!(
            table.get(&sid(1)).unwrap().packet.state,
            AgentState::Working
        );
    }

    #[test]
    fn packet_with_the_same_time_is_stored() {
        let mut table = AgentTable::new();
        let start = Instant::now();
        insert_a(
            &mut table,
            packet(&sid(1), AgentState::Working, 5000),
            start,
        );

        let outcome = insert_a(
            &mut table,
            packet(&sid(1), AgentState::NeedsYou, 5000),
            start,
        );

        assert_eq!(outcome, AgentOutcome::Stored);
    }

    #[test]
    fn per_source_cap_refuses_ninth_session() {
        // the real cap is 8; LIMITS uses 3, so the 4th session from one IP is refused
        let mut table = AgentTable::new();
        let now = Instant::now();
        for n in 1..=3 {
            let outcome = insert_a(&mut table, packet(&sid(n), AgentState::Working, 1), now);
            assert_eq!(outcome, AgentOutcome::Created { evicted: None });
        }

        let refused = insert_a(&mut table, packet(&sid(4), AgentState::Working, 1), now);
        let other_ip = insert_agent(
            &mut table,
            packet(&sid(5), AgentState::Working, 1),
            ip_b(),
            now,
            LIMITS,
        );

        assert_eq!(refused, AgentOutcome::SourceFull);
        assert_eq!(other_ip, AgentOutcome::Created { evicted: None });
        assert!(!table.contains_key(&sid(4)));
    }

    #[test]
    fn source_at_cap_can_still_update_its_sessions() {
        let mut table = AgentTable::new();
        let now = Instant::now();
        for n in 1..=3 {
            insert_a(&mut table, packet(&sid(n), AgentState::Working, 1), now);
        }

        let outcome = insert_a(&mut table, packet(&sid(2), AgentState::Done, 2), now);

        assert_eq!(outcome, AgentOutcome::Stored);
    }

    // fills the table with four sessions from four IPs, in the given states,
    // last seen 1, 2, 3 and 4 seconds after start
    fn full_table(states: [AgentState; 4], start: Instant) -> AgentTable {
        let mut table = AgentTable::new();
        for (i, state) in states.iter().enumerate() {
            let n = i as u32 + 1;
            let at = start + Duration::from_secs(n as u64);
            let outcome = insert_agent(
                &mut table,
                packet(&sid(n), *state, 1),
                ip(n as u8),
                at,
                LIMITS,
            );
            assert_eq!(outcome, AgentOutcome::Created { evicted: None });
        }
        table
    }

    #[test]
    fn full_table_evicts_oldest_done_or_idle() {
        let start = Instant::now();
        let mut table = full_table(
            [
                AgentState::Working,
                AgentState::Done,
                AgentState::Idle,
                AgentState::Done,
            ],
            start,
        );

        let now = start + Duration::from_secs(10);
        let outcome = insert_agent(
            &mut table,
            packet(&sid(9), AgentState::Working, 1),
            ip(9),
            now,
            LIMITS,
        );

        // session 2 (done, last seen at 2 s) is the oldest evictable one
        assert_eq!(
            outcome,
            AgentOutcome::Created {
                evicted: Some(sid(2))
            }
        );
        assert_eq!(table.len(), 4);
        assert!(!table.contains_key(&sid(2)));
        assert!(table.contains_key(&sid(9)));
    }

    #[test]
    fn full_table_never_evicts_working_or_needs_you() {
        let start = Instant::now();
        let mut table = full_table(
            [
                AgentState::Working,
                AgentState::NeedsYou,
                AgentState::Working,
                AgentState::NeedsYou,
            ],
            start,
        );

        let now = start + Duration::from_secs(10);
        let outcome = insert_agent(
            &mut table,
            packet(&sid(9), AgentState::Working, 1),
            ip(9),
            now,
            LIMITS,
        );

        assert_eq!(outcome, AgentOutcome::TableFull);
        assert_eq!(table.len(), 4);
        assert!(!table.contains_key(&sid(9)));
    }

    #[test]
    fn full_table_may_evict_error() {
        let start = Instant::now();
        let mut table = full_table(
            [
                AgentState::Working,
                AgentState::NeedsYou,
                AgentState::Error,
                AgentState::Working,
            ],
            start,
        );

        let now = start + Duration::from_secs(10);
        let outcome = insert_agent(
            &mut table,
            packet(&sid(9), AgentState::Idle, 1),
            ip(9),
            now,
            LIMITS,
        );

        assert_eq!(
            outcome,
            AgentOutcome::Created {
                evicted: Some(sid(3))
            }
        );
    }

    #[test]
    fn source_cap_is_checked_before_table_full() {
        let start = Instant::now();
        let mut table = AgentTable::new();
        for n in 1..=3 {
            insert_a(&mut table, packet(&sid(n), AgentState::Done, 1), start);
        }
        insert_agent(
            &mut table,
            packet(&sid(4), AgentState::Done, 1),
            ip_b(),
            start,
            LIMITS,
        );

        let outcome = insert_a(&mut table, packet(&sid(5), AgentState::Working, 1), start);

        assert_eq!(outcome, AgentOutcome::SourceFull);
        assert_eq!(table.len(), 4);
    }

    #[test]
    fn zero_max_agents_refuses_every_new_session() {
        let mut table = AgentTable::new();
        let limits = AgentLimits {
            max_agents: 0,
            max_per_source: 3,
            takeover_after: LIMITS.takeover_after,
            reorder_window: LIMITS.reorder_window,
            max_pending: LIMITS.max_pending,
        };

        let outcome = insert_agent(
            &mut table,
            packet(&sid(1), AgentState::Working, 1),
            ip_a(),
            Instant::now(),
            limits,
        );

        assert_eq!(outcome, AgentOutcome::TableFull);
        assert!(table.is_empty());
    }

    // ---- the denied-tool sequence from the T-038 spike ----

    #[test]
    fn needs_you_then_done_leaves_needs_you() {
        /*
         * a denied tool: PreToolUse (working), PermissionRequest (needs you),
         * the user denies, no PostToolUse or PostToolUseFailure comes, the
         * next event is Stop (done). The row must leave needs_you.
         */
        let mut table = AgentTable::new();
        let start = Instant::now();

        insert_a(
            &mut table,
            packet(&sid(1), AgentState::Working, 10_000),
            start,
        );
        insert_a(
            &mut table,
            packet(&sid(1), AgentState::NeedsYou, 10_100),
            start + Duration::from_millis(100),
        );
        let stop = insert_a(
            &mut table,
            packet(&sid(1), AgentState::Done, 14_000),
            start + Duration::from_secs(4),
        );

        assert_eq!(stop, AgentOutcome::Stored);
        let entry = table.get(&sid(1)).unwrap();
        assert_eq!(entry.packet.state, AgentState::Done);
        assert_eq!(entry.state_since, start + Duration::from_secs(4));
    }

    #[test]
    fn approved_tool_goes_back_to_working() {
        // PreToolUse, PermissionRequest, allow, the tool runs, PostToolUse
        let mut table = AgentTable::new();
        let start = Instant::now();

        insert_a(
            &mut table,
            packet(&sid(1), AgentState::Working, 10_000),
            start,
        );
        insert_a(
            &mut table,
            packet(&sid(1), AgentState::NeedsYou, 10_100),
            start,
        );
        insert_a(
            &mut table,
            packet(&sid(1), AgentState::Working, 12_000),
            start + Duration::from_secs(2),
        );

        assert_eq!(
            table.get(&sid(1)).unwrap().packet.state,
            AgentState::Working
        );
    }

    // ---- derived fields ----

    // a table with one session created at start, then the state set at state_at
    fn one_session(state: AgentState, start: Instant, state_at: Instant) -> AgentTable {
        let mut table = AgentTable::new();
        insert_a(&mut table, packet(&sid(1), AgentState::Idle, 1), start);
        insert_a(&mut table, packet(&sid(1), state, 2), state_at);
        table
    }

    #[test]
    fn session_carries_the_packet_fields_and_label() {
        let start = Instant::now();
        let table = one_session(AgentState::NeedsYou, start, start);
        let entry = table.get(&sid(1)).unwrap();

        let session = agent_session(&sid(1), entry, "my-pc", start, QUIET);

        assert_eq!(session.sid, sid(1));
        assert_eq!(session.device_id, "dp-3f9a0c41");
        assert_eq!(session.label, "my-pc");
        assert_eq!(session.project, "deskpane");
        assert_eq!(session.state, AgentState::NeedsYou);
        assert_eq!(session.ctx, None);
    }

    #[test]
    fn running_and_state_seconds_are_whole_seconds() {
        let start = Instant::now();
        let table = one_session(AgentState::Working, start, start + Duration::from_secs(20));
        let entry = table.get(&sid(1)).unwrap();

        let now = start + Duration::from_millis(65_900);
        let session = agent_session(&sid(1), entry, "", now, QUIET);

        assert_eq!(session.running_s, 65);
        assert_eq!(session.state_s, 45);
    }

    #[test]
    fn silent_seconds_count_from_the_last_packet() {
        /*
         * working since start, last packet at 20 s (same state, so state_s
         * keeps counting from start): silent_s is only the time since 20 s
         */
        let start = Instant::now();
        let mut table = AgentTable::new();
        insert_a(&mut table, packet(&sid(1), AgentState::Working, 1), start);
        insert_a(
            &mut table,
            packet(&sid(1), AgentState::Working, 20_000),
            start + Duration::from_secs(20),
        );
        let entry = table.get(&sid(1)).unwrap();

        let session = agent_session(
            &sid(1),
            entry,
            "",
            start + Duration::from_millis(80_900),
            QUIET,
        );

        assert_eq!(session.state_s, 80);
        assert_eq!(session.silent_s, 60);

        // a now before the last packet counts as 0 s silent
        let earlier = agent_session(&sid(1), entry, "", start, QUIET);
        assert_eq!(earlier.silent_s, 0);
    }

    #[test]
    fn working_becomes_quiet_after_quiet_window() {
        let start = Instant::now();
        let table = one_session(AgentState::Working, start, start);
        let entry = table.get(&sid(1)).unwrap();

        let before = agent_session(
            &sid(1),
            entry,
            "",
            start + QUIET - Duration::from_secs(1),
            QUIET,
        );
        let at = agent_session(&sid(1), entry, "", start + QUIET, QUIET);

        assert!(!before.quiet);
        assert!(at.quiet);
    }

    #[test]
    fn needs_you_is_never_quiet() {
        let start = Instant::now();
        let table = one_session(AgentState::NeedsYou, start, start);
        let entry = table.get(&sid(1)).unwrap();

        let session = agent_session(&sid(1), entry, "", start + QUIET * 3, QUIET);

        assert!(!session.quiet);
    }

    #[test]
    fn error_is_never_quiet() {
        let start = Instant::now();
        let table = one_session(AgentState::Error, start, start);
        let entry = table.get(&sid(1)).unwrap();

        let session = agent_session(&sid(1), entry, "", start + QUIET * 3, QUIET);

        assert!(!session.quiet);
    }

    #[test]
    fn done_and_idle_are_never_quiet() {
        let start = Instant::now();
        for state in [AgentState::Done, AgentState::Idle] {
            let table = one_session(state, start, start);
            let entry = table.get(&sid(1)).unwrap();
            let session = agent_session(&sid(1), entry, "", start + QUIET * 3, QUIET);
            assert!(!session.quiet);
        }
    }

    #[test]
    fn time_going_backwards_does_not_panic() {
        // a "now" before the entry's times counts as 0 s everywhere
        let start = Instant::now() + Duration::from_secs(60);
        let mut table = one_session(AgentState::Working, start, start);
        let earlier = start - Duration::from_secs(30);

        let entry = table.get(&sid(1)).unwrap();
        let session = agent_session(&sid(1), entry, "", earlier, QUIET);
        assert_eq!(session.running_s, 0);
        assert_eq!(session.state_s, 0);
        assert!(!session.quiet);

        // inserting and forgetting with an earlier time also stays calm
        let outcome = insert_agent(
            &mut table,
            packet(&sid(1), AgentState::Done, 3),
            ip_b(),
            earlier,
            LIMITS,
        );
        assert_eq!(outcome, AgentOutcome::NotOwner);
        let forgotten = forget_agents(&mut table, earlier, FORGET);
        assert!(forgotten.is_empty());
    }

    // ---- forget ----

    #[test]
    fn forget_removes_sessions_silent_for_forget_window() {
        let mut table = AgentTable::new();
        let start = Instant::now();
        insert_a(&mut table, packet(&sid(1), AgentState::NeedsYou, 1), start);
        insert_a(&mut table, packet(&sid(2), AgentState::Working, 1), start);
        insert_a(
            &mut table,
            packet(&sid(3), AgentState::Done, 1),
            start + Duration::from_secs(3600),
        );

        let now = start + FORGET + Duration::from_secs(1);
        let mut forgotten = forget_agents(&mut table, now, FORGET);

        // whatever their state; a HashMap has no fixed order, so sort first
        forgotten.sort();
        assert_eq!(forgotten, vec![sid(1), sid(2)]);
        assert_eq!(table.len(), 1);
        assert!(table.contains_key(&sid(3)));
    }

    #[test]
    fn session_silent_exactly_forget_window_is_kept() {
        let mut table = AgentTable::new();
        let start = Instant::now();
        insert_a(&mut table, packet(&sid(1), AgentState::Working, 1), start);

        let forgotten = forget_agents(&mut table, start + FORGET, FORGET);

        assert!(forgotten.is_empty());
        assert_eq!(table.len(), 1);
    }

    #[test]
    fn forget_on_empty_table_removes_nothing() {
        let mut table = AgentTable::new();
        assert!(forget_agents(&mut table, Instant::now(), FORGET).is_empty());
    }

    // ---- review fixes (T-045) ----

    // a packet from a subagent (sub key) with a doing text
    fn sub_packet(session: &str, sub: &str, state: AgentState, doing: &str, t: u64) -> AgentPacket {
        let mut p = packet(session, state, t);
        p.sub = Some(sub.to_string());
        p.doing = doing.to_string();
        p
    }

    // a packet from the main agent with a doing text
    fn main_packet(session: &str, state: AgentState, doing: &str, t: u64) -> AgentPacket {
        let mut p = packet(session, state, t);
        p.doing = doing.to_string();
        p
    }

    // the stored state and doing text of a session
    fn shown(table: &AgentTable, session: &str) -> (AgentState, String) {
        let entry = table.get(session).unwrap();
        (entry.packet.state, entry.packet.doing.clone())
    }

    #[test]
    fn two_parallel_subs_one_waiting_stays_needs_you() {
        let mut table = AgentTable::new();
        let now = Instant::now();
        insert_a(
            &mut table,
            main_packet(&sid(1), AgentState::Working, "Agent", 1000),
            now,
        );
        insert_a(
            &mut table,
            sub_packet(&sid(1), "0000000a", AgentState::Working, "Grep", 1100),
            now,
        );
        insert_a(
            &mut table,
            sub_packet(
                &sid(1),
                "0000000b",
                AgentState::NeedsYou,
                "allow Bash?",
                1200,
            ),
            now,
        );

        // the other subagent keeps working: the row must keep saying needs you
        let outcome = insert_a(
            &mut table,
            sub_packet(&sid(1), "0000000a", AgentState::Working, "Read", 1300),
            now,
        );
        insert_a(
            &mut table,
            main_packet(&sid(1), AgentState::Working, "Agent", 1400),
            now,
        );

        assert_eq!(outcome, AgentOutcome::Stored);
        assert_eq!(
            shown(&table, &sid(1)),
            (AgentState::NeedsYou, "allow Bash?".to_string())
        );
    }

    #[test]
    fn answered_sub_clears_its_own_key() {
        let mut table = AgentTable::new();
        let now = Instant::now();
        insert_a(
            &mut table,
            sub_packet(
                &sid(1),
                "0000000b",
                AgentState::NeedsYou,
                "allow Bash?",
                1000,
            ),
            now,
        );
        insert_a(
            &mut table,
            sub_packet(&sid(1), "0000000a", AgentState::Working, "Grep", 1100),
            now,
        );

        insert_a(
            &mut table,
            sub_packet(&sid(1), "0000000b", AgentState::Working, "Bash", 1200),
            now,
        );

        assert_eq!(
            shown(&table, &sid(1)),
            (AgentState::Working, "Bash".to_string())
        );
        assert!(table.get(&sid(1)).unwrap().pending.is_empty());
    }

    #[test]
    fn two_waiting_keys_need_two_answers() {
        let mut table = AgentTable::new();
        let now = Instant::now();
        insert_a(
            &mut table,
            sub_packet(
                &sid(1),
                "0000000a",
                AgentState::NeedsYou,
                "allow Edit?",
                1000,
            ),
            now,
        );
        insert_a(
            &mut table,
            sub_packet(
                &sid(1),
                "0000000b",
                AgentState::NeedsYou,
                "allow Bash?",
                1100,
            ),
            now,
        );

        insert_a(
            &mut table,
            sub_packet(&sid(1), "0000000a", AgentState::Working, "Edit", 1200),
            now,
        );

        // still waiting on b; the doing text is b's question
        assert_eq!(
            shown(&table, &sid(1)),
            (AgentState::NeedsYou, "allow Bash?".to_string())
        );
    }

    #[test]
    fn main_done_clears_every_pending_key() {
        let mut table = AgentTable::new();
        let now = Instant::now();
        insert_a(
            &mut table,
            sub_packet(
                &sid(1),
                "0000000a",
                AgentState::NeedsYou,
                "allow Edit?",
                1000,
            ),
            now,
        );
        insert_a(
            &mut table,
            main_packet(&sid(1), AgentState::NeedsYou, "allow Bash?", 1100),
            now,
        );

        insert_a(
            &mut table,
            main_packet(&sid(1), AgentState::Done, "", 2000),
            now,
        );

        assert_eq!(shown(&table, &sid(1)), (AgentState::Done, String::new()));
        assert!(table.get(&sid(1)).unwrap().pending.is_empty());
    }

    #[test]
    fn main_prompt_clears_pending_but_main_tool_work_does_not() {
        let mut table = AgentTable::new();
        let now = Instant::now();
        insert_a(
            &mut table,
            sub_packet(
                &sid(1),
                "0000000a",
                AgentState::NeedsYou,
                "allow Edit?",
                1000,
            ),
            now,
        );

        // a main tool call is not a new turn
        insert_a(
            &mut table,
            main_packet(&sid(1), AgentState::Working, "Bash", 1100),
            now,
        );
        assert_eq!(shown(&table, &sid(1)).0, AgentState::NeedsYou);

        // a new prompt (UserPromptSubmit sends working "thinking") is
        insert_a(
            &mut table,
            main_packet(&sid(1), AgentState::Working, "thinking", 1200),
            now,
        );
        assert_eq!(
            shown(&table, &sid(1)),
            (AgentState::Working, "thinking".to_string())
        );
    }

    #[test]
    fn subagent_done_does_not_clear_other_keys() {
        let mut table = AgentTable::new();
        let now = Instant::now();
        insert_a(
            &mut table,
            main_packet(&sid(1), AgentState::NeedsYou, "allow Bash?", 1000),
            now,
        );

        insert_a(
            &mut table,
            sub_packet(&sid(1), "0000000a", AgentState::Done, "", 1100),
            now,
        );

        assert_eq!(shown(&table, &sid(1)).0, AgentState::NeedsYou);
    }

    #[test]
    fn pending_set_is_capped() {
        // LIMITS.max_pending is 2 (the daemon uses 8)
        let mut table = AgentTable::new();
        let now = Instant::now();
        for (i, sub) in ["0000000a", "0000000b", "0000000c"].iter().enumerate() {
            let t = 1000 + i as u64;
            insert_a(
                &mut table,
                sub_packet(&sid(1), sub, AgentState::NeedsYou, "allow Bash?", t),
                now,
            );
        }

        let entry = table.get(&sid(1)).unwrap();
        assert_eq!(entry.pending.len(), 2);
        assert_eq!(entry.packet.state, AgentState::NeedsYou);
    }

    #[test]
    fn packets_from_different_keys_are_not_reordered_against_each_other() {
        // hooks of parallel subagents run in different processes, in any order
        let mut table = AgentTable::new();
        let now = Instant::now();
        insert_a(
            &mut table,
            sub_packet(
                &sid(1),
                "0000000b",
                AgentState::NeedsYou,
                "allow Bash?",
                10_100,
            ),
            now,
        );

        let outcome = insert_a(
            &mut table,
            sub_packet(&sid(1), "0000000a", AgentState::Working, "Grep", 10_000),
            now,
        );

        assert_eq!(outcome, AgentOutcome::Stored);
        assert_eq!(shown(&table, &sid(1)).0, AgentState::NeedsYou);
    }

    #[test]
    fn denied_tool_with_a_sub_key_still_ends_in_done() {
        // inside a subagent: PreToolUse, PermissionRequest, denied, then the main Stop
        let mut table = AgentTable::new();
        let now = Instant::now();
        insert_a(
            &mut table,
            sub_packet(&sid(1), "0000000a", AgentState::Working, "Bash", 10_000),
            now,
        );
        insert_a(
            &mut table,
            sub_packet(
                &sid(1),
                "0000000a",
                AgentState::NeedsYou,
                "allow Bash?",
                10_100,
            ),
            now,
        );

        insert_a(
            &mut table,
            main_packet(&sid(1), AgentState::Done, "", 14_000),
            now,
        );

        assert_eq!(shown(&table, &sid(1)).0, AgentState::Done);
    }

    #[test]
    fn packet_after_ended_inside_reorder_window_is_dropped() {
        // the Stop of a session arrives just after its SessionEnd
        let mut table = AgentTable::new();
        let start = Instant::now();
        insert_a(
            &mut table,
            packet(&sid(1), AgentState::Working, 1000),
            start,
        );
        insert_a(&mut table, packet(&sid(1), AgentState::Ended, 2000), start);

        let late = insert_a(
            &mut table,
            packet(&sid(1), AgentState::Done, 1900),
            start + Duration::from_millis(50),
        );

        assert_eq!(late, AgentOutcome::OutOfOrder);
        assert!(table.is_empty());
    }

    #[test]
    fn newer_packet_after_ended_starts_the_session_again() {
        let mut table = AgentTable::new();
        let start = Instant::now();
        insert_a(
            &mut table,
            packet(&sid(1), AgentState::Working, 1000),
            start,
        );
        insert_a(&mut table, packet(&sid(1), AgentState::Ended, 2000), start);

        let resumed = insert_a(&mut table, packet(&sid(1), AgentState::Idle, 9000), start);

        assert_eq!(resumed, AgentOutcome::Created { evicted: None });
    }

    #[test]
    fn ended_sid_is_forgotten_after_the_reorder_window() {
        let mut table = AgentTable::new();
        let start = Instant::now();
        insert_a(
            &mut table,
            packet(&sid(1), AgentState::Working, 1000),
            start,
        );
        insert_a(&mut table, packet(&sid(1), AgentState::Ended, 2000), start);

        let later = start + LIMITS.reorder_window;
        let outcome = insert_a(&mut table, packet(&sid(1), AgentState::Done, 1900), later);

        assert_eq!(outcome, AgentOutcome::Created { evicted: None });
        assert!(table.ended.is_empty());
    }

    #[test]
    fn recently_ended_list_is_capped() {
        let mut table = AgentTable::new();
        let start = Instant::now();
        let many = RECENTLY_ENDED_CAP as u32 + 5;
        for n in 1..=many {
            insert_agent(
                &mut table,
                packet(&sid(n), AgentState::Done, 1),
                ip(n as u8),
                start,
                LIMITS,
            );
            insert_agent(
                &mut table,
                packet(&sid(n), AgentState::Ended, 2),
                ip(n as u8),
                start,
                LIMITS,
            );
        }

        assert_eq!(table.ended.len(), RECENTLY_ENDED_CAP);
    }

    #[test]
    fn takeover_by_a_full_source_is_refused() {
        // ip_b already owns max_per_source sessions; it may not take over a fourth
        let mut table = AgentTable::new();
        let start = Instant::now();
        insert_a(&mut table, packet(&sid(1), AgentState::Working, 1), start);
        for n in 2..=4 {
            insert_agent(
                &mut table,
                packet(&sid(n), AgentState::Working, 1),
                ip_b(),
                start,
                LIMITS,
            );
        }

        let later = start + LIMITS.takeover_after;
        let outcome = insert_agent(
            &mut table,
            packet(&sid(1), AgentState::Done, 2),
            ip_b(),
            later,
            LIMITS,
        );

        assert_eq!(outcome, AgentOutcome::SourceFull);
        assert_eq!(table.get(&sid(1)).unwrap().owner, ip_a());
    }
}
