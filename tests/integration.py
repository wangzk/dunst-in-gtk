#!/usr/bin/env python3
"""Integration tests for dunst-in-gtk (ticket 01: minimal loop).

Runs the daemon under Xvfb + dbus-run-session and drives it over D-Bus
(jeepney, pure-python) and X11 (xdotool):

  1. Notify pops a GTK window; CloseNotification closes it; signal reason=3
  2. expire_timeout closes the window; signal reason=1
  3. notify-send (libnotify path) works
  4. a second daemon instance exits quietly with code 0
  5. SIGTERM shuts the daemon down gracefully with code 0

Usage: tests/integration.py [path-to-binary]
"""

import os
import signal
import subprocess
import sys
import time
from collections import deque

from jeepney import DBusAddress, MatchRule, new_method_call
from jeepney.io.blocking import open_dbus_connection

DISPLAY_NUM = ":97"
NOTIF_IFACE = "org.freedesktop.Notifications"
NOTIF_PATH = "/org/freedesktop/Notifications"
NOTIF_ADDR = DBusAddress(
    NOTIF_PATH, bus_name=NOTIF_IFACE, interface=NOTIF_IFACE
)


def log(msg):
    print(msg, flush=True)


def fail(msg):
    print(f"FAIL: {msg}", file=sys.stderr, flush=True)
    raise SystemExit(1)


def pass_(msg):
    print(f"PASS: {msg}", flush=True)


# ---------------------------------------------------------------- X11 helpers

def xdotool_windows(title):
    out = subprocess.run(
        ["xdotool", "search", "--name", title],
        capture_output=True,
        text=True,
    )
    return [w for w in out.stdout.split() if w]


def wait_for_window(title, timeout=5.0):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if xdotool_windows(title):
            return True
        time.sleep(0.1)
    return False


def wait_no_window(title, timeout=5.0):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if not xdotool_windows(title):
            return True
        time.sleep(0.1)
    return False


# ---------------------------------------------------------------- D-Bus layer

def notify(conn, app, summary, body, timeout_ms):
    """Call Notify; returns the notification id."""
    msg = new_method_call(
        NOTIF_ADDR,
        "Notify",
        "susssasa{sv}i",
        (app, 0, "dialog-information", summary, body, [], {}, timeout_ms),
    )
    reply = conn.send_and_get_reply(msg, timeout=10)
    return reply.body[0]


def close_notification(conn, nid):
    msg = new_method_call(NOTIF_ADDR, "CloseNotification", "u", (nid,))
    conn.send_and_get_reply(msg, timeout=10)


def wait_until_name_owned(conn, timeout=5.0):
    dbus = DBusAddress(
        "/org/freedesktop/DBus",
        bus_name="org.freedesktop.DBus",
        interface="org.freedesktop.DBus",
    )
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        msg = new_method_call(
            dbus, "GetNameOwner", "s", (NOTIF_IFACE,)
        )
        try:
            conn.send_and_get_reply(msg, timeout=2)
            return True
        except Exception:
            time.sleep(0.1)
    return False


def capture_notification_closed(conn, action, timeout=5.0):
    """Run `action(conn)` while a NotificationClosed match is live; return
    the (id, reason) tuple or None."""
    rule = MatchRule(
        type="signal", interface=NOTIF_IFACE, member="NotificationClosed"
    )
    queue = deque()
    with conn.filter(rule, queue=queue) as matches:
        action(conn)
        try:
            msg = conn.recv_until_filtered(matches, timeout=timeout)
        except TimeoutError:
            return None
    return (msg.body[0], msg.body[1])


# ------------------------------------------------------------- daemon harness

class Daemon:
    def __init__(self, binary, log_path):
        self.binary = binary
        self.log_path = log_path
        self.proc = None

    def start(self):
        self.log = open(self.log_path, "ab")
        self.proc = subprocess.Popen(
            [self.binary],
            stdout=self.log,
            stderr=subprocess.STDOUT,
            env=os.environ.copy(),
        )
        conn = open_dbus_connection(bus="SESSION")
        try:
            if not wait_until_name_owned(conn, timeout=5.0):
                tail = self.log_tail()
                fail(f"daemon did not acquire {NOTIF_IFACE}\n{tail}")
        finally:
            conn.close()
        return self

    def stop(self):
        if self.proc and self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait()
        if self.log:
            self.log.close()

    def log_tail(self, n=15):
        try:
            with open(self.log_path, "rb") as f:
                tail = f.read().decode(errors="replace").splitlines()[-n:]
            return "\n".join(tail)
        except OSError:
            return "(no daemon log)"


# --------------------------------------------------------------------- tests

