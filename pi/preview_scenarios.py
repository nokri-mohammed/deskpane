"""Scenario data for the local preview (T-061): the socket line the daemon
would send at second t of a scenario, so the real dashboard can be watched
on a PC. Pure stdlib, no pygame, never shipped to the Pi.

envelope_at(name, t) returns the envelope {"v": 1, "views": {"stats":
{"devices": [...]}, "agents": {"sessions": [...]}}} for second t, or None
while the mock daemon should be disconnected. Durations tick with t and
states change on a timeline. Every name, host and address here is made up
(addresses stay in the 192.168.1.x placeholder range).

The lines carry the optional fields of the newer tickets (battery_pct,
charging, local) the way the daemon sends them. model and tokens come with
T-063, once the contract and the golden fixture have them.
"""
import hashlib
import json
import math

# one source for the envelope version: the shared Python contract module
from deskpane_contract import ENVELOPE_VERSION

DEFAULT_SCENARIO = "busy"
ALIASES = {"normal": "busy"}


def resolve_scenario(name):
    """The scenario a name or alias stands for, or None if there is none."""
    name = ALIASES.get(name, name)
    if name in SCENARIOS:
        return name
    return None


def make_sid(scenario, index):
    """12 lowercase hex characters, the same for the same scenario row."""
    return hashlib.sha256(("%s-%d" % (scenario, index)).encode()).hexdigest()[:12]


def pad_to_bytes(text, size, fill="x"):
    """text cut or padded with fill to exactly size UTF-8 bytes (ASCII fill)."""
    data = text.encode()[:size]
    # never leave half a UTF-8 character at the cut
    while True:
        try:
            text = data.decode()
            break
        except UnicodeDecodeError:
            data = data[:-1]
    return text + fill * (size - len(data))


def device(spec, t):
    """One DeviceStatus at second t from a spec dict. An offline device keeps
    the numbers of its last report, as the daemon does."""
    off_at = spec.get("offline_at")
    online = spec.get("online", True) and (off_at is None or t < off_at)
    seen_t = t if off_at is None else min(t, off_at)
    # a little movement, so the rings glide
    wobble = spec.get("wobble", 8.0) * math.sin(seen_t / 4.0 + spec.get("phase", 0.0))
    cpu = max(0.0, min(100.0, spec.get("cpu", 20.0) + wobble))
    row = {
        "device_id": spec["id"],
        "label": spec["label"],
        "hostname": spec.get("hostname", spec["id"]),
        "ip": spec["ip"],
        "cpu_pct": round(cpu, 1),
        "mem_used_mb": float(spec.get("mem", 1024.0)),
        "mem_total_mb": float(spec.get("mem_total", 4096.0)),
        "disk_used_gb": float(spec.get("disk", 40.0)),
        "disk_total_gb": float(spec.get("disk_total", 256.0)),
        "temp_c": float(spec.get("temp", 48.0)),
        "uptime_s": float(spec.get("uptime", 7200) + seen_t),
        "online": online,
    }
    # the daemon sends local on every device: true for the Pi itself
    row["local"] = bool(spec.get("local"))
    battery = spec.get("battery")
    if battery is not None:
        step = battery(t)
        if step is not None:
            row["battery_pct"], row["charging"] = step
    return row


def session(scenario, index, spec, t, labels):
    """One AgentSession at second t from a spec dict, or None before it
    starts or after it ends. steps: [(at, state, doing)], the first at 0."""
    start = spec.get("starts_at", 0)
    end = spec.get("ends_at")
    if t < start or (end is not None and t >= end):
        return None
    local_t = t - start
    at, state, doing = spec["steps"][0]
    for step in spec["steps"]:
        if step[0] <= local_t:
            at, state, doing = step
    state_s = local_t - at + (spec.get("age", 0) if at == 0 else 0)
    # packets arrive every few seconds; from silent_from on, none at all
    silent_from = spec.get("silent_from")
    if silent_from is not None and local_t >= silent_from:
        silent_s = spec.get("silent", 0) + local_t - silent_from
    else:
        silent_s = min(state_s, spec.get("silent", 2))
    quiet_at = spec.get("quiet_at")
    quiet = spec.get("quiet", False) or (quiet_at is not None and local_t >= quiet_at)
    row = {
        "sid": make_sid(scenario, index),
        "device_id": spec["device"],
        # the daemon joins the label of the device with the same id
        "label": labels.get(spec["device"], ""),
        "project": spec["project"],
        "state": state,
        "doing": doing,
        "running_s": spec.get("running", 600) + local_t,
        "state_s": state_s,
        "silent_s": silent_s,
        "quiet": quiet,
        "ctx": spec.get("ctx"),
    }
    return row


