//! GTK-side notification state: the live set of notification windows and the
//! routing of D-Bus events onto the GTK main loop. Grows into the full state
//! machine (queue/timeouts/history/DnD) in ticket 05.
//!
//! Layout: every notification gets placed by the pure `layout` module against
//! the monitor resolved from the config (`follow`/`monitor`); any add/remove
//! triggers a full reflow (dunst semantics: the stack closes gaps).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::OnceLock;

use gtk4::gdk;
use gtk4::prelude::*;

use crate::config::{Config, Follow, Monitor};
use crate::dbus::DbusEvent;
use crate::layout::{resolve_size, stack_position, MonitorGeometry};
use crate::window::{emit_closed_signal, ClosedCb, NotificationWindow, WindowStyle};

// GTK-side daemon state. Lives in a thread-local: everything here is owned
// and touched exclusively by the GTK main thread (windows are !Send).
thread_local! {
    static DAEMON: RefCell<Option<Daemon>> = const { RefCell::new(None) };
}

/// Shared blocking D-Bus connection, created on the GTK thread. Used for
/// emitting signals (NotificationClosed, later ActionInvoked) straight from
/// the GTK main loop.
static CONN: OnceLock<zbus::blocking::Connection> = OnceLock::new();

pub fn connection() -> Option<&'static zbus::blocking::Connection> {
    CONN.get()
}

pub fn init(conn: zbus::blocking::Connection, config: std::sync::Arc<Config>) {
    let _ = CONN.set(conn);
    DAEMON.with(|d| {
        *d.borrow_mut() = Some(Daemon::new(config));
    });
}

pub fn handle(event: DbusEvent) {
    with_daemon(|d| d.handle(event));
}

fn with_daemon<F: FnOnce(&mut Daemon)>(f: F) {
    DAEMON.with(|d| {
        if let Some(d) = d.borrow_mut().as_mut() {
            f(d);
        }
    });
}

pub struct Daemon {
    /// id -> (window, urgency level)
    windows: HashMap<u32, (NotificationWindow, u8)>,
    config: std::sync::Arc<Config>,
}

impl Daemon {
    fn new(config: std::sync::Arc<Config>) -> Self {
        Self {
            windows: HashMap::new(),
            config,
        }
    }

    fn handle(&mut self, event: DbusEvent) {
        match event {
            DbusEvent::Show {
                id,
                app_name,
                app_icon,
                summary,
                body,
                client,
                expire_timeout,
                urgency,
            } => self.show(
                id,
                &app_name,
                &app_icon,
                &summary,
                &body,
                client,
                expire_timeout,
                urgency,
            ),
            DbusEvent::Close { id, reason } => self.close(id, reason),
        }
    }

    fn show(
        &mut self,
        id: u32,
        app_name: &str,
        app_icon: &str,
        summary: &str,
        body: &str,
        client: Option<String>,
        expire_timeout: i32,
        urgency: u8,
    ) {
        if self.windows.contains_key(&id) {
            log::warn!("duplicate notification id {id}, ignoring");
            return;
        }

        let style = WindowStyle::from_config(&self.config, urgency);
        let on_closed: ClosedCb = Rc::new(RefCell::new(Box::new(|id| {
            with_daemon(|d| {
                d.windows.remove(&id);
                d.relayout();
            });
        })));

        let nw = NotificationWindow::new(
            id,
            app_name,
            app_icon,
            summary,
            body,
            client,
            on_closed,
            &style,
        );

        // Timeout: explicit ms wins; -1 falls back to the urgency default
        // (seconds, 0 = never). The full state machine lands in ticket 05.
        let timeout_ms = if expire_timeout >= 0 {
            expire_timeout as u64
        } else {
            self.config.urgency(urgency).timeout as u64 * 1000
        };
        if timeout_ms > 0 {
            let id = id;
            glib::timeout_add_local_once(
                std::time::Duration::from_millis(timeout_ms),
                move || {
                    log::info!("notification {id} expired");
                    with_daemon(|d| d.close(id, 1));
                },
            );
        }

        self.windows.insert(id, (nw, urgency));
        self.relayout();
    }

    fn close(&mut self, id: u32, reason: u32) {
        if let Some((nw, _)) = self.windows.remove(&id) {
            if let Some(conn) = connection() {
                emit_closed_signal(conn, nw.client().clone(), id, reason);
            }
            nw.destroy();
            self.relayout();
        }
    }

    /// Recompute the stack: order (urgency desc by default, else arrival),
    /// resolve each window's size from its natural size and the config specs,
    /// then place every window against the resolved monitor.
    fn relayout(&mut self) {
        if self.windows.is_empty() {
            return;
        }
        let cfg = &self.config;
        let Some(monitor) = resolve_monitor(cfg) else {
            log::warn!("no monitor available for layout");
            return;
        };

        // Stacking order: outermost first.
        let mut ordered: Vec<(u32, u8)> = self
            .windows
            .iter()
            .map(|(id, (_, urgency))| (*id, *urgency))
            .collect();
        if cfg.global.sort {
            ordered.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        } else {
            ordered.sort_by_key(|a| a.0);
        }

        // Resolve sizes: width spec -> natural -> wrap height -> height spec.
        let mut resolved: Vec<(i32, i32)> = Vec::with_capacity(ordered.len());
        for (id, _) in &ordered {
            let (nw, _) = &self.windows[id];
            let (natural_w, _) = nw.natural_size();
            let w = resolve_size(cfg.global.width, natural_w, monitor.width);
            let natural_h_at_w = nw.height_for_width(w);
            let h = resolve_size(cfg.global.height, natural_h_at_w, monitor.height);
            resolved.push((w, h));
        }

        for (i, (id, _)) in ordered.iter().enumerate() {
            let (w, h) = resolved[i];
            let (x, y) = stack_position(
                cfg.global.origin,
                cfg.global.offset,
                cfg.global.gap_size,
                monitor,
                &resolved,
                i,
            );
            self.windows[id].0.apply_geometry(x, y, w, h);
        }
    }
}

/// Resolve the monitor to place notifications on, per `follow`/`monitor`.
fn resolve_monitor(cfg: &Config) -> Option<MonitorGeometry> {
    let display = gdk::Display::default()?;
    let model = display.monitors();
    let monitors: Vec<gdk::Monitor> = (0..model.n_items())
        .filter_map(|i| model.item(i).and_then(|o| o.downcast::<gdk::Monitor>().ok()))
        .collect();
    if monitors.is_empty() {
        return None;
    }

    let picked: Option<gdk::Monitor> = match cfg.global.follow {
        Follow::Mouse => {
            let surface = display
                .default_seat()
                .and_then(|seat| seat.pointer())
                .and_then(|dev| dev.surface_at_position().0);
            match surface {
                Some(s) => display.monitor_at_surface(&s).or_else(|| monitors.first().cloned()),
                None => monitors.first().cloned(),
            }
        }
        // Keyboard-focus tracking is L2; fall back to the configured monitor.
        Follow::Keyboard | Follow::None => match &cfg.global.monitor {
            Monitor::Number(n) => {
                let n = (*n).max(0) as usize;
                monitors.get(n).cloned().or_else(|| monitors.first().cloned())
            }
            Monitor::Name(name) => monitors
                .iter()
                .find(|m| m.connector().map(|c| c.contains(name.as_str())).unwrap_or(false))
                .cloned()
                .or_else(|| monitors.first().cloned()),
        },
    };

    picked.map(|m| {
        let g = m.geometry();
        MonitorGeometry {
            x: g.x(),
            y: g.y(),
            width: g.width(),
            height: g.height(),
        }
    })
}