def test_show_close(daemon, conn):
    log("== test: show + CloseNotification + NotificationClosed ==")

    def action(conn):
        nid = notify(conn, "itest", "Title", "Body", 5000)
        if not wait_for_window("dunst-in-gtk itest"):
            fail("notification window did not appear")
        close_notification(conn, nid)

    result = capture_notification_closed(conn, action, timeout=5.0)
    if result is None:
        fail("NotificationClosed not observed")
    nid, reason = result
    if reason != 3:
        fail(f"expected reason 3 (CloseNotification), got {reason}")
    if not wait_no_window("dunst-in-gtk itest"):
        fail("window still present after CloseNotification")
    pass_(f"Notify pops a window; CloseNotification closes it (id={nid}, reason=3)")


def test_expiry(daemon, conn):
    log("== test: expire_timeout ==")

    def action(conn):
        nid = notify(conn, "itest", "Ephemeral", "gone soon", 800)
        if not wait_for_window("dunst-in-gtk itest"):
            fail("window missing before expiry")
        if not wait_no_window("dunst-in-gtk itest", timeout=5.0):
            fail("window still present after expiry")
        return nid

    # The signal arrives shortly after the window closes; capture after.
    rule = MatchRule(
        type="signal", interface=NOTIF_IFACE, member="NotificationClosed"
    )
    queue = deque()
    with conn.filter(rule, queue=queue) as matches:
        nid = action(conn)
        try:
            msg = conn.recv_until_filtered(matches, timeout=3.0)
        except TimeoutError:
            msg = None
    if msg is None:
        fail("NotificationClosed (expiry) not observed")
    if (msg.body[0], msg.body[1]) != (nid, 1):
        fail(f"expected ({nid}, 1), got {msg.body}")
    pass_(f"expire_timeout closes the window and emits reason=1 (id={nid})")


def test_notify_send(daemon, conn):
    log("== test: notify-send (libnotify path) ==")
    subprocess.run(
        ["notify-send", "-t", "800", "Integration", "Hello from notify-send"],
        check=True,
        timeout=10,
    )
    if not wait_for_window("dunst-in-gtk notify-send"):
        fail("notify-send window did not appear")
    if not wait_no_window("dunst-in-gtk notify-send", timeout=5.0):
        fail("notify-send window did not expire")
    pass_("notify-send pops a window that expires")


def test_name_conflict(binary):
    log("== test: bus-name conflict ==")
    proc = subprocess.run([binary], capture_output=True, timeout=10)
    if proc.returncode != 0:
        fail(f"second instance exited with {proc.returncode}")
    pass_("second daemon instance exits with code 0")


def test_sigterm(daemon):
    log("== test: SIGTERM graceful shutdown ==")
    daemon.proc.send_signal(signal.SIGTERM)
    try:
        rc = daemon.proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        fail("daemon did not exit after SIGTERM")
    if rc != 0:
        fail(f"daemon exited with {rc} after SIGTERM, expected 0")
    daemon.proc = None
    pass_("SIGTERM shuts the daemon down with exit code 0")


# --------------------------------------------------------------------- driver

def run_tests(binary):
    daemon = Daemon(binary, os.path.join(os.environ.get("TMPDIR", "/tmp"), "dig-test.log"))
    conn = open_dbus_connection(bus="SESSION")
    try:
        daemon.start()
        test_show_close(daemon, conn)
        test_expiry(daemon, conn)
        test_notify_send(daemon, conn)
        test_name_conflict(binary)
        test_sigterm(daemon)
        log("")
        log("== all integration tests passed ==")
    finally:
        daemon.stop()
        conn.close()


def main():
    binary = sys.argv[1] if len(sys.argv) > 1 else "target/debug/dunst-in-gtk"
    binary = os.path.abspath(binary)
    if not os.path.exists(binary):
        fail(f"binary not found: {binary} (build first: cargo build)")

    if "--inside" in sys.argv:
        run_tests(binary)
        return 0

    # Outer: Xvfb + dbus-run-session, then re-run ourselves inside the bus.
    xvfb = subprocess.Popen(
        ["Xvfb", DISPLAY_NUM, "-screen", "0", "1280x800x24", "-nolisten", "tcp"],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        time.sleep(0.5)
        env = dict(os.environ, DISPLAY=DISPLAY_NUM)
        rc = subprocess.call(
            ["dbus-run-session", "--", sys.executable, os.path.abspath(__file__), binary, "--inside"],
            env=env,
        )
    finally:
        xvfb.terminate()
        try:
            xvfb.wait(timeout=5)
        except subprocess.TimeoutExpired:
            xvfb.kill()
    return rc


if __name__ == "__main__":
    sys.exit(main())
