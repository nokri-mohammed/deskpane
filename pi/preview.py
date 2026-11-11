"""Local preview of the Pi dashboard (T-061): runs the real dashboard.py on
this PC, in a window or offscreen, against a mock daemon (scenario data)
or the real Rust daemon on loopback. Started by scripts/preview.sh, which
makes the venv with pygame and sets PYTHONPATH=pi. Never shipped to the Pi.

pygame is imported only inside main(), so the pure parts (arguments, paths,
the frame server's routes, the pack choice) are tested without it.

Character packs are only read in place, never copied. Frames can contain
pack art, so they go to the cache folder outside the repo.
"""
import argparse
import http.server
import os
import subprocess
import sys
import threading
import time

import preview_scenarios

PI_DIR = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(PI_DIR)
SYSTEM_PACK_DIR = "/var/lib/deskpane/characters"
ASSETS_PACK_DIR = os.path.join(REPO, "assets", "characters")
# --real: the daemon listens here, away from a real daemon on 8765
REAL_LISTEN_ADDR = "127.0.0.1:18765"
REAL_DEVICE_ID = "preview-pc"
REAL_LABEL = "Preview PC"
# deskpane agent test takes about 10 s; one new fake session this often
REAL_AGENT_TEST_EVERY_S = 20
MIN_PORT = 1024
MAX_PORT = 65535


def log(message):
    print("preview: " + message, file=sys.stderr, flush=True)


def scenario_name(value):
    """argparse type: a scenario name or alias, as its real name."""
    name = preview_scenarios.resolve_scenario(value)
    if name is None:
        choices = ", ".join(sorted(list(preview_scenarios.SCENARIOS) + list(preview_scenarios.ALIASES)))
        raise argparse.ArgumentTypeError("unknown scenario %r (one of: %s)" % (value, choices))
    return name


def parse_args(argv):
    parser = argparse.ArgumentParser(
        prog="scripts/preview.sh",
        description="Run the Pi dashboard on this PC: a window by default, or headless PNG frames.")
    parser.add_argument("--scenario", type=scenario_name, default=None,
                        help="mock data: %s (default %s; normal = busy)"
                        % (", ".join(preview_scenarios.SCENARIOS), preview_scenarios.DEFAULT_SCENARIO))
    parser.add_argument("--scale", type=int, default=2, help="window size, 1 to 4 times 480 x 320 (default 2)")
    parser.add_argument("--headless", action="store_true", help="no window: write PNG frames")
    parser.add_argument("--serve", type=int, metavar="PORT", default=None,
                        help="with --headless: show the live frames at http://localhost:PORT (127.0.0.1 only)")
    parser.add_argument("--frames", type=int, default=0, help="with --headless: write N numbered frames, then exit")
    parser.add_argument("--tap-every", type=float, default=None, metavar="S",
                        help="tap every S seconds, to walk through the pages")
    parser.add_argument("--real", action="store_true",
                        help="the real daemon, a reporter and 'deskpane agent test' on loopback instead of the mock")
    parser.add_argument("--no-pack", action="store_true", help="built-in icons even if a character pack is installed")
    parser.add_argument("--out-dir", default=None, help="where frames go (default ~/.cache/deskpane-preview)")
    args = parser.parse_args(argv)

    if not 1 <= args.scale <= 4:
        parser.error("--scale must be 1 to 4")
    if args.serve is not None:
        if not args.headless:
            parser.error("--serve needs --headless")
        if not MIN_PORT <= args.serve <= MAX_PORT:
            parser.error("--serve PORT must be %d to %d" % (MIN_PORT, MAX_PORT))
    if args.frames < 0:
        parser.error("--frames must be 0 or more")
    if args.tap_every is not None and args.tap_every <= 0:
        parser.error("--tap-every must be more than 0")
    if args.real and args.scenario is not None:
        parser.error("--real uses the real daemon, not a scenario")
    if args.scenario is None:
        args.scenario = preview_scenarios.DEFAULT_SCENARIO
    if args.tap_every is None:
        args.tap_every = 0.0
    return args


def preview_paths(env, pid):
    """(cache dir for frames, socket path) from the environment. The socket
    goes to the runtime dir, else into the cache dir; the pid keeps two
    previews apart."""
    home = env.get("HOME") or os.path.expanduser("~")
    cache_root = env.get("XDG_CACHE_HOME") or os.path.join(home, ".cache")
    cache = os.path.join(cache_root, "deskpane-preview")
    runtime = env.get("XDG_RUNTIME_DIR") or cache
    return cache, os.path.join(runtime, "deskpane-preview-%d.sock" % pid)


# ---- frame server ----

