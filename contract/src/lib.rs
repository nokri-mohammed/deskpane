//! The JSON shape every reporting device sends, and the shape the daemon
//! relays to the dashboard. See docs/CONTRACT.md for the full spec.

use std::collections::BTreeMap;
use std::net::IpAddr;

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
    // battery charge in percent, 0 to 100; left out by devices without one
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub battery_pct: Option<f64>,
    // true while on external power; left out when unknown
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charging: Option<bool>,
}

/// What the daemon pushes to the dashboard over the Unix socket
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceStatus {
    #[serde(flatten)]
    pub report: Report,
    pub online: bool,
    // true when the report came from a loopback address (the Pi itself)
    #[serde(default)]
    pub local: bool,
}

/// What arrives over UDP: a report plus optional request flags. `Report`
/// itself stays unchanged, so the dashboard's shape does not move.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportPacket {
    #[serde(flatten)]
    pub report: Report,
    // true asks the daemon to answer with an ack reply (deskpane report --once)
    #[serde(default)]
    pub ack: bool,
}

/// The one reply the daemon may send back to a reporter. Absent fields are
/// left out of the JSON; unknown fields are ignored when parsing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reply {
    pub v: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
    #[serde(default)]
    pub device_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_ip: Option<String>,
}

// reply format version; a client ignores a reply with another one
pub const REPLY_VERSION: u32 = 1;
// the only error code in reply version 1
pub const ERROR_ID_IN_USE: &str = "id_in_use";
// a reply is never bigger than this, and always smaller than the report that caused it
pub const MAX_REPLY_BYTES: usize = 192;

impl Reply {
    // "this id belongs to another machine that is still online"
    pub fn id_in_use(device_id: &str, owner: IpAddr) -> Self {
        Reply {
            v: REPLY_VERSION,
            error: Some(ERROR_ID_IN_USE.to_string()),
            ok: None,
            device_id: device_id.to_string(),
            owner_ip: Some(owner.to_string()),
        }
    }

    // "your report was stored", only sent when the report asked for it
    pub fn ack(device_id: &str) -> Self {
        Reply {
            v: REPLY_VERSION,
            error: None,
            ok: Some(true),
            device_id: device_id.to_string(),
            owner_ip: None,
        }
    }
}

/*
 * serializes a reply, but only if it is safe to send: at most
 * MAX_REPLY_BYTES and smaller than the request, so the daemon can never
 * amplify traffic
 *
 * reply: the reply to send
 * request_bytes: size of the report packet that caused it
 * returns: the bytes to send, or None to send nothing
 */
pub fn build_reply(reply: &Reply, request_bytes: usize) -> Option<Vec<u8>> {
    let serialized = serde_json::to_vec(reply);
    let bytes = match serialized {
        Ok(b) => b,
        Err(_) => return None,
    };

    if bytes.len() > MAX_REPLY_BYTES {
        return None;
    }
    if bytes.len() >= request_bytes {
        return None;
    }

    Some(bytes)
}

// limits a report must respect before the daemon stores it (see docs/CONTRACT.md)
pub const MAX_NAME_BYTES: usize = 64;
pub const MAX_IP_BYTES: usize = 45;
pub const MIN_TEMP_C: f64 = -50.0;
pub const MAX_TEMP_C: f64 = 150.0;
pub const MIN_BATTERY_PCT: f64 = 0.0;
pub const MAX_BATTERY_PCT: f64 = 100.0;
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
    // device_id: the strict id rule; other text fields: length, empty or not, no control characters
    let device_id_checked = check_device_id(&report.device_id);
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

    // battery is optional; when sent it is a finite percent (charging is any bool)
    match report.battery_pct {
        Some(pct) => {
            let battery_checked = check_range("battery_pct", pct, MIN_BATTERY_PCT, MAX_BATTERY_PCT);
            match battery_checked {
                Ok(()) => {}
                Err(reason) => return Err(reason),
            }
        }
        None => {}
    }

    Ok(())
}

/*
 * checks a device id against ^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$, so ids are
 * safe in logs, unit files, shell lines and replies
 *
 * device_id: the id to check
 * returns: Ok(()) or Err with a reason starting "device_id:"; the reason never
 *          repeats the value
 */
pub fn check_device_id(device_id: &str) -> Result<(), String> {
    if device_id.is_empty() {
        return Err("device_id: must not be empty".to_string());
    }

    let length = device_id.len();
    if length > MAX_NAME_BYTES {
        return Err(format!(
            "device_id: {length} bytes, longer than the limit of {MAX_NAME_BYTES}"
        ));
    }

    // the first character must be a letter or digit, so it never reads as a flag
    let bytes = device_id.as_bytes();
    if !bytes[0].is_ascii_alphanumeric() {
        return Err("device_id: must start with a letter or digit".to_string());
    }

    // the rest: ASCII letters, digits, dot, dash, underscore
    for b in bytes {
        let allowed;
        if b.is_ascii_alphanumeric() || *b == b'.' || *b == b'-' || *b == b'_' {
            allowed = true;
        } else {
            allowed = false;
        }

        if !allowed {
            return Err(
                "device_id: only letters, digits, dot, dash and underscore are allowed".to_string(),
            );
        }
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

// ---- agent packets (Claude Code session status, docs/plans/2026-11-08-agent-status-view.md) ----

// the kind word of an agent packet; a packet with no kind is a report
pub const PACKET_KIND_AGENT: &str = "agent";
// agent packet version; any other value is dropped
pub const AGENT_PACKET_VERSION: u32 = 1;
// longest project name and "doing" text, in bytes (UTF-8)
pub const MAX_PROJECT_BYTES: usize = 32;
pub const MAX_DOING_BYTES: usize = 32;
// a session id on the wire is exactly this many lowercase hex characters
pub const SID_LEN: usize = 12;
// context used, in percent (reserved for later, not sent in the first beta)
pub const MAX_CTX_PCT: u32 = 100;
// a subagent key on the wire is exactly this many lowercase hex characters
pub const SUB_LEN: usize = 8;
// the "doing" text of a prompt (UserPromptSubmit): a new turn of the main agent
pub const DOING_THINKING: &str = "thinking";

/*
 * true for a Unicode format character (category Cf): bidi overrides and
 * isolates, zero-width spaces and joiners, the byte order mark, tag
 * characters, ... They draw nothing but can reorder or hide text on screen
 */
pub fn is_format_char(c: char) -> bool {
    let code = c as u32;
    for (first, last) in FORMAT_CHAR_RANGES {
        if code >= first && code <= last {
            return true;
        }
    }
    false
}

// every code point of Unicode category Cf, as inclusive ranges
const FORMAT_CHAR_RANGES: [(u32, u32); 21] = [
    (0x00AD, 0x00AD),
    (0x0600, 0x0605),
    (0x061C, 0x061C),
    (0x06DD, 0x06DD),
    (0x070F, 0x070F),
    (0x0890, 0x0891),
    (0x08E2, 0x08E2),
    (0x180E, 0x180E),
    (0x200B, 0x200F),
    (0x202A, 0x202E),
    (0x2060, 0x2064),
    (0x2066, 0x206F),
    (0xFEFF, 0xFEFF),
    (0xFFF9, 0xFFFB),
    (0x110BD, 0x110BD),
    (0x110CD, 0x110CD),
    (0x13430, 0x1343F),
    (0x1BCA0, 0x1BCA3),
    (0x1D173, 0x1D17A),
    (0xE0001, 0xE0001),
    (0xE0020, 0xE007F),
];

/// Only the kind of a packet; every other field is ignored at this step.
#[derive(Debug, Clone, Deserialize)]
pub struct PacketKind {
    #[serde(default)]
    pub kind: Option<String>,
}

/// The state of one agent session, as the client maps hook events to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    Idle,
    Working,
    Done,
    NeedsYou,
    Error,
    Ended,
}

impl AgentState {
    // the word on the wire, e.g. needs_you
    pub fn as_str(&self) -> &'static str {
        match self {
            AgentState::Idle => "idle",
            AgentState::Working => "working",
            AgentState::Done => "done",
            AgentState::NeedsYou => "needs_you",
            AgentState::Error => "error",
            AgentState::Ended => "ended",
        }
    }
}

