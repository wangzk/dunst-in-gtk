//! D-Bus layer: serves `org.freedesktop.Notifications` (the freedesktop
//! notification spec) and (later) the `org.dunstproject.cmd0` interface that
//! `dunstctl` talks to.
//!
//! Architecture: the daemon runs the GTK main loop on the main thread. All
//! D-Bus serving happens on a dedicated thread using `zbus::blocking::Connection`
//! (zbus spawns its own internal executor thread, so method calls are
//! dispatched automatically while our main thread sits in the GTK loop).
//! D-Bus -> GTK events travel over a `glib` channel; GTK -> D-Bus signal
//! emission goes through a cloned `blocking::Connection` on the GTK thread.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

use zbus::blocking::Connection;
use zbus::message::Header;
use zbus::zvariant::Value;

/// Well-known name / object path per the freedesktop notification spec.
pub const DBUS_NAME: &str = "org.freedesktop.Notifications";
pub const DBUS_PATH: &str = "/org/freedesktop/Notifications";
pub const DBUS_IFACE: &str = "org.freedesktop.Notifications";

/// Events sent from the D-Bus thread to the GTK main thread.
#[derive(Debug, Clone)]
pub enum DbusEvent {
    /// A new notification arrived (or replaces an existing one, handled later).
    Show {
        id: u32,
        app_name: String,
        app_icon: String,
        summary: String,
        body: String,
        /// Unique bus name of the client that created the notification; the
        /// `NotificationClosed` signal is directed back to it.
        client: Option<String>,
        /// -1 = use default, 0 = never expire, >0 = milliseconds.
        expire_timeout: i32,
        /// From the `urgency` hint: 0 low, 1 normal, 2 critical.
        urgency: u8,
    },
    /// Close a notification (e.g. via CloseNotification).
    Close { id: u32, reason: u32 },
}

/// The `org.freedesktop.Notifications` interface implementation.
///
/// Shared between the D-Bus thread (method dispatch) and the GTK thread via
/// the channel sender. ID assignment and liveness tracking live here.
pub struct Notifications {
    tx: async_channel::Sender<DbusEvent>,
    next_id: AtomicU32,
    live_ids: Mutex<HashSet<u32>>,
}

impl Notifications {
    pub fn new(tx: async_channel::Sender<DbusEvent>) -> Self {
        Self {
            tx,
            next_id: AtomicU32::new(1),
            live_ids: Mutex::new(HashSet::new()),
        }
    }

    fn is_live(&self, id: u32) -> bool {
        self.live_ids.lock().unwrap().contains(&id)
    }

    fn emit_closed(&self, id: u32, reason: u32) {
        self.live_ids.lock().unwrap().remove(&id);
        self.tx
            .try_send(DbusEvent::Close { id, reason })
            .expect("GTK main loop channel closed");
    }
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Notifications {
    async fn notify(
        &self,
        app_name: String,
        _replaces_id: u32, // replaces_id handling lands with the state machine
        app_icon: String,
        summary: String,
        body: String,
        _actions: Vec<String>, // action handling lands with mouse interaction
        _hints: HashMap<String, Value<'_>>,
        expire_timeout: i32,
        #[zbus(header)]
        hdr: Header<'_>,
    ) -> u32 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.live_ids.lock().unwrap().insert(id);
        let client = hdr.sender().map(|s| s.to_string());
        // The `urgency` hint (byte, 0-2) selects the style + default timeout.
        let urgency = _hints
            .get("urgency")
            .and_then(|v| v.downcast_ref::<u8>().ok())
            .unwrap_or(1);

        log::info!("Notify id={id} app={app_name:?} summary={summary:?} timeout={expire_timeout} urgency={urgency}");
        self.tx
            .try_send(DbusEvent::Show {
                id,
                app_name,
                app_icon,
                summary,
                body,
                client,
                expire_timeout,
                urgency,
            })
            .expect("GTK main loop channel closed");
        id
    }

    async fn close_notification(&self, id: u32) {
        log::info!("CloseNotification id={}", id);
        if self.is_live(id) {
            // reason 3 = CloseNotification per the spec
            self.emit_closed(id, 3);
        } else {
            log::debug!("CloseNotification: unknown id {} ignored", id);
        }
    }

    async fn get_capabilities(&self) -> Vec<String> {
        // Honest subset: expanded as features land.
        vec!["body".to_string()]
    }

    async fn get_server_information(&self) -> (String, String, String, String) {
        (
            "dunst-in-gtk".to_string(),
            "dunst-in-gtk".to_string(),
            env!("CARGO_PKG_VERSION").to_string(),
            "1.2".to_string(),
        )
    }
}

/// Runs the D-Bus server on a dedicated thread. Blocks forever.
pub fn serve(tx: async_channel::Sender<DbusEvent>) {
    let conn = match Connection::session() {
        Ok(c) => c,
        Err(e) => {
            log::error!("cannot connect to session bus: {e}");
            std::process::exit(1);
        }
    };

    if let Err(e) = conn.object_server().at(DBUS_PATH, Notifications::new(tx)) {
        log::error!("cannot register object at {DBUS_PATH}: {e}");
        std::process::exit(1);
    }

    match conn.request_name_with_flags(
        DBUS_NAME,
        zbus::fdo::RequestNameFlags::DoNotQueue.into(),
    ) {
        Ok(zbus::fdo::RequestNameReply::PrimaryOwner) => {
            log::info!("acquired bus name {DBUS_NAME}");
        }
        Ok(_) => {
            // Another notification daemon already owns the name. Exit quietly,
            // like dunst does when it cannot take over.
            log::info!("{DBUS_NAME} already owned by another daemon, exiting");
            std::process::exit(0);
        }
        Err(zbus::Error::NameTaken) => {
            // zbus maps a taken name to this error even with DoNotQueue.
            log::info!("{DBUS_NAME} already owned by another daemon, exiting");
            std::process::exit(0);
        }
        Err(e) => {
            log::error!("cannot request name {DBUS_NAME}: {e}");
            std::process::exit(1);
        }
    }

    // All dispatch happens on zbus's internal executor thread; park forever.
    loop {
        std::thread::park();
    }
}
