"""Tests for pi/preview.py, the local preview runner (T-061): arguments,
paths, the frame server's routes and the pack choice. Plain unittest; the
module imports pygame only inside main(), so these need no pygame.

The smoke test at the end runs the real headless preview, so it needs the
preview venv (`scripts/preview.sh` creates .venv-preview once) and is
skipped without it (CI has no pygame).
"""
import os
import subprocess
import sys
import tempfile
import threading
import time
import unittest

PI_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
REPO = os.path.dirname(PI_DIR)
sys.path.insert(0, PI_DIR)

import charpack  # noqa: E402
import preview  # noqa: E402

VENV_PYTHON = os.path.join(REPO, ".venv-preview", "bin", "python")


class ParseArgsTest(unittest.TestCase):
    def test_defaults(self):
        args = preview.parse_args([])
        self.assertEqual(args.scenario, "busy")
        self.assertEqual(args.scale, 2)
        self.assertFalse(args.headless)
        self.assertIsNone(args.serve)
        self.assertFalse(args.real)
        self.assertFalse(args.no_pack)
        self.assertEqual(args.frames, 0)

    def test_alias_and_options(self):
        args = preview.parse_args(["--scenario", "normal", "--scale", "3", "--headless", "--serve", "8780",
                                   "--tap-every", "3", "--frames", "5"])
        self.assertEqual(args.scenario, "busy")
        self.assertEqual((args.scale, args.serve, args.tap_every, args.frames), (3, 8780, 3.0, 5))
        self.assertTrue(args.headless)

    def test_bad_values_are_usage_errors(self):
        bad = [
            ["--scenario", "nope"],
            ["--scale", "0"],
            ["--scale", "5"],
            ["--serve", "8780"],
            ["--headless", "--serve", "80"],
            ["--headless", "--serve", "70000"],
            ["--frames", "-1"],
            ["--tap-every", "0"],
            ["--real", "--scenario", "worst"],
        ]
        for argv in bad:
            with self.subTest(argv=argv):
                with self.assertRaises(SystemExit) as caught:
                    with open(os.devnull, "w") as quiet:
                        old = sys.stderr
                        sys.stderr = quiet
                        try:
                            preview.parse_args(argv)
                        finally:
                            sys.stderr = old
                self.assertEqual(caught.exception.code, 2)


class PathsTest(unittest.TestCase):
    def test_cache_and_runtime_dirs_from_the_environment(self):
        env = {"XDG_CACHE_HOME": "/c", "XDG_RUNTIME_DIR": "/r", "HOME": "/h"}
        cache, sock = preview.preview_paths(env, 42)
        self.assertEqual(cache, "/c/deskpane-preview")
        self.assertEqual(sock, "/r/deskpane-preview-42.sock")

    def test_fallbacks(self):
        cache, sock = preview.preview_paths({"HOME": "/h"}, 7)
        self.assertEqual(cache, "/h/.cache/deskpane-preview")
        self.assertEqual(sock, "/h/.cache/deskpane-preview/deskpane-preview-7.sock")


class RouteTest(unittest.TestCase):
    """Only the page and the latest frame are served, nothing else."""

    def test_routes(self):
        self.assertEqual(preview.route_request("/"), "index")
        self.assertEqual(preview.route_request("/index.html"), "index")
        self.assertEqual(preview.route_request("/latest.png"), "frame")
        self.assertEqual(preview.route_request("/latest.png?t=123"), "frame")

    def test_everything_else_is_missing(self):
        for path in ("/../latest.png", "/latest.png/..", "/etc/passwd", "/frame-0001.png", "", "latest.png",
                     "/%2e%2e/latest.png", "//latest.png"):
            with self.subTest(path=path):
                self.assertEqual(preview.route_request(path), "missing")

    def test_only_loopback_host_headers_are_served(self):
        # DNS rebinding guard: a page from another site that resolves its own
        # name to 127.0.0.1 still sends its own name as the Host header
        for host in ("localhost:8780", "127.0.0.1:8780", "LOCALHOST:8780"):
            with self.subTest(host=host):
                self.assertTrue(preview.host_allowed(host, 8780))
        for host in (None, "", "evil.example:8780", "localhost", "127.0.0.1", "localhost:8781",
                     "127.0.0.1:80", "localhost.evil.example:8780", "[::1]:8780", "0.0.0.0:8780",
                     "localhost:8780 ", "localhost:08780"):
            with self.subTest(host=host):
                self.assertFalse(preview.host_allowed(host, 8780))

    def test_the_frame_server_answers_403_to_another_host(self):
        import http.client
        with tempfile.TemporaryDirectory() as out_dir:
            server = preview.start_frame_server(0, out_dir)
            try:
                port = server.server_address[1]
                results = {}
                for host in ("localhost:%d" % port, "evil.example:%d" % port):
                    conn = http.client.HTTPConnection("127.0.0.1", port, timeout=5)
                    conn.putrequest("GET", "/", skip_host=True)
                    conn.putheader("Host", host)
                    conn.endheaders()
                    results[host] = conn.getresponse().status
                    conn.close()
            finally:
                server.shutdown()
                server.server_close()
        self.assertEqual(results["localhost:%d" % port], 200)
        self.assertEqual(results["evil.example:%d" % port], 403)

    def test_the_page_reloads_the_frame_and_loads_nothing_external(self):
        page = preview.index_html()
        self.assertIn(b"latest.png", page)
        self.assertIn(b"pixelated", page)
        self.assertIn(b"500", page)
        self.assertNotIn(b"://", page)


