"""Pure helpers for optional character packs (pack format v1): where each
frame sits on a sheet, which look a project gets, which pose and emote a
state shows, animation frames, and the safety plan for installing a pack
zip. No pygame and no Pillow import, so tests run anywhere.

Pack format v1 (docs/CHARACTERS.md):
  NPC_<n>.png, n from 1 to 15 at the zip root, each 128 x 208 px: 16 px
  frames, 8 columns by 13 rows. Row 0 is a header. Columns 0-3 idle, 4-7
  walk. Rows 1-4, 5-8, 9-12 are variants 0, 1, 2; inside a variant the
  rows are the directions front, left, back, right.
  Emotes/<Name>_emote_16x16.png, each 64 x 16 px: 4 pop-in frames.
"""
import collections
import hashlib
import re
import stat

# frame and sheet geometry
FRAME = 16
SHEET_COLS = 8
SHEET_ROWS = 13
SHEET_W = FRAME * SHEET_COLS
SHEET_H = FRAME * SHEET_ROWS
HEADER_ROWS = 1
VARIANTS = 3
DIRECTIONS = ("front", "left", "back", "right")
MOTIONS = ("idle", "walk")
FRAMES_PER_MOTION = 4
MAX_CHARACTERS = 15

# emote strips
EMOTE_W = 64
EMOTE_H = 16
EMOTE_FRAMES = 4
EMOTE_NAMES = (
    "Angry", "Evil", "Exclamation", "Happy", "HeartBreak",
    "Heart", "Interrogation", "Sad", "Star", "Surprised",
)

# animation and timing (section 10 of the agent status plan)
CHAR_FPS = 5
HAPPY_FOR_S = 5.0
SURPRISED_FOR_S = 2.0
WALK_OFF_S = 2.0

# drawing scales: rows 3x with a 2x emote, the single card up to 8x
ROW_CHAR_SCALE = 3
ROW_EMOTE_SCALE = 2
CARD_MAX_SCALE = 8

# salt for the look start index; changing it reshuffles every look
LOOK_SALT = "deskpane-look-v1:"

# install limits: the real pack is about 100 KB in 32 entries
MAX_ZIP_BYTES = 4 * 1024 * 1024
MAX_ENTRIES = 64
MAX_ENTRY_BYTES = 64 * 1024
MAX_TOTAL_BYTES = 2 * 1024 * 1024

PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"

# exact allowed names, matched with fullmatch: "$" alone would also accept a
# trailing newline ("NPC_1.png\n"); NPC numbers without leading zeros
NPC_NAME = re.compile(r"NPC_([1-9][0-9]?)\.png")
EMOTE_NAME = re.compile(r"Emotes/([A-Za-z]+)_emote_16x16\.png")
# a drive letter at the start, with or without a slash: "C:/x", "c:x"
DRIVE_PREFIX = re.compile(r"^[A-Za-z]:")

# what the installer needs to know about one zip entry
PackEntry = collections.namedtuple("PackEntry", "name size flag_bits external_attr")


class PackError(Exception):
    """The pack zip is refused. The message never repeats entry names or
    content from the zip, only fixed words and our own allowlisted names."""


def frame_rect(variant, direction, motion, frame):
    """(x, y, w, h) of one character frame on a sheet.
    variant 0..2, direction front/left/back/right, motion idle/walk,
    frame 0..3."""
    if variant not in range(VARIANTS):
        raise ValueError("variant must be 0 to 2")
    if direction not in DIRECTIONS:
        raise ValueError("unknown direction")
    if motion not in MOTIONS:
        raise ValueError("unknown motion")
    if frame not in range(FRAMES_PER_MOTION):
        raise ValueError("frame must be 0 to 3")

    # walk frames sit right of the four idle frames
    column = frame
    if motion == "walk":
        column = FRAMES_PER_MOTION + frame
    row = HEADER_ROWS + len(DIRECTIONS) * variant + DIRECTIONS.index(direction)
    return FRAME * column, FRAME * row, FRAME, FRAME


def emote_rect(frame):
    """(x, y, w, h) of one emote frame on its 64 x 16 strip."""
    if frame not in range(EMOTE_FRAMES):
        raise ValueError("emote frame must be 0 to 3")
    return FRAME * frame, 0, FRAME, FRAME


def look_count(characters):
    """Looks in a pack: one per character per variant."""
    return max(0, characters) * VARIANTS


