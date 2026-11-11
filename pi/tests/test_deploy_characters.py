"""Fake-root tests for the character pack shell steps (T-051, T-053):
install.sh --characters / --remove-characters, uninstall.sh --purge,
get.sh --characters, package-pi-bundle.sh and the licence guard script.

The real scripts run under bash with DESKPANE_DESTDIR pointing at a temp
folder, and with stub commands first on PATH for everything that would
touch the real system (id, uname, systemctl, useradd, chown, ...). Every
zip is a fixture built in code. Skipped where bash is not available.
Run from the repo root with:
    python3 -m unittest discover -s pi/tests
"""
import hashlib
import io
import os
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import unittest

TESTS_DIR = os.path.dirname(os.path.abspath(__file__))
PI_DIR = os.path.dirname(TESTS_DIR)
REPO = os.path.dirname(PI_DIR)
sys.path.insert(0, PI_DIR)

import fixtures  # noqa: E402

BASH = shutil.which("bash")
CHARACTERS = "var/lib/deskpane/characters"

# stub commands; each logs its arguments to $STUB_LOG
STUBS = {
    "id": 'if [ "$1" = "-u" ]; then echo 0; else exec /usr/bin/id "$@"; fi',
    "uname": 'case "$1" in -m) echo aarch64 ;; -s) echo Linux ;; *) echo Linux ;; esac',
    "systemctl": 'echo "systemctl $*" >>"$STUB_LOG"; exit 0',
    "dpkg-query": "exit 0",
    # GETENT_MISSING="group:pisugar" makes that one lookup fail
    "getent": 'case " ${GETENT_MISSING:-} " in *" $1:$2 "*) exit 2 ;; esac; exit 0',
    "groupadd": 'echo "groupadd $*" >>"$STUB_LOG"; exit 0',
    "useradd": 'echo "useradd $*" >>"$STUB_LOG"; exit 0',
    "userdel": 'echo "userdel $*" >>"$STUB_LOG"; exit 0',
    "apt-get": 'echo "apt-get $*" >>"$STUB_LOG"; exit 0',
    "ss": "exit 0",
    "chown": 'echo "chown $*" >>"$STUB_LOG"; exit 0',
    "hostname": "echo 192.168.1.50",
    "sleep": "exit 0",
    # install without -o/-g, which need real root
    "install": (
        'args=(); while [ $# -gt 0 ]; do case "$1" in -o|-g) shift 2 ;; *) args+=("$1"); shift ;; esac; done; '
        'exec /usr/bin/install "${args[@]}"'
    ),
    "file": 'echo "ELF 64-bit LSB executable, ARM aarch64"',
    "ssh": 'echo "ssh $*" >>"$STUB_LOG"; exit 0',
    "scp": 'echo "scp $*" >>"$STUB_LOG"; exit 0',
}

FAKE_OVERLAY = b"fake overlay for tests\n"
FAKE_OVERLAY_URL = "https://raw.githubusercontent.com/goodtft/LCD-show/" + "0" * 40 + "/usr/tft35a-overlay.dtb"


def tree(root):
    found = []
    for folder, dirs, files in os.walk(root):
        for name in dirs + files:
            found.append(os.path.relpath(os.path.join(folder, name), root))
    return sorted(found)


