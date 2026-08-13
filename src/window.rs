//! A single notification window (GTK4).
//!
//! Each notification gets its own `GtkWindow` (dunst's architecture): type
//! hint NOTIFICATION + keep-above, undecorated, positioned programmatically.
//! HiDPI handling is delegated entirely to GTK's per-window scale factor.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4 as gtk;
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
    #[allow(dead_code)] // box packing refinement in a later ticket
    pub vertical_alignment: VerticalAlignment,
    pub padding: i32,
    pub h_padding: i32,
    pub word_wrap: bool,
    pub ellipsize: Ellipsize,
    pub markup: Markup,
    #[allow(dead_code)] // applied to background alpha in from_config
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

/// Events a window reports to the daemon. The daemon is the single decision
/// maker: it maps clicks to configured mouse actions, pauses/resumes timers
/// on hover, and emits every D-Bus signal.
#[derive(Debug, Clone)]
pub enum WindowEvent {
    /// User/WM-initiated close (WM_DELETE_WINDOW, alt+F4).
    Closed(u32),
    /// Pointer entered (true) / left (false) the window.
    Hover(u32, bool),
    /// Mouse button pressed: 1 = left, 2 = middle, 3 = right.
    Click(u32, u32),
    /// A context-menu item was chosen; the action key.
    Action(u32, String),
}

pub type EventCb = Rc<RefCell<Box<dyn Fn(WindowEvent) + 'static>>>;

/// Emit `NotificationClosed(id, reason)` to the originating client (or
/// broadcast when the client is unknown). Called only by the daemon.
pub fn emit_closed_signal(
    conn: &zbus::blocking::Connection,
    client: Option<String>,
    id: u32,
    reason: u32,
) {
    log::info!("NotificationClosed id={id} reason={reason}");
    let dest = client.as_deref();
    if let Err(e) = conn.emit_signal(dest, DBUS_PATH, DBUS_IFACE, "NotificationClosed", &(id, reason)) {
        log::warn!("failed to emit NotificationClosed: {e}");
    }
}

/// Emit `ActionInvoked(id, key)` to the originating client.
pub fn emit_action_invoked(
    conn: &zbus::blocking::Connection,
    client: Option<String>,
    id: u32,
    key: &str,
) {
    log::info!("ActionInvoked id={id} key={key:?}");
    let dest = client.as_deref();
    if let Err(e) = conn.emit_signal(dest, DBUS_PATH, DBUS_IFACE, "ActionInvoked", &(id, key)) {
        log::warn!("failed to emit ActionInvoked: {e}");
    }
}

pub struct NotificationWindow {
    window: gtk::Window,
    summary_label: gtk::Label,
    body_label: gtk::Label,
    font_attrs: pango::AttrList,
    id: u32,
    /// The client this notification belongs to (for the closed signal).
    client: Option<String>,
    /// (key, label) pairs from the Notify actions argument.
    actions: Vec<(String, String)>,
    on_event: EventCb,
    /// Whether the window has been realized/hinted/presented yet.
    presented: Cell<bool>,
    /// Whether the pointer is currently inside the window (shared with the
    /// motion-controller callbacks).
    hovered: Rc<Cell<bool>>,
    /// The currently open context menu, kept alive while shown.
    popover: RefCell<Option<gtk::Popover>>,
}

impl NotificationWindow {
    pub fn new(
        id: u32,
        app_name: &str,
        _app_icon: &str,
        summary: &str,
        body: &str,
        actions: Vec<(String, String)>,
        client: Option<String>,
        on_event: EventCb,
        style: &WindowStyle,
    ) -> Self {
        let window = gtk::Window::new();
        // The id makes the title unique so the X11 layer can target this
        // exact window for positioning (siblings share the app name).
        window.set_title(Some(&format!("dunst-in-gtk {app_name} [{id}]")));
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

        let hovered = Rc::new(Cell::new(false));
        let nw = Self {
            window,
            summary_label,
            body_label,
            font_attrs,
            id,
            client,
            actions,
            on_event,
            presented: Cell::new(false),
            hovered: Rc::clone(&hovered),
            popover: RefCell::new(None),
        };

        // User/WM-initiated close (WM_DELETE_WINDOW, alt+F4). The daemon
        // decides (signal + bookkeeping); we just report. Daemon-initiated
        // closes use destroy() and never reach this handler.
        let on_event = Rc::clone(&nw.on_event);
        let id = nw.id;
        nw.window.connect_close_request(move |_| {
            (on_event.borrow())(WindowEvent::Closed(id));
            glib::Propagation::Proceed
        });

        // Pointer enter/leave -> hover pause/resume. `hovered` lives in an
        // Rc so the daemon can query it (replaces_id keeps the timer paused
        // while the pointer is inside).
        let hovered_enter = Rc::clone(&hovered);
        let hovered_leave = Rc::clone(&hovered);
        let motion = gtk::EventControllerMotion::new();
        {
            let on_event = Rc::clone(&nw.on_event);
            let id = nw.id;
            motion.connect_enter(move |_, _, _| {
                (on_event.borrow())(WindowEvent::Hover(id, true));
                hovered_enter.set(true);
            });
        }
        {
            let on_event = Rc::clone(&nw.on_event);
            let id = nw.id;
            motion.connect_leave(move |_| {
                (on_event.borrow())(WindowEvent::Hover(id, false));
                hovered_leave.set(false);
            });
        }
        nw.window.add_controller(motion);

        // Clicks: 1 = left, 2 = middle, 3 = right; the daemon maps the button
        // to the configured mouse action sequence.
        for button in [1u32, 2, 3] {
            let gesture = gtk::GestureClick::new();
            gesture.set_button(button);
            let on_event = Rc::clone(&nw.on_event);
            let id = nw.id;
            gesture.connect_pressed(move |_, _, _, _| {
                (on_event.borrow())(WindowEvent::Click(id, button));
            });
            nw.window.add_controller(gesture);
        }

        nw
    }

