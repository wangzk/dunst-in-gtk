//! A single notification window (GTK4).
//!
//! Each notification gets its own `GtkWindow` (dunst's architecture): type
//! hint NOTIFICATION + keep-above, undecorated, positioned programmatically.
//! HiDPI handling is delegated entirely to GTK's per-window scale factor.

use std::cell::RefCell;
use std::rc::Rc;

use gtk4 as gtk;
use gtk4::gdk;
use gtk4::prelude::*;

use crate::dbus::{DBUS_IFACE, DBUS_PATH};

/// Callback invoked when the window is closed by the user (WM close, reason 2).
pub type ClosedCb = Rc<RefCell<Box<dyn Fn(u32) + 'static>>>;

/// Emit `NotificationClosed(id, reason)` to the originating client (or
/// broadcast when the client is unknown).
pub fn emit_closed_signal(conn: &zbus::blocking::Connection, client: Option<String>, id: u32, reason: u32) {
    log::info!("NotificationClosed id={id} reason={reason}");
    let dest = client.as_deref();
    if let Err(e) = conn.emit_signal(
        dest,
        DBUS_PATH,
        DBUS_IFACE,
        "NotificationClosed",
        &(id, reason),
    ) {
        log::warn!("failed to emit NotificationClosed: {e}");
    }
}

pub struct NotificationWindow {
    window: gtk::Window,
    id: u32,
    /// The client this notification belongs to (for the closed signal).
    client: Option<String>,
    on_closed: ClosedCb,
}

impl NotificationWindow {
    pub fn new(
        id: u32,
        app_name: &str,
        _app_icon: &str,
        summary: &str,
        body: &str,
        client: Option<String>,
        on_closed: ClosedCb,
    ) -> Self {
        let window = gtk::Window::new();
        window.set_title(Some(&format!("dunst-in-gtk {app_name}")));
        window.set_decorated(false);
        window.set_resizable(false);

        // Minimal default styling; dunstrc-driven styling lands in ticket 02.
        let css = gtk::CssProvider::new();
        css.load_from_data(
            r#"
            window.notification {
                background-color: #2e3440;
                border: 1px solid #4c566a;
                border-radius: 12px;
            }
            window.notification label {
                color: #eceff4;
            }
            "#,
        );
        window
            .style_context()
            .add_provider(&css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);

        let summary_label = gtk::Label::new(None);
        summary_label.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(summary)));
        summary_label.set_halign(gtk::Align::Start);

        let body_label = gtk::Label::new(None);
        body_label.set_markup(&format!(
            "<span color=\"#d8dee9\">{}</span>",
            glib::markup_escape_text(body)
        ));
        body_label.set_halign(gtk::Align::Start);
        body_label.set_wrap(true);
        body_label.set_max_width_chars(72);

        let box_ = gtk::Box::new(gtk::Orientation::Vertical, 6);
        box_.set_margin_top(12);
        box_.set_margin_bottom(12);
        box_.set_margin_start(14);
        box_.set_margin_end(14);
        box_.append(&summary_label);
        box_.append(&body_label);

        window.set_child(Some(&box_));

        let nw = Self {
            window,
            id,
            client,
            on_closed,
        };

        // User/WM-initiated close (WM_DELETE_WINDOW, alt+F4): dismissed (2).
        // Daemon-initiated closes use destroy() and never reach this handler.
        let id = nw.id;
        let client = nw.client.clone();
        let on_closed = Rc::clone(&nw.on_closed);
        nw.window.connect_close_request(move |_| {
            if let Some(conn) = crate::daemon::connection() {
                emit_closed_signal(conn, client.clone(), id, 2);
            }
            (on_closed.borrow())(id);
            glib::Propagation::Proceed
        });

        nw.position_and_present();
        nw
    }

    /// Close the window without emitting close-request (daemon-initiated).
    /// The caller emits `NotificationClosed` itself.
    pub fn destroy(&self) {
        self.window.destroy();
    }

    pub fn client(&self) -> &Option<String> {
        &self.client
    }

    #[allow(dead_code)] // used by the state machine (ticket 05)
    pub fn id(&self) -> u32 {
        self.id
    }

    /// Position at the top-right corner of the monitor containing the window
    /// (full layout semantics land in ticket 03), then realize, apply X11
    /// hints and present. The X11 calls run before the main loop iterates, so
    /// the server sees position/hints before GTK's map request.
    fn position_and_present(&self) {
        let content = self.window.child().expect("window has a child");
        let (_, natural_w, _, _) = content.measure(gtk::Orientation::Horizontal, -1);
        let (_, natural_h, _, _) = content.measure(gtk::Orientation::Vertical, -1);
        let (w, h) = (natural_w.max(1), natural_h.max(1));
        self.window.set_default_size(w, h);

        <gtk::Widget as gtk::prelude::WidgetExt>::realize(self.window.upcast_ref::<gtk::Widget>());

        let (x, y) = self.corner_position(w, h);
        crate::x11::apply_window_hints_and_position(&self.window, x, y, w as u32, h as u32);

        self.window.present();
    }

    /// Top-right corner of the monitor the window landed on, minus a margin.
    fn corner_position(&self, w: i32, _h: i32) -> (i32, i32) {
        if let Some(display) = gdk::Display::default() {
            if let Some(monitor) = display
                .monitor_at_surface(self.window.surface().as_ref().unwrap())
                .or_else(|| first_monitor(&display))
            {
                let geo = monitor.geometry();
                let margin = 12;
                let x = geo.x() + geo.width() - w - margin;
                let y = geo.y() + margin;
                return (x, y);
            }
        }
        (0, 0)
    }
}

fn first_monitor(display: &gdk::Display) -> Option<gdk::Monitor> {
    let model = display.monitors();
    if model.n_items() > 0 {
        model.item(0).and_then(|o| o.downcast::<gdk::Monitor>().ok())
    } else {
        None
    }
}