def build(scenario, t, devices, sessions):
    """The envelope for second t from device and session specs."""
    device_rows = [device(spec, t) for spec in devices]
    labels = {row["device_id"]: row["label"] for row in device_rows}
    session_rows = []
    for index, spec in enumerate(sessions):
        row = session(scenario, index, spec, t, labels)
        if row is not None:
            session_rows.append(row)
    return {"v": ENVELOPE_VERSION, "views": {"stats": {"devices": device_rows},
                                             "agents": {"sessions": session_rows}}}


# ---- the devices most scenarios share ----

PI = {"id": "pi-zero", "label": "Pi", "ip": "192.168.1.50", "cpu": 14.0, "mem": 210.0, "mem_total": 416.0,
      "disk": 4.5, "disk_total": 29.0, "temp": 47.0, "uptime": 86400 * 3 + 3600, "local": True}
DESK = {"id": "desk-pc", "label": "Desk PC", "hostname": "desk-pc", "ip": "192.168.1.60", "cpu": 35.0,
        "mem": 14200.0, "mem_total": 32768.0, "disk": 610.0, "disk_total": 1000.0, "temp": 61.0,
        "uptime": 5 * 3600, "phase": 1.0}
LAPTOP = {"id": "laptop", "label": "Laptop", "hostname": "laptop", "ip": "192.168.1.61", "cpu": 70.0,
          "mem": 7100.0, "mem_total": 8192.0, "disk": 230.0, "disk_total": 256.0, "temp": 78.0,
          "uptime": 1800, "phase": 2.0}
NAS = {"id": "nas-box", "label": "NAS", "hostname": "nas-box", "ip": "192.168.1.62", "cpu": 6.0,
       "mem": 900.0, "mem_total": 2048.0, "disk": 3400.0, "disk_total": 4000.0, "temp": 41.0,
       "uptime": 86400 * 40}


def busy(t):
    """3 devices, 6 sessions in every state on 2 machines; one turns needs
    you at 20 s, which makes the dashboard jump to the agents page."""
    sessions = [
        {"device": "desk-pc", "project": "deskpane", "age": 40, "running": 1800, "ctx": 41,
         "steps": [(0, "working", "Edit dashboard.py"), (12, "working", "Bash"), (24, "working", "Read layout.py")]},
        {"device": "desk-pc", "project": "homelab-ca", "age": 300, "running": 4000, "steps": [(0, "idle", "")]},
        {"device": "laptop", "project": "notes", "age": 120, "running": 900, "steps": [(0, "done", "")]},
        {"device": "laptop", "project": "api-server", "age": 15, "running": 2500, "steps": [(0, "error", "Bash")]},
        {"device": "desk-pc", "project": "website", "age": 5, "running": 300,
         "steps": [(0, "working", "Read README.md"), (20, "needs_you", "Bash")]},
        {"device": "laptop", "project": "scripts", "age": 2400, "running": 5000, "steps": [(0, "done", "")]},
    ]
    return build("busy", t, [PI, DESK, LAPTOP], sessions)


def single(t):
    """One session: the big card, working, then needs you, then done."""
    sessions = [
        {"device": "desk-pc", "project": "deskpane", "age": 12, "running": 1500, "ctx": 55,
         "steps": [(0, "working", "Edit layout.py"), (15, "needs_you", "Bash"), (30, "done", "")]},
    ]
    return build("single", t, [PI, DESK], sessions)


def needs_you(t):
    """One session needs you from the start, a second one at 10 s."""
    sessions = [
        {"device": "desk-pc", "project": "deskpane", "age": 45, "running": 900, "steps": [(0, "needs_you", "Bash")]},
        {"device": "laptop", "project": "api-server", "age": 30, "running": 1200,
         "steps": [(0, "working", "Edit server.rs"), (10, "needs_you", "WebFetch")]},
        {"device": "desk-pc", "project": "notes", "age": 200, "running": 3000, "steps": [(0, "idle", "")]},
    ]
    return build("needs-you", t, [PI, DESK, LAPTOP], sessions)