def look_of(index):
    """(character number from 1, variant) for a look index."""
    return index // VARIANTS + 1, index % VARIANTS


def look_key(project, device_id):
    """The key a look is chosen by: the project name, or for a hidden
    project "?@" plus the machine's id, so two hidden projects differ."""
    if project:
        return project
    return "?@" + (device_id or "")


def look_for_key(key, count):
    """Start look index for a key: first 8 bytes of SHA-256 over the salt
    and key, modulo the look count. Stable across restarts, unlike hash()."""
    if count <= 0:
        return 0
    digest = hashlib.sha256((LOOK_SALT + key).encode("utf-8")).digest()
    return int.from_bytes(digest[:8], "big") % count


def assign_looks(keys_on_screen, previous, count):
    """Look index per key for every key on screen.

    A key that already has a look in `previous` keeps it. A new key takes
    its start index, or the next free one after it (wrapping) when taken.
    With more keys than looks, a key that finds nothing free reuses its
    start index. Keys are handled in sorted order, so the result does not
    depend on the order sessions arrive in."""
    if count <= 0:
        return {}

    unique_keys = sorted(set(keys_on_screen))
    looks = {}
    taken = set()

    # existing looks first, so a look never jumps when another key leaves
    for key in unique_keys:
        old = previous.get(key)
        if isinstance(old, int) and 0 <= old < count and old not in taken:
            looks[key] = old
            taken.add(old)

    # then new keys: start index, else the next free one
    for key in unique_keys:
        if key in looks:
            continue
        start = look_for_key(key, count)
        chosen = start
        for step in range(count):
            candidate = (start + step) % count
            if candidate not in taken:
                chosen = candidate
                break
        looks[key] = chosen
        taken.add(chosen)
    return looks


def character_pose(state, quiet, state_s, dim, new_for_s, leaving_for_s):
    """What the character does for a session, as a dict:
      direction  front/left/back/right
      motion     idle or walk
      still      True: hold the first frame (dimmed done)
      dim        True: draw faded
      emote      emote name, or None
      emote_for_s seconds since the emote began (for the pop-in)
      offset     0.0 to 1.0, walk-off progress to the right

    state_s: seconds in the current state. new_for_s: seconds since the
    session first appeared, or None when it is not new. leaving_for_s:
    seconds since it left the socket line, or None."""
    state_s = max(0.0, float(state_s or 0))
    pose = {
        "direction": "front",
        "motion": "idle",
        "still": False,
        "dim": bool(dim),
        "emote": None,
        "emote_for_s": state_s,
        "offset": 0.0,
    }

    # a session that left walks off to the right, nothing else
    if leaving_for_s is not None:
        pose["direction"] = "right"
        pose["motion"] = "walk"
        pose["dim"] = False
        pose["offset"] = min(1.0, max(0.0, leaving_for_s) / WALK_OFF_S)
        return pose

    if state == "needs_you":
        pose["emote"] = "Exclamation"
    elif state == "idle":
        pose["emote"] = "Interrogation"
    elif state == "error":
        pose["emote"] = "Sad"
    elif state == "working":
        pose["direction"] = "right"
        # quiet: stopped walking, no news for a while
        if not quiet:
            pose["motion"] = "walk"
    elif state == "done":
        if dim:
            pose["still"] = True
        elif state_s < HAPPY_FOR_S:
            pose["emote"] = "Happy"

    # a new session says Surprised first, then the state's emote pops in fresh
    if new_for_s is not None:
        new_for_s = max(0.0, new_for_s)
        if new_for_s < SURPRISED_FOR_S:
            pose["emote"] = "Surprised"
            pose["emote_for_s"] = new_for_s
        elif pose["emote"] is not None:
            pose["emote_for_s"] = min(state_s, new_for_s - SURPRISED_FOR_S)
    return pose


