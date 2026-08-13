//! GTK-side notification state: the live set of notification windows and the
//! routing of D-Bus events onto the GTK main loop. Grows into the full state
//! machine (queue/timeouts/history/DnD) in ticket 05.
//!
//! The daemon is single-threaded: everything here is owned and touched
//! exclusively by the GTK main thread (windows are !Send). The thread-local
//! and the OnceLock are conveniences so timer and window callbacks can reach
//! the state without threading gymnastics. Contention is nil.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::OnceLock;

use crate::dbus::DbusEvent;
use crate::window::{emit_closed_signal, ClosedCb, NotificationWindow};

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

pub fn init(conn: zbus::blocking::Connection) {
    let _ = CONN.set(conn);
    DAEMON.with(|d| {
        *d.borrow_mut() = Some(Daemon::new());
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
    windows: HashMap<u32, NotificationWindow>,
}

impl Daemon {
    fn new() -> Self {
        Self {
            windows: HashMap::new(),
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
            } => self.show(
                id,
                &app_name,
                &app_icon,
                &summary,
                &body,
                client,
                expire_timeout,
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
    ) {
        if self.windows.contains_key(&id) {
            log::warn!("duplicate notification id {id}, ignoring");
            return;
        }

        let on_closed: ClosedCb = Rc::new(RefCell::new(Box::new(|id| {
            with_daemon(|d| {
                d.windows.remove(&id);
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
        );

        // Basic expiry; the full timeout state machine lands in ticket 05.
        if expire_timeout > 0 {
            let id = id;
            glib::timeout_add_local_once(
                std::time::Duration::from_millis(expire_timeout as u64),
                move || {
                    log::info!("notification {id} expired");
                    with_daemon(|d| d.close(id, 1));
                },
            );
        }

        self.windows.insert(id, nw);
    }

    fn close(&mut self, id: u32, reason: u32) {
        if let Some(nw) = self.windows.remove(&id) {
            if let Some(conn) = connection() {
                emit_closed_signal(conn, nw.client().clone(), id, reason);
            }
            nw.destroy();
        }
    }
}
