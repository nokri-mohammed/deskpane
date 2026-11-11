"""The wire contract on the Python side: the socket envelope the daemon
pushes to the dashboard, and the parser that reads one line of it. This is
the Python mirror of the Rust `deskpane_contract` crate, so both planes name
one shared contract. No pygame and no other deskpane import, so plugins and
tests can use it on its own.

The daemon validates every field before it reaches this socket, so the
dashboard side only needs the envelope shape and a tolerant parser; the
per-field limits and checks live in the Rust crate (daemon/reporter side).
The one exception is the metrics view: plugin and key names become panel
field names, so parse_metrics enforces the same name rules and caps again.
"""

import re

# the socket envelope version this dashboard reads; the daemon sends
# {"v": ENVELOPE_VERSION, "views": {"stats": {...}, "agents": {...}, "metrics": {...}}}
# (metrics only while a plugin reports)
ENVELOPE_VERSION = 1

# the metric packet limits, the same as deskpane_contract (Rust)
MAX_METRIC_PLUGINS = 16
MAX_METRIC_VALUES = 16
MAX_METRIC_ABS = 1e9
# a plugin name: a-z first, then a-z, 0-9 and -, 32 at most
PLUGIN_NAME = re.compile(r"[a-z][a-z0-9-]{0,31}")
# a value key: a-z first, then a-z, 0-9 and _, 32 at most
METRIC_KEY = re.compile(r"[a-z][a-z0-9_]{0,31}")


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


def _real_number(value):
    """value as a float inside the contract bound (finite, at most 1e9 either
    way), or None for anything else. bool is not a number here, and an int
    too large for a float is None instead of an OverflowError."""
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return None
    try:
        number = float(value)
    except (OverflowError, ValueError):
        return None
    # NaN is the only value not equal to itself; infinity fails the bound
    if number != number or abs(number) > MAX_METRIC_ABS:
        return None
    return number


def parse_metrics(obj):
    """The plugin metrics view of one decoded socket line, as
    {plugin: {"values": {key: float}, "age_s": int or None, "stale": bool}}.
    The daemon leaves the view out while no plugin reports, and older daemons
    never send it, so both read as {}. Anything odd is dropped, never raised:
    a value that is not a finite number is left out, and an entry without a
    trustworthy age reads as stale, so its panel shows the missing text.
    Plugin and key names that break the contract rules are dropped, and at
    most MAX_METRIC_PLUGINS plugins with MAX_METRIC_VALUES values each are kept
    (sorted by name, so the cut is the same every line)."""
    if not isinstance(obj, dict) or obj.get("v") != ENVELOPE_VERSION:
        return {}
    views = obj.get("views")
    if not isinstance(views, dict):
        return {}
    metrics = views.get("metrics")
    if not isinstance(metrics, dict):
        return {}

    result = {}
    for plugin in sorted(name for name in metrics if isinstance(name, str)):
        if len(result) >= MAX_METRIC_PLUGINS:
            break
        entry = metrics[plugin]
        if not PLUGIN_NAME.fullmatch(plugin) or not isinstance(entry, dict):
            continue
        values = {}
        raw_values = entry.get("values")
        if isinstance(raw_values, dict):
            for key in sorted(name for name in raw_values if isinstance(name, str)):
                if len(values) >= MAX_METRIC_VALUES:
                    break
                if not METRIC_KEY.fullmatch(key):
                    continue
                number = _real_number(raw_values[key])
                if number is not None:
                    values[key] = number
        age_s = entry.get("age_s")
        if isinstance(age_s, bool) or not isinstance(age_s, int) or age_s < 0:
            age_s = None
        stale = entry.get("stale")
        if age_s is None or not isinstance(stale, bool):
            stale = True
        result[plugin] = {"values": values, "age_s": age_s, "stale": stale}
    return result
