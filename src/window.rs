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
    Alignment, Color, Config, Ellipsize, IconPosition, Markup, VerticalAlignment,
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
    // ---- icons (ticket 07)
    pub icons: bool,
    pub icon_position: IconPosition,
    pub min_icon_size: i32,
    pub max_icon_size: i32,
    pub text_icon_padding: i32,
    // ---- progress bar (ticket 07)
    pub progress_bar: bool,
    pub progress_bar_height: i32,
    pub progress_bar_frame_width: i32,
    pub progress_bar_min_width: i32,
    pub progress_bar_max_width: i32,
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
            icons: cfg.global.icons,
            icon_position: cfg.global.icon_position,
            min_icon_size: cfg.global.min_icon_size,
            max_icon_size: cfg.global.max_icon_size,
            text_icon_padding: cfg.global.text_icon_padding,
            progress_bar: cfg.global.progress_bar,
            progress_bar_height: cfg.global.progress_bar_height,
            progress_bar_frame_width: cfg.global.progress_bar_frame_width,
            progress_bar_min_width: cfg.global.progress_bar_min_width,
            progress_bar_max_width: cfg.global.progress_bar_max_width,
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
    let fw = style.frame_width.max(0);
    let radius = style.corner_radius.max(0);
    let mut progress = String::new();
    if style.progress_bar {
        // dunst: progress_bar_frame_width defaults to frame_width when unset;
        // a negative value means "inherit" in the config, clamp to >= 0.
        let pfw = if style.progress_bar_frame_width >= 0 {
            style.progress_bar_frame_width
        } else {
            fw
        };
        let mut rules = String::new();
        if style.progress_bar_height > 0 {
            rules.push_str(&format!("min-height: {}px;", style.progress_bar_height));
        }
        if style.progress_bar_min_width > 0 {
            rules.push_str(&format!("min-width: {}px;", style.progress_bar_min_width));
        }
        if style.progress_bar_max_width > 0 {
            rules.push_str(&format!("max-width: {}px;", style.progress_bar_max_width));
        }
        progress = format!(
            r#"
window.notification progressbar.progress {{
    {rules}
}}
window.notification progressbar trough {{
    background-color: transparent;
    border: {pfw}px solid {frame};
    border-radius: 0;
}}
window.notification progressbar progress {{
    background-color: {fg};
    border-radius: 0;
}}
"#
        );
    }
    format!(
        r#"
window.notification {{
    background-color: transparent;
}}
window.notification > box.notification {{
    background-color: {bg};
    border: {fw}px solid {frame};
    border-radius: {radius}px;
}}
window.notification label {{
    color: {fg};
}}
{progress}"#
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

/// The renderable content of one notification.
#[derive(Debug, Clone)]
pub struct NotificationContent {
    /// Icon name or file path; the `image-path` hint has already been
    /// resolved into this (dunst replaces `app_icon` with it).
    pub app_icon: String,
    pub summary: String,
    pub body: String,
    /// Progress 0-100 from the `value` hint; None = no progress bar.
    pub value: Option<i32>,
}

/// Remove every child of a container widget (GTK4 has no `children()`;
/// walk `first_child` and unparent instead).
fn clear_children(container: &gtk::Widget) {
    while let Some(child) = container.first_child() {
        child.unparent();
    }
}

pub struct NotificationWindow {
    window: gtk::Window,
    summary_label: gtk::Label,
    body_label: gtk::Label,
    font_attrs: pango::AttrList,
    /// The app name, used for the missing-icon placeholder letter.
    app_name: String,
    /// Holds the current icon widget; rebuilt on content updates.
    icon_slot: gtk::Box,
    /// Holds the progress bar; empty when there is no `value` hint.
    progress_slot: gtk::Box,
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
        content: &NotificationContent,
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
        summary_label.set_markup(&format!(
            "<b>{}</b>",
            render_text(style.markup, &content.summary)
        ));
        summary_label.set_halign(align_of(style.alignment));
        summary_label.set_attributes(Some(&font_attrs));

        let body_label = gtk::Label::new(None);
        body_label.set_markup(&render_text(style.markup, &content.body));
        body_label.set_halign(align_of(style.alignment));
        body_label.set_wrap(style.word_wrap);
        body_label.set_ellipsize(ellipsize_of(style.ellipsize));
        body_label.set_attributes(Some(&font_attrs));

        // Text column: summary, body, then the progress-bar slot.
        let text_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        text_box.append(&summary_label);
        text_box.append(&body_label);
        let progress_slot = gtk::Box::new(gtk::Orientation::Vertical, 0);
        text_box.append(&progress_slot);

        // Icon slot next to (or above) the text, per `icon_position`.
        let icon_slot = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let child: gtk::Widget = if style.icons && style.icon_position != IconPosition::Off {
            let orientation = match style.icon_position {
                IconPosition::Left | IconPosition::Right => gtk::Orientation::Horizontal,
                IconPosition::Top | IconPosition::Off => gtk::Orientation::Vertical,
            };
            let content_box = gtk::Box::new(orientation, style.text_icon_padding.max(0));
            if style.icon_position == IconPosition::Right {
                content_box.append(&text_box);
                content_box.append(&icon_slot);
            } else {
                content_box.append(&icon_slot);
                content_box.append(&text_box);
            }
            content_box.add_css_class("notification");
            content_box.upcast()
        } else {
            text_box.add_css_class("notification");
            text_box.upcast()
        };
        child.set_margin_top(style.padding);
        child.set_margin_bottom(style.padding);
        child.set_margin_start(style.h_padding);
        child.set_margin_end(style.h_padding);
        window.set_child(Some(&child));

        let hovered = Rc::new(Cell::new(false));
        let nw = Self {
            window,
            summary_label,
            body_label,
            font_attrs,
            app_name: app_name.to_string(),
            icon_slot,
            progress_slot,
            id,
            client,
            actions,
            on_event,
            presented: Cell::new(false),
            hovered: Rc::clone(&hovered),
            popover: RefCell::new(None),
        };
        nw.set_icon_and_progress(content, style);

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
    pub fn update_content(&self, content: &NotificationContent, style: &WindowStyle) {
        let css = gtk::CssProvider::new();
        css.load_from_data(&style_css(style));
        self.window
            .style_context()
            .add_provider(&css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);

        self.summary_label.set_attributes(Some(&self.font_attrs));
        self.body_label.set_attributes(Some(&self.font_attrs));
        self.summary_label.set_markup(&format!(
            "<b>{}</b>",
            render_text(style.markup, &content.summary)
        ));
        self.summary_label.set_halign(align_of(style.alignment));
        self.body_label
            .set_markup(&render_text(style.markup, &content.body));
        self.body_label.set_halign(align_of(style.alignment));
        self.body_label.set_wrap(style.word_wrap);
        self.body_label.set_ellipsize(ellipsize_of(style.ellipsize));
        self.set_icon_and_progress(content, style);
    }

    /// Rebuild the icon and progress-bar widgets for the current content
    /// (both live in slots so replaces_id updates can swap them).
    fn set_icon_and_progress(&self, content: &NotificationContent, style: &WindowStyle) {
        clear_children(self.icon_slot.upcast_ref::<gtk::Widget>());
        if let Some(icon) = crate::icons::icon_widget(
            &content.app_icon,
            &self.app_name,
            style,
            &WidgetExt::display(&self.window),
        ) {
            self.icon_slot.append(&icon);
        }

        clear_children(self.progress_slot.upcast_ref::<gtk::Widget>());
        let Some(value) = content.value.filter(|_| style.progress_bar) else {
            return;
        };
        let bar = gtk::ProgressBar::new();
        bar.set_fraction((value.clamp(0, 100) as f64) / 100.0);
        bar.set_halign(gtk::Align::Start);
        bar.add_css_class("progress");
        self.progress_slot.append(&bar);
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
            icons: true,
            icon_position: IconPosition::Left,
            min_icon_size: 32,
            max_icon_size: 64,
            text_icon_padding: 8,
            progress_bar: true,
            progress_bar_height: 10,
            progress_bar_frame_width: 1,
            progress_bar_min_width: 150,
            progress_bar_max_width: 300,
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
    fn progress_css_contains_configured_bounds() {
        let css = style_css(&style());
        assert!(css.contains("min-height: 10px;"), "{css}");
        assert!(css.contains("min-width: 150px;"), "{css}");
        assert!(css.contains("max-width: 300px;"), "{css}");
        assert!(css.contains("progressbar trough"), "{css}");
        assert!(css.contains("border-radius: 0;"), "{css}");
    }

    #[test]
    fn no_progress_css_when_disabled() {
        let mut s = style();
        s.progress_bar = false;
        let css = style_css(&s);
        assert!(!css.contains("progressbar"), "{css}");
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
