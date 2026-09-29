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
import re
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

def notify(conn, app, summary, body, timeout_ms, actions=None, urgency=None, replaces_id=0,
           icon=None, hints=None):
    """Call Notify; returns the notification id.

    `icon` overrides the default app_icon; `hints` merges extra
    (sig, value) pairs on top of the urgency hint.
    """
    hints = dict(hints or {})
    if urgency is not None:
        hints["urgency"] = ("y", urgency)
    msg = new_method_call(
        NOTIF_ADDR,
        "Notify",
        "susssasa{sv}i",
        (app, replaces_id, icon if icon is not None else "dialog-information", summary, body, actions or [], hints, timeout_ms),
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
    with signal_filter(conn, rule) as matches:
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


def xrandr_monitors():
    """[(name, x, y, w, h, primary)] for every connected output (X11).

    The geometry assertions below are derived from this instead of hardcoded
    Xvfb coordinates, so the suite also runs against a real multi-monitor
    session (`DISPLAY=:0 ... --inside`).
    """
    out = subprocess.run(["xrandr", "--query"], capture_output=True, text=True).stdout
    mons = []
    for line in out.splitlines():
        m = re.match(
            r"^(\S+) connected (primary )?(\d+)x(\d+)\+(-?\d+)\+(-?\d+)", line
        )
        if m:
            mons.append(
                (
                    m.group(1),
                    int(m.group(5)),
                    int(m.group(6)),
                    int(m.group(3)),
                    int(m.group(4)),
                    bool(m.group(2)),
                )
            )
    return mons


def monitor_with_right_edge(right, top):
    """(x, y, w, h) of the monitor whose right edge/top are `right`/`top`."""
    for _name, x, y, w, h, _primary in xrandr_monitors():
        if x + w == right and y == top:
            return (x, y, w, h)
    return None


def monitor_at_origin(x, y):
    """(x, y, w, h) of the monitor whose top-left is exactly (x, y)."""
    for _name, mx, my, w, h, _primary in xrandr_monitors():
        if mx == x and my == y:
            return (mx, my, w, h)
    return None


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
        if w0 != 200:
            fail(f"expected width 200 (width spec), got {geoms}")
        # The daemon places on a monitor (default: monitor 0, whatever GDK
        # reports); identify it from the placement itself so the assertion
        # holds on any display size / monitor count.
        mon = monitor_with_right_edge(x0 + w0 + 10, y0 - 10)
        if mon is None:
            fail(f"expected top-right of a monitor with offset 10, got {geoms}")
        mon_x, mon_y, mon_w, _mon_h = mon
        if (x0, y0) != (mon_x + mon_w - 10 - 200, mon_y + 10):
            fail(f"expected top-right at x={mon_x + mon_w - 210} y={mon_y + 10}, got {geoms}")

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
        if len(geoms) != 1 or geoms[0][1] != mon_y + 10:
            fail(f"expected reflow to y={mon_y + 10}, got {geoms}")

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
        # With GDK_SCALE=2 the logical monitor space is the xrandr rect / 2,
        # so the physical top-right is still `right_edge - (10+200)*2`.
        if w != 400:
            fail(f"expected physical width 400 for logical 200@scale2, got {geoms}")
        mon = monitor_with_right_edge(x + 420, y - 20)
        if mon is None:
            fail(f"expected top-right of a monitor (scale 2), got {geoms}")
        mon_x, mon_y, mon_w, _mon_h = mon
        expected = (mon_x + mon_w - 420, mon_y + 20)
        if (x, y) != expected:
            fail(f"expected physical {expected} for logical 200@scale2, got {geoms}")
    finally:
        daemon.stop()
    pass_("GDK_SCALE=2 doubles the physical size and offsets")


# ------------------------------------------------------- ticket 07 (icons/markup/progress)

T07_DUNSTRC = """
[global]
origin = top-left
offset = (10, 10)
width = (0, 400)
height = (0, 800)
icons = yes
icon_position = left
min_icon_size = 48
max_icon_size = 48
progress_bar = yes
progress_bar_height = 10
progress_bar_min_width = 150
markup = yes
"""

T07_MARKUP_OFF_DUNSTRC = T07_DUNSTRC + "markup = no\n"


def shot(path):
    """Grab the root window to `path`."""
    subprocess.run(["import", "-window", "root", path], check=True)


def window_origin(title="dunst-in-gtk t07"):
    """Top-left of the first matching window (pixel scans are cropped here).

    The notification is not necessarily at the root window's top-left corner:
    on a real multi-monitor session the selected monitor can start at any
    offset, so every pixel scan is anchored at the window itself."""
    return window_box(title)[:2]


def window_box(title="dunst-in-gtk t07"):
    """(x, y, w, h) of the first matching window.

    Pixel scans must also be *limited* to the window: scanning past its edges
    picks up the desktop/wallpaper (which made a "no icon" check fail on a
    machine with a colored wallpaper)."""
    geoms = window_geoms(title)
    if not geoms:
        fail(f"window {title!r} not found for pixel scan")
    return geoms[0]


def window_pixels(path, xmax=500, ymax=400, origin=(0, 0)):
    """Return (w, h, px) of the root screenshot cropped at `origin`.

    px[y][x] is a (r, g, b) tuple relative to `origin` (the notification
    window's top-left corner)."""
    from PIL import Image

    im = Image.open(path).convert("RGB")
    im = im.crop((origin[0], origin[1], origin[0] + xmax, origin[1] + ymax))
    w, h = im.size
    return w, h, im.load()


def is_colorful(rgb, min_delta=60):
    """Clearly colored pixel (not the gray notification background / black
    text)."""
    r, g, b = rgb
    return max(r, g, b) - min(r, g, b) >= min_delta


def count_colorful(px, xmax, ymax, min_delta=60):
    """Count colored pixels.

    Icon assertions must not depend on the developer's icon theme: a retro
    theme can ship a completely different "firefox" glyph (Chicago95 does, for
    example), so the tests check that an icon *graphic* was drawn (saturated
    pixels) rather than a specific brand color. The letter placeholder shown
    for a missing icon is black on the notification background, i.e.
    unsaturated, so it does not count."""
    n = 0
    for y in range(0, ymax):
        for x in range(0, xmax):
            if is_colorful(px[x, y], min_delta):
                n += 1
    return n


def diff_pixels(path_a, path_b, xmax=500, ymax=400, step=1, origin=(0, 0)):
    """Count pixels that differ between two screenshots (scaled: step 1)."""
    from PIL import Image

    box = (origin[0], origin[1], origin[0] + xmax, origin[1] + ymax)
    a = Image.open(path_a).convert("RGB").crop(box).load()
    b = Image.open(path_b).convert("RGB").crop(box).load()
    n = 0
    for y in range(0, ymax, step):
        for x in range(0, xmax, step):
            if a[x, y] != b[x, y]:
                n += 1
    return n


def test_icons_markup_progress(binary, conn):
    log("== test: icons, markup, progress bar (ticket 07) ==")
    tmp = os.environ.get("TMPDIR", "/tmp")
    cfg_path = os.path.join(tmp, "dig-t07-dunstrc")
    cfg_off_path = os.path.join(tmp, "dig-t07-markup-off-dunstrc")
    with open(cfg_path, "w") as f:
        f.write(T07_DUNSTRC)
    with open(cfg_off_path, "w") as f:
        f.write(T07_MARKUP_OFF_DUNSTRC)

    daemon = Daemon(binary, os.path.join(tmp, "dig-t07.log"))
    try:
        daemon.start(args=["-config", cfg_path])
        if not wait_until_name_owned(conn, timeout=5.0):
            fail("t07 daemon did not acquire the bus name")

        # --- 1. themed icon renders (theme-independent: an icon graphic is
        #        drawn, whatever glyph the active icon theme provides) ---
        nid = notify(conn, "t07", "icon", "body", 5000, icon="firefox")
        if not wait_for_window("dunst-in-gtk t07"):
            fail("icon notification window missing")
        time.sleep(0.4)
        shot(os.path.join(tmp, "t07-icon.png"))
        bx, by, bw, bh = window_box()
        _, _, px = window_pixels(os.path.join(tmp, "t07-icon.png"),
                                 xmax=bw, ymax=bh, origin=(bx, by))
        if count_colorful(px, bw, bh) < 40:
            fail("theme icon did not render (no icon graphic drawn)")
        close_notification(conn, nid)
        wait_no_window("dunst-in-gtk t07")

        # --- 2. missing icon name -> placeholder letter (taller window) ---
        nid = notify(conn, "t07", "noicon", "body", 5000, icon="")
        wait_for_window("dunst-in-gtk t07")
        time.sleep(0.4)
        shot(os.path.join(tmp, "t07-noicon.png"))
        bx, by, bw, bh = window_box()
        _, _, px = window_pixels(os.path.join(tmp, "t07-noicon.png"),
                                 xmax=bw, ymax=bh, origin=(bx, by))
        _, _, noicon_w, noicon_h = window_box()
        # No icon requested: the icon *column* (left 48px + padding) must stay
        # free of any icon graphic (text is unsaturated, so it does not count).
        if count_colorful(px, min(60, bw), bh) != 0:
            fail("no-icon notification must not draw an icon")
        close_notification(conn, nid)
        wait_no_window("dunst-in-gtk t07")

        # An unknown icon name must still produce an icon column: the theme
        # fallback icon ("dialog-information") or, when even that is missing,
        # the letter placeholder. Which of the two it is depends on the icon
        # theme, so assert the layout effect (a wider window) instead of a
        # specific glyph. icon_position = left puts the icon beside the text,
        # so the column widens the window rather than making it taller.
        nid = notify(conn, "t07", "missing", "body", 5000,
                     icon="definitely-not-an-icon-name-xyz")
        wait_for_window("dunst-in-gtk t07")
        time.sleep(0.4)
        _mx, _my, miss_w, _mh = window_box()
        close_notification(conn, nid)
        wait_no_window("dunst-in-gtk t07")
        if miss_w < noicon_w + 20:
            fail(
                f"icon fallback should keep an icon column "
                f"(window width {miss_w} vs no-icon {noicon_w})"
            )

        # --- 3. progress bar from the `value` hint; replaces_id updates it ---
        nid = notify(conn, "t07", "progress", "body", 5000, icon="", hints={"value": ("i", 10)})
        wait_for_window("dunst-in-gtk t07")
        time.sleep(0.4)
        shot(os.path.join(tmp, "t07-prog10.png"))
        bx, by, bw, bh = window_box()
        _, _, px = window_pixels(os.path.join(tmp, "t07-prog10.png"),
                                 xmax=bw, ymax=bh, origin=(bx, by))
        _, _, _pw, prog_h = window_box()
        # Progress bar adds height beyond the plain text window.
        if prog_h <= noicon_h + 8:
            fail(f"value hint should add a progress bar (height {prog_h} vs plain {noicon_h})")

        # replaces_id: same id, value 10 -> 90; no new window, bar changes.
        nid2 = notify(conn, "t07", "progress", "body", 5000, icon="",
                      replaces_id=nid, hints={"value": ("i", 90)})
        if nid2 != nid:
            fail(f"replaces_id should keep the id, got {nid} -> {nid2}")
        if len(xdotool_windows("dunst-in-gtk t07")) != 1:
            fail("replaces_id update opened a new window")
        time.sleep(0.4)
        shot(os.path.join(tmp, "t07-prog90.png"))
        # The filled bar length changes 10% -> 90%: real pixel difference.
        bx, by, bw, bh = window_box()
        d = diff_pixels(os.path.join(tmp, "t07-prog10.png"), os.path.join(tmp, "t07-prog90.png"),
                        xmax=bw, ymax=bh, origin=(bx, by))
        if d < 80:
            fail(f"progress bar did not visually update on replaces_id (diff={d} px)")
        close_notification(conn, nid)
        wait_no_window("dunst-in-gtk t07")
    finally:
        daemon.stop()

    # --- 4. markup=no renders the literal text (wider window) ---
    body_markup = "<b>BOLD</b>"
    try:
        daemon = Daemon(binary, os.path.join(tmp, "dig-t07-off.log"))
        daemon.start(args=["-config", cfg_off_path])
        if not wait_until_name_owned(conn, timeout=5.0):
            fail("t07 markup-off daemon did not acquire the bus name")
        nid = notify(conn, "t07", "markup", body_markup, 5000, icon="")
        wait_for_window("dunst-in-gtk t07")
        time.sleep(0.4)
        w_off = window_geoms("dunst-in-gtk t07")[0][2]
        close_notification(conn, nid)
        wait_no_window("dunst-in-gtk t07")
    finally:
        daemon.stop()

    try:
        daemon = Daemon(binary, os.path.join(tmp, "dig-t07-on.log"))
        daemon.start(args=["-config", cfg_path])
        if not wait_until_name_owned(conn, timeout=5.0):
            fail("t07 markup daemon did not acquire the bus name")
        nid = notify(conn, "t07", "markup", body_markup, 5000, icon="")
        wait_for_window("dunst-in-gtk t07")
        time.sleep(0.4)
        w_on = window_geoms("dunst-in-gtk t07")[0][2]
        close_notification(conn, nid)
        wait_no_window("dunst-in-gtk t07")
        if w_off <= w_on:
            fail(f"markup=no should render literal <b> tags (wider), got {w_off} vs {w_on}")
    finally:
        daemon.stop()
    pass_(
        f"icons (themed+placeholder), markup yes/no ({w_on}px vs {w_off}px), "
        "progress bar + replaces_id update verified"
    )


def icon_band_height(path, xmax=300, ymax=200, origin=(0, 0)):
    """Y-extent of the colored icon pixels in a screenshot.

    An icon does not fill its square, and which glyph it is depends on the
    icon theme, so the HiDPI test compares the scale-2 band height against
    the scale-1 band height (≈2x) instead of asserting absolute pixels.
    """
    _, _, px = window_pixels(path, xmax, ymax, origin=origin)
    ys = [y for y in range(ymax) for x in range(xmax) if is_colorful(px[x, y])]
    if not ys:
        return 0
    return max(ys) - min(ys) + 1


def test_icons_hidpi(binary, conn):
    """GDK_SCALE=2: the 48px-logical icon renders at 2x the physical size
    (icon band height doubles from scale 1 to scale 2)."""
    log("== test: icon HiDPI scaling (GDK_SCALE=2) ==")
    tmp = os.environ.get("TMPDIR", "/tmp")
    cfg_path = os.path.join(tmp, "dig-t07-dunstrc")
    heights = {}
    for scale in ("1", "2"):
        daemon = Daemon(binary, os.path.join(tmp, f"dig-t07-hidpi{scale}.log"))
        try:
            daemon.start(args=["-config", cfg_path], env={"GDK_SCALE": scale})
            if not wait_until_name_owned(conn, timeout=5.0):
                fail(f"t07 hidpi daemon (scale {scale}) did not acquire the bus name")
            nid = notify(conn, "t07", "icon", "body", 5000, icon="firefox")
            wait_for_window("dunst-in-gtk t07")
            time.sleep(0.4)
            path = os.path.join(tmp, f"t07-icon{scale}x.png")
            shot(path)
            # The window geometry must be read while it is still mapped.
            bx, by, bw, bh = window_box()
            close_notification(conn, nid)
            wait_no_window("dunst-in-gtk t07")
            heights[scale] = icon_band_height(path, xmax=bw, ymax=bh, origin=(bx, by))
        finally:
            daemon.stop()
    h1, h2 = heights.get("1", 0), heights.get("2", 0)
    if h1 < 15:
        fail(f"no icon pixels at scale 1 (band={h1})")
    if not (h2 >= 1.6 * h1 and h2 >= h1 + 15):
        fail(f"icon should render ~2x physical size at scale 2, got {h1} -> {h2}")
    pass_(f"icon renders at 2x physical size under GDK_SCALE=2 ({h1}px -> {h2}px)")


def test_monitor_selection(binary, conn):
    """resolve_monitor paths: monitor number, out-of-range fallback,
    monitor name, follow=mouse.

    Under Xvfb the RandR version is < 1.5, so no real dual-monitor layout can
    be created; every selection path is exercised and must land on the
    top-left corner of *some* monitor (on the real session: the selected one).
    The name case pins a connector reported by xrandr.
    """
    log("== test: monitor selection paths (single screen) ==")
    tmp = os.environ.get("TMPDIR", "/tmp")
    # NOTE: the placeholder is the whole key=value line; a previous version
    # had "monitor = {monitor}" here while the cases already carried
    # "monitor = ...", producing "monitor = monitor = 0" — the value failed to
    # parse and every case silently ran with the default monitor, so the name
    # path was never actually exercised.
    base = """
[global]
origin = top-left
offset = (10, 10)
width = (200, 400)
{monitor}
"""
    cases = [
        ("number-0", "monitor = 0"),
        ("number-oob", "monitor = 99"),  # falls back to the primary monitor
        ("follow-mouse", "follow = mouse"),
    ]
    for name, mon_line in cases:
        cfg_path = os.path.join(tmp, f"dig-mon-{name}-dunstrc")
        with open(cfg_path, "w") as f:
            f.write(base.format(monitor=mon_line))
        daemon = Daemon(binary, os.path.join(tmp, f"dig-mon-{name}.log"))
        try:
            daemon.start(args=["-config", cfg_path])
            if not wait_until_name_owned(conn, timeout=5.0):
                fail(f"monitor test {name}: daemon did not acquire the bus name")
            nid = notify(conn, "mon", name, "body", 5000)
            geoms = wait_window_geometry("dunst-in-gtk mon", 1)
            if len(geoms) != 1:
                fail(f"monitor test {name}: window missing")
            x, y, w, h = geoms[0]
            if w != 200:
                fail(f"monitor test {name}: expected width 200, got {geoms}")
            # The selection paths (number / out-of-range / follow=mouse) may
            # resolve to any monitor; require the top-left corner of *some*
            # monitor plus the configured offset.
            mon = monitor_at_origin(x - 10, y - 10)
            if mon is None:
                fail(f"monitor test {name}: expected top-left of a monitor, got {geoms}")
            close_notification(conn, nid)
            wait_no_window("dunst-in-gtk mon")
        finally:
            daemon.stop()
    # Monitor *name*: pin the config to a connector xrandr reports and
    # require the window on exactly that monitor.
    mons = xrandr_monitors()
    if not mons:
        fail("no monitors reported by xrandr")
    mon_name, mon_x, mon_y, _mon_w, _mon_h, _primary = mons[0]
    cfg_path = os.path.join(tmp, "dig-mon-name-dunstrc")
    with open(cfg_path, "w") as f:
        f.write(base.format(monitor=f"monitor = {mon_name}"))
    daemon = Daemon(binary, os.path.join(tmp, "dig-mon-name.log"))
    try:
        daemon.start(args=["-config", cfg_path])
        if not wait_until_name_owned(conn, timeout=5.0):
            fail("monitor-name daemon did not acquire the bus name")
        nid = notify(conn, "mon", "name", "body", 5000)
        geoms = wait_window_geometry("dunst-in-gtk mon", 1)
        if len(geoms) != 1 or geoms[0][:2] != (mon_x + 10, mon_y + 10):
            fail(f"monitor name {mon_name!r}: expected ({mon_x + 10},{mon_y + 10}), got {geoms}")
        close_notification(conn, nid)
        wait_no_window("dunst-in-gtk mon")
    finally:
        daemon.stop()
    pass_(f"monitor number / out-of-range / name({mon_name}) / follow=mouse all resolve")



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
    with signal_filter(conn, rule) as matches:
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
    with signal_filter(conn, rule) as matches:
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
        with signal_filter(conn, rule) as matches:
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
    with signal_filter(conn, rule) as matches:
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
        with signal_filter(conn, rule) as matches:
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

            # Activate the second item ("Other") via keyboard navigation
            # (robust: no geometry guessing). GtkMenu grabs the keyboard;
            # Down moves the selection, Return activates.
            time.sleep(0.3)
            subprocess.run(["xdotool", "key", "Down"])
            time.sleep(0.15)
            subprocess.run(["xdotool", "key", "Down"])
            time.sleep(0.15)
            subprocess.run(["xdotool", "key", "Return"])
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


LIMIT_DUNSTRC = """
[global]
notification_limit = 1
"""


def props_get(prop):
    out = subprocess.run(
        [
            "gdbus", "call", "--session",
            "--dest", "org.freedesktop.Notifications",
            "--object-path", "/org/freedesktop/Notifications",
            "--method", "org.freedesktop.DBus.Properties.Get",
            "org.dunstproject.cmd0", prop,
        ],
        capture_output=True,
        text=True,
    )
    return out.stdout.strip()


def props_set(prop, value):
    subprocess.run(
        [
            "gdbus", "call", "--session",
            "--dest", "org.freedesktop.Notifications",
            "--object-path", "/org/freedesktop/Notifications",
            "--method", "org.freedesktop.DBus.Properties.Set",
            "org.dunstproject.cmd0", prop, value,
        ],
        check=True,
        capture_output=True,
        text=True,
    )


def test_replaces_id(daemon, conn):
    log("== test: replaces_id updates in place ==")
    nid = notify(conn, "itest", "Old", "old body", 5000)
    geoms = wait_window_geometry("dunst-in-gtk itest", 1)
    if len(geoms) != 1:
        fail("replace test: initial window missing")

    # Replace with the same id: no second window should appear.
    reply_id = notify(conn, "itest", "New", "new body", 5000, replaces_id=nid)
    if reply_id != nid:
        fail(f"replace: Notify returned {reply_id}, expected the replaces_id {nid}")
    time.sleep(0.8)
    if len(xdotool_windows("dunst-in-gtk itest")) != 1:
        fail("replace: expected exactly one window after replaces_id")
    pass_(f"replaces_id updates in place, one window (id={nid})")


def test_queue_limit(binary, conn):
    log("== test: notification_limit queues and promotes ==")
    cfg_path = os.path.join(os.environ.get("TMPDIR", "/tmp"), "dig-limit-dunstrc")
    with open(cfg_path, "w") as f:
        f.write(LIMIT_DUNSTRC)
    daemon = Daemon(binary, os.path.join(os.environ.get("TMPDIR", "/tmp"), "dig-limit.log"))
    try:
        daemon.start(args=["-config", cfg_path])
        if not wait_until_name_owned(conn, timeout=5.0, pid=daemon.proc.pid):
            fail("limit daemon did not acquire the bus name")

        n1 = notify(conn, "itest", "First", "shown", 5000)
        if not wait_window_count("dunst-in-gtk itest", 1):
            fail("first notification not displayed")
        n2 = notify(conn, "itest", "Second", "waiting", 5000)
        time.sleep(0.5)
        if xdotool_windows("dunst-in-gtk itest"):
            # still exactly one window (the first)
            if len(xdotool_windows("dunst-in-gtk itest")) != 1:
                fail("second notification displayed despite the limit")
        if "uint32 1" not in props_get("waitingLength"):
            fail(f"expected waitingLength 1, got {props_get('waitingLength')}")
        if "uint32 1" not in props_get("displayedLength"):
            fail(f"expected displayedLength 1, got {props_get('displayedLength')}")

        # Closing the first promotes the second.
        close_notification(conn, n1)
        if not wait_window_count("dunst-in-gtk itest", 1):
            fail("promoted notification did not display")
        if "uint32 0" not in props_get("waitingLength"):
            fail(f"expected waitingLength 0 after promotion, got {props_get('waitingLength')}")
        close_notification(conn, n2)
    finally:
        daemon.stop()
    pass_(f"notification_limit queues excess and promotes on close ({n1=} {n2=})")


def test_do_not_disturb(binary, conn):
    log("== test: do-not-disturb (pause level) ==")
    daemon = Daemon(binary, os.path.join(os.environ.get("TMPDIR", "/tmp"), "dig-dnd.log"))
    try:
        daemon.start()
        if not wait_until_name_owned(conn, timeout=5.0, pid=daemon.proc.pid):
            fail("dnd daemon did not acquire the bus name")

        props_set("pauseLevel", "<uint32 1>")
        if "true" not in props_get("paused"):
            fail(f"expected paused=true after pauseLevel=1, got {props_get('paused')}")

        nid = notify(conn, "itest", "Quiet", "not now", 5000)
        time.sleep(0.6)
        if xdotool_windows("dunst-in-gtk itest"):
            fail("notification displayed while paused")
        if "uint32 1" not in props_get("waitingLength"):
            fail(f"expected waitingLength 1 while paused, got {props_get('waitingLength')}")

        # Unpause: the waiting notification appears.
        props_set("pauseLevel", "<uint32 0>")
        if not wait_window_count("dunst-in-gtk itest", 1):
            fail("waiting notification did not display after unpause")
        if "uint32 0" not in props_get("waitingLength"):
            fail(f"expected waitingLength 0 after unpause, got {props_get('waitingLength')}")
        close_notification(conn, nid)
    finally:
        daemon.stop()
    pass_("do-not-disturb queues notifications; unpause promotes them")


CMD0_ADDR = DBusAddress(
    "/org/freedesktop/Notifications",
    bus_name="org.freedesktop.Notifications",
    interface="org.dunstproject.cmd0",
)


def cmd0_call(conn, method, sig="", body=()):
    return conn.send_and_get_reply(new_method_call(CMD0_ADDR, method, sig, body), timeout=10)


def signal_filter(conn, rule):
    """jeepney filters are client-side only; broadcast signals need a
    bus-side AddMatch, otherwise only directed signals arrive."""
    try:
        conn.bus_proxy.AddMatch(rule)
    except Exception:
        pass  # a duplicate match rule is harmless
    return conn.filter(rule)


def test_history(binary, conn):
    log("== test: history (list/show/clear/remove) ==")
    daemon = Daemon(binary, os.path.join(os.environ.get("TMPDIR", "/tmp"), "dig-hist.log"))
    try:
        daemon.start()
        if not wait_until_name_owned(conn, timeout=5.0, pid=daemon.proc.pid):
            fail("history daemon did not acquire the bus name")

        # Two notifications, closed by timeout, both land in history.
        n1 = notify(conn, "itest", "HistA", "aaa", 500)
        n2 = notify(conn, "itest", "HistB", "bbb", 500)
        if not wait_window_count("dunst-in-gtk itest", 2, timeout=5.0):
            n = len(xdotool_windows("dunst-in-gtk itest"))
            tree = subprocess.run(["xwininfo", "-root", "-tree"], capture_output=True, text=True).stdout
            log(f"    DEBUG: only {n} window(s) found; tree tail:")
            for line in tree.splitlines()[-6:]:
                log(f"    DEBUG: {line}")
            fail("history test windows did not appear")
        if not wait_no_window("dunst-in-gtk itest", timeout=5.0):
            fail("history test notifications did not expire")
        time.sleep(0.3)

        hist = cmd0_call(conn, "NotificationListHistory").body[0]
        log(f"    history entries: {len(hist)}, historyLength={props_get('historyLength')}, n1={n1} n2={n2}")
        if len(hist) != 2:
            fail(f"expected 2 history entries, got {len(hist)}")
        # jeepney decodes a{sv} values as (signature, value) tuples.
        by_id = {entry["id"][1]: entry for entry in hist}
        if by_id.get(n1, {}).get("summary", (None, None))[1] != "HistA" \
                or by_id.get(n2, {}).get("summary", (None, None))[1] != "HistB":
            fail(f"history entries mismatch: {hist}")

        # Show re-displays the newest entry.
        cmd0_call(conn, "NotificationShow")
        if not wait_window_count("dunst-in-gtk itest", 1):
            fail("history show did not display a window")
        close_notification(conn, n2)  # the re-displayed one has the original id

        # Remove + its signal.
        rule = MatchRule(type="signal", interface="org.dunstproject.cmd0", member="NotificationHistoryRemoved")
        queue = deque()
        with signal_filter(conn, rule) as matches:
            cmd0_call(conn, "NotificationRemoveFromHistory", "u", (n1,))
            msg = conn.recv_until_filtered(matches, timeout=3.0)
            if msg.body[0] != n1:
                fail(f"NotificationHistoryRemoved({n1}) expected, got {msg.body}")

        # Clear + its signal + counter.
        rule = MatchRule(type="signal", interface="org.dunstproject.cmd0", member="NotificationHistoryCleared")
        queue = deque()
        with signal_filter(conn, rule) as matches:
            cmd0_call(conn, "NotificationClearHistory")
            msg = conn.recv_until_filtered(matches, timeout=3.0)
            if msg.body[0] < 1:
                fail(f"NotificationHistoryCleared count expected >=1, got {msg.body}")
        if "uint32 0" not in props_get("historyLength"):
            fail(f"expected historyLength 0 after clear, got {props_get('historyLength')}")
    finally:
        daemon.stop()
    pass_(f"history list/show/remove/clear verified ({n1=} {n2=})")


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
        test_icons_markup_progress(binary, conn)
        test_icons_hidpi(binary, conn)
        test_monitor_selection(binary, conn)
        daemon.start()
        test_hover_pauses_timeout(daemon, conn)
        test_middle_click_closes(daemon, conn)
        daemon.stop()
        test_left_click_default_action(binary, conn)
        test_context_menu(binary, conn)
        daemon.start()
        test_replaces_id(daemon, conn)
        daemon.stop()
        test_queue_limit(binary, conn)
        test_do_not_disturb(binary, conn)
        test_history(binary, conn)
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