/// One agent status packet, sent by `deskpane agent` from a Claude Code hook.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentPacket {
    pub kind: String,
    pub v: u32,
    pub sid: String,
    pub device_id: String,
    pub project: String,
    pub state: AgentState,
    pub doing: String,
    // sender wall clock, milliseconds since the Unix epoch
    pub t: u64,
    // context used in percent; reserved, not sent in the first beta
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ctx: Option<u32>,
    // which subagent sent it (8 hex, hashed agent_id); absent for the main agent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sub: Option<String>,
}

/// One agent session as the daemon pushes it to the dashboard, with the
/// fields derived at broadcast time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentSession {
    pub sid: String,
    pub device_id: String,
    // label of the device with the same device_id, else empty
    pub label: String,
    pub project: String,
    pub state: AgentState,
    pub doing: String,
    // seconds since the daemon first saw the session
    pub running_s: u64,
    // seconds since the state last changed
    pub state_s: u64,
    // seconds since the daemon last heard from the session (for "no news for 12m")
    pub silent_s: u64,
    // working, but no packet for AGENT_QUIET_AFTER
    pub quiet: bool,
    // always present, null until context usage is sent (T-048)
    pub ctx: Option<u32>,
}

// version of the socket line envelope
pub const ENVELOPE_VERSION: u32 = 1;

/// The stats view: every device, as the bare array was before the envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatsView {
    pub devices: Vec<DeviceStatus>,
}

/// The agents view: every agent session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentsView {
    pub sessions: Vec<AgentSession>,
}

/// One plugin's numbers as the daemon pushes them to the dashboard.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricView {
    // the plugin's latest values, key -> number
    pub values: BTreeMap<String, f64>,
    // seconds since the daemon last heard from the plugin
    pub age_s: u64,
    // older than the packet's ttl_s: the panel shows its missing text
    pub stale: bool,
}

/// The views in one socket line; later views add keys here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Views {
    pub stats: StatsView,
    pub agents: AgentsView,
    // plugin name -> its numbers; left out of the line when no plugin reports,
    // and absent from daemons older than the metric packet
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metrics: BTreeMap<String, MetricView>,
}

/// One line on the dashboard socket: {"v":1,"views":{"stats":..,"agents":..}}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub v: u32,
    pub views: Views,
}

/// A packet after the kind probe: a report, an agent packet or a metric packet.
#[derive(Debug, Clone)]
pub enum ParsedPacket {
    Report(ReportPacket),
    Agent(AgentPacket),
    Metric(MetricPacket),
}

/// Why a packet could not be parsed. Every reason is safe to log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PacketError {
    // not a JSON object with a text kind (or no kind)
    Malformed(String),
    // no kind, but not a valid report either (the serde reason, as before)
    BadReport(String),
    // kind agent, but not the agent shape; the reason never repeats a value
    BadAgent(String),
    // kind metric, but not the metric shape; the reason never repeats a value
    BadMetric(String),
    // a kind this daemon does not know; the kind itself is never repeated
    UnknownKind,
}

/*
 * reads one datagram: first only its kind, then the full shape for that kind
 *
 * bytes: the datagram as received
 * returns: a report (no kind, as old reporters send), an agent packet
 *          (kind "agent"), or why it was dropped. Only report errors carry
 *          the serde reason (as before); the others never repeat a value.
 */
pub fn parse_packet(bytes: &[u8]) -> Result<ParsedPacket, PacketError> {
    // the probe: only the kind, every other field is ignored at this step
    let probed = serde_json::from_slice::<PacketKind>(bytes);
    let probe = match probed {
        Ok(p) => p,
        Err(e) => {
            let column = e.column();
            return Err(PacketError::Malformed(format!(
                "not a JSON object with a text kind (column {column})"
            )));
        }
    };

    let kind = match probe.kind {
        Some(k) => k,
        None => {
            // no kind: a report, parsed exactly as before
            let parsed = serde_json::from_slice::<ReportPacket>(bytes);
            return match parsed {
                Ok(p) => Ok(ParsedPacket::Report(p)),
                Err(e) => Err(PacketError::BadReport(report_error_reason(&e))),
            };
        }
    };

    if kind == PACKET_KIND_METRIC {
        // serde messages can quote the bad value, so only the column is kept
        let parsed = serde_json::from_slice::<MetricPacket>(bytes);
        return match parsed {
            Ok(p) => Ok(ParsedPacket::Metric(p)),
            Err(e) => {
                let column = e.column();
                Err(PacketError::BadMetric(format!(
                    "does not match the metric packet shape (column {column})"
                )))
            }
        };
    }

    if kind != PACKET_KIND_AGENT {
        return Err(PacketError::UnknownKind);
    }

    // serde messages can quote the bad value, so only the column is kept
    let parsed = serde_json::from_slice::<AgentPacket>(bytes);
    match parsed {
        Ok(p) => Ok(ParsedPacket::Agent(p)),
        Err(e) => {
            let column = e.column();
            Err(PacketError::BadAgent(format!(
                "does not match the agent packet shape (column {column})"
            )))
        }
    }
}

/*
 * a report parse error that is safe to log: the missing field's name (one of
 * our own field names), else only the column. serde's own message can quote
 * the bad value, so it is never passed on
 */
fn report_error_reason(error: &serde_json::Error) -> String {
    let message = error.to_string();
    let stripped = message.strip_prefix("missing field `");
    match stripped {
        Some(rest) => {
            let closing = rest.find('`');
            match closing {
                Some(end) => {
                    let field = &rest[..end];
                    return format!("missing field {field}");
                }
                None => {}
            }
        }
        None => {}
    }

    let column = error.column();
    format!("does not match the report shape (column {column})")
}

/*
 * checks an agent packet against the contract limits, like validate does for
 * reports
 *
 * packet: the parsed agent packet to check
 * returns: Ok(()) or Err with a reason that starts with the field name and
 *          never repeats a bad text value
 */