class PackChoiceTest(unittest.TestCase):
    """The installed pack first, then folders under assets/characters."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = self._tmp.name

    def tearDown(self):
        self._tmp.cleanup()

    def test_order_and_missing_folders(self):
        system = os.path.join(self.root, "var-lib")
        assets = os.path.join(self.root, "assets", "characters")
        os.makedirs(os.path.join(assets, "b-pack"))
        os.makedirs(os.path.join(assets, "a-pack"))
        with open(os.path.join(assets, "note.txt"), "w") as f:
            f.write("not a folder")
        self.assertEqual(preview.pack_candidates(system, assets),
                         [os.path.join(assets, "a-pack"), os.path.join(assets, "b-pack")])
        os.makedirs(system)
        self.assertEqual(preview.pack_candidates(system, assets)[0], system)
        self.assertEqual(preview.pack_candidates(os.path.join(self.root, "none"), os.path.join(self.root, "nope")), [])

    def test_choose_takes_the_first_that_loads_else_a_missing_folder(self):
        chosen = preview.choose_pack_dir(["/a", "/b", "/c"], lambda folder: folder == "/b", "/missing")
        self.assertEqual(chosen, "/b")
        self.assertEqual(preview.choose_pack_dir(["/a"], lambda folder: False, "/missing"), "/missing")


class RealReporterTest(unittest.TestCase):
    def test_battery_only_with_a_pisugar_socket(self):
        plain = preview.real_reporter_argv("deskpane", {})
        self.assertNotIn("--battery", plain)
        self.assertEqual(plain[:4], ["deskpane", "report", "--daemon", preview.REAL_LISTEN_ADDR])
        with_socket = preview.real_reporter_argv("deskpane", {"PISUGAR_SOCKET": "/tmp/fake.sock"})
        self.assertEqual(with_socket[-2:], ["--battery", "pisugar"])
        self.assertNotIn("--battery", preview.real_reporter_argv("deskpane", {"PISUGAR_SOCKET": ""}))


class FakeChild:
    """Stands in for subprocess.Popen in the RealStack tests."""

    def __init__(self, on_start):
        self.terminated = False
        self.returncode = None
        on_start()

    def poll(self):
        return self.returncode

    def terminate(self):
        self.terminated = True
        self.returncode = -15

    def kill(self):
        self.terminate()

    def wait(self, timeout=None):
        return self.returncode


class RealStackStopTest(unittest.TestCase):
    def test_a_test_spawned_while_stopping_is_still_ended(self):
        """stop() runs while an agent test is being spawned: the new child
        must still be terminated, never left behind."""
        with tempfile.TemporaryDirectory() as out_dir:
            stack = preview.RealStack(os.path.join(out_dir, "s.sock"), out_dir)
            stack._client = "deskpane"
            stoppers = []
            children = []

            def stop_now():
                # stop() from another thread, during the spawn
                stopper = threading.Thread(target=stack.stop)
                stopper.start()
                stoppers.append(stopper)
                time.sleep(0.2)

            def fake_popen(argv, **kwargs):
                child = FakeChild(stop_now)
                children.append(child)
                return child

            real_popen = preview.subprocess.Popen
            preview.subprocess.Popen = fake_popen
            try:
                stack._agent_tests()
                for stopper in stoppers:
                    stopper.join(timeout=5)
            finally:
                preview.subprocess.Popen = real_popen
        self.assertEqual(len(children), 1)
        self.assertTrue(children[0].terminated)

    def test_no_test_is_spawned_after_stop(self):
        with tempfile.TemporaryDirectory() as out_dir:
            stack = preview.RealStack(os.path.join(out_dir, "s.sock"), out_dir)
            stack._client = "deskpane"
            stack.stop()
            spawned = []
            stack._spawn = lambda argv, env: spawned.append(argv)
            stack._agent_tests()
        self.assertEqual(spawned, [])


@unittest.skipUnless(os.path.exists(VENV_PYTHON), "no .venv-preview (run scripts/preview.sh once)")
class HeadlessSmokeTest(unittest.TestCase):
    """The real dashboard draws offscreen against the mock: three 480 x 320 PNGs."""

    def test_headless_frames(self):
        with tempfile.TemporaryDirectory() as out:
            env = dict(os.environ, PYTHONPATH=PI_DIR, XDG_RUNTIME_DIR=out)
            result = subprocess.run(
                [VENV_PYTHON, os.path.join(PI_DIR, "preview.py"), "--headless", "--frames", "3", "--no-pack",
                 "--out-dir", out, "--scenario", "worst", "--tap-every", "0.3"],
                env=env, capture_output=True, text=True, timeout=60,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            names = sorted(n for n in os.listdir(out) if n.endswith(".png"))
            self.assertEqual(names, ["frame-0001.png", "frame-0002.png", "frame-0003.png", "latest.png"])
            for name in names:
                with open(os.path.join(out, name), "rb") as f:
                    self.assertEqual(charpack.png_size(f.read(24)), (480, 320))
            self.assertNotIn("draw error", result.stderr)


if __name__ == "__main__":
    unittest.main()
