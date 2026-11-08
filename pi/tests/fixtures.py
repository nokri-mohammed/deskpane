"""Test fixtures built in code: tiny PNG files and character pack zips.
No real pack art is ever used in tests. A fixture PNG has a correct
signature and IHDR header with the asked size, and a few bytes of
compressed image data, which is all the pure checks look at.
"""
import io
import stat
import struct
import zipfile
import zlib

PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"

# the ten emote names of pack format v1
EMOTE_NAMES = (
    "Angry", "Evil", "Exclamation", "Happy", "HeartBreak",
    "Heart", "Interrogation", "Sad", "Star", "Surprised",
)


def png_chunk(kind, data):
    """One PNG chunk: length, type, data, CRC over type and data."""
    crc = zlib.crc32(kind + data) & 0xFFFFFFFF
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", crc)


def make_png(width, height):
    """A valid RGBA PNG of the given size, fully transparent except one
    opaque pixel per row, so a decoder sees real pixels."""
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    rows = []
    for y in range(height):
        # filter byte 0, then RGBA pixels; pixel x == y % width is opaque
        row = bytearray(1 + width * 4)
        x = y % width
        row[1 + x * 4: 1 + x * 4 + 4] = b"\xc0\x40\x40\xff"
        rows.append(bytes(row))
    idat = zlib.compress(b"".join(rows), 9)
    return PNG_SIGNATURE + png_chunk(b"IHDR", ihdr) + png_chunk(b"IDAT", idat) + png_chunk(b"IEND", b"")


def sheet_png():
    """A character sheet of the v1 size, 128 x 208."""
    return make_png(128, 208)


def emote_png():
    """An emote strip of the v1 size, 64 x 16."""
    return make_png(64, 16)


def zip_bytes(entries):
    """A zip from a list of (name, data) or (ZipInfo, data) pairs."""
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as zf:
        for name, data in entries:
            zf.writestr(name, data)
    return buf.getvalue()


def full_pack_entries(characters=15, emotes=EMOTE_NAMES, extras=True):
    """Entries shaped like the real pack: sheets, emotes, and the harmless
    extras (text files, a palette folder, folder entries)."""
    entries = []
    for n in range(1, characters + 1):
        entries.append(("NPC_%d.png" % n, sheet_png()))
    if emotes:
        entries.append(("Emotes/", b""))
        for name in emotes:
            entries.append(("Emotes/%s_emote_16x16.png" % name, emote_png()))
    if extras:
        entries.append(("Patrons.txt", b"thank you\n"))
        entries.append(("Thanks.txt", b"thanks\n"))
        entries.append(("Palette/", b""))
        entries.append(("Palette/Palette.txt", b"#000000\n"))
        entries.append(("Palette/rosyandblue.png", make_png(8, 2)))
    return entries


def symlink_info(name):
    """A ZipInfo that marks the entry as a Unix symlink."""
    info = zipfile.ZipInfo(name)
    info.create_system = 3
    info.external_attr = (stat.S_IFLNK | 0o777) << 16
    return info


def write_zip(path, entries):
    """Writes a fixture zip to path and returns the path."""
    with open(path, "wb") as f:
        f.write(zip_bytes(entries))
    return path


def corrupt_deflate_zip(path):
    """A zip whose NPC_1.png entry has a broken deflate stream: headers and
    sizes are fine, but the first compressed byte selects an invalid block
    type, so reading it raises zlib.error inside zipfile."""
    raw = bytearray(zip_bytes([("NPC_1.png", sheet_png())]))
    with zipfile.ZipFile(io.BytesIO(bytes(raw))) as zf:
        info = zf.getinfo("NPC_1.png")
    # local header: 30 fixed bytes, then the name and extra field
    name_len, extra_len = struct.unpack("<HH", bytes(raw[info.header_offset + 26: info.header_offset + 30]))
    data_start = info.header_offset + 30 + name_len + extra_len
    raw[data_start] = 0xFF
    with open(path, "wb") as f:
        f.write(bytes(raw))
    return path
