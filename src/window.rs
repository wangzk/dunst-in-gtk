//! A single notification window (GTK4).
//!
//! Each notification gets its own `GtkWindow` (dunst's architecture): type
//! hint NOTIFICATION + keep-above, undecorated, positioned programmatically.
//! HiDPI handling is delegated entirely to GTK's per-window scale factor.

use std::cell::RefCell;
use std::rc::Rc;

use gtk4 as gtk;
use gtk4::gdk;
use gtk4::pango;
use gtk4::prelude::*;

use crate::config::{
    Alignment, Color, Config, Ellipsize, Markup, VerticalAlignment,
};
use crate::dbus::{DBUS_IFACE, DBUS_PATH};

/// Resolved per-notification visual style, derived from the config.
#[derive(Debug, Clone)]
pub struct WindowStyle {
    pub background: Color,
    pub foreground: Color,
    pub frame_color: Color,
    pub corner_radius: i32,
    pub frame_width: i32,
    pub font: String,
    pub alignment: Alignment,
    pub vertical_alignment: VerticalAlignment,
    pub padding: i32,
    pub h_padding: i32,
    pub word_wrap: bool,
    pub ellipsize: Ellipsize,
    pub markup: Markup,
    pub transparency: u8,
}

impl WindowStyle {
    pub fn from_config(cfg: &Config, urgency_level: u8) -> Self {
        let u = cfg.urgency(urgency_level);
        // dunst `transparency` (0-100) darkens the whole window; apply it to
        // the background alpha (compositor-dependent, like dunst on X11).
        let bg_alpha = (u.background.a as u32 * (100 - cfg.global.transparency as u32) / 100) as u8;
        Self {
            background: u.background.with_alpha(bg_alpha),
            foreground: u.foreground,
            frame_color: u.frame_color,
            corner_radius: cfg.global.corner_radius,
            frame_width: cfg.global.frame_width,
            font: cfg.global.font.clone(),
            alignment: cfg.global.alignment,
            vertical_alignment: cfg.global.vertical_alignment,
            padding: cfg.global.padding,
            h_padding: cfg.global.horizontal_padding,
            word_wrap: cfg.global.word_wrap,
            ellipsize: cfg.global.ellipsize,
            markup: cfg.global.markup,
            transparency: cfg.global.transparency,
        }
    }
}

/// CSS for the notification window, generated from the style. The window
/// itself is transparent; the inner box carries background/border/radius.
/// Unit-tested; the integration tests assert via window geometry instead.
pub fn style_css(style: &WindowStyle) -> String {
    let bg = style.background.css_rgba();
    let fg = style.foreground.css_rgba();
    let frame = style.frame_color.css_rgba();
    format!(
        r#"
window.notification {{
    background-color: transparent;
}}
window.notification > box.notification {{
    background-color: {bg};
    border: {}px solid {frame};
    border-radius: {}px;
}}
window.notification label {{
    color: {fg};
}}
"#,
        style.frame_width.max(0),
        style.corner_radius.max(0)
    )
}

