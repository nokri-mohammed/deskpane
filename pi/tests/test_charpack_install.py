"""Tests for pi/charpack_install.py: installing a pack zip into a folder.
Every zip is a fixture built in code, written to a temp folder.
Run from the repo root with:
    python3 -m unittest discover -s pi/tests
"""
import os
import stat
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

PI_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, PI_DIR)

import charpack  # noqa: E402
import charpack_install  # noqa: E402
import fixtures  # noqa: E402

INSTALLER = os.path.join(PI_DIR, "charpack_install.py")


def tree(root):
    """Sorted relative paths of every file and folder under root."""
    found = []
    for folder, dirs, files in os.walk(root):
        for name in dirs + files:
            found.append(os.path.relpath(os.path.join(folder, name), root))
    return sorted(found)


class InstallTest(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = self._tmp.name
        self.target = os.path.join(self.tmp, "lib", "characters")

    def tearDown(self):
        self._tmp.cleanup()

    def zip_at(self, entries, name="pack.zip"):
        return fixtures.write_zip(os.path.join(self.tmp, name), entries)

    def test_install_writes_only_fixed_names_into_target(self):
        zip_path = self.zip_at(fixtures.full_pack_entries(characters=3))
        result = charpack_install.install_pack(zip_path, self.target)
        self.assertEqual(result["characters"], 3)
        self.assertEqual(result["emotes"], 10)
        self.assertEqual(result["skipped"], 6)
        expected = ["Emotes"] + ["NPC_%d.png" % n for n in (1, 2, 3)]
        expected += ["Emotes/%s_emote_16x16.png" % n for n in fixtures.EMOTE_NAMES]
        self.assertEqual(tree(self.target), sorted(expected))
        # nothing left next to the target
        self.assertEqual(sorted(os.listdir(os.path.dirname(self.target))), ["characters"])

    def test_install_copies_the_bytes_and_sets_modes(self):
        png = fixtures.sheet_png()
        zip_path = self.zip_at([("NPC_1.png", png)])
        charpack_install.install_pack(zip_path, self.target)
        with open(os.path.join(self.target, "NPC_1.png"), "rb") as f:
            self.assertEqual(f.read(), png)
        self.assertEqual(stat.S_IMODE(os.stat(os.path.join(self.target, "NPC_1.png")).st_mode), 0o644)
        self.assertEqual(stat.S_IMODE(os.stat(self.target).st_mode), 0o755)

    def test_install_replaces_an_old_pack_completely(self):
        charpack_install.install_pack(self.zip_at(fixtures.full_pack_entries(characters=4), "a.zip"), self.target)
        charpack_install.install_pack(self.zip_at([("NPC_1.png", fixtures.sheet_png())], "b.zip"), self.target)
        self.assertEqual(tree(self.target), ["NPC_1.png"])

    def test_install_failure_leaves_old_pack_in_place(self):
        charpack_install.install_pack(self.zip_at(fixtures.full_pack_entries(characters=2), "good.zip"), self.target)
        before = tree(self.target)
        bad_cases = [
            [("NPC_1.png", fixtures.sheet_png()), ("../escape.txt", b"x")],
            [("NPC_1.png", fixtures.make_png(128, 16))],
            [("NPC_1.png", b"not a png")],
            [("NPC_2.png", fixtures.sheet_png())],
        ]
        for i, entries in enumerate(bad_cases):
            with self.subTest(case=i):
                with self.assertRaises(charpack.PackError):
                    charpack_install.install_pack(self.zip_at(entries, "bad%d.zip" % i), self.target)
                self.assertEqual(tree(self.target), before)
                self.assertEqual(sorted(os.listdir(os.path.dirname(self.target))), ["characters"])

    def test_zip_slip_writes_nothing_anywhere(self):
        zip_path = self.zip_at([("NPC_1.png", fixtures.sheet_png()), ("../../evil.txt", b"x")])
        with self.assertRaises(charpack.PackError):
            charpack_install.install_pack(zip_path, self.target)
        self.assertFalse(os.path.exists(os.path.dirname(self.target)))
        self.assertNotIn("evil.txt", os.listdir(self.tmp))

    def test_symlink_zip_path_is_refused(self):
        real = self.zip_at([("NPC_1.png", fixtures.sheet_png())])
        link = os.path.join(self.tmp, "link.zip")
        os.symlink(real, link)
        with self.assertRaises(charpack.PackError) as ctx:
            charpack_install.install_pack(link, self.target)
        self.assertIn("regular file", str(ctx.exception))

    def test_zip_over_4_mib_is_refused(self):
        big = os.path.join(self.tmp, "big.zip")
        with open(big, "wb") as f:
            f.truncate(charpack.MAX_ZIP_BYTES + 1)
        with self.assertRaises(charpack.PackError) as ctx:
            charpack_install.install_pack(big, self.target)
        self.assertIn("4 MiB", str(ctx.exception))

    def test_not_a_zip_is_refused(self):
        path = os.path.join(self.tmp, "x.zip")
        with open(path, "wb") as f:
            f.write(b"hello")
        with self.assertRaises(charpack.PackError):
            charpack_install.install_pack(path, self.target)

    def test_entry_over_the_cap_is_refused(self):
        # a valid PNG padded past the 64 KiB cap
        zip_path = self.zip_at([("NPC_1.png", fixtures.sheet_png() + b"\x00" * (charpack.MAX_ENTRY_BYTES + 10))])
        with self.assertRaises(charpack.PackError):
            charpack_install.install_pack(zip_path, self.target)

    def test_check_only_writes_nothing(self):
        zip_path = self.zip_at(fixtures.full_pack_entries(characters=2))
        result = charpack_install.install_pack(zip_path, self.target, check_only=True)
        self.assertEqual(result["characters"], 2)
        self.assertFalse(os.path.exists(os.path.dirname(self.target)))

    def test_stale_new_folder_is_cleared(self):
        os.makedirs(self.target + ".new")
        with open(os.path.join(self.target + ".new", "junk"), "w") as f:
            f.write("x")
        charpack_install.install_pack(self.zip_at([("NPC_1.png", fixtures.sheet_png())]), self.target)
        self.assertEqual(tree(self.target), ["NPC_1.png"])
        self.assertFalse(os.path.exists(self.target + ".new"))

    # item 4: a broken compressed stream is a refusal, not a traceback
    def test_corrupted_deflate_entry_is_refused_cleanly(self):
        zip_path = fixtures.corrupt_deflate_zip(os.path.join(self.tmp, "corrupt.zip"))
        with self.assertRaises(charpack.PackError) as ctx:
            charpack_install.install_pack(zip_path, self.target)
        self.assertIn("cannot be read", str(ctx.exception))
        self.assertFalse(os.path.exists(os.path.dirname(self.target)))

    # item 3: the zip is opened once, without following links or blocking
    def test_fifo_zip_is_refused_without_blocking(self):
        fifo = os.path.join(self.tmp, "pipe.zip")
        os.mkfifo(fifo)
        with self.assertRaises(charpack.PackError) as ctx:
            charpack_install.install_pack(fifo, self.target)
        self.assertIn("regular file", str(ctx.exception))

    def test_missing_zip_is_refused(self):
        with self.assertRaises(charpack.PackError):
            charpack_install.install_pack(os.path.join(self.tmp, "nope.zip"), self.target)

    def test_zip_is_opened_with_nofollow(self):
        zip_path = self.zip_at([("NPC_1.png", fixtures.sheet_png())])
        real_open = os.open
        flags_seen = []

        def spy(path, flags, *args):
            if path == zip_path:
                flags_seen.append(flags)
            return real_open(path, flags, *args)

        with mock.patch.object(charpack_install.os, "open", side_effect=spy):
            charpack_install.install_pack(zip_path, self.target, check_only=True)
        self.assertEqual(len(flags_seen), 1)
        self.assertTrue(flags_seen[0] & os.O_NOFOLLOW)
        self.assertTrue(flags_seen[0] & os.O_NONBLOCK)

    # item 5: a failed swap puts the old pack back
    def test_failed_swap_restores_the_old_pack(self):
        charpack_install.install_pack(self.zip_at(fixtures.full_pack_entries(characters=2), "old.zip"), self.target)
        before = tree(self.target)
        real_rename = os.rename

        def failing_rename(src, dst):
            # moving the new folder into place fails; every other rename works
            if src == self.target + ".new":
                raise OSError("disk full")
            return real_rename(src, dst)

        with mock.patch.object(charpack_install.os, "rename", side_effect=failing_rename):
            with self.assertRaises(charpack.PackError) as ctx:
                charpack_install.install_pack(self.zip_at([("NPC_1.png", fixtures.sheet_png())], "new.zip"), self.target)
        self.assertIn("old pack", str(ctx.exception))
        self.assertEqual(tree(self.target), before)
        self.assertEqual(sorted(os.listdir(os.path.dirname(self.target))), ["characters"])

    def test_failed_first_install_leaves_nothing_behind(self):
        real_rename = os.rename

        def failing_rename(src, dst):
            if src == self.target + ".new":
                raise OSError("disk full")
            return real_rename(src, dst)

        with mock.patch.object(charpack_install.os, "rename", side_effect=failing_rename):
            with self.assertRaises(charpack.PackError):
                charpack_install.install_pack(self.zip_at([("NPC_1.png", fixtures.sheet_png())]), self.target)
        self.assertEqual(os.listdir(os.path.dirname(self.target)), [])

    def test_unwritable_parent_is_a_refusal_not_a_traceback(self):
        # the parent of the target is a file, so no folder can be made
        blocker = os.path.join(self.tmp, "lib")
        with open(blocker, "w") as f:
            f.write("x")
        with self.assertRaises(charpack.PackError):
            charpack_install.install_pack(self.zip_at([("NPC_1.png", fixtures.sheet_png())]), self.target)

    # item 6: files and folders reach the disk before the swap is reported
    def test_files_and_folders_are_fsynced(self):
        zip_path = self.zip_at(fixtures.full_pack_entries(characters=2, emotes=("Sad",), extras=False))
        real_fsync = os.fsync
        calls = []

        def spy(fd):
            calls.append(fd)
            return real_fsync(fd)

        with mock.patch.object(charpack_install.os, "fsync", side_effect=spy):
            charpack_install.install_pack(zip_path, self.target)
        # 3 files, the new folder, Emotes/, and the parent after the swap
        self.assertGreaterEqual(len(calls), 6)


class CommandLineTest(unittest.TestCase):
    """The script as install.sh runs it: python3 -I charpack_install.py."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = self._tmp.name
        self.target = os.path.join(self.tmp, "characters")

    def tearDown(self):
        self._tmp.cleanup()

    def run_installer(self, *args):
        return subprocess.run(
            [sys.executable, "-I", INSTALLER] + list(args),
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, universal_newlines=True,
            cwd=self.tmp,
        )

    def test_good_zip_exits_0_and_reports_counts(self):
        zip_path = fixtures.write_zip(os.path.join(self.tmp, "p.zip"), fixtures.full_pack_entries(characters=2))
        result = self.run_installer(zip_path, self.target)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("2 characters", result.stdout)
        self.assertIn("skipped 6 extra entries", result.stdout)

    def test_bad_zip_exits_1_with_the_reason_on_stderr(self):
        zip_path = fixtures.write_zip(os.path.join(self.tmp, "p.zip"), [("NPC_1.png", fixtures.sheet_png()), ("/abs", b"x")])
        result = self.run_installer(zip_path, self.target)
        self.assertEqual(result.returncode, 1)
        self.assertIn("unsafe path", result.stderr)
        self.assertFalse(os.path.exists(self.target))

    def test_check_flag_writes_nothing(self):
        zip_path = fixtures.write_zip(os.path.join(self.tmp, "p.zip"), [("NPC_1.png", fixtures.sheet_png())])
        result = self.run_installer("--check", zip_path, self.target)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(os.path.exists(self.target))

    def test_corrupted_zip_reports_without_a_traceback(self):
        zip_path = fixtures.corrupt_deflate_zip(os.path.join(self.tmp, "c.zip"))
        result = self.run_installer(zip_path, self.target)
        self.assertEqual(result.returncode, 1)
        self.assertIn("cannot be read", result.stderr)
        self.assertNotIn("Traceback", result.stderr)

    def test_fifo_does_not_hang_the_installer(self):
        fifo = os.path.join(self.tmp, "pipe.zip")
        os.mkfifo(fifo)
        result = subprocess.run(
            [sys.executable, "-I", INSTALLER, "--check", fifo, self.target],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, universal_newlines=True, timeout=10,
        )
        self.assertEqual(result.returncode, 1)

    def test_wrong_arguments_exit_2(self):
        self.assertEqual(self.run_installer().returncode, 2)
        self.assertEqual(self.run_installer("a", "b", "c").returncode, 2)

    def test_relative_target_is_refused(self):
        zip_path = fixtures.write_zip(os.path.join(self.tmp, "p.zip"), [("NPC_1.png", fixtures.sheet_png())])
        self.assertEqual(self.run_installer(zip_path, "characters").returncode, 2)

    def test_a_planted_module_in_the_working_folder_is_not_imported(self):
        # -I keeps the working folder off sys.path; only the script's own folder is added
        with open(os.path.join(self.tmp, "zipfile.py"), "w") as f:
            f.write("raise SystemExit(99)\n")
        zip_path = fixtures.write_zip(os.path.join(self.tmp, "p.zip"), [("NPC_1.png", fixtures.sheet_png())])
        result = self.run_installer(zip_path, self.target)
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == "__main__":
    unittest.main()