def worst_devices():
    """32 devices: every limit at once (64-byte label, full disk, hot CPU,
    no memory total, no temperature, a year of uptime, some offline)."""
    devices = []
    for number in range(32):
        spec = {"id": "host-%02d" % number, "label": "Host %02d" % number, "hostname": "host-%02d" % number,
                "ip": "192.168.1.%d" % (100 + number), "cpu": (number * 37) % 101, "phase": number,
                "mem": 1000.0 + number * 100, "mem_total": 8192.0, "disk": 200.0, "disk_total": 256.0,
                "temp": 40.0 + number, "uptime": 3600 * number}
        devices.append(spec)
    devices[0].update({"id": "pi-zero", "label": "Pi", "hostname": "pi-zero", "local": True})
    devices[1].update({"label": pad_to_bytes("A very long device label that keeps on going to the limit ", 64),
                       "hostname": pad_to_bytes("a-very-long-hostname-that-fills-the-whole-field-", 64, "z"),
                       "cpu": 99.9, "wobble": 0.0, "temp": 149.0, "disk": 999.9, "disk_total": 1000.0})
    devices[2].update({"mem": 0.0, "mem_total": 0.0, "temp": 0.0, "uptime": 86400 * 400})
    devices[3].update({"online": False})
    devices[4].update({"offline_at": 10})
    devices[5].update({"label": "Küche \U0001F600", "ip": "192.168.1.255"})
    return devices


def worst(t):
    """32 devices and 16 sessions (4 agents pages): 32-byte projects and
    doing texts, a quiet session, an unknown state, and one
    session that leaves at 20 s (the ghost walk-off with a pack)."""
    long_project = pad_to_bytes("a-very-long-project-name-here", 32)
    long_doing = pad_to_bytes("mcp__server__a_very_long_tool_name", 32)
    states = ["needs_you", "error", "working", "idle", "done"]
    sessions = []
    for number in range(16):
        state = states[number % len(states)]
        doing = long_doing if state in ("working", "needs_you", "error") else ""
        spec = {"device": "host-%02d" % (number % 8), "project": "project-%02d" % number,
                "age": 60 * number, "running": 3600 * number,
                "steps": [(0, state, doing)]}
        sessions.append(spec)
    sessions[0].update({"project": long_project, "device": "host-01"})
    sessions[1].update({"project": pad_to_bytes("Küche \U0001F600 ", 32, "-")})
    sessions[2].update({"quiet": True, "silent": 720, "silent_from": 0, "age": 900})
    sessions[3].update({"steps": [(0, "paused", "Thinking")]})
    sessions[4].update({"ends_at": 20})
    sessions[5].update({"project": "", "ctx": 100, "running": 86400 * 9})
    sessions[6].update({"device": "unknown-device"})
    return build("worst", t, worst_devices(), sessions)


def offline(t):
    """The NAS goes offline at 10 s; its session goes quiet and its silence grows."""
    nas = dict(NAS)
    nas["offline_at"] = 10
    sessions = [
        {"device": "nas-box", "project": "backup-job", "age": 50, "running": 1300, "silent": 1,
         "silent_from": 10, "quiet_at": 10, "steps": [(0, "working", "Bash")]},
        {"device": "desk-pc", "project": "deskpane", "age": 20, "running": 800, "steps": [(0, "working", "Edit README.md")]},
    ]
    return build("offline", t, [PI, DESK, nas], sessions)


# the battery scenario cycles one level per second, so a short headless run
# (20 frames at 3 fps) sees all of them: 87 % charging, 45 %, 15 % (low,
# red), then no battery fields at all (nothing drawn)
BATTERY_STEPS = ((87, True), (45, False), (15, False), None)


def battery_step(t):
    """The (battery_pct, charging) of second t, or None for no fields."""
    return BATTERY_STEPS[t % len(BATTERY_STEPS)]


def battery(t):
    """The Pi itself (local: true) with a PiSugar battery (T-064 drawing)."""
    pi = dict(PI)
    pi["battery"] = battery_step
    sessions = [
        {"device": "desk-pc", "project": "deskpane", "age": 30, "running": 700, "steps": [(0, "working", "Edit battery.rs")]},
    ]
    return build("battery", t, [pi, DESK], sessions)


EMPTY_DISCONNECT_AT = 20


def empty(t):
    """No devices and no sessions, then the daemon goes away at 20 s."""
    if t >= EMPTY_DISCONNECT_AT:
        return None
    return build("empty", t, [], [])


SCENARIOS = {
    "busy": busy,
    "single": single,
    "needs-you": needs_you,
    "worst": worst,
    "offline": offline,
    "battery": battery,
    "empty": empty,
}


def envelope_at(name, t):
    """The envelope at whole second t of a scenario, or None while the
    mock daemon is disconnected. KeyError for an unknown name."""
    return SCENARIOS[name](int(t))


def line_at(name, t):
    """The socket line (compact JSON plus a newline), or None."""
    envelope = envelope_at(name, t)
    if envelope is None:
        return None
    return json.dumps(envelope, ensure_ascii=False, separators=(",", ":")) + "\n"
