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

def notify(conn, app, summary, body, timeout_ms, actions=None, urgency=None):
    """Call Notify; returns the notification id."""
    hints = {}
    if urgency is not None:
        hints["urgency"] = ("y", urgency)
    msg = new_method_call(
        NOTIF_ADDR,
        "Notify",
        "susssasa{sv}i",
        (app, 0, "dialog-information", summary, body, actions or [], hints, timeout_ms),
    )
    reply = conn.send_and_get_reply(msg, timeout=10)
    return reply.body[0]


def close_notification(conn, nid):
    msg = new_method_call(NOTIF_ADDR, "CloseNotification", "u", (nid,))
    conn.send_and_get_reply(msg, timeout=10)


def wait_until_name_owned(conn, timeout=5.0, pid=None):
    """Wait until org.freedesktop.Notifications has an owner; when `pid` is
    given, additionally require that the owner process is that pid (a freshly
    dead daemon's name lingers on the bus for a moment, which would otherwise
    fool the check)."""
    dbus = DBusAddress(
        "/org/freedesktop/DBus",
        bus_name="org.freedesktop.DBus",
        interface="org.freedesktop.DBus",
    )
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            reply = conn.send_and_get_reply(
                new_method_call(dbus, "GetNameOwner", "s", (NOTIF_IFACE,)), timeout=2
            )
        except Exception:
            time.sleep(0.1)
            continue
        if pid is None:
            return True
        owner = reply.body[0]
        try:
            p = conn.send_and_get_reply(
                new_method_call(dbus, "GetConnectionUnixProcessID", "s", (owner,)),
                timeout=2,
            )
            if p.body[0] == pid:
                return True
        except Exception:
            pass
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


def window_geoms(title):
    """[(x, y, w, h), ...] for every window whose title matches."""
    geoms = []
    for wid in xdotool_windows(title):
        r = subprocess.run(
            ["xdotool", "getwindowgeometry", "--shell", wid],
            capture_output=True,
            text=True,
        )
        kv = {}
        for line in r.stdout.splitlines():
            if "=" in line:
                k, v = line.split("=", 1)
                kv[k.strip()] = int(v)
        if kv:
            geoms.append((kv.get("X", -1), kv.get("Y", -1), kv.get("WIDTH", -1), kv.get("HEIGHT", -1)))
    return geoms


def wait_window_count(title, n, timeout=5.0):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if len(xdotool_windows(title)) == n:
            return True
        time.sleep(0.1)
    return False


