# dunst-in-gtk

A [dunst](https://dunst-project.org/)-compatible desktop notification daemon written in Rust, rendering notifications with GTK3.

- **Protocol compatible**: `org.freedesktop.Notifications` (Notify / CloseNotification / GetCapabilities / GetServerInformation) plus the dunst extension interface `org.dunstproject.cmd0` (works with `dunstctl` out of the box)
- **Config compatible**: reads dunst's `dunstrc` (both the new `width/height/origin/offset` format and the legacy `geometry` format)
- **Rendering**: borderless GTK3 windows with official EWMH hints (`type_hint` / `keep_above` / `accept_focus`, …), corner-stacking layout, icons (theme / file), Pango markup, progress bars, action menus

## Building

Requires Rust (≥ 1.75) and GTK3 development libraries (≥ 3.24, long-term maintained on Ubuntu/Debian).

```bash
# Debian/Ubuntu
sudo apt install libgtk-3-dev
# Fedora
sudo dnf install gtk3-devel

cargo build --release
# Binary: target/release/dunst-in-gtk
```

## Running

```bash
# Run directly (without arguments the config is looked up at
#   $XDG_CONFIG_HOME/dunst/dunstrc → ~/.config/dunst/dunstrc)
target/release/dunst-in-gtk

# With an explicit config file
target/release/dunst-in-gtk -config /path/to/dunstrc

# Usage
dunst-in-gtk [-config <dunstrc>]
```

- No systemd unit or autostart entry needed: just add the command to your session startup
- If the `org.freedesktop.Notifications` bus name is already taken (another notification daemon is running), the program exits quietly with code 0
- It also runs without any config file (built-in defaults); see [`dunstrc.example`](dunstrc.example) for the full key list with defaults

## Switching from dunst

```bash
# Stop dunst
killall dunst

# Start this daemon (it reuses your ~/.config/dunst/dunstrc)
target/release/dunst-in-gtk &

# Switch back
killall dunst-in-gtk
dunst &
```

End-to-end check with `notify-send`:

```bash
notify-send "hello" "world"
notify-send -u critical "important" "urgent message"
notify-send --hint int:value:30 "download" "in progress…"          # progress bar
notify-send --hint string:image-path:/path/to/pic.png "picture"     # file icon
```

## dunstctl compatibility

The `org.dunstproject.cmd0` interface is implemented and the system `dunstctl` script works directly. Verified commands:

| Command | Description |
|---------|-------------|
| `dunstctl count` | displayed / waiting / history counts |
| `dunstctl history` | notification history list (aa{sv}, JSON via busctl, dunst-compatible fields) |
| `dunstctl history-pop [id]` | re-display the latest (or a given) history notification |
| `dunstctl history-rm <id>` | remove from history |
| `dunstctl history-clear` | clear history |
| `dunstctl close` / `close-all` | close the latest / all notifications |
| `dunstctl action [id]` | invoke a notification action |
| `dunstctl context` | pop up the action menu |
| `dunstctl is-paused` / `set-paused` | do-not-disturb state |
| `dunstctl reload` | reload the config and re-style displayed notifications |

Not implemented (the local dunstctl version has no subcommands for them, and the cmd0 methods are missing too): `ping`, `debug`, `stack`, `rule`, `mouse`, `color`.

## Supported dunstrc keys

See [`dunstrc.example`](dunstrc.example) (every key documented with its default and allowed values). Coverage:

- Layout: `width` / `height` / `origin` / `offset` / `gap_size` (plus legacy `geometry`)
- Appearance: `font` / `background` / `foreground` / `frame_color` / `corner_radius` / `frame_width` / `transparency` / `markup` / `word_wrap` / `ellipsize` / `alignment` / `vertical_alignment`
- Icons: `icons` / `icon_position` / `min_icon_size` / `max_icon_size` / `padding` / `horizontal_padding` / `text_icon_padding`
- Progress bar: `progress_bar` / `progress_bar_height` / `progress_bar_frame_width` / `progress_bar_min_width` / `progress_bar_max_width`
- Behavior: `history_length` / `notification_limit` / `monitor` / `follow` / `mouse_left_click` / `mouse_middle_click` / `mouse_right_click` / `timeout` (per urgency section)
- Urgency sections `[urgency_low]` / `[urgency_normal]` / `[urgency_critical]` override colors and timeouts

Unknown keys and unimplemented sections like `[shortcuts]` / `[rules]` produce a warning in the log but do not prevent startup.

## Known limitations

- X11 only (on Wayland, GTK3 window positioning and EWMH hints are not supported; it can run under XWayland)
- GTK3 is no longer developed upstream (long-term maintained by Ubuntu); it was chosen because it keeps the full set of official window-hint APIs (GTK4 removed them, which made notifications steal the keyboard focus on i3 and forced positioning/keep-above through raw xcb)
- Xvfb lacks RandR 1.5, so a dual-monitor layout cannot be constructed; the `monitor` number/name/out-of-range-fallback/`follow = mouse` selection paths are covered by single-screen integration tests — on real multi-monitor setups, rely on your config
- The `icon-data` / `icon_data` hint (inline image data) is not implemented; `image-path` / `image_path` is supported

## Development

```bash
# Unit tests (parser / layout / queue / state machine / style)
cargo test

# Integration tests (Xvfb + dbus-run-session + xdotool, driving D-Bus end to end)
tests/integration.py
```

Integration coverage: popup/close/signals, expiry, queue & do-not-disturb, replaces_id, corner stacking & reflow, HiDPI geometry, icons/markup/progress bar (with GDK_SCALE=2 pixel assertions), mouse interaction, action menus, history, dunstctl properties.

---

Chinese version: [README.zh-CN.md](README.zh-CN.md)
