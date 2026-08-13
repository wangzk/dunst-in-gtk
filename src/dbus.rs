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
use std::sync::{Arc, Mutex};

use crate::daemon::{CmdAction, DaemonCounters, HistoryAction, HistoryStore};

use zbus::blocking::Connection;
use zbus::message::Header;
use zbus::zvariant::Value;

/// Well-known name / object path per the freedesktop notification spec.
pub const DBUS_NAME: &str = "org.freedesktop.Notifications";
pub const DBUS_PATH: &str = "/org/freedesktop/Notifications";
pub const DBUS_IFACE: &str = "org.freedesktop.Notifications";
/// The dunstctl control interface (same name/path as the spec interface).
pub const CMD0_IFACE: &str = "org.dunstproject.cmd0";

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
        /// Pairs of (key, label) from the `actions` argument.
        actions: Vec<(String, String)>,
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
    /// Do-not-disturb level changed (org.dunstproject.cmd0 pauseLevel/paused).
    SetPauseLevel(u32),
    /// History operation (show/pop/clear/remove).
    History(HistoryAction),
    /// Control operation (close-last/close-all/context/action/reload).
    Cmd(CmdAction),
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
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        _hints: HashMap<String, Value<'_>>,
        expire_timeout: i32,
        #[zbus(header)]
        hdr: Header<'_>,
    ) -> u32 {
        // dunst semantics: a positive replaces_id becomes the notification id
        // (the daemon updates the existing notification in place).
        let id = if replaces_id > 0 {
            replaces_id
        } else {
            self.next_id.fetch_add(1, Ordering::Relaxed)
        };
        self.live_ids.lock().unwrap().insert(id);
        let client = hdr.sender().map(|s| s.to_string());
        // Actions arrive as [key1, label1, key2, label2, ...].
        let actions: Vec<(String, String)> = actions
            .chunks(2)
            .filter_map(|c| match c {
                [k, l] => Some((k.clone(), l.clone())),
                _ => None,
            })
            .collect();
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
                actions,
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
        vec!["actions".to_string(), "body".to_string()]
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
/// The `org.dunstproject.cmd0` interface, which `dunstctl` talks to.
/// Ticket 05 provides the state properties (paused/pauseLevel and the length
/// counters); ticket 06 adds the control methods (history, close-all, ...).
pub struct Cmd0 {
    tx: async_channel::Sender<DbusEvent>,
    counters: Arc<DaemonCounters>,
    history: Arc<HistoryStore>,
}

impl Cmd0 {
    pub fn new(
        tx: async_channel::Sender<DbusEvent>,
        counters: Arc<DaemonCounters>,
        history: Arc<HistoryStore>,
    ) -> Self {
        Self {
            tx,
            counters,
            history,
        }
    }

    fn request_pause_level(&self, level: u32) {
        self.tx
            .try_send(DbusEvent::SetPauseLevel(level))
            .expect("GTK main loop channel closed");
    }

    fn send(&self, event: DbusEvent) {
        self.tx.try_send(event).expect("GTK main loop channel closed");
    }
}

#[zbus::interface(name = "org.dunstproject.cmd0")]
impl Cmd0 {
    #[zbus(property, name = "displayedLength")]
    fn displayed_length(&self) -> u32 {
        self.counters.displayed.load(Ordering::Relaxed)
    }

    #[zbus(property, name = "waitingLength")]
    fn waiting_length(&self) -> u32 {
        self.counters.waiting.load(Ordering::Relaxed)
    }

    #[zbus(property, name = "historyLength")]
    fn history_length(&self) -> u32 {
        self.history.lock().unwrap().len() as u32
    }

    #[zbus(property, name = "paused")]
    fn paused(&self) -> bool {
        self.counters.pause_level.load(Ordering::Relaxed) > 0
    }

    #[zbus(property, name = "paused")]
    fn set_paused(&self, paused: bool) {
        self.request_pause_level(if paused { 1 } else { 0 });
    }

    #[zbus(property, name = "pauseLevel")]
    fn pause_level(&self) -> u32 {
        self.counters.pause_level.load(Ordering::Relaxed)
    }

    #[zbus(property, name = "pauseLevel")]
    fn set_pause_level(&self, level: u32) {
        self.request_pause_level(level);
    }

    // ----------------------------------------------------------- methods

    fn ping(&self) {}

    fn notification_action(&self, id: u32) {
        self.send(DbusEvent::Cmd(CmdAction::Action(id)));
    }

    fn notification_close_last(&self) {
        self.send(DbusEvent::Cmd(CmdAction::CloseLast));
    }

    fn notification_close_all(&self) {
        self.send(DbusEvent::Cmd(CmdAction::CloseAll));
    }

    fn context_menu_call(&self) {
        self.send(DbusEvent::Cmd(CmdAction::ContextMenu));
    }

    fn config_reload(&self, configs: Vec<String>) {
        self.send(DbusEvent::Cmd(CmdAction::Reload(configs)));
    }

    fn notification_show(&self) {
        self.send(DbusEvent::History(HistoryAction::Show(0)));
    }

    fn notification_pop_history(&self, id: u32) {
        self.send(DbusEvent::History(HistoryAction::Pop(id)));
    }

    fn notification_remove_from_history(&self, id: u32) {
        self.send(DbusEvent::History(HistoryAction::Remove(id)));
    }

    fn notification_clear_history(&self) {
        self.send(DbusEvent::History(HistoryAction::Clear));
    }

    /// The full history as `aa{sv}` (dunst-compatible); dunstctl formats it
    /// as JSON via `busctl --json`.
    fn notification_list_history(&self) -> Vec<HashMap<String, Value<'static>>> {
        let hist = self.history.lock().unwrap();
        // Reverse chronological, like dunst.
        hist.iter()
            .rev()
            .map(|h| {
                let mut m = HashMap::new();
                m.insert("id".to_string(), Value::from(h.id as i32));
                m.insert("appname".to_string(), Value::from(h.app_name.clone()));
                m.insert("summary".to_string(), Value::from(h.summary.clone()));
                m.insert("body".to_string(), Value::from(h.body.clone()));
                m.insert("icon_path".to_string(), Value::from(h.app_icon.clone()));
                m.insert("category".to_string(), Value::from(String::new()));
                m.insert(
                    "urgency".to_string(),
                    Value::from(match h.urgency {
                        0 => "low",
                        2 => "critical",
                        _ => "normal",
                    }),
                );
                m.insert("timeout".to_string(), Value::from(h.expire_timeout as i64));
                m.insert("timestamp".to_string(), Value::from(h.timestamp as i64));
                m.insert("progress".to_string(), Value::from(0i32));
                m
            })
            .collect()
    }
}

pub fn serve(
    tx: async_channel::Sender<DbusEvent>,
    counters: Arc<DaemonCounters>,
    history: Arc<HistoryStore>,
) {
    let conn = match Connection::session() {
        Ok(c) => c,
        Err(e) => {
            log::error!("cannot connect to session bus: {e}");
            std::process::exit(1);
        }
    };

    if let Err(e) = conn
        .object_server()
        .at(DBUS_PATH, Notifications::new(tx.clone()))
    {
        log::error!("cannot register object at {DBUS_PATH}: {e}");
        std::process::exit(1);
    }
    if let Err(e) = conn
        .object_server()
        .at(DBUS_PATH, Cmd0::new(tx, counters, history))
    {
        log::error!("cannot register cmd0 interface at {DBUS_PATH}: {e}");
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
