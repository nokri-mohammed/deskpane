"""The wire contract on the Python side: the socket envelope the daemon
pushes to the dashboard, and the parser that reads one line of it. This is
the Python mirror of the Rust `deskpane_contract` crate, so both planes name
one shared contract. No pygame and no other deskpane import, so plugins and
tests can use it on its own.

The daemon validates every field before it reaches this socket, so the
dashboard side only needs the envelope shape and a tolerant parser; the
per-field limits and checks live in the Rust crate (daemon/reporter side).
"""

# the socket envelope version this dashboard reads; the daemon sends
# {"v": ENVELOPE_VERSION, "views": {"stats": {...}, "agents": {...}}}
ENVELOPE_VERSION = 1


def dict_items(value):
    """Only the dict items of a list; anything else is an empty list."""
    if not isinstance(value, list):
        return []
    return [item for item in value if isinstance(item, dict)]


def parse_socket_line(obj):
    """(devices, sessions) from one decoded socket line: the envelope
    {"v": 1, "views": {"stats": {"devices": [...]}, "agents": {"sessions":
    [...]}}}, or a legacy bare array of devices (one release of overlap).
    None for anything else, so the caller keeps the last good line."""
    if isinstance(obj, list):
        return dict_items(obj), []
    if not isinstance(obj, dict) or obj.get("v") != ENVELOPE_VERSION:
        return None
    views = obj.get("views")
    if not isinstance(views, dict):
        return None
    stats = views.get("stats")
    agents = views.get("agents")
    devices = dict_items(stats.get("devices")) if isinstance(stats, dict) else []
    sessions = dict_items(agents.get("sessions")) if isinstance(agents, dict) else []
    return devices, sessions