def anim_frame(now_ms, fps, frame_count):
    """Looping frame number at `fps` for a clock in milliseconds."""
    if fps <= 0 or frame_count <= 0 or now_ms <= 0:
        return 0
    return int(now_ms * fps // 1000) % frame_count


def emote_frame(shown_for_s, fps):
    """Emote pop-in: frames 0 to 3 play once, then frame 3 holds."""
    if shown_for_s <= 0 or fps <= 0:
        return 0
    return min(EMOTE_FRAMES - 1, int(shown_for_s * fps))


def best_scale(avail_w, avail_h, size, max_scale=CARD_MAX_SCALE):
    """Largest integer scale at which a `size` px square fits, at most
    max_scale. 0 when it does not fit even at 1x."""
    if size <= 0:
        return 0
    scale = min(avail_w // size, avail_h // size, max_scale)
    return max(0, int(scale))


def track_sessions(tracker, sessions, now):
    """Remembers what the previous socket line held, so the screen knows
    which sessions are new (Surprised) and which just left (walk-off).

    tracker: the previous result, or None before the first line.
    sessions: the sessions of this line, in display order.
    now: local time in seconds.

    Returns {"seen": {sid: {"new_at", "state", "state_at"}},
             "ghosts": {sid: {"session", "left_at", "slot"}},
             "order": [sid, ...]}.
    On the first line nothing is new, so a restart is quiet."""
    first_line = tracker is None
    old_seen = {} if first_line else tracker["seen"]
    old_ghosts = {} if first_line else tracker["ghosts"]
    old_order = [] if first_line else tracker["order"]
    old_rows = {} if first_line else tracker.get("rows", {})

    seen = {}
    rows = {}
    order = []
    for session in sessions:
        sid = session.get("sid")
        if not isinstance(sid, str):
            continue
        order.append(sid)
        rows[sid] = session
        state = session.get("state")
        before = old_seen.get(sid)
        if before is None:
            # the daemon's state_s dates a state we never saw change
            state_s = session.get("state_s") or 0
            if not isinstance(state_s, (int, float)):
                state_s = 0
            seen[sid] = {
                "new_at": None if first_line else now,
                "state": state,
                "state_at": now - max(0, state_s) if first_line else now,
            }
        elif before["state"] != state:
            seen[sid] = {"new_at": before["new_at"], "state": state, "state_at": now}
        else:
            seen[sid] = before

    # sessions that left become ghosts in their old slot for WALK_OFF_S
    ghosts = {}
    for sid, ghost in old_ghosts.items():
        if sid not in seen and now - ghost["left_at"] < WALK_OFF_S:
            ghosts[sid] = ghost
    for slot, sid in enumerate(old_order):
        if sid not in seen and sid not in ghosts and sid in old_rows:
            ghosts[sid] = {"session": old_rows[sid], "left_at": now, "slot": slot}

    return {"seen": seen, "ghosts": ghosts, "order": order, "rows": rows}


def with_ghosts(live, ghosts):
    """The live sessions with each ghost put back at its old slot, marked
    with "ghost": True. Ghosts never count in summaries or focus."""
    merged = list(live)
    for sid, ghost in sorted(ghosts.items(), key=lambda item: item[1]["slot"]):
        row = dict(ghost["session"])
        row["ghost"] = True
        row["left_at"] = ghost["left_at"]
        merged.insert(min(ghost["slot"], len(merged)), row)
    return merged


def png_size(data):
    """(width, height) from a PNG's first 24 bytes, or None when the bytes
    are not a PNG that starts with an IHDR chunk."""
    if len(data) < 24 or data[:8] != PNG_SIGNATURE:
        return None
    # chunk length 13, chunk type IHDR, then width and height big-endian
    if data[8:12] != b"\x00\x00\x00\x0d" or data[12:16] != b"IHDR":
        return None
    width = int.from_bytes(data[16:20], "big")
    height = int.from_bytes(data[20:24], "big")
    return width, height


def allowed_targets():
    """Every file name an install may write, in a fixed order."""
    names = ["NPC_%d.png" % n for n in range(1, MAX_CHARACTERS + 1)]
    names += ["Emotes/%s_emote_16x16.png" % name for name in EMOTE_NAMES]
    return names


def target_for(name):
    """The allowlisted target name for a zip entry name, or None to skip it.
    The target is built from our own constants, never copied from the zip."""
    match = NPC_NAME.fullmatch(name)
    if match:
        number = int(match.group(1))
        if 1 <= number <= MAX_CHARACTERS:
            return "NPC_%d.png" % number
        return None
    match = EMOTE_NAME.fullmatch(name)
    if match and match.group(1) in EMOTE_NAMES:
        return "Emotes/%s_emote_16x16.png" % EMOTE_NAMES[EMOTE_NAMES.index(match.group(1))]
    return None


def expected_size(target):
    """(width, height) a target file must have."""
    if target.startswith("NPC_"):
        return SHEET_W, SHEET_H
    return EMOTE_W, EMOTE_H


def entry_from_zipinfo(info):
    """PackEntry from a zipfile.ZipInfo. orig_filename keeps a NUL that
    zipfile would silently cut from filename."""
    return PackEntry(info.orig_filename, info.file_size, info.flag_bits, info.external_attr)


def is_unsafe_name(name):
    """True for a name that could write outside the target folder, on any
    OS: empty, NUL, backslash, absolute, drive letter, or a ".." part."""
    if not name or "\x00" in name or "\\" in name:
        return True
    if name.startswith("/") or DRIVE_PREFIX.match(name):
        return True
    return ".." in name.split("/")


def plan_pack_install(entries):
    """Checks a zip's entry list and returns what to copy:
      {"files": [{"entry", "target", "size"}], "skipped": n,
       "characters": n, "emotes": n}

    The whole zip is refused (PackError) if any entry, even one that would
    be skipped, is unsafe: a bad path, a symlink, encryption, a duplicate
    name, or more than MAX_ENTRIES entries. Allowed entries must also fit
    the size caps. Harmless extras (text files, folders) are skipped."""
    if len(entries) > MAX_ENTRIES:
        raise PackError("too many entries in the zip (more than %d)" % MAX_ENTRIES)

    names_seen = set()
    files = []
    targets_seen = set()
    skipped = 0
    total = 0
    for entry in entries:
        name = entry.name
        if is_unsafe_name(name):
            raise PackError("unsafe path in the zip")
        if name in names_seen:
            raise PackError("an entry name appears twice in the zip")
        names_seen.add(name)

        # Unix mode lives in the top 16 bits of external_attr
        mode = (entry.external_attr >> 16) & 0xFFFF
        if stat.S_ISLNK(mode):
            raise PackError("symlink entry in the zip")
        if entry.flag_bits & 0x1:
            raise PackError("encrypted entry in the zip")

        target = target_for(name)
        if target is None:
            skipped += 1
            continue
        if target in targets_seen:
            raise PackError("%s appears twice in the zip" % target)
        targets_seen.add(target)
        if entry.size > MAX_ENTRY_BYTES:
            raise PackError("%s is too big (over %d bytes)" % (target, MAX_ENTRY_BYTES))
        total += entry.size
        files.append({"entry": name, "target": target, "size": expected_size(target)})

    if total > MAX_TOTAL_BYTES:
        raise PackError("the pack is too big (over %d bytes)" % MAX_TOTAL_BYTES)

    # characters must start at NPC_1 and have no gaps
    numbers = sorted(int(f["target"][4:-4]) for f in files if f["target"].startswith("NPC_"))
    if not numbers or numbers[0] != 1:
        raise PackError("the pack has no NPC_1.png")
    if numbers != list(range(1, len(numbers) + 1)):
        raise PackError("character numbers are not contiguous from NPC_1.png")

    files.sort(key=lambda f: allowed_targets().index(f["target"]))
    return {
        "files": files,
        "skipped": skipped,
        "characters": len(numbers),
        "emotes": len(files) - len(numbers),
    }


def check_pack_file(target, data):
    """Checks the bytes read for one planned file: within the size cap, a
    PNG, and exactly the size the format needs. Raises PackError."""
    if len(data) > MAX_ENTRY_BYTES:
        raise PackError("%s is too big (over %d bytes)" % (target, MAX_ENTRY_BYTES))
    size = png_size(data[:24])
    if size is None:
        raise PackError("%s is not a PNG" % target)
    want = expected_size(target)
    if size != want:
        raise PackError("%s must be %d x %d px" % (target, want[0], want[1]))


def pose_times(session, seen, now):
    """(state_s, new_for_s, leaving_for_s) for character_pose.

    seen: the session's track_sessions entry, or None. With an entry, the
    time in state comes from the local clock (smooth between socket lines)
    and a new session reports how long ago it appeared; without one, the
    daemon's whole-second state_s is used. A ghost reports how long ago it
    left."""
    state_s = session.get("state_s") or 0
    new_for_s = None
    if seen is not None:
        state_s = now - seen["state_at"]
        if seen["new_at"] is not None:
            new_for_s = now - seen["new_at"]
    leaving_for_s = None
    if session.get("ghost"):
        leaving_for_s = now - session.get("left_at", now)
    return state_s, new_for_s, leaving_for_s