def wait_window_geometry(title, n, timeout=5.0):
    """Wait until `n` windows with real geometry exist; return their geoms.

    A window that is realized but not yet configured reports (0, 0, 1, 1)
    from xdotool, so we poll until the geometry is meaningful."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        geoms = window_geoms(title)
        if len(geoms) == n and all(w > 1 and x >= 0 for (x, y, w, h) in geoms):
            return geoms
        time.sleep(0.1)
    return window_geoms(title)


LAYOUT_DUNSTRC = """
[global]
origin = top-right
offset = (10, 10)
gap_size = 8
width = (200, 400)
height = (0, 1000)
"""


def test_layout(binary, conn):
    log("== test: corner stacking + reflow ==")
    cfg_path = os.path.join(os.environ.get("TMPDIR", "/tmp"), "dig-layout-dunstrc")
    with open(cfg_path, "w") as f:
        f.write(LAYOUT_DUNSTRC)
    daemon = Daemon(binary, os.path.join(os.environ.get("TMPDIR", "/tmp"), "dig-layout.log"))
    try:
        daemon.start(args=["-config", cfg_path])
        if not wait_until_name_owned(conn, timeout=5.0):
            fail("layout daemon did not acquire the bus name")

        nid_a = notify(conn, "itest", "First", "AAA", 5000)
        geoms = wait_window_geometry("dunst-in-gtk itest", 1)
        if len(geoms) != 1:
            fail("first notification window missing")
        x0, y0, w0, h0 = geoms[0]
        if (x0, w0) != (1070, 200):
            fail(f"expected top-right at x=1070 w=200 (1280-10-200), got {geoms}")

        nid_b = notify(conn, "itest", "Second", "BBB", 5000)
        geoms = wait_window_geometry("dunst-in-gtk itest", 2)
        if len(geoms) != 2:
            fail("second notification window missing")
        geoms = sorted(geoms)
        (xa, ya, wa, ha), (xb, yb, wb, hb) = geoms[0], geoms[1]
        if ya >= yb:
            fail(f"expected first above second, got {geoms}")
        if xa != xb:
            fail(f"expected right-aligned stack, got {geoms}")
        if yb - ya != ha + 8:
            fail(f"expected gap 8 between notifications, got {geoms}")

        close_notification(conn, nid_a)
        if not wait_window_count("dunst-in-gtk itest", 1):
            fail("window count did not drop after close")
        geoms = window_geoms("dunst-in-gtk itest")
        if len(geoms) != 1 or geoms[0][1] != 10:
            fail(f"expected reflow to y=10, got {geoms}")

        close_notification(conn, nid_b)
        if not wait_window_count("dunst-in-gtk itest", 0):
            fail("second window did not close")
    finally:
        daemon.stop()
    pass_(f"stacking + reflow verified (nid_a={nid_a} nid_b={nid_b})")


def test_hidpi(binary, conn):
    log("== test: HiDPI (GDK_SCALE=2) ==")
    cfg_path = os.path.join(os.environ.get("TMPDIR", "/tmp"), "dig-layout-dunstrc")
    daemon = Daemon(binary, os.path.join(os.environ.get("TMPDIR", "/tmp"), "dig-hidpi.log"))
    try:
        daemon.start(args=["-config", cfg_path], env={"GDK_SCALE": "2"})
        if not wait_until_name_owned(conn, timeout=5.0):
            fail("hidpi daemon did not acquire the bus name")

        notify(conn, "itest", "HiDPI", "scaled", 5000)
        geoms = wait_window_geometry("dunst-in-gtk itest", 1)
        if len(geoms) != 1:
            fail("hidpi notification window missing")
        x, y, w, h = geoms[0]
        # 200 logical px -> 400 physical; offset 10 logical -> 20 physical.
        if (w, x, y) != (400, 860, 20):
            fail(f"expected physical (400, 860, 20) for logical 200@scale2, got {geoms}")
    finally:
        daemon.stop()
    pass_("GDK_SCALE=2 doubles the physical size and offsets")




class Daemon:
    def __init__(self, binary, log_path):
        self.binary = binary
        self.log_path = log_path
        self.proc = None

    def start(self, args=None, env=None):
        self.log = open(self.log_path, "ab")
        full_env = os.environ.copy()
        if env:
            full_env.update(env)
        self.proc = subprocess.Popen(
            [self.binary] + (args or []),
            stdout=self.log,
            stderr=subprocess.STDOUT,
            env=full_env,
        )
        conn = open_dbus_connection(bus="SESSION")
        try:
            if not wait_until_name_owned(conn, timeout=5.0, pid=self.proc.pid):
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


def test_name_conflict(binary, conn):
    log("== test: bus-name conflict ==")
    holder = Daemon(binary, os.path.join(os.environ.get("TMPDIR", "/tmp"), "dig-nc.log"))
    try:
        holder.start()
        if not wait_until_name_owned(conn, timeout=5.0):
            fail("holder daemon did not acquire the bus name")
        proc = subprocess.run([binary], capture_output=True, timeout=10)
        if proc.returncode != 0:
            fail(f"second instance exited with {proc.returncode}")
    finally:
        holder.stop()
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


CONTEXT_DUNSTRC = """
[global]
origin = top-right
offset = (10, 10)
width = (200, 400)
mouse_right_click = context
"""


def visible_window_ids():
    out = subprocess.run(
        ["xdotool", "search", "--onlyvisible", "--name", ".*"],
        capture_output=True,
        text=True,
    )
    return set(out.stdout.split())


def window_geoms_of(wid):
    r = subprocess.run(
        ["xdotool", "getwindowgeometry", "--shell", wid],
        capture_output=True,
        text=True,
    )
    kv = {}
    for line in r.stdout.splitlines():
        if "=" in line:
            k, v = line.split("=", 1)
            kv[k.strip()] = int(v)
    return (kv.get("X", 0), kv.get("Y", 0), kv.get("WIDTH", 0), kv.get("HEIGHT", 0))


def test_hover_pauses_timeout(daemon, conn):
    log("== test: hover pauses the timeout ==")
    # Consume the closed signal too; otherwise it lingers on the bus and the
    # next signal test reads a stale one.
    rule = MatchRule(type="signal", interface=NOTIF_IFACE, member="NotificationClosed")
    queue = deque()
    with conn.filter(rule, queue=queue) as matches:
        nid = notify(conn, "itest", "Hover", "pause me", 800)
        geoms = wait_window_geometry("dunst-in-gtk itest", 1)
        if len(geoms) != 1:
            fail("hover test window missing")
        wid = xdotool_windows("dunst-in-gtk itest")[0]
        x, y, w, h = geoms[0]

        subprocess.run(["xdotool", "mousemove", "--window", wid, str(w // 2), str(h // 2)])
        time.sleep(1.5)  # well past the 800 ms timeout, but the pointer is inside
        if not xdotool_windows("dunst-in-gtk itest"):
            fail("notification expired while hovered")

        subprocess.run(["xdotool", "mousemove", "1", "1"])  # leave the window
        if not wait_no_window("dunst-in-gtk itest", timeout=4.0):
            fail("notification did not expire after the pointer left")
        try:
            msg = conn.recv_until_filtered(matches, timeout=3.0)
        except TimeoutError:
            fail("NotificationClosed not observed after the pointer left")
        if (msg.body[0], msg.body[1]) != (nid, 1):
            fail(f"expected closed reason 1 (expired), got {msg.body}")
    pass_(f"hover pauses the expiry timer (id={nid})")


def test_left_click_default_action(binary, conn):
    # The default dunst mouse binding closes on left click; this test uses a
    # config that binds the left button to the default action instead.
    log("== test: left click invokes the default action ==")
    cfg_path = os.path.join(os.environ.get("TMPDIR", "/tmp"), "dig-leftclick-dunstrc")
    with open(cfg_path, "w") as f:
        f.write("[global]\nmouse_left_click = do_action\n")
    daemon = Daemon(binary, os.path.join(os.environ.get("TMPDIR", "/tmp"), "dig-leftclick.log"))
    try:
        daemon.start(args=["-config", cfg_path])
        if not wait_until_name_owned(conn, timeout=5.0, pid=daemon.proc.pid):
            fail("left-click daemon did not acquire the bus name")

        rule = MatchRule(type="signal", interface=NOTIF_IFACE, member="ActionInvoked")
        queue = deque()
        with conn.filter(rule, queue=queue) as matches:
            nid = notify(
                conn, "itest", "Actions", "click me", 5000,
                actions=["default", "Open", "other", "Other"],
            )
            geoms = wait_window_geometry("dunst-in-gtk itest", 1)
            if len(geoms) != 1:
                fail("action notification window missing")
            wid = xdotool_windows("dunst-in-gtk itest")[0]
            x, y, w, h = geoms[0]
            subprocess.run(["xdotool", "mousemove", "--window", wid, str(w // 2), str(h // 2)])
            subprocess.run(["xdotool", "click", "1"])
            try:
                msg = conn.recv_until_filtered(matches, timeout=3.0)
            except TimeoutError:
                fail("ActionInvoked not observed after left click")
            if msg.body != (nid, "default"):
                fail(f"expected ActionInvoked({nid}, 'default'), got {msg.body}")
        if not wait_no_window("dunst-in-gtk itest", timeout=3.0):
            fail("notification did not close after the action")
    finally:
        daemon.stop()
    pass_(f"left click invokes the default action (id={nid})")


def test_middle_click_closes(daemon, conn):
    log("== test: middle click closes ==")
    rule = MatchRule(type="signal", interface=NOTIF_IFACE, member="NotificationClosed")
    queue = deque()
    with conn.filter(rule, queue=queue) as matches:
        nid = notify(conn, "itest", "Middle", "click me", 5000)
        geoms = wait_window_geometry("dunst-in-gtk itest", 1)
        if len(geoms) != 1:
            fail("middle-click test window missing")
        wid = xdotool_windows("dunst-in-gtk itest")[0]
        x, y, w, h = geoms[0]
        subprocess.run(["xdotool", "mousemove", "--window", wid, str(w // 2), str(h // 2)])
        subprocess.run(["xdotool", "click", "2"])
        try:
            msg = conn.recv_until_filtered(matches, timeout=3.0)
        except TimeoutError:
            fail("NotificationClosed not observed after middle click")
        if (msg.body[0], msg.body[1]) != (nid, 2):
            fail(f"expected closed reason 2 (dismissed), got {msg.body}")
    pass_(f"middle click closes with reason 2 (id={nid})")


def test_context_menu(binary, conn):
    log("== test: right click opens the context menu ==")
    cfg_path = os.path.join(os.environ.get("TMPDIR", "/tmp"), "dig-context-dunstrc")
    with open(cfg_path, "w") as f:
        f.write(CONTEXT_DUNSTRC)
    daemon = Daemon(binary, os.path.join(os.environ.get("TMPDIR", "/tmp"), "dig-context.log"))
    try:
        daemon.start(args=["-config", cfg_path])
        if not wait_until_name_owned(conn, timeout=5.0, pid=daemon.proc.pid):
            fail("context-menu daemon did not acquire the bus name")

        rule = MatchRule(type="signal", interface=NOTIF_IFACE, member="ActionInvoked")
        queue = deque()
        with conn.filter(rule, queue=queue) as matches:
            nid = notify(
                conn, "itest", "Menu", "right click", 5000,
                actions=["default", "Open", "other", "Other"],
            )
            geoms = wait_window_geometry("dunst-in-gtk itest", 1)
            if len(geoms) != 1:
                fail("context-menu test window missing")
            wid = xdotool_windows("dunst-in-gtk itest")[0]
            x, y, w, h = geoms[0]

            before = visible_window_ids()
            subprocess.run(["xdotool", "mousemove", "--window", wid, str(w // 2), str(h // 2)])
            subprocess.run(["xdotool", "click", "3"])

            # The popover is a new visible X window (a 1x1 input-only sibling
            # also appears; pick the one with real geometry).
            popover = None
            deadline = time.monotonic() + 3.0
            while time.monotonic() < deadline:
                for w in visible_window_ids() - before:
                    px, py, pw_, ph_ = window_geoms_of(w)
                    if pw_ > 1 and ph_ > 1:
                        popover = w
                        break
                if popover:
                    break
                time.sleep(0.1)
            if popover is None:
                fail("context menu (popover) did not appear")
            log(f"    popover window: {popover}")

            # Click the second item ("Other"). Button heights are derived from
            # the popover height: 3 items (Open/Other/Close) in a 6 px-margin
            # box with 2 px spacing.
            n_items = 3
            margin, spacing = 6, 2
            btn_h = (ph_ - 2 * margin - spacing * (n_items - 1)) / n_items
            target_y = py + margin + (btn_h + spacing) + btn_h / 2  # item index 1
            click_x, click_y = px + pw_ // 2, int(target_y)
            log(f"    clicking popover item at ({click_x}, {click_y}); popover=({px},{py},{pw_}x{ph_}) btn_h={btn_h:.0f}")
            subprocess.run(["xdotool", "mousemove", str(click_x), str(click_y)])
            time.sleep(0.2)
            subprocess.run(["xdotool", "click", "1"])
            try:
                msg = conn.recv_until_filtered(matches, timeout=3.0)
            except TimeoutError:
                fail("ActionInvoked not observed after menu item click")
            if msg.body != (nid, "other"):
                fail(f"expected ActionInvoked({nid}, 'other'), got {msg.body}")
        if not wait_no_window("dunst-in-gtk itest", timeout=3.0):
            fail("notification did not close after the menu action")
    finally:
        daemon.stop()
    pass_(f"context menu item click invokes its action (id={nid})")


# --------------------------------------------------------------------- driver

def run_tests(binary):
    os.environ.setdefault("RUST_LOG", "debug")
    daemon = Daemon(binary, os.path.join(os.environ.get("TMPDIR", "/tmp"), "dig-test.log"))
    conn = open_dbus_connection(bus="SESSION")
    try:
        daemon.start()
        test_show_close(daemon, conn)
        test_expiry(daemon, conn)
        test_notify_send(daemon, conn)
        # These tests run their own daemon, so free the bus name first.
        daemon.stop()
        test_layout(binary, conn)
        test_hidpi(binary, conn)
        daemon.start()
        test_hover_pauses_timeout(daemon, conn)
        test_middle_click_closes(daemon, conn)
        daemon.stop()
        test_left_click_default_action(binary, conn)
        test_context_menu(binary, conn)
        test_name_conflict(binary, conn)
        daemon.start()
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
