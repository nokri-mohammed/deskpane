"""Pure helpers for the dashboard screen: the installed version and where
the overview page puts things. No pygame import, so tests run anywhere.
"""
import os
import re

# the same version pattern install.sh accepts for the bundle's VERSION file
VERSION_PATTERN = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$")
MAX_VERSION_CHARS = 32
# shown when there is no usable VERSION file (running from the repo)
DEV_VERSION = "dev"

# vertical space between the clock and the date, and the date and the summary
GAP_CLOCK_DATE = 12
GAP_DATE_SUMMARY = 12
# the version text sits this far from the left and bottom edges
VERSION_MARGIN = 8


def read_version(dashboard_dir):
    """Returns the installed version, e.g. "0.1.0-beta.3", from the VERSION
    file one folder above the dashboard (install.sh puts it in
    /opt/deskpane/releases/<v>/ next to dashboard/). Returns "dev" when the
    file is missing, unreadable, too long or not a version."""
    path = os.path.join(dashboard_dir, os.pardir, "VERSION")
    try:
        with open(path, encoding="utf-8") as f:
            # read a little past the cap, so a huge file is never loaded whole
            text = f.read(MAX_VERSION_CHARS + 16)
    except (OSError, UnicodeDecodeError):
        return DEV_VERSION

    version = text.strip()
    if len(version) > MAX_VERSION_CHARS:
        return DEV_VERSION
    # fullmatch, so a trailing newline or second line is not accepted
    if not VERSION_PATTERN.fullmatch(version):
        return DEV_VERSION
    return version


def version_text(version):
    """The text drawn on screen: "v0.1.0-beta.3", or "dev" as it is."""
    if version == DEV_VERSION:
        return DEV_VERSION
    return "v" + version


def overview_layout(width, height, clock_h, date_h, summary_h):
    """Top y of the clock, date and summary lines on the overview page, as a
    dict. The three lines form one block centred vertically, so a different
    clock font size moves the block instead of breaking it.

    width, height: screen size in pixels (width is unused today, kept so
                   callers pass the full screen)
    clock_h, date_h, summary_h: rendered heights of the three lines
    """
    block_h = clock_h + GAP_CLOCK_DATE + date_h + GAP_DATE_SUMMARY + summary_h
    clock_y = max(0, (height - block_h) // 2)
    date_y = clock_y + clock_h + GAP_CLOCK_DATE
    summary_y = date_y + date_h + GAP_DATE_SUMMARY
    return {
        "clock_y": clock_y,
        "date_y": date_y,
        "summary_y": summary_y,
    }


def version_position(height, version_h):
    """Top-left (x, y) of the version text: bottom left corner, 8 px in. The
    bottom centre has the page dots and the bottom right the network icon."""
    return VERSION_MARGIN, height - VERSION_MARGIN - version_h
