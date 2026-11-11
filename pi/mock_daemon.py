"""A stand-in for the daemon's Unix socket, for the local preview (T-061).
Stdlib only, never shipped to the Pi.

MockDaemon listens on a Unix socket and writes one scenario line a second
to every client, exactly the line shape the real daemon broadcasts. When
the scenario says the daemon is gone (envelope None), it closes every
client and the listening socket, so the dashboard shows "daemon not
reachable" the same way it would on the Pi.
"""
import os
import socket
import stat
import threading
import time

import preview_scenarios

# how long accept() waits before it checks for stop again
ACCEPT_POLL_S = 0.2


def remove_stale_socket(path):
    """Removes an old socket file at path; anything else there is left alone."""
    try:
        info = os.lstat(path)
    except FileNotFoundError:
        return
    if stat.S_ISSOCK(info.st_mode):
        os.unlink(path)


class MockDaemon:
    def __init__(self, path, scenario, interval_s=1.0, clock=time.monotonic):
        self._path = path
        self._scenario = scenario
        self._interval_s = interval_s
        self._clock = clock
        self._lock = threading.Lock()
        self._clients = []
        self._stop = threading.Event()
        self._closed = threading.Event()
        self._listener = None
        self._threads = []
        self._t0 = None

    def start(self):
        """Binds the socket (only the owner may connect) and starts the threads."""
        remove_stale_socket(self._path)
        listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        old_mask = os.umask(0o177)
        try:
            listener.bind(self._path)
        finally:
            os.umask(old_mask)
        listener.listen(4)
        listener.settimeout(ACCEPT_POLL_S)
        self._listener = listener
        self._t0 = self._clock()
        for target in (self._accept_loop, self._send_loop):
            thread = threading.Thread(target=target, daemon=True)
            thread.start()
            self._threads.append(thread)

    def current_line(self):
        """The scenario line for now, or None while disconnected."""
        return preview_scenarios.line_at(self._scenario, self._clock() - self._t0)

    def _accept_loop(self):
        while not self._stop.is_set() and not self._closed.is_set():
            try:
                client, _ = self._listener.accept()
            except socket.timeout:
                continue
            except OSError:
                return
            line = self.current_line()
            if line is None:
                client.close()
                continue
            # a new client gets the current line at once, then every tick
            try:
                client.sendall(line.encode())
            except OSError:
                client.close()
                continue
            with self._lock:
                self._clients.append(client)

    def _send_loop(self):
        while not self._stop.wait(self._interval_s):
            line = self.current_line()
            if line is None:
                self._close_all()
                return
            data = line.encode()
            with self._lock:
                clients = list(self._clients)
            for client in clients:
                try:
                    client.sendall(data)
                except OSError:
                    # the dashboard went away; it reconnects on its own
                    with self._lock:
                        if client in self._clients:
                            self._clients.remove(client)
                    client.close()

    def _close_all(self):
        """Closes every client and the listener and removes the socket file."""
        with self._lock:
            clients = list(self._clients)
            self._clients = []
        for client in clients:
            client.close()
        if self._listener is not None:
            self._listener.close()
        remove_stale_socket(self._path)
        self._closed.set()

    def wait_closed(self, timeout):
        """True once the scenario has closed the socket."""
        return self._closed.wait(timeout)

    def stop(self):
        self._stop.set()
        self._close_all()
        for thread in self._threads:
            thread.join(timeout=2)
