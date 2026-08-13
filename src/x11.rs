//! X11 integration for notification windows.
//!
//! GTK4 removed all programmatic toplevel positioning (`gtk_window_move`,
//! `gdk_toplevel_move`, even `GdkToplevelLayout` position fields) and the
//! legacy window hints (`keep_above`, `skip_taskbar`, `type_hint`); gtk4-rs
//! 0.11 additionally dropped `gdk::x11::X11Surface`. Windows are positioned
//! and hinted by the window manager — except that a notification daemon needs
//! precise corner placement, so we talk to the X11 server directly (like dunst
//! does): find our own windows by `_NET_WM_PID` (title fallback) and
//!   - configure their position/size before the map is processed, and
//!   - set the EWMH hints `_NET_WM_WINDOW_TYPE_NOTIFICATION`,
//!     `_NET_WM_STATE_ABOVE | SKIP_TASKBAR | SKIP_PAGER`.
//!
//! `apply_window_hints_and_position` must run after the window is realized
//! (the X window exists) and before `present()` is called / the main loop
//! iterates (so the requests land on the server before GTK's map).

use std::sync::Mutex;

use gtk4 as gtk;
use gtk4::prelude::*;
use xcb::x;

fn with_conn<F: FnOnce(&xcb::Connection) -> R, R>(f: F) -> Option<R> {
    static XCB: Mutex<Option<xcb::Connection>> = Mutex::new(None);
    let mut guard = XCB.lock().unwrap();
    if guard.is_none() {
        match xcb::Connection::connect(None) {
            Ok((conn, _screen)) => *guard = Some(conn),
            Err(e) => {
                log::warn!("cannot open X11 connection: {e}");
                return None;
            }
        }
    }
    Some(f(guard.as_ref().unwrap()))
}

fn intern_atom(conn: &xcb::Connection, name: &str) -> Option<x::Atom> {
    let cookie = conn.send_request(&x::InternAtom {
        only_if_exists: false,
        name: name.as_bytes(),
    });
    conn.wait_for_reply(cookie)
        .ok()
        .map(|r: x::InternAtomReply| r.atom())
}

fn get_text_prop(conn: &xcb::Connection, window: x::Window, prop: x::Atom) -> Option<String> {
    // ATOM_ANY: _NET_WM_NAME is UTF8_STRING, WM_NAME is STRING; accept both.
    let cookie = conn.send_request(&x::GetProperty {
        delete: false,
        window,
        property: prop,
        r#type: x::ATOM_ANY,
        long_offset: 0,
        long_length: 1024,
    });
    let reply: x::GetPropertyReply = conn.wait_for_reply(cookie).ok()?;
    let bytes: &[u8] = reply.value();
    Some(String::from_utf8_lossy(bytes).into_owned())
}

/// Find the XID of our window with the exact `_NET_WM_NAME` title.
/// Titles are unique per notification (the id is part of the title), so this
/// targets exactly one window even when several notifications are up.
fn find_our_windows(conn: &xcb::Connection, title: &str) -> Vec<x::Window> {
    let Some(setup) = conn.get_setup().roots().next() else {
        return vec![];
    };
    let root = setup.root();
    let Ok(reply) = conn.wait_for_reply(conn.send_request(&x::QueryTree { window: root })) else {
        return vec![];
    };

    let name_atom = intern_atom(conn, "_NET_WM_NAME");
    let mut found = vec![];
    for &child in reply.children() {
        let name_match = name_atom
            .and_then(|a| get_text_prop(conn, child, a))
            .map(|name| name == title)
            .unwrap_or(false);
        if name_match {
            found.push(child);
        }
    }
    found
}

/// Position and hint a realized notification window. No-op on Wayland.
pub fn apply_window_hints_and_position(
    window: &gtk::Window,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) {
    // All inputs are logical (device-independent) pixels; the X server works
    // in physical pixels, so scale by the surface's scale factor (HiDPI).
    let scale = window.surface().map(|s| s.scale_factor()).unwrap_or(1).max(1);
    let (x, y, width, height) = (x * scale, y * scale, width * scale as u32, height * scale as u32);
    with_conn(|conn| {
        let title = window.title().unwrap_or_default();

        // The window is realized but may not have its properties up yet; poll
        // briefly. This runs before present(), so there is no visible flicker.
        let mut xids = vec![];
        for _ in 0..20 {
            xids = find_our_windows(conn, &title);
            if !xids.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        if xids.is_empty() {
            log::warn!("could not find our X11 window for hints (title prefix {title:?})");
            return;
        }

        let atoms = (
            intern_atom(conn, "_NET_WM_WINDOW_TYPE_NOTIFICATION"),
            intern_atom(conn, "_NET_WM_WINDOW_TYPE"),
            intern_atom(conn, "_NET_WM_STATE"),
            intern_atom(conn, "_NET_WM_STATE_ABOVE"),
            intern_atom(conn, "_NET_WM_STATE_SKIP_TASKBAR"),
            intern_atom(conn, "_NET_WM_STATE_SKIP_PAGER"),
        );
        let (Some(type_notification), Some(net_wm_window_type), Some(net_wm_state), Some(above), Some(skip_taskbar), Some(skip_pager)) = atoms else {
            log::warn!("cannot intern EWMH atoms");
            return;
        };

        for xid in &xids {
            // Position/size, applied before GTK's map request is processed.
            conn.send_request(&x::ConfigureWindow {
                window: *xid,
                value_list: &[
                    x::ConfigWindow::X(x),
                    x::ConfigWindow::Y(y),
                    x::ConfigWindow::Width(width),
                    x::ConfigWindow::Height(height),
                ],
            });
            // _NET_WM_WINDOW_TYPE = _NET_WM_WINDOW_TYPE_NOTIFICATION
            conn.send_request(&x::ChangeProperty {
                mode: x::PropMode::Replace,
                window: *xid,
                property: net_wm_window_type,
                r#type: x::ATOM_ATOM,
                data: &[type_notification],
            });
            // _NET_WM_STATE = ABOVE | SKIP_TASKBAR | SKIP_PAGER
            conn.send_request(&x::ChangeProperty {
                mode: x::PropMode::Replace,
                window: *xid,
                property: net_wm_state,
                r#type: x::ATOM_ATOM,
                data: &[above, skip_taskbar, skip_pager],
            });
        }
        if let Err(e) = conn.flush() {
            log::warn!("cannot flush X11 requests: {e}");
        }
        log::debug!("configured windows {xids:?} at ({x},{y}) {width}x{height}");
    });
}