def route_request(path):
    """What a GET path serves: "index" (the page), "frame" (latest.png) or
    "missing". Only these exact paths; a query string is ignored."""
    bare = path.split("?", 1)[0]
    if bare in ("/", "/index.html"):
        return "index"
    if bare == "/latest.png":
        return "frame"
    return "missing"


def host_allowed(host, port):
    """True when a request's Host header names this loopback server exactly:
    localhost:PORT or 127.0.0.1:PORT (case does not matter). Guards against
    DNS rebinding: another site whose name resolves to 127.0.0.1 still sends
    its own name, so it gets 403 and cannot read the frames."""
    if not host:
        return False
    allowed = ("localhost:%d" % port, "127.0.0.1:%d" % port)
    return host.lower() in allowed


def index_html():
    """A small page that reloads latest.png every 500 ms, 2x and pixelated.
    No external assets."""
    page = """<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>deskpane preview</title>
<style>
  :root { color-scheme: dark; }
  body { margin: 0; padding: 16px; background: #101115; color: #82868f;
         font: 14px system-ui, sans-serif; }
  img { display: block; width: 960px; max-width: 100%; height: auto;
        image-rendering: pixelated; background: #000; }
  p { margin: 8px 0 0; }
</style>
</head>
<body>
<img id="frame" src="latest.png" alt="dashboard frame" width="960" height="640">
<p>deskpane preview, 480 x 320 panel shown at 2x. Frames refresh every 500 ms.</p>
<script>
  const img = document.getElementById("frame");
  setInterval(() => {
    const next = new Image();
    next.onload = () => { img.src = next.src; };
    next.src = "latest.png?t=" + Date.now();
  }, 500);
</script>
</body>
</html>
"""
    return page.encode()


def make_handler(out_dir):
    class FrameHandler(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            # only a Host header naming this loopback server (DNS rebinding guard)
            if not host_allowed(self.headers.get("Host"), self.server.server_address[1]):
                self.reply(403, "text/plain; charset=utf-8", b"forbidden\n")
                return
            route = route_request(self.path)
            if route == "index":
                self.reply(200, "text/html; charset=utf-8", index_html())
                return
            if route == "frame":
                try:
                    with open(os.path.join(out_dir, "latest.png"), "rb") as f:
                        body = f.read()
                except OSError:
                    self.reply(503, "text/plain; charset=utf-8", b"no frame yet\n")
                    return
                self.reply(200, "image/png", body)
                return
            self.reply(404, "text/plain; charset=utf-8", b"not found\n")

        def reply(self, status, content_type, body):
            self.send_response(status)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, format, *args):
            # one line per frame request would flood the terminal
            pass

    return FrameHandler


def start_frame_server(port, out_dir):
    """Serves the page and latest.png on 127.0.0.1 only, in a thread."""
    server = http.server.ThreadingHTTPServer(("127.0.0.1", port), make_handler(out_dir))
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


# ---- character pack ----

def pack_candidates(system_dir, assets_dir):
    """The installed pack folder if it exists, then each folder under
    assets/characters in name order."""
    found = []
    if os.path.isdir(system_dir):
        found.append(system_dir)
    if os.path.isdir(assets_dir):
        for name in sorted(os.listdir(assets_dir)):
            path = os.path.join(assets_dir, name)
            if os.path.isdir(path):
                found.append(path)
    return found


def choose_pack_dir(candidates, loads, missing):
    """The first folder that loads as a pack, else missing (built-in icons)."""
    for folder in candidates:
        if loads(folder):
            return folder
    return missing


# ---- --real: the Rust daemon, a reporter and agent tests ----

def real_reporter_argv(client, env):
    """The reporter command of --real. With PISUGAR_SOCKET set (for example
    to a fake pisugar-server socket) it also reads the battery (T-064)."""
    argv = [client, "report", "--daemon", REAL_LISTEN_ADDR, "--device-id", REAL_DEVICE_ID,
            "--label", REAL_LABEL, "--interval", "2"]
    if env.get("PISUGAR_SOCKET"):
        argv += ["--battery", "pisugar"]
    return argv