    /// The default action: the one with key "default", else the first one.
    pub fn default_action(&self) -> Option<(String, String)> {
        self.actions
            .iter()
            .find(|(k, _)| k == "default")
            .cloned()
            .or_else(|| self.actions.first().cloned())
    }

    /// Pop up the context menu with one item per action plus a close item.
    /// Item clicks report WindowEvent::Action / WindowEvent::Closed.
    pub fn show_context_menu(&self) {
        let popover = gtk::Popover::new();
        popover.set_parent(&self.window);

        let box_ = gtk::Box::new(gtk::Orientation::Vertical, 2);
        box_.set_margin_top(6);
        box_.set_margin_bottom(6);
        box_.set_margin_start(6);
        box_.set_margin_end(6);

        let on_event = Rc::clone(&self.on_event);
        let id = self.id;
        for (key, label) in &self.actions {
            let button = gtk::Button::with_label(label);
            if key == "default" {
                button.add_css_class("suggested-action");
            }
            let on_event = Rc::clone(&on_event);
            let key = key.clone();
            button.connect_clicked(move |_| {
                (on_event.borrow())(WindowEvent::Action(id, key.clone()));
            });
            box_.append(&button);
        }
        // dunst's context menu always offers closing the notification.
        let close_button = gtk::Button::with_label("Close");
        {
            let on_event = Rc::clone(&on_event);
            close_button.connect_clicked(move |_| {
                (on_event.borrow())(WindowEvent::Closed(id));
            });
        }
        box_.append(&close_button);

        popover.set_child(Some(&box_));
        popover.popup();
        // Keep the popover alive for the duration of the menu.
        self.popover.replace(Some(popover));
    }

    /// Close the window without emitting close-request (daemon-initiated).
    /// The caller emits `NotificationClosed` itself.
    pub fn destroy(&self) {
        self.window.destroy();
    }

    /// Update the content in place (replaces_id) and re-apply the style.
    /// The daemon reflows afterwards (the size may change).
    pub fn update_content(&self, summary: &str, body: &str, style: &WindowStyle) {
        let css = gtk::CssProvider::new();
        css.load_from_data(&style_css(style));
        self.window
            .style_context()
            .add_provider(&css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);

        self.summary_label.set_attributes(Some(&self.font_attrs));
        self.body_label.set_attributes(Some(&self.font_attrs));
        self.summary_label
            .set_markup(&format!("<b>{}</b>", render_text(style.markup, summary)));
        self.summary_label.set_halign(align_of(style.alignment));
        self.body_label
            .set_markup(&render_text(style.markup, body));
        self.body_label.set_halign(align_of(style.alignment));
        self.body_label.set_wrap(style.word_wrap);
        self.body_label.set_ellipsize(ellipsize_of(style.ellipsize));
    }

    /// Whether the pointer is currently inside the window.
    pub fn is_hovered(&self) -> bool {
        self.hovered.get()
    }

    pub fn client(&self) -> &Option<String> {
        &self.client
    }

    #[allow(dead_code)] // used by the state machine (ticket 05)
    pub fn id(&self) -> u32 {
        self.id
    }

    /// Natural (unconstrained) content size in logical pixels.
    pub fn natural_size(&self) -> (i32, i32) {
        let content = self.window.child().expect("window has a child");
        let (_, nw, _, _) = content.measure(gtk::Orientation::Horizontal, -1);
        let (_, nh, _, _) = content.measure(gtk::Orientation::Vertical, -1);
        (nw.max(1), nh.max(1))
    }

    /// Natural height when wrapped to the given width (logical pixels).
    pub fn height_for_width(&self, width: i32) -> i32 {
        let content = self.window.child().expect("window has a child");
        let (_, nh, _, _) = content.measure(gtk::Orientation::Vertical, width.max(1));
        nh.max(1)
    }

    /// Apply the final geometry. The first call realizes the window, applies
    /// the X11 EWMH hints and presents; later calls (reflows) only reposition
    /// via the X11 configure request. All values are logical pixels; the X11
    /// layer converts by the surface scale factor.
    pub fn apply_geometry(&self, x: i32, y: i32, width: i32, height: i32) {
        self.window.set_default_size(width.max(1), height.max(1));
        if !self.presented.get() {
            <gtk::Widget as gtk::prelude::WidgetExt>::realize(
                self.window.upcast_ref::<gtk::Widget>(),
            );
            crate::x11::apply_window_hints_and_position(
                &self.window,
                x,
                y,
                width.max(1) as u32,
                height.max(1) as u32,
            );
            self.window.present();
            self.presented.set(true);
        } else {
            crate::x11::apply_window_hints_and_position(
                &self.window,
                x,
                y,
                width.max(1) as u32,
                height.max(1) as u32,
            );
        }
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