pub fn validate_agent(packet: &AgentPacket) -> Result<(), String> {
    if packet.kind != PACKET_KIND_AGENT {
        return Err(format!("kind: must be {PACKET_KIND_AGENT}"));
    }

    if packet.v != AGENT_PACKET_VERSION {
        let v = packet.v;
        return Err(format!(
            "v: {v} is not the supported version {AGENT_PACKET_VERSION}"
        ));
    }

    let sid_checked = check_lower_hex("sid", &packet.sid, SID_LEN);
    match sid_checked {
        Ok(()) => {}
        Err(reason) => return Err(reason),
    }

    // sub is optional: only subagent hooks send it
    match &packet.sub {
        Some(sub) => {
            let sub_checked = check_lower_hex("sub", sub, SUB_LEN);
            match sub_checked {
                Ok(()) => {}
                Err(reason) => return Err(reason),
            }
        }
        None => {}
    }

    let device_id_checked = check_device_id(&packet.device_id);
    match device_id_checked {
        Ok(()) => {}
        Err(reason) => return Err(reason),
    }

    // project and doing may be empty (the screen shows "?" for no project)
    let project_checked = check_text("project", &packet.project, MAX_PROJECT_BYTES, true);
    match project_checked {
        Ok(()) => {}
        Err(reason) => return Err(reason),
    }

    let doing_checked = check_text("doing", &packet.doing, MAX_DOING_BYTES, true);
    match doing_checked {
        Ok(()) => {}
        Err(reason) => return Err(reason),
    }

    // format characters (bidi overrides, zero-width) could hide or reorder screen text
    let project_format = check_no_format_chars("project", &packet.project);
    match project_format {
        Ok(()) => {}
        Err(reason) => return Err(reason),
    }
    let doing_format = check_no_format_chars("doing", &packet.doing);
    match doing_format {
        Ok(()) => {}
        Err(reason) => return Err(reason),
    }

    // t is unsigned, so the parser already refused a negative time
    // ctx is optional; a missing one has nothing to check
    let ctx = match packet.ctx {
        Some(c) => c,
        None => 0,
    };
    if ctx > MAX_CTX_PCT {
        return Err(format!("ctx: {ctx} is outside 0..={MAX_CTX_PCT}"));
    }

    Ok(())
}

// ---- metric packets (plugin numbers, docs/plans/2026-11-11-plugin-phase2.md) ----

// the kind word of a metric packet
pub const PACKET_KIND_METRIC: &str = "metric";
// metric packet version; any other value is dropped
pub const METRIC_PACKET_VERSION: u32 = 1;
// longest plugin name and value key, in bytes (both are ASCII slugs)
pub const MAX_PLUGIN_NAME_BYTES: usize = 32;
pub const MAX_METRIC_KEY_BYTES: usize = 32;
// most values in one packet
pub const MAX_METRIC_VALUES: usize = 16;
// largest absolute value a metric may have
pub const MAX_METRIC_ABS: f64 = 1e9;
// how long a packet's values stay fresh, in seconds
pub const MIN_METRIC_TTL_S: u32 = 10;
pub const MAX_METRIC_TTL_S: u32 = 86_400;

/// One metric packet: a plugin's named numbers, sent by a data-source plugin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricPacket {
    pub kind: String,
    pub v: u32,
    // the plugin's manifest name, e.g. weather
    pub plugin: String,
    // how long these values stay fresh, in seconds
    pub ttl_s: u32,
    // key -> number, e.g. temp_c -> 12.4
    pub values: BTreeMap<String, f64>,
}

/*
 * checks a metric packet against the contract limits, like validate_agent does
 * for agent packets
 *
 * packet: the parsed metric packet to check
 * returns: Ok(()) or Err with a reason that starts with the field name and
 *          never repeats a bad name
 */
pub fn validate_metric(packet: &MetricPacket) -> Result<(), String> {
    if packet.kind != PACKET_KIND_METRIC {
        return Err(format!("kind: must be {PACKET_KIND_METRIC}"));
    }

    if packet.v != METRIC_PACKET_VERSION {
        let v = packet.v;
        return Err(format!(
            "v: {v} is not the supported version {METRIC_PACKET_VERSION}"
        ));
    }

    let plugin_checked = check_slug(&packet.plugin, MAX_PLUGIN_NAME_BYTES, b'-');
    match plugin_checked {
        Ok(()) => {}
        Err(rule) => return Err(format!("plugin: {rule}")),
    }

    let ttl_s = packet.ttl_s;
    if ttl_s < MIN_METRIC_TTL_S || ttl_s > MAX_METRIC_TTL_S {
        return Err(format!(
            "ttl_s: {ttl_s} is outside {MIN_METRIC_TTL_S}..={MAX_METRIC_TTL_S}"
        ));
    }

    let count = packet.values.len();
    if count == 0 || count > MAX_METRIC_VALUES {
        return Err(format!(
            "values: {count} entries, must be 1 to {MAX_METRIC_VALUES}"
        ));
    }

    for (key, value) in &packet.values {
        // the key first: once it passes it is a safe slug to name in a reason
        let key_checked = check_slug(key, MAX_METRIC_KEY_BYTES, b'_');
        match key_checked {
            Ok(()) => {}
            Err(rule) => return Err(format!("values: a key {rule}")),
        }

        if !value.is_finite() {
            return Err(format!("values: {key} is not a finite number"));
        }
        if value.abs() > MAX_METRIC_ABS {
            return Err(format!(
                "values: {key} is outside -{MAX_METRIC_ABS}..={MAX_METRIC_ABS}"
            ));
        }
    }

    Ok(())
}

/*
 * checks a plugin name with the same rule validate_metric applies, for
 * callers that read plugin names from elsewhere (the daemon's PANELS list)
 *
 * name: the plugin name to check
 * returns: Ok(()) or Err with the rule (never the name)
 */
pub fn check_plugin_name(name: &str) -> Result<(), String> {
    check_slug(name, MAX_PLUGIN_NAME_BYTES, b'-')
}

/*
 * checks a name is a lowercase ASCII slug: a letter first, then letters,
 * digits and the one separator byte, at most max_bytes long
 *
 * value: the name to check
 * max_bytes: its longest allowed length
 * separator: b'-' for plugin names, b'_' for value keys
 * returns: Ok(()) or Err with the rule (never the value), for the caller to
 *          prefix with the field name
 */
fn check_slug(value: &str, max_bytes: usize, separator: u8) -> Result<(), String> {
    let separator_char = separator as char;
    let rule = format!(
        "must be 1 to {max_bytes} characters of a-z, 0-9 and {separator_char}, starting with a letter"
    );

    let bytes = value.as_bytes();
    if bytes.is_empty() || bytes.len() > max_bytes {
        return Err(rule);
    }

    if !bytes[0].is_ascii_lowercase() {
        return Err(rule);
    }

    for b in bytes {
        let allowed;
        if b.is_ascii_lowercase() || b.is_ascii_digit() || *b == separator {
            allowed = true;
        } else {
            allowed = false;
        }

        if !allowed {
            return Err(rule);
        }
    }

    Ok(())
}

// checks a text field has no Unicode format characters (category Cf)
fn check_no_format_chars(name: &str, value: &str) -> Result<(), String> {
    for c in value.chars() {
        if is_format_char(c) {
            return Err(format!("{name}: contains a format character"));
        }
    }
    Ok(())
}

