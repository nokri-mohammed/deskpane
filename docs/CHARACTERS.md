# Character packs

The agents page of the screen can draw a small pixel character for each project, with a speech
bubble that shows the state. A character pack is optional. Without one, the page shows
built-in icons and works the same.

## Licence note

deskpane does not ship a pack and does not need a specific one.

- A paid pack is licensed to the person who bought it. Never commit it, put it in a release
  bundle, or show its art in docs, screenshots or issues. The repository has two guards:
  `scripts/check-no-pack-files.sh` fails CI when git tracks a pack file, and the bundle script
  refuses to pack one. `assets/characters/` is in `.gitignore`.
- A free pack (for example one with a CC0 licence) with the same layout works the same way.
  Check its licence before you share anything.
- This page describes the format in words only. It has no pack art.

## Pack format v1

A pack is a zip file. The installer uses only these files and skips everything else.

| File | Count | Notes |
|---|---|---|
| `NPC_<n>.png` at the root of the zip | 1 to 15 | `n` from 1 up with no gaps. At least `NPC_1.png`. |
| `Emotes/<Name>_emote_16x16.png` | 0 to 10 | A missing emote is just not drawn. |

The ten emote names are Angry, Evil, Exclamation, Happy, HeartBreak, Heart, Interrogation, Sad,
Star and Surprised. The screen uses five: Exclamation, Interrogation, Happy, Sad and Surprised.

### Character sheet

Each `NPC_<n>.png` is exactly 128 x 208 pixels with transparency, cut into 16 x 16 frames: 8
columns and 13 rows.

- Row 0 is a header and is skipped.
- Columns 0 to 3 are the 4 idle frames. Columns 4 to 7 are the 4 walk frames.
- Rows 1 to 12 hold three variants (for example skin tones), 4 rows each: rows 1 to 4, rows 5
  to 8 and rows 9 to 12.
- Inside a variant, the 4 rows are the directions in this order: **front** (facing you),
  **left**, **back**, **right**. The right row is read from the sheet. It is not made by
  mirroring the left one.

The frame at column `c` (0 to 7), variant `v` (0 to 2) and direction `d` (front 0, left 1, back
2, right 3) starts at x = 16 times `c` and y = 16 times (1 + 4 times `v` + `d`).

A full pack has 15 characters and 3 variants, which gives 45 looks. A look is one character in
one variant. Each project gets one look, chosen from a hash of the project name, so it stays
the same between restarts. Two projects on screen never share a look while there are free
looks. A smaller pack just reuses looks.

### Emote sheet

Each emote file is exactly 64 x 16 pixels: 4 frames of 16 x 16. The 4 frames are a pop-in, and
the last one is the full bubble.

### What the installer checks

- The zip is at most 4 MiB, has at most 64 entries and has no unsafe name (absolute path, `..`,
  backslash, symlink, encrypted or duplicate entry).
- Each used file is at most 64 KiB, starts with the PNG signature and has the exact size in its
  header. The used files together are at most 2 MiB.

A pack that breaks a rule is refused as a whole and nothing is changed. Details are in
[SECURITY.md](../SECURITY.md#character-packs).

## Install

Copy the zip to the Pi first. Then, on the Pi:

```bash
# one-line installer
curl -fsSL https://raw.githubusercontent.com/nokri-mohammed/deskpane/main/get.sh | sudo bash -s -- --characters ~/my-pack.zip

# or from an extracted bundle
sudo ./install.sh --characters ~/my-pack.zip
```

From your own computer, `scripts/provision-pi.sh pi-zero --characters my-pack.zip` copies the zip
over SSH and runs the installer for you.

The pack goes to `/var/lib/deskpane/characters`, and the dashboard restarts to load it. An
upgrade without `--characters` keeps it. `sudo ./install.sh --remove-characters` removes it.
More steps are in [INSTALL.md](INSTALL.md#character-packs-optional).

If a file in the installed folder cannot be loaded, the dashboard skips that file and logs one
line. If `NPC_1.png` fails, it treats the whole pack as missing and uses the icons.

## What the character does

| State | Pose | Emote |
|---|---|---|
| `needs_you` | front, idle frames | Exclamation. The row is also filled amber. |
| `idle` (waits for a prompt) | front, idle frames | Interrogation |
| `working` | right, walk frames in place | none |
| `working` and quiet (no packet for 10 minutes) | right, idle frames (stopped walking) | none. Line 2 says `no news for 12m`. |
| `done` | front, idle frames | Happy for the first 5 seconds, then none |
| `done` for more than 30 minutes | front, first idle frame only, dimmed | none |
| `error` | front, idle frames | Sad |
| a new session | appears | Surprised for 2 seconds, then the emote of its state |
| the session ended | walks off to the right for about 2 seconds, word ENDED in grey | none |

The colour bar, the amber fill and the state word stay with a pack, so the state is still clear
without reading the sprite.

## How it is drawn

- Sprites are scaled by whole numbers only, without smoothing, so pixels stay sharp.
- In a row, the character is 3 times larger (48 pixels) and the emote 2 times (32 pixels). With
  one session, the card uses the largest whole scale that fits, up to 8 times.
- The animation runs at 5 frames a second, only while the agents page shows a moving character.
  Every other page stays at 3 frames a second.