@unittest.skipUnless(BASH and os.path.exists("/usr/bin/install"), "needs bash and coreutils")
class ShellTestCase(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = self._tmp.name
        self.root = os.path.join(self.tmp, "root")
        os.makedirs(os.path.join(self.root, "boot/firmware"))
        with open(os.path.join(self.root, "boot/firmware/config.txt"), "w") as f:
            f.write("[all]\n")
        with open(os.path.join(self.root, "boot/firmware/cmdline.txt"), "w") as f:
            f.write("console=tty1 root=PARTUUID=1234 rootwait\n")
        self.stub_dir = os.path.join(self.tmp, "stubs")
        os.makedirs(self.stub_dir)
        for name, body in STUBS.items():
            path = os.path.join(self.stub_dir, name)
            with open(path, "w") as f:
                f.write("#!/usr/bin/env bash\n" + body + "\n")
            os.chmod(path, 0o755)
        self.log = os.path.join(self.tmp, "stub.log")
        open(self.log, "w").close()

    def tearDown(self):
        self._tmp.cleanup()

    def env(self, **extra):
        env = {
            "PATH": self.stub_dir + os.pathsep + os.environ.get("PATH", "/usr/bin:/bin"),
            "STUB_LOG": self.log,
            "DESKPANE_DESTDIR": self.root,
            "HOME": self.tmp,
            "LC_ALL": "C",
        }
        env.update(extra)
        return env

    def run_script(self, script, *args, **env):
        return subprocess.run(
            [BASH, script] + list(args), stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            universal_newlines=True, env=self.env(**env), cwd=self.tmp, timeout=60,
        )

    def stub_log(self):
        with open(self.log) as f:
            return f.read()

    def make_bundle(self, name="bundle"):
        """A Pi bundle laid out like package-pi-bundle.sh makes it, with
        fake binaries and an overlay.env pinned to a fake overlay."""
        bundle = os.path.join(self.tmp, name)
        for sub in ("bin", "dashboard", "systemd", "config"):
            os.makedirs(os.path.join(bundle, sub))
        with open(os.path.join(bundle, "VERSION"), "w") as f:
            f.write("0.2.0-beta.1\n")
        shutil.copy(os.path.join(REPO, "LICENSE"), bundle)
        for script in ("install.sh", "uninstall.sh"):
            shutil.copy(os.path.join(REPO, "deploy", script), bundle)
        for binary in ("deskpane-daemon", "deskpane"):
            path = os.path.join(bundle, "bin", binary)
            with open(path, "w") as f:
                f.write("#!/bin/sh\nexit 0\n")
            os.chmod(path, 0o755)
        for name in os.listdir(PI_DIR):
            if name.endswith(".py"):
                shutil.copy(os.path.join(PI_DIR, name), os.path.join(bundle, "dashboard"))
        for name in os.listdir(os.path.join(REPO, "deploy", "systemd")):
            shutil.copy(os.path.join(REPO, "deploy", "systemd", name), os.path.join(bundle, "systemd"))
        shutil.copy(os.path.join(REPO, "deploy", "env.example"), os.path.join(bundle, "config"))
        self.overlay_file = os.path.join(self.tmp, "tft35a.dtbo")
        with open(self.overlay_file, "wb") as f:
            f.write(FAKE_OVERLAY)
        with open(os.path.join(bundle, "overlay.env"), "w") as f:
            f.write("OVERLAY_URL=%s\nOVERLAY_SHA256=%s\n" % (FAKE_OVERLAY_URL, hashlib.sha256(FAKE_OVERLAY).hexdigest()))
        return bundle

    def good_zip(self, characters=2):
        return fixtures.write_zip(os.path.join(self.tmp, "pack.zip"), fixtures.full_pack_entries(characters=characters))

    def slip_zip(self):
        entries = fixtures.full_pack_entries(characters=1) + [("../../etc/evil", b"x")]
        return fixtures.write_zip(os.path.join(self.tmp, "slip.zip"), entries)


class InstallCharactersTest(ShellTestCase):
    def install(self, *args):
        bundle = self.make_bundle()
        return self.run_script(os.path.join(bundle, "install.sh"), "--no-reporter",
                               "--overlay-file", self.overlay_file, *args)

    def test_install_without_characters_still_works(self):
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(os.path.isdir(os.path.join(self.root, "opt/deskpane/current/dashboard")))
        self.assertFalse(os.path.exists(os.path.join(self.root, CHARACTERS)))
        # the release carries the new pack modules and the shared contract module
        for name in ("charpack.py", "charpack_install.py", "sprites.py", "display.py", "deskpane_contract.py"):
            self.assertTrue(os.path.isfile(os.path.join(self.root, "opt/deskpane/current/dashboard", name)))

    def test_good_zip_fills_the_folder_root_owned_0644(self):
        result = self.install("--characters", self.good_zip())
        self.assertEqual(result.returncode, 0, result.stderr)
        target = os.path.join(self.root, CHARACTERS)
        files = tree(target)
        self.assertIn("NPC_1.png", files)
        self.assertIn("NPC_2.png", files)
        self.assertIn("Emotes/Sad_emote_16x16.png", files)
        self.assertNotIn("Patrons.txt", files)
        self.assertEqual(stat.S_IMODE(os.stat(target).st_mode), 0o755)
        self.assertEqual(stat.S_IMODE(os.stat(os.path.join(target, "NPC_1.png")).st_mode), 0o644)
        self.assertIn("chown -R root:root %s" % target, self.stub_log())
        self.assertIn("skipped 6 extra entries", result.stdout)

    def test_zip_slip_fails_and_changes_nothing(self):
        result = self.install("--characters", self.slip_zip())
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("unsafe path", result.stderr)
        self.assertFalse(os.path.exists(os.path.join(self.root, "opt")))
        self.assertFalse(os.path.exists(os.path.join(self.root, "var")))
        self.assertFalse(os.path.exists(os.path.join(self.root, "etc/deskpane")))
        self.assertFalse(os.path.exists(os.path.join(self.tmp, "etc/evil")))
        with open(os.path.join(self.root, "boot/firmware/config.txt")) as f:
            self.assertEqual(f.read(), "[all]\n")
        self.assertNotIn("systemctl", self.stub_log())

    def test_upgrade_without_flag_keeps_the_folder_untouched(self):
        target = os.path.join(self.root, CHARACTERS)
        os.makedirs(target)
        with open(os.path.join(target, "NPC_1.png"), "wb") as f:
            f.write(b"old pack")
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        with open(os.path.join(target, "NPC_1.png"), "rb") as f:
            self.assertEqual(f.read(), b"old pack")

    def test_remove_characters_removes_only_the_folder(self):
        target = os.path.join(self.root, CHARACTERS)
        os.makedirs(target)
        other = os.path.join(self.root, "var/lib/deskpane/keep-me")
        with open(other, "w") as f:
            f.write("x")
        result = self.install("--remove-characters")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(os.path.exists(target))
        self.assertTrue(os.path.exists(other))

    def test_symlink_zip_is_refused(self):
        link = os.path.join(self.tmp, "link.zip")
        os.symlink(self.good_zip(), link)
        result = self.install("--characters", link)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("regular file", result.stderr)
        self.assertFalse(os.path.exists(os.path.join(self.root, "opt")))

    def test_missing_zip_is_refused(self):
        result = self.install("--characters", os.path.join(self.tmp, "nope.zip"))
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(os.path.exists(os.path.join(self.root, "opt")))

    def test_zip_over_4_mib_is_refused(self):
        big = os.path.join(self.tmp, "big.zip")
        with open(big, "wb") as f:
            f.truncate(4 * 1024 * 1024 + 1)
        result = self.install("--characters", big)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("4 MiB", result.stderr)

    def test_characters_needs_the_display(self):
        result = self.install("--no-display", "--characters", self.good_zip())
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("--no-display", result.stderr)

    def test_characters_and_remove_together_is_refused(self):
        result = self.install("--characters", self.good_zip(), "--remove-characters")
        self.assertNotEqual(result.returncode, 0)

    def test_characters_needs_a_value(self):
        result = self.install("--characters")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("--characters needs", result.stderr)

    def test_pack_install_failure_does_not_abort_the_install(self):
        # /var/lib/deskpane is a file, so the pack folder cannot be made;
        # the check passes (it writes nothing), the install must still finish
        os.makedirs(os.path.join(self.root, "var/lib"))
        with open(os.path.join(self.root, "var/lib/deskpane"), "w") as f:
            f.write("in the way")
        result = self.install("--characters", self.good_zip())
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("the character pack could not be installed; continuing with the built-in icons", result.stderr)
        self.assertNotIn("chown -R", self.stub_log())
        self.assertIn("systemctl enable", self.stub_log())
        self.assertTrue(os.path.isdir(os.path.join(self.root, "opt/deskpane/current/dashboard")))

    def test_fifo_zip_is_refused_without_hanging(self):
        fifo = os.path.join(self.tmp, "pipe.zip")
        os.mkfifo(fifo)
        result = self.install("--characters", fifo)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("regular file", result.stderr)

    def test_pack_is_installed_from_a_private_copy(self):
        # the user's zip is copied once into the root-only work folder; the
        # installed files come from that copy, checked and installed as one
        result = self.install("--characters", self.good_zip())
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertRegex(result.stdout, r"checking the character pack .*/characters\.zip")

    def test_cdpath_does_not_change_the_zip_path(self):
        # with CDPATH set, a plain cd can jump elsewhere and print the folder
        os.makedirs(os.path.join(self.tmp, "decoy", "sub"))
        os.makedirs(os.path.join(self.tmp, "sub"))
        fixtures.write_zip(os.path.join(self.tmp, "sub", "pack.zip"), fixtures.full_pack_entries(characters=1))
        bundle = self.make_bundle()
        result = self.run_script(os.path.join(bundle, "install.sh"), "--no-reporter", "--overlay-file",
                                 self.overlay_file, "--characters", "sub/pack.zip",
                                 CDPATH=os.path.join(self.tmp, "decoy"))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(os.path.isfile(os.path.join(self.root, CHARACTERS, "NPC_1.png")))

    def test_relative_zip_path_works(self):
        self.good_zip()
        result = self.install("--characters", "pack.zip")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(os.path.isfile(os.path.join(self.root, CHARACTERS, "NPC_1.png")))


class PiSugarGroupTest(ShellTestCase):
    """T-064: the reporter unit names SupplementaryGroups=pisugar, and systemd
    refuses a unit whose group does not exist, so install.sh creates the
    system group when it is missing and leaves an existing one alone."""

    def install(self, *args, **env):
        bundle = self.make_bundle()
        return self.run_script(os.path.join(bundle, "install.sh"), "--overlay-file", self.overlay_file,
                               *args, **env)

    def test_missing_group_is_created_as_a_system_group(self):
        result = self.install(GETENT_MISSING="group:pisugar")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("groupadd --system pisugar", self.stub_log())

    def test_existing_group_is_left_alone(self):
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("groupadd", self.stub_log())

    def test_no_reporter_needs_no_group(self):
        result = self.install("--no-reporter", GETENT_MISSING="group:pisugar")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("groupadd", self.stub_log())

    def test_only_the_reporter_unit_gets_the_group(self):
        units = os.path.join(REPO, "deploy", "systemd")
        for name in sorted(os.listdir(units)):
            with open(os.path.join(units, name)) as f:
                text = f.read()
            with self.subTest(unit=name):
                if name == "deskpane-reporter.service":
                    self.assertIn("\nSupplementaryGroups=pisugar\n", text)
                    self.assertIn("AF_UNIX", text)
                    self.assertNotIn("InaccessiblePaths", text)
                else:
                    self.assertNotIn("pisugar", text.lower())


class UninstallTest(ShellTestCase):
    def setUp(self):
        super().setUp()
        self.target = os.path.join(self.root, CHARACTERS)
        os.makedirs(self.target)
        os.makedirs(os.path.join(self.root, "opt/deskpane"))
        self.uninstall = os.path.join(REPO, "deploy", "uninstall.sh")

    def test_uninstall_keeps_the_folder(self):
        result = self.run_script(self.uninstall)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(os.path.isdir(self.target))
        self.assertFalse(os.path.exists(os.path.join(self.root, "opt/deskpane")))

    def test_uninstall_purge_removes_var_lib_deskpane(self):
        result = self.run_script(self.uninstall, "--purge")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(os.path.exists(os.path.join(self.root, "var/lib/deskpane")))


class GetShTest(ShellTestCase):
    get_sh = os.path.join(REPO, "get.sh")

    def test_reporter_only_with_characters_is_refused(self):
        result = self.run_script(self.get_sh, "--reporter-only", "--characters", self.good_zip())
        self.assertEqual(result.returncode, 1)
        self.assertIn("--characters", result.stderr)
        self.assertIn("--reporter-only", result.stderr)

    def test_characters_needs_an_existing_file(self):
        result = self.run_script(self.get_sh, "--characters", os.path.join(self.tmp, "nope.zip"))
        self.assertEqual(result.returncode, 1)
        self.assertIn("not found", result.stderr)

    def release_files(self, install_body, with_validator=True):
        """A fake release folder (file://) with a bundle whose install.sh is
        install_body, and its SHA256SUMS; returns the env for get.sh. With
        with_validator the bundle also carries the real pack modules."""
        release = os.path.join(self.tmp, "release", "v0.2.0-beta.1")
        os.makedirs(release)
        top = "deskpane-0.2.0-beta.1-pi-aarch64/"
        members = [("install.sh", install_body.encode(), 0o755)]
        if with_validator:
            for name in ("charpack.py", "charpack_install.py"):
                with open(os.path.join(PI_DIR, name), "rb") as f:
                    members.append(("dashboard/" + name, f.read(), 0o644))
        raw = io.BytesIO()
        with tarfile.open(fileobj=raw, mode="w:gz") as tar:
            for name, data, mode in members:
                info = tarfile.TarInfo(top + name)
                info.size = len(data)
                info.mode = mode
                tar.addfile(info, io.BytesIO(data))
        archive = os.path.join(release, "deskpane-pi-aarch64.tar.gz")
        with open(archive, "wb") as f:
            f.write(raw.getvalue())
        with open(os.path.join(release, "SHA256SUMS"), "w") as f:
            f.write("%s  deskpane-pi-aarch64.tar.gz\n" % hashlib.sha256(raw.getvalue()).hexdigest())
        # a Raspberry Pi with Bookworm, inside the fake root
        os.makedirs(os.path.join(self.root, "proc/device-tree"))
        with open(os.path.join(self.root, "proc/device-tree/model"), "w") as f:
            f.write("Raspberry Pi Zero 2 W Rev 1.0\x00")
        os.makedirs(os.path.join(self.root, "etc"), exist_ok=True)
        with open(os.path.join(self.root, "etc/os-release"), "w") as f:
            f.write('VERSION_ID="12"\n')
        return {"DESKPANE_DOWNLOAD_URL": "file://" + os.path.join(self.tmp, "release")}

    def plant_old_unit(self):
        """An old pre-rename unit that remove_old_install would delete."""
        unit_dir = os.path.join(self.root, "etc/systemd/system")
        os.makedirs(unit_dir)
        path = os.path.join(unit_dir, "homelab-daemon.service")
        with open(path, "w") as f:
            f.write("[Unit]\nDescription=pi-homelab-dashboard daemon\n[Service]\nExecStart=/opt/old/daemon\n")
        return path

    def test_bad_pack_is_refused_before_anything_changes(self):
        ran = os.path.join(self.tmp, "ran")
        body = '#!/usr/bin/env bash\n# knows --no-summary and --characters\ntouch "%s"\n' % ran
        env = self.release_files(body)
        old_unit = self.plant_old_unit()
        result = self.run_script(self.get_sh, "--version", "v0.2.0-beta.1", "--no-reboot",
                                 "--characters", self.slip_zip(), **env)
        self.assertEqual(result.returncode, 1)
        self.assertIn("the character pack was refused; nothing was changed", result.stderr)
        self.assertIn("unsafe path", result.stderr)
        self.assertFalse(os.path.exists(ran))
        # remove_old_install and check_udp_port did not run yet
        self.assertTrue(os.path.exists(old_unit))
        self.assertNotIn("systemctl", self.stub_log())

    def test_good_pack_is_checked_then_installed(self):
        args_file = os.path.join(self.tmp, "install-args")
        body = ('#!/usr/bin/env bash\n# knows --no-summary and --characters\n'
                'printf "%%s\\n" "$@" >"%s"\n' % args_file)
        env = self.release_files(body)
        result = self.run_script(self.get_sh, "--version", "v0.2.0-beta.1", "--no-reboot", "--no-display",
                                 "--characters", self.good_zip(), **env)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("pack is valid", result.stdout)
        self.assertFalse(os.path.exists(os.path.join(self.root, CHARACTERS)))

    def test_cdpath_does_not_change_the_zip_path(self):
        args_file = os.path.join(self.tmp, "install-args")
        body = ('#!/usr/bin/env bash\n# knows --no-summary and --characters\n'
                'printf "%%s\\n" "$@" >"%s"\n' % args_file)
        env = self.release_files(body)
        os.makedirs(os.path.join(self.tmp, "decoy", "sub"))
        os.makedirs(os.path.join(self.tmp, "sub"))
        fixtures.write_zip(os.path.join(self.tmp, "sub", "pack.zip"), fixtures.full_pack_entries(characters=1))
        env["CDPATH"] = os.path.join(self.tmp, "decoy")
        result = self.run_script(self.get_sh, "--version", "v0.2.0-beta.1", "--no-reboot", "--no-display",
                                 "--characters", "sub/pack.zip", **env)
        self.assertEqual(result.returncode, 0, result.stderr)
        with open(args_file) as f:
            args = f.read().split("\n")
        self.assertEqual(args[args.index("--characters") + 1], os.path.join(self.tmp, "sub", "pack.zip"))

    def test_characters_passes_an_absolute_path_to_install_sh(self):
        args_file = os.path.join(self.tmp, "install-args")
        body = ('#!/usr/bin/env bash\n# knows --no-summary and --characters\n'
                'printf "%%s\\n" "$@" >"%s"\n' % args_file)
        env = self.release_files(body)
        self.good_zip()
        result = self.run_script(self.get_sh, "--version", "v0.2.0-beta.1", "--no-reboot", "--no-display",
                                 "--characters", "pack.zip", **env)
        self.assertEqual(result.returncode, 0, result.stderr)
        with open(args_file) as f:
            args = f.read().split("\n")
        index = args.index("--characters")
        self.assertEqual(args[index + 1], os.path.join(self.tmp, "pack.zip"))

    def test_older_bundle_without_the_flag_is_refused(self):
        ran = os.path.join(self.tmp, "ran")
        body = '#!/usr/bin/env bash\n# knows --no-summary only\ntouch "%s"\n' % ran
        env = self.release_files(body, with_validator=False)
        result = self.run_script(self.get_sh, "--version", "v0.2.0-beta.1", "--no-reboot",
                                 "--characters", self.good_zip(), **env)
        self.assertEqual(result.returncode, 1)
        self.assertIn("does not support --characters", result.stderr)
        self.assertFalse(os.path.exists(ran))


class PackageBundleTest(ShellTestCase):
    """package-pi-bundle.sh in a copy of the repo, so a planted pack file
    never lands in the real working tree."""

    def setUp(self):
        super().setUp()
        self.fake_repo = os.path.join(self.tmp, "repo")
        os.makedirs(os.path.join(self.fake_repo, "scripts"))
        shutil.copy(os.path.join(REPO, "scripts", "package-pi-bundle.sh"), os.path.join(self.fake_repo, "scripts"))
        shutil.copytree(os.path.join(REPO, "deploy"), os.path.join(self.fake_repo, "deploy"))
        os.makedirs(os.path.join(self.fake_repo, "pi"))
        for name in os.listdir(PI_DIR):
            if name.endswith(".py"):
                shutil.copy(os.path.join(PI_DIR, name), os.path.join(self.fake_repo, "pi"))
        shutil.copy(os.path.join(REPO, "LICENSE"), self.fake_repo)
        with open(os.path.join(self.fake_repo, "Cargo.toml"), "w") as f:
            f.write('[workspace.package]\nversion = "0.2.0-beta.1"\n')
        self.bin_dir = os.path.join(self.tmp, "bin")
        os.makedirs(self.bin_dir)
        for binary in ("deskpane-daemon", "deskpane"):
            with open(os.path.join(self.bin_dir, binary), "w") as f:
                f.write("fake\n")

    def package(self):
        return self.run_script(os.path.join(self.fake_repo, "scripts", "package-pi-bundle.sh"),
                               "--bin-dir", self.bin_dir, "--out-dir", os.path.join(self.tmp, "dist"))

    def bundle_names(self):
        with tarfile.open(os.path.join(self.tmp, "dist", "deskpane-pi-aarch64.tar.gz")) as tar:
            return tar.getnames()

    def test_clean_tree_builds_and_ships_the_pack_modules(self):
        result = self.package()
        self.assertEqual(result.returncode, 0, result.stderr)
        names = self.bundle_names()
        for name in ("charpack.py", "charpack_install.py", "sprites.py", "display.py", "deskpane_contract.py"):
            self.assertIn("deskpane-0.2.0-beta.1-pi-aarch64/dashboard/" + name, names)
        self.assertFalse([n for n in names if n.endswith((".png", ".zip"))])
        # the PC preview (T-061) never ships to the Pi
        for name in ("preview.py", "mock_daemon.py", "preview_scenarios.py"):
            self.assertNotIn("deskpane-0.2.0-beta.1-pi-aarch64/dashboard/" + name, names)

    def test_pack_file_in_a_bundle_source_folder_fails_the_build(self):
        for planted in ("pi/NPC_1.png", "deploy/Emotes/Sad_emote_16x16.png", "pi/bot-characters.zip"):
            with self.subTest(planted=planted):
                path = os.path.join(self.fake_repo, planted)
                os.makedirs(os.path.dirname(path), exist_ok=True)
                with open(path, "wb") as f:
                    f.write(b"x")
                result = self.package()
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("character pack", result.stderr)
                self.assertFalse(os.path.exists(os.path.join(self.tmp, "dist", "deskpane-pi-aarch64.tar.gz")))
                os.remove(path)

    def test_many_pack_files_still_print_the_message(self):
        # enough matches to fill the pipe: the old `find | head` died of
        # SIGPIPE under pipefail (exit 141) before printing anything
        for n in range(1, 2001):
            with open(os.path.join(self.fake_repo, "pi", "NPC_%d.png" % n), "wb") as f:
                f.write(b"x")
        result = self.package()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("character pack", result.stderr)


@unittest.skipUnless(BASH, "needs bash")
class ProvisionTest(ShellTestCase):
    """provision-pi.sh checks --characters before it touches the network."""

    def test_missing_characters_zip_fails_before_any_ssh(self):
        result = self.run_script(os.path.join(REPO, "scripts", "provision-pi.sh"), "pi-zero",
                                 "--characters", os.path.join(self.tmp, "nope.zip"))
        self.assertEqual(result.returncode, 1)
        self.assertIn("--characters", result.stderr)
        self.assertNotIn("ssh", self.stub_log())
        self.assertNotIn("downloading", result.stdout)

    def test_symlink_characters_zip_fails_before_any_ssh(self):
        link = os.path.join(self.tmp, "link.zip")
        os.symlink(self.good_zip(), link)
        result = self.run_script(os.path.join(REPO, "scripts", "provision-pi.sh"), "pi-zero", "--characters", link)
        self.assertEqual(result.returncode, 1)
        self.assertNotIn("ssh", self.stub_log())


@unittest.skipUnless(shutil.which("git"), "needs git")
class LicenceGuardTest(ShellTestCase):
    """scripts/check-no-pack-files.sh, run by CI: fails when git tracks
    anything under assets/characters/ or any pack-shaped file name."""

    def setUp(self):
        super().setUp()
        self.git_repo = os.path.join(self.tmp, "gitrepo")
        os.makedirs(os.path.join(self.git_repo, "scripts"))
        shutil.copy(os.path.join(REPO, "scripts", "check-no-pack-files.sh"), os.path.join(self.git_repo, "scripts"))
        self.git("init", "-q")
        self.git("add", "scripts")

    def git(self, *args):
        subprocess.run(["git", "-C", self.git_repo] + list(args), check=True,
                       stdout=subprocess.PIPE, stderr=subprocess.PIPE)

    def check(self):
        return self.run_script(os.path.join(self.git_repo, "scripts", "check-no-pack-files.sh"))

    def plant(self, rel):
        path = os.path.join(self.git_repo, rel)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "wb") as f:
            f.write(b"x")
        self.git("add", "-f", rel)

    def test_clean_repo_passes(self):
        result = self.check()
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_tracked_pack_files_fail(self):
        for rel in ("assets/characters/readme.txt", "docs/media/NPC_3.png", "x/Emotes/Happy_emote_16x16.png",
                    "pack.zip", "docs/Bot-Characters.ZIP", "docs/media/npc_4.PNG", "x/sad_EMOTE_16x16.png"):
            with self.subTest(rel=rel):
                self.plant(rel)
                result = self.check()
                self.assertEqual(result.returncode, 1)
                self.assertIn(rel, result.stderr)
                self.git("rm", "-q", "--cached", rel)


if __name__ == "__main__":
    unittest.main()
