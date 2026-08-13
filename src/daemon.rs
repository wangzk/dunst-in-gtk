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
use std::time::{Duration, Instant};

use gtk4::gdk;
use gtk4::prelude::*;

use crate::config::{Config, Follow, Monitor, MouseAction};
use crate::dbus::DbusEvent;
use crate::layout::{resolve_size, stack_position, MonitorGeometry};
use crate::window::{
    emit_action_invoked, emit_closed_signal, EventCb, NotificationWindow, WindowEvent,
    WindowStyle,
};

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
        // try_borrow: GTK signal handlers can nest (e.g. destroying a window
        // makes its motion controller emit a final leave event while we are
        // already inside with_daemon); nested events are dropped, which is
        // safe because the outer call is the one mutating the state.
        match d.try_borrow_mut() {
            Ok(mut guard) => {
                if let Some(d) = guard.as_mut() {
                    f(d);
                }
            }
            Err(_) => log::debug!("dropping nested daemon event"),
        }
    });
}

/// Remaining-time tracking for one notification's expiry timer. The timer is
/// a glib source that closes the notification when it fires; hover pauses it
/// by removing the source and keeping the deadline.
struct TimeoutState {
    deadline: Instant,
    source: Option<glib::SourceId>,
}

pub struct Daemon {
    /// id -> (window, urgency level)
    windows: HashMap<u32, (NotificationWindow, u8)>,
    timeouts: HashMap<u32, TimeoutState>,
    config: std::sync::Arc<Config>,
}

impl Daemon {
    fn new(config: std::sync::Arc<Config>) -> Self {
        Self {
            windows: HashMap::new(),
            timeouts: HashMap::new(),
            config,
        }
    }

    // ------------------------------------------------------------- timeouts

    /// Arm (or re-arm) the expiry timer for a notification; ms == 0 = never.
    fn arm_timeout(&mut self, id: u32, ms: u64) {
        self.cancel_timeout(id);
        if ms == 0 {
            return;
        }
        let deadline = Instant::now() + Duration::from_millis(ms);
        let source = Self::spawn_timeout_source(id, deadline);
        self.timeouts.insert(id, TimeoutState { deadline, source: Some(source) });
    }

    fn spawn_timeout_source(id: u32, deadline: Instant) -> glib::SourceId {
        let delay = deadline.saturating_duration_since(Instant::now());
        glib::timeout_add_local_once(delay, move || {
            log::info!("notification {id} expired");
            with_daemon(|d| d.close(id, 1));
        })
    }

    fn cancel_timeout(&mut self, id: u32) {
        if let Some(t) = self.timeouts.remove(&id) {
            if let Some(s) = t.source {
                s.remove();
            }
        }
    }

    fn pause_timeout(&mut self, id: u32) {
        if let Some(t) = self.timeouts.get_mut(&id) {
            if let Some(s) = t.source.take() {
                s.remove();
            }
        }
    }

    fn resume_timeout(&mut self, id: u32) {
        let Some(remaining) = self.timeouts.get(&id).and_then(|t| {
            if t.source.is_some() {
                None
            } else {
                Some(t.deadline.saturating_duration_since(Instant::now()))
            }
        }) else {
            return;
        };
        if remaining.is_zero() {
            self.close(id, 1);
            return;
        }
        let deadline = Instant::now() + remaining;
        let source = Self::spawn_timeout_source(id, deadline);
        if let Some(t) = self.timeouts.get_mut(&id) {
            t.deadline = deadline;
            t.source = Some(source);
        }
    }

    // ------------------------------------------------------- window events

    fn handle_window_event(&mut self, event: WindowEvent) {
        match event {
            WindowEvent::Closed(id) => self.on_user_closed(id),
            WindowEvent::Hover(id, entered) => {
                if entered {
                    self.pause_timeout(id);
                } else {
                    self.resume_timeout(id);
                }
            }
            WindowEvent::Click(id, button) => self.handle_click(id, button),
            WindowEvent::Action(id, key) => self.invoke_action(id, &key),
        }
    }

    fn handle_click(&mut self, id: u32, button: u32) {
        let sequence: Vec<MouseAction> = match button {
            1 => self.config.global.mouse_left_click.clone(),
            2 => self.config.global.mouse_middle_click.clone(),
            _ => self.config.global.mouse_right_click.clone(),
        };
        for action in sequence {
            match action {
                MouseAction::None => {}
                MouseAction::CloseCurrent => self.close(id, 2),
                MouseAction::CloseAll => self.close_all(),
                MouseAction::DoAction => self.do_default_action(id),
                MouseAction::Context => {
                    if let Some((nw, _)) = self.windows.get(&id) {
                        nw.show_context_menu();
                    }
                }
            }
        }
    }

    fn do_default_action(&mut self, id: u32) {
        let Some((nw, _)) = self.windows.get(&id) else {
            return;
        };
        match nw.default_action() {
            Some((key, _)) => self.invoke_action(id, &key),
            None => self.close(id, 2), // no actions: clicking dismisses
        }
    }

    /// Emit ActionInvoked, then close the notification (reason 5).
    fn invoke_action(&mut self, id: u32, key: &str) {
        let Some((nw, _)) = self.windows.get(&id) else {
            return;
        };
        let client = nw.client().clone();
        if let Some(conn) = connection() {
            emit_action_invoked(conn, client, id, key);
        }
        self.close(id, 5);
    }

    fn close_all(&mut self) {
        let ids: Vec<u32> = self.windows.keys().copied().collect();
        for id in ids {
            self.close(id, 2);
        }
    }

    /// The user/WM closed the window itself (GTK is destroying it); we only
    /// emit the signal and drop bookkeeping.
    fn on_user_closed(&mut self, id: u32) {
        self.cancel_timeout(id);
        if let Some((nw, _)) = self.windows.remove(&id) {
            if let Some(conn) = connection() {
                emit_closed_signal(conn, nw.client().clone(), id, 2);
            }
            self.relayout();
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
                actions,
                client,
                expire_timeout,
                urgency,
            } => self.show(
                id,
                &app_name,
                &app_icon,
                &summary,
                &body,
                actions,
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
        actions: Vec<(String, String)>,
        client: Option<String>,
        expire_timeout: i32,
        urgency: u8,
    ) {
        if self.windows.contains_key(&id) {
            log::warn!("duplicate notification id {id}, ignoring");
            return;
        }

        let style = WindowStyle::from_config(&self.config, urgency);
        let on_event: EventCb = Rc::new(RefCell::new(Box::new(|event| {
            with_daemon(|d| d.handle_window_event(event));
        })));

        let nw = NotificationWindow::new(
            id,
            app_name,
            app_icon,
            summary,
            body,
            actions,
            client,
            on_event,
            &style,
        );

        // Timeout: explicit ms wins; -1 falls back to the urgency default
        // (seconds, 0 = never). The full state machine lands in ticket 05.
        let timeout_ms = if expire_timeout >= 0 {
            expire_timeout as u64
        } else {
            self.config.urgency(urgency).timeout as u64 * 1000
        };
        self.arm_timeout(id, timeout_ms);

        self.windows.insert(id, (nw, urgency));
        self.relayout();
    }

    fn close(&mut self, id: u32, reason: u32) {
        self.cancel_timeout(id);
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