// checks a field is exactly len lowercase hex characters (sid, sub)
fn check_lower_hex(name: &str, value: &str, len: usize) -> Result<(), String> {
    let reason = format!("{name}: must be exactly {len} lowercase hex characters");

    if value.len() != len {
        return Err(reason);
    }

    for b in value.as_bytes() {
        let allowed;
        if b.is_ascii_digit() || (b'a'..=b'f').contains(b) {
            allowed = true;
        } else {
            allowed = false;
        }

        if !allowed {
            return Err(reason);
        }
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
            battery_pct: None,
            charging: None,
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

    // ---- device_id rule (T-032): ^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$ ----

    #[test]
    fn device_id_starting_with_dash_is_rejected() {
        let mut report = valid_report();
        report.device_id = "-rm".to_string();
        assert_rejected(&report, "device_id");
    }

    #[test]
    fn device_id_starting_with_dot_is_rejected() {
        let mut report = valid_report();
        report.device_id = ".hidden".to_string();
        assert_rejected(&report, "device_id");
    }

    #[test]
    fn device_id_with_space_is_rejected() {
        let mut report = valid_report();
        report.device_id = "my pc".to_string();
        assert_rejected(&report, "device_id");
    }

    #[test]
    fn device_id_with_non_ascii_is_rejected() {
        let mut report = valid_report();
        report.device_id = "küche".to_string();
        assert_rejected(&report, "device_id");
    }

    #[test]
    fn device_id_with_slash_or_quote_is_rejected() {
        let mut report = valid_report();
        report.device_id = "a/b".to_string();
        assert_rejected(&report, "device_id");
        report.device_id = "a\"b".to_string();
        assert_rejected(&report, "device_id");
    }

    #[test]
    fn device_id_with_dot_dash_underscore_passes() {
        let mut report = valid_report();
        report.device_id = "My_PC-2.lan".to_string();
        assert_accepted(&report);
    }

    #[test]
    fn single_character_device_id_passes() {
        let mut report = valid_report();
        report.device_id = "7".to_string();
        assert_accepted(&report);
    }

    #[test]
    fn auto_style_device_id_passes() {
        let mut report = valid_report();
        report.device_id = "dp-3f9a0c41".to_string();
        assert_accepted(&report);
    }

    #[test]
    fn device_id_reason_does_not_echo_the_value() {
        let mut report = valid_report();
        report.device_id = "evil id".to_string();
        let checked = validate(&report);
        match checked {
            Ok(()) => panic!("expected device_id to be rejected"),
            Err(reason) => assert!(!reason.contains("evil")),
        }
    }

    #[test]
    fn check_device_id_accepts_and_rejects_directly() {
        assert!(check_device_id("pi-zero").is_ok());
        assert!(check_device_id("").is_err());
        assert!(check_device_id(&"a".repeat(65)).is_err());
    }

    // ---- battery (T-064) ----

    #[test]
    fn report_without_battery_passes_and_sends_no_battery_keys() {
        let report = valid_report();
        assert_accepted(&report);
        let json = serde_json::to_value(&report).unwrap();
        assert!(json.get("battery_pct").is_none());
        assert!(json.get("charging").is_none());
    }

    #[test]
    fn battery_boundaries_pass() {
        let mut report = valid_report();
        report.battery_pct = Some(0.0);
        report.charging = Some(false);
        assert_accepted(&report);
        report.battery_pct = Some(100.0);
        report.charging = Some(true);
        assert_accepted(&report);
        // charging alone (a board that gave no percent) is harmless
        report.battery_pct = None;
        assert_accepted(&report);
    }

    #[test]
    fn battery_out_of_range_or_not_finite_is_rejected() {
        for pct in [-0.1, 100.1, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut report = valid_report();
            report.battery_pct = Some(pct);
            assert_rejected(&report, "battery_pct");
        }
    }

    #[test]
    fn old_reports_without_battery_still_parse() {
        let json = r#"{"device_id":"my-pc","label":"a","hostname":"","ip":"","cpu_pct":0,"mem_used_mb":0,"mem_total_mb":0,"disk_used_gb":0,"disk_total_gb":0,"temp_c":0,"uptime_s":0}"#;
        let report: Report = serde_json::from_str(json).unwrap();
        assert_eq!(report.battery_pct, None);
        assert_eq!(report.charging, None);
    }

    #[test]
    fn battery_fields_parse_and_wrong_types_are_refused() {
        let good = r#"{"device_id":"pi-zero","label":"Pi","hostname":"","ip":"","cpu_pct":0,"mem_used_mb":0,"mem_total_mb":0,"disk_used_gb":0,"disk_total_gb":0,"temp_c":0,"uptime_s":0,"battery_pct":87.5,"charging":true}"#;
        let report: Report = serde_json::from_str(good).unwrap();
        assert_eq!(report.battery_pct, Some(87.5));
        assert_eq!(report.charging, Some(true));

        let wrong = r#"{"device_id":"pi-zero","label":"Pi","hostname":"","ip":"","cpu_pct":0,"mem_used_mb":0,"mem_total_mb":0,"disk_used_gb":0,"disk_total_gb":0,"temp_c":0,"uptime_s":0,"battery_pct":"full","charging":"yes"}"#;
        let parsed: Result<Report, serde_json::Error> = serde_json::from_str(wrong);
        assert!(parsed.is_err());
    }

    #[test]
    fn device_status_carries_battery_and_local() {
        let mut report = valid_report();
        report.battery_pct = Some(45.0);
        report.charging = Some(false);
        let status = DeviceStatus {
            report: report,
            online: true,
            local: true,
        };
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["battery_pct"], serde_json::json!(45.0));
        assert_eq!(json["charging"], serde_json::json!(false));
        assert_eq!(json["local"], serde_json::json!(true));
    }

    // ---- report packet and reply (T-033) ----

    // the smallest valid report, as the contract measures it, with the given id
    fn smallest_report(device_id: &str) -> Report {
        Report {
            device_id: device_id.to_string(),
            label: "a".to_string(),
            hostname: String::new(),
            ip: String::new(),
            cpu_pct: 0.0,
            mem_used_mb: 0.0,
            mem_total_mb: 0.0,
            disk_used_gb: 0.0,
            disk_total_gb: 0.0,
            temp_c: 0.0,
            uptime_s: 0.0,
            battery_pct: None,
            charging: None,
        }
    }

    // the longest IP text Rust prints (IPv6, every group 4 digits)
    fn longest_ip() -> std::net::IpAddr {
        "1111:2222:3333:4444:5555:6666:7777:8888".parse().unwrap()
    }

    #[test]
    fn id_in_use_reply_has_version_error_id_and_owner() {
        let owner: std::net::IpAddr = "192.168.1.60".parse().unwrap();
        let reply = Reply::id_in_use("dev-server", owner);

        let bytes = build_reply(&reply, 1000).unwrap();

        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            r#"{"v":1,"error":"id_in_use","device_id":"dev-server","owner_ip":"192.168.1.60"}"#
        );
    }

    #[test]
    fn ack_reply_has_version_ok_and_id() {
        let reply = Reply::ack("dp-3f9a0c41");

        let bytes = build_reply(&reply, 1000).unwrap();

        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            r#"{"v":1,"ok":true,"device_id":"dp-3f9a0c41"}"#
        );
    }

    #[test]
    fn reply_is_never_larger_than_the_request() {
        // worst case: a 64-byte id, the longest IP, against the smallest valid report
        let device_id = "a".repeat(64);
        let packet = ReportPacket {
            report: smallest_report(&device_id),
            ack: false,
        };
        let request = serde_json::to_vec(&packet).unwrap();
        let reply = Reply::id_in_use(&device_id, longest_ip());

        let bytes = build_reply(&reply, request.len()).unwrap();

        assert!(bytes.len() < request.len());
        assert!(bytes.len() <= MAX_REPLY_BYTES);
    }

    #[test]
    fn reply_not_smaller_than_the_request_is_not_built() {
        let reply = Reply::ack("dp-3f9a0c41");
        let size = build_reply(&reply, 1000).unwrap().len();

        assert!(build_reply(&reply, size).is_none());
        assert!(build_reply(&reply, size + 1).is_some());
        assert!(build_reply(&reply, 0).is_none());
    }

    #[test]
    fn reply_over_max_bytes_is_not_built() {
        // an id far past the contract limit makes the reply too big to send
        let reply = Reply::ack(&"a".repeat(200));
        assert!(build_reply(&reply, 10_000).is_none());
    }

    #[test]
    fn packet_without_ack_parses_with_ack_false() {
        let report = smallest_report("my-pc");
        let json = serde_json::to_string(&report).unwrap();

        let packet: ReportPacket = serde_json::from_str(&json).unwrap();

        assert!(!packet.ack);
        assert_eq!(packet.report.device_id, "my-pc");
    }

    #[test]
    fn packet_with_ack_true_parses() {
        let json = r#"{"device_id":"my-pc","label":"a","hostname":"","ip":"","cpu_pct":0,"mem_used_mb":0,"mem_total_mb":0,"disk_used_gb":0,"disk_total_gb":0,"temp_c":0,"uptime_s":0,"ack":true}"#;

        let packet: ReportPacket = serde_json::from_str(json).unwrap();

        assert!(packet.ack);
        assert_eq!(packet.report.label, "a");
    }

    #[test]
    fn packet_missing_a_report_field_is_refused() {
        let json = r#"{"device_id":"my-pc","ack":true}"#;
        let parsed = serde_json::from_str::<ReportPacket>(json);
        assert!(parsed.is_err());
    }

    #[test]
    fn report_without_ack_still_parses_as_report() {
        // a packet with ack still parses as a plain Report, so old daemons keep working
        let packet = ReportPacket {
            report: smallest_report("my-pc"),
            ack: true,
        };
        let json = serde_json::to_string(&packet).unwrap();

        let report: Report = serde_json::from_str(&json).unwrap();

        assert_eq!(report.device_id, "my-pc");
    }

    #[test]
    fn reply_parses_back_and_ignores_unknown_fields() {
        let json = r#"{"v":2,"error":"something_new","device_id":"x","extra":5}"#;

        let reply: Reply = serde_json::from_str(json).unwrap();

        assert_eq!(reply.v, 2);
        assert_eq!(reply.error, Some("something_new".to_string()));
        assert_eq!(reply.ok, None);
        assert_eq!(reply.owner_ip, None);
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

    // ---- agent packets (T-039) ----

    // an agent packet that respects every limit
    fn valid_agent() -> AgentPacket {
        AgentPacket {
            kind: "agent".to_string(),
            v: 1,
            sid: "3f9a0c41b2d7".to_string(),
            device_id: "dp-3f9a0c41".to_string(),
            project: "deskpane".to_string(),
            state: AgentState::NeedsYou,
            doing: "allow Bash?".to_string(),
            t: 1794152400123,
            ctx: None,
            sub: None,
        }
    }

    // fails the test unless validate_agent rejects the packet with a reason naming `field`
    fn assert_agent_rejected(packet: &AgentPacket, field: &str) {
        let checked = validate_agent(packet);
        match checked {
            Ok(()) => panic!("expected {field} to be rejected, but the packet passed"),
            Err(reason) => {
                let prefix = format!("{field}:");
                assert!(
                    reason.starts_with(&prefix),
                    "reason '{reason}' should start with '{prefix}'"
                );
            }
        }
    }

    // fails the test unless validate_agent accepts the packet
    fn assert_agent_accepted(packet: &AgentPacket) {
        let checked = validate_agent(packet);
        match checked {
            Ok(()) => {}
            Err(reason) => panic!("expected the agent packet to pass, got: {reason}"),
        }
    }

    // the agent packet from the plan, as JSON text
    const AGENT_JSON: &str = r#"{"kind":"agent","v":1,"sid":"3f9a0c41b2d7","device_id":"dp-3f9a0c41","project":"deskpane","state":"needs_you","doing":"allow Bash?","t":1794152400123}"#;

    #[test]
    fn packet_without_kind_is_a_report() {
        let report = smallest_report("my-pc");
        let json = serde_json::to_vec(&report).unwrap();

        let parsed = parse_packet(&json);

        match parsed {
            Ok(ParsedPacket::Report(packet)) => assert_eq!(packet.report.device_id, "my-pc"),
            other => panic!("expected a report, got {other:?}"),
        }
    }

    #[test]
    fn packet_with_null_kind_is_a_report() {
        let json = r#"{"kind":null,"device_id":"my-pc","label":"a","hostname":"","ip":"","cpu_pct":0,"mem_used_mb":0,"mem_total_mb":0,"disk_used_gb":0,"disk_total_gb":0,"temp_c":0,"uptime_s":0}"#;

        let parsed = parse_packet(json.as_bytes());

        match parsed {
            Ok(ParsedPacket::Report(_)) => {}
            other => panic!("expected a report, got {other:?}"),
        }
    }

    #[test]
    fn packet_with_kind_agent_parses_as_agent() {
        let parsed = parse_packet(AGENT_JSON.as_bytes());

        match parsed {
            Ok(ParsedPacket::Agent(packet)) => assert_eq!(packet, valid_agent()),
            other => panic!("expected an agent packet, got {other:?}"),
        }
    }

    #[test]
    fn unknown_kind_is_rejected_without_echoing_it() {
        let json = r#"{"kind":"evil\u001b[2J","device_id":"x"}"#;

        let parsed = parse_packet(json.as_bytes());

        match parsed {
            Err(PacketError::UnknownKind) => {}
            other => panic!("expected UnknownKind, got {other:?}"),
        }
    }

    #[test]
    fn kind_that_is_not_text_is_malformed() {
        // a number kind must never fall through to the report parser
        let report = smallest_report("my-pc");
        let mut value = serde_json::to_value(&report).unwrap();
        value["kind"] = serde_json::json!(57);
        let json = serde_json::to_vec(&value).unwrap();

        let parsed = parse_packet(&json);

        match parsed {
            Err(PacketError::Malformed(reason)) => assert!(!reason.contains("57")),
            other => panic!("expected Malformed, got {other:?}"),
        }
    }

    #[test]
    fn junk_is_malformed() {
        let parsed = parse_packet(b"not json");
        match parsed {
            Err(PacketError::Malformed(_)) => {}
            other => panic!("expected Malformed, got {other:?}"),
        }
    }

    #[test]
    fn report_missing_a_field_is_a_bad_report() {
        let parsed = parse_packet(br#"{"device_id":"my-pc"}"#);
        match parsed {
            Err(PacketError::BadReport(reason)) => assert!(reason.contains("label")),
            other => panic!("expected BadReport, got {other:?}"),
        }
    }

    #[test]
    fn bad_agent_shape_reason_does_not_echo_values() {
        let json = r#"{"kind":"agent","v":1,"sid":"3f9a0c41b2d7","device_id":"dp-1","project":"p","state":"evil-state","doing":"","t":1}"#;

        let parsed = parse_packet(json.as_bytes());

        match parsed {
            Err(PacketError::BadAgent(reason)) => {
                assert!(
                    !reason.contains("evil"),
                    "reason '{reason}' echoes the value"
                )
            }
            other => panic!("expected BadAgent, got {other:?}"),
        }
    }

    #[test]
    fn valid_agent_packet_passes() {
        assert_agent_accepted(&valid_agent());
    }

    #[test]
    fn agent_packet_wrong_kind_is_rejected() {
        let mut packet = valid_agent();
        packet.kind = "report".to_string();
        assert_agent_rejected(&packet, "kind");
    }

    #[test]
    fn agent_packet_wrong_version_is_rejected() {
        let mut packet = valid_agent();
        packet.v = 2;
        assert_agent_rejected(&packet, "v");
        packet.v = 0;
        assert_agent_rejected(&packet, "v");
    }

    #[test]
    fn sid_must_be_12_lowercase_hex() {
        let mut packet = valid_agent();
        packet.sid = "3f9a0c41b2d".to_string();
        assert_agent_rejected(&packet, "sid");
        packet.sid = "3f9a0c41b2d70".to_string();
        assert_agent_rejected(&packet, "sid");
        packet.sid = "3F9A0C41B2D7".to_string();
        assert_agent_rejected(&packet, "sid");
        packet.sid = "3f9a0c41b2dz".to_string();
        assert_agent_rejected(&packet, "sid");
        packet.sid = String::new();
        assert_agent_rejected(&packet, "sid");
        // 12 bytes, but not 12 hex characters
        packet.sid = "3f9a0c41b\u{e9}d".to_string();
        assert_agent_rejected(&packet, "sid");
        packet.sid = "0123456789ab".to_string();
        assert_agent_accepted(&packet);
    }

    #[test]
    fn agent_device_id_uses_the_device_id_rule() {
        let mut packet = valid_agent();
        packet.device_id = "-rm".to_string();
        assert_agent_rejected(&packet, "device_id");
        packet.device_id = String::new();
        assert_agent_rejected(&packet, "device_id");
        packet.device_id = "a".repeat(65);
        assert_agent_rejected(&packet, "device_id");
        packet.device_id = "a".repeat(64);
        assert_agent_accepted(&packet);
    }

    #[test]
    fn project_over_32_bytes_is_rejected() {
        let mut packet = valid_agent();
        packet.project = "p".repeat(32);
        assert_agent_accepted(&packet);
        packet.project = "p".repeat(33);
        assert_agent_rejected(&packet, "project");
        // 9 emoji are 9 characters but 36 bytes
        packet.project = "\u{1F600}".repeat(9);
        assert_agent_rejected(&packet, "project");
    }

    #[test]
    fn doing_over_32_bytes_is_rejected() {
        let mut packet = valid_agent();
        packet.doing = "d".repeat(32);
        assert_agent_accepted(&packet);
        packet.doing = "d".repeat(33);
        assert_agent_rejected(&packet, "doing");
    }

    #[test]
    fn empty_project_and_doing_pass() {
        let mut packet = valid_agent();
        packet.project = String::new();
        packet.doing = String::new();
        assert_agent_accepted(&packet);
    }

    #[test]
    fn agent_text_with_control_characters_is_rejected() {
        let mut packet = valid_agent();
        packet.project = "deskpane\u{001B}[2J".to_string();
        assert_agent_rejected(&packet, "project");

        let mut packet = valid_agent();
        packet.doing = "a\nb".to_string();
        assert_agent_rejected(&packet, "doing");

        let mut packet = valid_agent();
        packet.doing = "\u{009B}2J".to_string();
        assert_agent_rejected(&packet, "doing");
    }

    #[test]
    fn agent_reason_does_not_echo_the_value() {
        let mut packet = valid_agent();
        packet.project = "evil\u{0007}".to_string();
        let checked = validate_agent(&packet);
        match checked {
            Ok(()) => panic!("expected project to be rejected"),
            Err(reason) => assert!(!reason.contains("evil")),
        }
    }

    #[test]
    fn unknown_state_is_rejected() {
        let json = AGENT_JSON.replace("needs_you", "sleeping");
        let parsed = parse_packet(json.as_bytes());
        match parsed {
            Err(PacketError::BadAgent(reason)) => assert!(!reason.contains("sleeping")),
            other => panic!("expected BadAgent, got {other:?}"),
        }
    }

    #[test]
    fn every_state_word_parses() {
        let words = [
            ("idle", AgentState::Idle),
            ("working", AgentState::Working),
            ("done", AgentState::Done),
            ("needs_you", AgentState::NeedsYou),
            ("error", AgentState::Error),
            ("ended", AgentState::Ended),
        ];
        for (word, state) in words {
            let json = AGENT_JSON.replace("needs_you", word);
            let packet: AgentPacket = serde_json::from_str(&json).unwrap();
            assert_eq!(packet.state, state);
            assert_eq!(state.as_str(), word);
        }
    }

    #[test]
    fn agent_packet_unknown_fields_are_ignored() {
        let json = AGENT_JSON.replace(r#""t":"#, r#""future":{"a":[1,2]},"t":"#);
        let packet: AgentPacket = serde_json::from_str(&json).unwrap();
        assert_eq!(packet, valid_agent());
    }

    #[test]
    fn ctx_out_of_range_is_rejected() {
        let mut packet = valid_agent();
        packet.ctx = Some(101);
        assert_agent_rejected(&packet, "ctx");
        packet.ctx = Some(100);
        assert_agent_accepted(&packet);
        packet.ctx = Some(0);
        assert_agent_accepted(&packet);
    }

    #[test]
    fn missing_ctx_is_fine() {
        let packet: AgentPacket = serde_json::from_str(AGENT_JSON).unwrap();
        assert_eq!(packet.ctx, None);
        assert_agent_accepted(&packet);
        // a packet without ctx is sent without the field
        let json = serde_json::to_string(&packet).unwrap();
        assert!(!json.contains("ctx"));
    }

    #[test]
    fn negative_time_is_refused_by_the_parser() {
        let json = AGENT_JSON.replace("1794152400123", "-1");
        assert!(serde_json::from_str::<AgentPacket>(&json).is_err());
    }

    #[test]
    fn largest_valid_agent_packet_fits_in_1500_bytes() {
        // 64-byte id, 32-byte project and doing in 4-byte characters, longest state, biggest numbers
        let packet = AgentPacket {
            kind: "agent".to_string(),
            v: AGENT_PACKET_VERSION,
            sid: "f".repeat(SID_LEN),
            device_id: "a".repeat(MAX_NAME_BYTES),
            project: "\u{1F600}".repeat(8),
            state: AgentState::NeedsYou,
            doing: "\u{1F600}".repeat(8),
            t: u64::MAX,
            ctx: Some(MAX_CTX_PCT),
            sub: Some("f".repeat(SUB_LEN)),
        };
        assert_agent_accepted(&packet);

        let bytes = serde_json::to_vec(&packet).unwrap();

        assert!(bytes.len() < 1500);
        assert!(bytes.len() < 300, "{} bytes", bytes.len());
    }

    // ---- review fixes (T-045) ----

    #[test]
    fn sub_must_be_8_lowercase_hex_when_present() {
        let mut packet = valid_agent();
        packet.sub = Some("0a1b2c3d".to_string());
        assert_agent_accepted(&packet);
        for bad in [
            "0a1b2c3",
            "0a1b2c3d4",
            "0A1B2C3D",
            "0a1b2c3z",
            "",
            "0a1b2c\u{e9}",
        ] {
            packet.sub = Some(bad.to_string());
            assert_agent_rejected(&packet, "sub");
        }
    }

    #[test]
    fn packet_without_sub_still_parses_and_is_sent_without_it() {
        // old clients send no sub
        let packet: AgentPacket = serde_json::from_str(AGENT_JSON).unwrap();
        assert_eq!(packet.sub, None);
        assert_agent_accepted(&packet);
        assert!(!serde_json::to_string(&packet).unwrap().contains("sub"));
    }

    #[test]
    fn bad_report_reason_does_not_echo_values() {
        let json = r#"{"device_id":"my-pc","label":"a","hostname":"","ip":"","cpu_pct":"evil-text","mem_used_mb":0,"mem_total_mb":0,"disk_used_gb":0,"disk_total_gb":0,"temp_c":0,"uptime_s":0}"#;

        let parsed = parse_packet(json.as_bytes());

        match parsed {
            Err(PacketError::BadReport(reason)) => {
                assert!(
                    !reason.contains("evil"),
                    "reason '{reason}' echoes the value"
                );
                assert!(reason.contains("column"));
            }
            other => panic!("expected BadReport, got {other:?}"),
        }
    }

    #[test]
    fn bad_report_reason_names_only_the_missing_field() {
        let parsed = parse_packet(br#"{"device_id":"my-pc","label":"x"}"#);
        match parsed {
            Err(PacketError::BadReport(reason)) => {
                assert_eq!(reason, "missing field hostname");
            }
            other => panic!("expected BadReport, got {other:?}"),
        }
    }

    #[test]
    fn format_characters_are_rejected_in_project_and_doing() {
        // right-to-left override, zero-width space, byte order mark, a tag character
        for c in [
            '\u{202E}',
            '\u{200B}',
            '\u{FEFF}',
            '\u{E0041}',
            '\u{2066}',
            '\u{00AD}',
        ] {
            assert!(is_format_char(c), "{c:?} should be a format character");

            let mut packet = valid_agent();
            packet.project = format!("desk{c}pane");
            assert_agent_rejected(&packet, "project");

            let mut packet = valid_agent();
            packet.doing = format!("allow{c} Bash?");
            assert_agent_rejected(&packet, "doing");
        }
    }

    #[test]
    fn ordinary_text_is_not_format_characters() {
        for c in ['a', 'Z', '0', ' ', '\u{e9}', '\u{1F600}', '\u{4E2D}', '?'] {
            assert!(!is_format_char(c), "{c:?}");
        }
        let mut packet = valid_agent();
        packet.project = "K\u{fc}che \u{1F600}".to_string();
        assert_agent_accepted(&packet);
    }

    // ---- golden socket line, shared with the dashboard preview mock (T-061) ----

    // the fixture the Python preview mock is checked against
    const ENVELOPE_FIXTURE: &str = include_str!("../../pi/tests/fixtures/envelope_v1.json");

    // a full envelope: one device, one session, every optional field set
    fn fixture_envelope() -> Envelope {
        let report = Report {
            device_id: "pi-zero".to_string(),
            label: "Pi".to_string(),
            hostname: "pi-zero".to_string(),
            ip: "192.168.1.50".to_string(),
            cpu_pct: 12.5,
            mem_used_mb: 210.0,
            mem_total_mb: 416.0,
            disk_used_gb: 4.5,
            disk_total_gb: 29.0,
            temp_c: 47.5,
            uptime_s: 3600.0,
            battery_pct: Some(87.0),
            charging: Some(true),
        };
        let device = DeviceStatus {
            report: report,
            online: true,
            local: true,
        };
        let session = AgentSession {
            sid: "0123456789ab".to_string(),
            device_id: "pi-zero".to_string(),
            label: "Pi".to_string(),
            project: "deskpane".to_string(),
            state: AgentState::NeedsYou,
            doing: "Bash".to_string(),
            running_s: 600,
            state_s: 30,
            silent_s: 5,
            quiet: false,
            ctx: Some(42),
        };
        let mut weather_values = BTreeMap::new();
        weather_values.insert("rain_pct".to_string(), 40.0);
        weather_values.insert("temp_c".to_string(), 12.4);
        let weather = MetricView {
            values: weather_values,
            age_s: 42,
            stale: false,
        };
        let mut metrics = BTreeMap::new();
        metrics.insert("weather".to_string(), weather);
        let views = Views {
            stats: StatsView {
                devices: vec![device],
            },
            agents: AgentsView {
                sessions: vec![session],
            },
            metrics: metrics,
        };
        Envelope {
            v: ENVELOPE_VERSION,
            views: views,
        }
    }

    // with no plugin reporting, the line is exactly the line before metrics existed
    #[test]
    fn empty_metrics_are_left_out_of_the_line() {
        let views = Views {
            stats: StatsView {
                devices: Vec::new(),
            },
            agents: AgentsView {
                sessions: Vec::new(),
            },
            metrics: BTreeMap::new(),
        };
        let envelope = Envelope {
            v: ENVELOPE_VERSION,
            views: views,
        };
        assert_eq!(
            serde_json::to_string(&envelope).unwrap(),
            r#"{"v":1,"views":{"stats":{"devices":[]},"agents":{"sessions":[]}}}"#
        );
    }

    // check_plugin_name applies the packet's plugin rule and never repeats the name
    #[test]
    fn check_plugin_name_matches_the_packet_rule() {
        assert_eq!(check_plugin_name("ups-2"), Ok(()));
        assert!(check_plugin_name("").is_err());
        for bad in ["Ups", "ups_2", "2ups"] {
            let reason = check_plugin_name(bad).unwrap_err();
            assert!(!reason.contains(bad), "{reason}");
        }
    }

    // an envelope from a daemon older than the metrics view still reads, with no metrics
    #[test]
    fn envelope_without_metrics_reads_as_empty() {
        let line = r#"{"v":1,"views":{"stats":{"devices":[]},"agents":{"sessions":[]}}}"#;
        let envelope: Envelope = serde_json::from_str(line).unwrap();
        assert!(envelope.views.metrics.is_empty());
    }

    // ---- metric packets (plugin numbers, docs/plans/2026-11-11-plugin-phase2.md) ----

    // a metric packet that respects every limit, used as the base for each test
    fn valid_metric() -> MetricPacket {
        let mut values = BTreeMap::new();
        values.insert("temp_c".to_string(), 12.4);
        values.insert("rain_pct".to_string(), 40.0);
        MetricPacket {
            kind: PACKET_KIND_METRIC.to_string(),
            v: METRIC_PACKET_VERSION,
            plugin: "weather".to_string(),
            ttl_s: 1800,
            values: values,
        }
    }

    // fails the test unless validate_metric rejects the packet with a reason naming `field`
    fn assert_metric_rejected(packet: &MetricPacket, field: &str) {
        let checked = validate_metric(packet);
        match checked {
            Ok(()) => panic!("expected {field} to be rejected, but the packet passed"),
            Err(reason) => assert!(
                reason.starts_with(field),
                "reason {reason:?} should start with {field}"
            ),
        }
    }

    // a packet with the given values, everything else valid
    fn metric_with_values(entries: &[(&str, f64)]) -> MetricPacket {
        let mut packet = valid_metric();
        packet.values = BTreeMap::new();
        for (key, value) in entries {
            packet.values.insert(key.to_string(), *value);
        }
        packet
    }

    #[test]
    fn valid_metric_packet_passes() {
        assert_eq!(validate_metric(&valid_metric()), Ok(()));
    }

    #[test]
    fn metric_kind_and_version_must_match() {
        let mut packet = valid_metric();
        packet.kind = "agent".to_string();
        assert_metric_rejected(&packet, "kind");

        let mut packet = valid_metric();
        packet.v = 2;
        assert_metric_rejected(&packet, "v");
    }

    #[test]
    fn plugin_names_follow_the_slug_rule() {
        for good in ["a", "weather", "ups-2", "a1-b2"] {
            let mut packet = valid_metric();
            packet.plugin = good.to_string();
            assert_eq!(validate_metric(&packet), Ok(()), "{good}");
        }
        let too_long = "a".repeat(MAX_PLUGIN_NAME_BYTES + 1);
        for bad in [
            "",
            "Weather",
            "1weather",
            "-weather",
            "wea_ther",
            "wea ther",
            "wéather",
            too_long.as_str(),
        ] {
            let mut packet = valid_metric();
            packet.plugin = bad.to_string();
            assert_metric_rejected(&packet, "plugin");
        }
        let longest = "a".repeat(MAX_PLUGIN_NAME_BYTES);
        let mut packet = valid_metric();
        packet.plugin = longest;
        assert_eq!(validate_metric(&packet), Ok(()));
    }

    #[test]
    fn ttl_must_be_within_its_range() {
        for good in [MIN_METRIC_TTL_S, 1800, MAX_METRIC_TTL_S] {
            let mut packet = valid_metric();
            packet.ttl_s = good;
            assert_eq!(validate_metric(&packet), Ok(()), "{good}");
        }
        for bad in [0, MIN_METRIC_TTL_S - 1, MAX_METRIC_TTL_S + 1] {
            let mut packet = valid_metric();
            packet.ttl_s = bad;
            assert_metric_rejected(&packet, "ttl_s");
        }
    }

    #[test]
    fn values_count_is_one_to_sixteen() {
        assert_metric_rejected(&metric_with_values(&[]), "values");

        let mut keys: Vec<String> = Vec::new();
        for index in 0..MAX_METRIC_VALUES + 1 {
            keys.push(format!("k{index}"));
        }
        let mut entries: Vec<(&str, f64)> = Vec::new();
        for key in &keys {
            entries.push((key.as_str(), 1.0));
        }
        assert_metric_rejected(&metric_with_values(&entries), "values");
        entries.pop();
        assert_eq!(validate_metric(&metric_with_values(&entries)), Ok(()));
    }

    #[test]
    fn value_keys_follow_the_slug_rule() {
        for good in ["t", "temp_c", "rain_pct_2"] {
            assert_eq!(
                validate_metric(&metric_with_values(&[(good, 1.0)])),
                Ok(()),
                "{good}"
            );
        }
        let too_long = "k".repeat(MAX_METRIC_KEY_BYTES + 1);
        for bad in [
            "",
            "Temp",
            "2temp",
            "_temp",
            "temp-c",
            "temp.c",
            too_long.as_str(),
        ] {
            assert_metric_rejected(&metric_with_values(&[(bad, 1.0)]), "values");
        }
    }

    #[test]
    fn values_must_be_finite_and_bounded() {
        for good in [0.0, -MAX_METRIC_ABS, MAX_METRIC_ABS, 12.4, -40.0] {
            assert_eq!(
                validate_metric(&metric_with_values(&[("x", good)])),
                Ok(()),
                "{good}"
            );
        }
        for bad in [
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            MAX_METRIC_ABS * 1.000001,
            -MAX_METRIC_ABS * 1.000001,
        ] {
            assert_metric_rejected(&metric_with_values(&[("x", bad)]), "values");
        }
    }

    // a bad key or plugin name is never repeated in the reason (it could be anything)
    #[test]
    fn metric_reasons_never_repeat_a_bad_name() {
        let mut packet = valid_metric();
        packet.plugin = "SECRET-PLUGIN".to_string();
        let reason = validate_metric(&packet).unwrap_err();
        assert!(!reason.contains("SECRET"), "{reason}");

        let packet = metric_with_values(&[("SECRET_KEY", 1.0)]);
        let reason = validate_metric(&packet).unwrap_err();
        assert!(!reason.contains("SECRET"), "{reason}");
    }

    #[test]
    fn parse_packet_reads_a_metric_packet() {
        let bytes = br#"{"kind":"metric","v":1,"plugin":"weather","ttl_s":1800,"values":{"temp_c":12.4,"rain_pct":40}}"#;
        let parsed = parse_packet(bytes);
        match parsed {
            Ok(ParsedPacket::Metric(packet)) => {
                assert_eq!(packet, valid_metric());
            }
            other => panic!("expected a metric packet, got {other:?}"),
        }
    }

    #[test]
    fn parse_packet_refuses_a_bad_metric_shape_without_repeating_values() {
        let shapes: [&[u8]; 4] = [
            br#"{"kind":"metric","v":1,"plugin":"weather","ttl_s":1800,"values":{"temp_c":"SECRET"}}"#,
            br#"{"kind":"metric","v":1,"plugin":"weather","ttl_s":-5,"values":{"t":1}}"#,
            br#"{"kind":"metric","v":1,"plugin":"weather","values":{"t":1}}"#,
            br#"{"kind":"metric","v":1,"plugin":"weather","ttl_s":60,"values":{"t":1e400}}"#,
        ];
        for bytes in shapes {
            let parsed = parse_packet(bytes);
            match parsed {
                Err(PacketError::BadMetric(reason)) => {
                    assert!(!reason.contains("SECRET"), "{reason}");
                }
                other => panic!("expected BadMetric, got {other:?}"),
            }
        }
    }

    #[test]
    fn unknown_kinds_are_still_refused() {
        let parsed = parse_packet(br#"{"kind":"metrics","v":1}"#);
        assert_eq!(parsed.unwrap_err(), PacketError::UnknownKind);
    }

    #[test]
    fn metric_packet_round_trips_through_json() {
        let packet = valid_metric();
        let text = serde_json::to_string(&packet).unwrap();
        let back: MetricPacket = serde_json::from_str(&text).unwrap();
        assert_eq!(back, packet);
    }

    #[test]
    fn envelope_fixture_matches_file() {
        // the daemon's real serialization, read back as a JSON value
        let serialized = serde_json::to_value(fixture_envelope()).unwrap();
        // the file the preview mock is tested against
        let expected: serde_json::Value = serde_json::from_str(ENVELOPE_FIXTURE).unwrap();
        assert_eq!(serialized, expected);
    }
}
