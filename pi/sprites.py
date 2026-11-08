"""Thin pygame loader for an installed character pack, with a cache of
scaled frames. All decisions (which frame, which look, which pose) are in
charpack.py; this file only loads PNGs, cuts frames and scales them.

No pack, or a bad NPC_1.png, means load() returns None and the dashboard
draws its built-in icons instead.
"""
import os
import stat
import sys

import pygame

import charpack

# the emotes the agents page uses (section 10 of the agent status plan)
USED_EMOTES = ("Exclamation", "Interrogation", "Happy", "Sad", "Surprised")
# a dimmed done character is drawn at this alpha
DIM_ALPHA = 110


def log(message):
    print("dashboard: " + message, file=sys.stderr, flush=True)


def load_png(path, want_size):
    """A Surface for a regular PNG file of exactly want_size, or None. The
    file size is checked before decoding, so a huge file is never read."""
    try:
        info = os.lstat(path)
    except OSError:
        return None
    if not stat.S_ISREG(info.st_mode) or info.st_size > charpack.MAX_ENTRY_BYTES:
        log("character pack: skipped %s (not a small regular file)" % os.path.basename(path))
        return None
    try:
        surface = pygame.image.load(path)
    except (pygame.error, OSError, ValueError) as error:
        log("character pack: skipped %s (%s)" % (os.path.basename(path), error))
        return None
    if surface.get_size() != want_size:
        log("character pack: skipped %s (wrong size %dx%d)" % ((os.path.basename(path),) + surface.get_size()))
        return None
    return surface


class CharacterPack:
    """The sheets and emotes of one installed pack, plus scaled frames."""

    def __init__(self, sheets, emotes):
        # sheets[0] is NPC_1; numbers are contiguous from 1
        self.sheets = sheets
        self.emotes = emotes
        self.looks = charpack.look_count(len(sheets))
        self._frames = {}
        self._emote_frames = {}

    @classmethod
    def load(cls, folder):
        """Loads the pack in folder, or returns None (no pack, or NPC_1.png
        is missing or bad). Later bad sheets end the list there, so the
        numbers stay contiguous; a bad emote is skipped on its own."""
        if not os.path.isdir(folder):
            log("no character pack (built-in icons)")
            return None

        sheets = []
        for number in range(1, charpack.MAX_CHARACTERS + 1):
            path = os.path.join(folder, "NPC_%d.png" % number)
            if not os.path.lexists(path):
                break
            sheet = load_png(path, (charpack.SHEET_W, charpack.SHEET_H))
            if sheet is None:
                break
            sheets.append(sheet)
        if not sheets:
            log("no usable character pack in %s (built-in icons)" % folder)
            return None

        emotes = {}
        for name in USED_EMOTES:
            path = os.path.join(folder, "Emotes", "%s_emote_16x16.png" % name)
            if not os.path.lexists(path):
                continue
            emote = load_png(path, (charpack.EMOTE_W, charpack.EMOTE_H))
            if emote is not None:
                emotes[name] = emote

        log("character pack: %d characters, %d emotes" % (len(sheets), len(emotes)))
        return cls(sheets, emotes)

    def frame(self, look, direction, motion, frame, scale, dim=False):
        """One character frame for a look, scaled `scale` times with no
        smoothing. Cached per look, pose, frame and scale."""
        key = (look, direction, motion, frame, scale, dim)
        cached = self._frames.get(key)
        if cached is not None:
            return cached
        character, variant = charpack.look_of(look % self.looks)
        sheet = self.sheets[character - 1]
        rect = pygame.Rect(charpack.frame_rect(variant, direction, motion, frame))
        size = charpack.FRAME * scale
        surface = pygame.transform.scale(sheet.subsurface(rect), (size, size))
        if dim:
            surface.set_alpha(DIM_ALPHA)
        self._frames[key] = surface
        return surface

    def emote(self, name, frame, scale):
        """One emote frame scaled `scale` times, or None if the pack has no
        such emote (a missing emote is just not drawn)."""
        strip = self.emotes.get(name)
        if strip is None:
            return None
        key = (name, frame, scale)
        cached = self._emote_frames.get(key)
        if cached is not None:
            return cached
        rect = pygame.Rect(charpack.emote_rect(frame))
        size = charpack.FRAME * scale
        surface = pygame.transform.scale(strip.subsurface(rect), (size, size))
        self._emote_frames[key] = surface
        return surface

    def keep_only(self, looks_in_use):
        """Drops cached frames of looks no session uses any more."""
        in_use = set(looks_in_use)
        for key in [key for key in self._frames if key[0] not in in_use]:
            del self._frames[key]