class RealStack:
    """The real daemon on loopback, one reporter and a fake agent session
    now and then. Their output goes to real.log in the cache dir."""

    def __init__(self, socket_path, out_dir):
        self._socket_path = socket_path
        self._log_path = os.path.join(out_dir, "real.log")
        self._children = []
        self._stop = threading.Event()
        # held while an agent test is spawned and while stop() takes the child
        # list, so a test spawned during stop() is still ended
        self._lock = threading.Lock()
        self._log = None

    def start(self):
        cargo = os.environ.get("CARGO") or os.path.expanduser("~/.cargo/bin/cargo")
        log("building the daemon and the reporter (cargo build)")
        built = subprocess.run([cargo, "build", "--locked", "-p", "daemon", "-p", "reporter"], cwd=REPO)
        if built.returncode != 0:
            raise SystemExit("preview: cargo build failed")
        target = os.path.join(REPO, "target", "debug")
        self._daemon = os.path.join(target, "deskpane-daemon")
        self._client = os.path.join(target, "deskpane")
        self._log = open(self._log_path, "a")
        env = dict(os.environ, LISTEN_ADDR=REAL_LISTEN_ADDR, SOCKET_PATH=self._socket_path)
        self._spawn([self._daemon], env)
        # give the daemon a moment to bind; exit 11 means the port is taken
        time.sleep(1.0)
        if self._children[0].poll() is not None:
            raise SystemExit("preview: the daemon exited (%s), see %s" % (self._children[0].returncode,
                                                                          self._log_path))
        self._spawn(real_reporter_argv(self._client, os.environ), os.environ)
        threading.Thread(target=self._agent_tests, daemon=True).start()
        log("real daemon on %s, log in %s" % (REAL_LISTEN_ADDR, self._log_path))

    def _spawn(self, argv, env):
        child = subprocess.Popen(argv, env=env, stdin=subprocess.DEVNULL, stdout=self._log, stderr=self._log)
        self._children.append(child)

    def _agent_tests(self):
        while True:
            # check stop again under the lock: stop() may have started meanwhile
            with self._lock:
                if self._stop.is_set():
                    return
                # kept in the child list, so stop() ends a test that is still running
                self._spawn([self._client, "agent", "test", "--daemon", REAL_LISTEN_ADDR, "--device-id",
                             REAL_DEVICE_ID], os.environ)
                child = self._children[-1]
            child.wait()
            self._stop.wait(REAL_AGENT_TEST_EVERY_S)

    def stop(self):
        # after this block no new child can be spawned, and every child is in the list
        with self._lock:
            self._stop.set()
            children = list(self._children)
        for child in reversed(children):
            if child.poll() is None:
                child.terminate()
        for child in children:
            try:
                child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                child.kill()
        if self._log is not None:
            self._log.close()


def main(argv=None):
    args = parse_args(sys.argv[1:] if argv is None else argv)
    cache_dir, socket_path = preview_paths(os.environ, os.getpid())
    out_dir = args.out_dir or cache_dir
    os.makedirs(out_dir, exist_ok=True)
    os.makedirs(os.path.dirname(socket_path), exist_ok=True)

    # the dashboard reads these at import
    os.environ["SOCKET_PATH"] = socket_path
    os.environ["NET_IFACE"] = "none"
    if args.headless:
        os.environ["SDL_VIDEODRIVER"] = "dummy"
    os.environ.setdefault("SDL_AUDIODRIVER", "dummy")
    os.environ["PYGAME_HIDE_SUPPORT_PROMPT"] = "1"

    import sprites
    missing = os.path.join(out_dir, "no-pack")
    if args.no_pack:
        pack_dir = missing
    else:
        pack_dir = choose_pack_dir(pack_candidates(SYSTEM_PACK_DIR, ASSETS_PACK_DIR),
                                   lambda folder: sprites.CharacterPack.load(folder) is not None, missing)
    os.environ["CHARACTERS_DIR"] = pack_dir

    import dashboard
    import display

    stack = None
    mock = None
    server = None
    try:
        if args.real:
            stack = RealStack(socket_path, out_dir)
            stack.start()
        else:
            import mock_daemon
            mock = mock_daemon.MockDaemon(socket_path, args.scenario)
            mock.start()
            log("scenario %s on %s" % (args.scenario, socket_path))

        if args.headless:
            screen = display.PngDisplay(out_dir, frames=args.frames, tap_every=args.tap_every)
        else:
            screen = display.WindowDisplay(args.scale, out_dir)
            if args.tap_every > 0:
                log("--tap-every applies to --headless only")
        if args.serve is not None:
            try:
                server = start_frame_server(args.serve, out_dir)
            except OSError as error:
                screen.close()
                raise SystemExit("preview: cannot serve on 127.0.0.1:%d: %s" % (args.serve, error))
            log("frames at http://localhost:%d (127.0.0.1 only), Ctrl+C to stop" % args.serve)
        if args.headless:
            log("frames in %s" % out_dir)
        dashboard.main(screen)
    finally:
        if server is not None:
            server.shutdown()
            server.server_close()
        if mock is not None:
            mock.stop()
        if stack is not None:
            stack.stop()
            if os.path.exists(socket_path):
                os.unlink(socket_path)
    return 0


if __name__ == "__main__":
    sys.exit(main())