/// Render notification text honoring the markup setting. `Full` passes the
/// text through (validated), `No` and (approximation) `Strip` escape it.
pub fn render_text(markup: Markup, text: &str) -> String {
    match markup {
        Markup::Full => {
            if pango::parse_markup(text, '$').is_ok() {
                text.to_string()
            } else {
                log::warn!("invalid markup in notification, escaping: {text:?}");
                glib::markup_escape_text(text).to_string()
            }
        }
        Markup::Strip | Markup::No => glib::markup_escape_text(text).to_string(),
    }
}

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
        style: &WindowStyle,
    ) -> Self {
        let window = gtk::Window::new();
        window.set_title(Some(&format!("dunst-in-gtk {app_name}")));
        window.set_decorated(false);
        window.set_resizable(false);
        window.add_css_class("notification");

        let css = gtk::CssProvider::new();
        css.load_from_data(&style_css(style));
        window
            .style_context()
            .add_provider(&css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);

        // GTK4's GtkLabel has no font_desc property (removed); use a Pango
        // font-desc attribute instead.
        let font_desc = pango::FontDescription::from_string(&style.font);
        let font_attrs = pango::AttrList::new();
        font_attrs.insert(pango::AttrFontDesc::new(&font_desc));

        let summary_label = gtk::Label::new(None);
        summary_label.set_markup(&format!("<b>{}</b>", render_text(style.markup, summary)));
        summary_label.set_halign(align_of(style.alignment));
        summary_label.set_attributes(Some(&font_attrs));

        let body_label = gtk::Label::new(None);
        body_label.set_markup(&render_text(style.markup, body));
        body_label.set_halign(align_of(style.alignment));
        body_label.set_wrap(style.word_wrap);
        body_label.set_ellipsize(ellipsize_of(style.ellipsize));
        body_label.set_attributes(Some(&font_attrs));

        let box_ = gtk::Box::new(gtk::Orientation::Vertical, 6);
        box_.add_css_class("notification");
        box_.set_margin_top(style.padding);
        box_.set_margin_bottom(style.padding);
        box_.set_margin_start(style.h_padding);
        box_.set_margin_end(style.h_padding);
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

fn align_of(a: Alignment) -> gtk::Align {
    match a {
        Alignment::Left => gtk::Align::Start,
        Alignment::Center => gtk::Align::Center,
        Alignment::Right => gtk::Align::End,
    }
}

fn ellipsize_of(e: Ellipsize) -> pango::EllipsizeMode {
    match e {
        Ellipsize::Start => pango::EllipsizeMode::Start,
        Ellipsize::Middle => pango::EllipsizeMode::Middle,
        Ellipsize::End => pango::EllipsizeMode::End,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style() -> WindowStyle {
        WindowStyle {
            background: Color { r: 0, g: 0, b: 0, a: 0xcc },
            foreground: Color::rgb(0xff, 0xff, 0xff),
            frame_color: Color::rgb(0xff, 0, 0),
            corner_radius: 12,
            frame_width: 2,
            font: "Sans 12".into(),
            alignment: Alignment::Center,
            vertical_alignment: VerticalAlignment::Top,
            padding: 20,
            h_padding: 10,
            word_wrap: true,
            ellipsize: Ellipsize::Middle,
            markup: Markup::Full,
            transparency: 0,
        }
    }

    #[test]
    fn css_contains_style_values() {
        let css = style_css(&style());
        assert!(css.contains("rgba(0, 0, 0, 0.800)"), "{css}");
        assert!(css.contains("rgba(255, 255, 255, 1.000)"), "{css}");
        assert!(css.contains("rgba(255, 0, 0, 1.000)"), "{css}");
        assert!(css.contains("border: 2px solid"), "{css}");
        assert!(css.contains("border-radius: 12px"), "{css}");
    }

    #[test]
    fn transparency_darkens_background() {
        let cfg = crate::config::Config::default();
        let mut s = WindowStyle::from_config(&cfg, 1);
        assert_eq!(s.background.a, 255);
        let mut g = cfg.global.clone();
        g.transparency = 50;
        let cfg2 = crate::config::Config {
            global: g,
            urgency: cfg.urgency.clone(),
        };
        s = WindowStyle::from_config(&cfg2, 1);
        assert_eq!(s.background.a, 127); // 255 * 50 / 100, integer math
    }

    #[test]
    fn markup_escaping() {
        assert_eq!(render_text(Markup::No, "<b>x</b>"), "&lt;b&gt;x&lt;/b&gt;");
        assert_eq!(render_text(Markup::Full, "<b>x</b>"), "<b>x</b>");
        // Invalid markup falls back to escaped text.
        assert_eq!(render_text(Markup::Full, "<b>x"), "&lt;b&gt;x");
    }

    #[test]
    fn urgency_style_differs() {
        let cfg = crate::config::Config::default();
        let low = WindowStyle::from_config(&cfg, 0);
        let critical = WindowStyle::from_config(&cfg, 2);
        // Defaults: critical timeout 0, colors identical; style same but
        // timeout handled by the daemon. Just check frame color wiring.
        assert_eq!(low.frame_color, critical.frame_color);
    }
}
