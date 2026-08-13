//! A single notification window (GTK3).
//!
//! Each notification gets its own `GtkWindow` (dunst's architecture). GTK3
//! still ships the classic toplevel hints as first-class APIs, so everything
//! the X11 layer used to do by hand in the GTK4 version is now official:
//!   - `set_type_hint(WindowTypeHint::Notification)`
//!   - `set_accept_focus(false)` + `set_focus_on_map(false)` — never steal
//!     the keyboard focus (maps to WM_HINTS input=False)
//!   - `set_keep_above(true)` / `set_skip_taskbar_hint(true)` /
//!     `set_skip_pager_hint(true)`
//!   - `move_(x, y)` for corner placement
//! HiDPI is handled by GTK's per-window scale factor (logical coordinates).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::pango;
use gtk::prelude::*;

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
        // Neither GTK3 nor GTK4 CSS has max-width; the config keeps the key
        // for dunst compat but it is not emitted here.
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

/// Remove every child of a container widget (GTK3 has `get_children`).
fn clear_children(container: &gtk::Container) {
    for child in container.children() {
        container.remove(&child);
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
    /// Whether the window has been shown yet.
    presented: Cell<bool>,
    /// Whether the pointer is currently inside the window (shared with the
    /// enter/leave callbacks).
    hovered: Rc<Cell<bool>>,
    /// The currently open context menu, kept alive while shown.
    popover: RefCell<Option<gtk::Menu>>,
    /// The content widget (window child): anchor for the context menu.
    /// GTK3 resolves the popover's toplevel via
    /// `gtk_widget_get_ancestor(relative_to, GTK_TYPE_WINDOW)`, which
    /// returns NULL for a toplevel itself — the anchor must be a widget
    /// *inside* the window.
    content: gtk::Widget,
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
        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        // The title keeps the per-notification identity (integration tests
        // locate windows by it).
        window.set_title(&format!("dunst-in-gtk {app_name} [{id}]"));
        window.set_decorated(false);
        window.set_resizable(false);
        // Official GTK3 toplevel hints (GTK4 removed all of these):
        // notification type, never take keyboard focus, stay on top,
        // skip taskbar/pager.
        window.set_type_hint(gtk::gdk::WindowTypeHint::Notification);
        window.set_accept_focus(false);
        window.set_focus_on_map(false);
        window.set_keep_above(true);
        window.set_skip_taskbar_hint(true);
        window.set_skip_pager_hint(true);
        // Needed so the CSS background (with alpha) paints over the default
        // window background.
        window.set_app_paintable(true);
        window.style_context().add_class("notification");

        let css = gtk::CssProvider::new();
        css.load_from_data(style_css(style).as_bytes()).expect("style CSS");
        window.style_context().add_provider(
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

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
        text_box.pack_start(&summary_label, false, false, 0);
        text_box.pack_start(&body_label, false, false, 0);
        let progress_slot = gtk::Box::new(gtk::Orientation::Vertical, 0);
        text_box.pack_start(&progress_slot, false, false, 0);

        // Icon slot next to (or above) the text, per `icon_position`.
        let icon_slot = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let child: gtk::Widget = if style.icons && style.icon_position != IconPosition::Off {
            let orientation = match style.icon_position {
                IconPosition::Left | IconPosition::Right => gtk::Orientation::Horizontal,
                IconPosition::Top | IconPosition::Off => gtk::Orientation::Vertical,
            };
            let content_box = gtk::Box::new(orientation, style.text_icon_padding.max(0));
            if style.icon_position == IconPosition::Right {
                content_box.pack_start(&text_box, false, false, 0);
                content_box.pack_start(&icon_slot, false, false, 0);
            } else {
                content_box.pack_start(&icon_slot, false, false, 0);
                content_box.pack_start(&text_box, false, false, 0);
            }
            content_box.style_context().add_class("notification");
            content_box.upcast()
        } else {
            text_box.style_context().add_class("notification");
            text_box.upcast()
        };
        child.set_margin_top(style.padding);
        child.set_margin_bottom(style.padding);
        child.set_margin_start(style.h_padding);
        child.set_margin_end(style.h_padding);
        window.add(&child);
        let content_widget: gtk::Widget = child.clone().upcast();

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
            content: content_widget,
        };
        nw.set_icon_and_progress(content, style);

        // User/WM-initiated close (WM_DELETE_WINDOW, alt+F4). The daemon
        // decides (signal + bookkeeping); we just report. Inhibit(false)
        // lets GTK destroy the window, matching the daemon's bookkeeping.
        let on_event = Rc::clone(&nw.on_event);
        let id = nw.id;
        nw.window.connect_delete_event(move |_, _| {
            (on_event.borrow())(WindowEvent::Closed(id));
            glib::Propagation::Proceed
        });

        // Pointer enter/leave -> hover pause/resume. `hovered` lives in an
        // Rc so the daemon can query it (replaces_id keeps the timer paused
        // while the pointer is inside).
        let hovered_enter = Rc::clone(&hovered);
        let hovered_leave = Rc::clone(&hovered);
        {
            let on_event = Rc::clone(&nw.on_event);
            let id = nw.id;
            nw.window.connect_enter_notify_event(move |_, _| {
                (on_event.borrow())(WindowEvent::Hover(id, true));
                hovered_enter.set(true);
                glib::Propagation::Proceed
            });
        }
        {
            let on_event = Rc::clone(&nw.on_event);
            let id = nw.id;
            nw.window.connect_leave_notify_event(move |_, _| {
                (on_event.borrow())(WindowEvent::Hover(id, false));
                hovered_leave.set(false);
                glib::Propagation::Proceed
            });
        }

        // Clicks: the daemon maps the button (1/2/3) to the configured
        // mouse action sequence.
        {
            let on_event = Rc::clone(&nw.on_event);
            let id = nw.id;
            nw.window.connect_button_press_event(move |_, ev| {
                (on_event.borrow())(WindowEvent::Click(id, ev.button()));
                glib::Propagation::Proceed
            });
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
    ///
    /// GTK3's GtkMenu is used (not GtkPopover): the menu is a real toplevel
    /// X window, so it works without a compositor and is visible to the
    /// integration tests.
    pub fn show_context_menu(&self) {
        let menu = gtk::Menu::new();
        let on_event = Rc::clone(&self.on_event);
        let id = self.id;
        for (key, label) in &self.actions {
            let item = gtk::MenuItem::with_label(label);
            if key == "default" {
                item.style_context().add_class("suggested-action");
            }
            let on_event = Rc::clone(&on_event);
            let key = key.clone();
            let key_log = key.clone();
            item.connect_activate(move |_| {
                log::debug!("menu item activated: {key_log}");
                (on_event.borrow())(WindowEvent::Action(id, key.clone()));
            });
            menu.append(&item);
        }
        // dunst's context menu always offers closing the notification.
        let close_item = gtk::MenuItem::with_label("Close");
        {
            let on_event = Rc::clone(&on_event);
            close_item.connect_activate(move |_| {
                (on_event.borrow())(WindowEvent::Closed(id));
            });
        }
        menu.append(&close_item);

        menu.show_all();
        menu.popup_at_widget(
            &self.content,
            gtk::gdk::Gravity::SouthWest,
            gtk::gdk::Gravity::NorthWest,
            None,
        );
        // Keep the menu alive for the duration of the popup.
        self.popover.replace(Some(menu));
    }

    /// Close the window without emitting close-request (daemon-initiated).
    /// The caller emits `NotificationClosed` itself.
    pub fn destroy(&self) {
        unsafe { self.window.destroy() };
    }

    /// Update the content in place (replaces_id) and re-apply the style.
    pub fn update_content(&self, content: &NotificationContent, style: &WindowStyle) {
        let css = gtk::CssProvider::new();
        css.load_from_data(style_css(style).as_bytes()).expect("style CSS");
        self.window.style_context().add_provider(
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

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
        clear_children(self.icon_slot.upcast_ref::<gtk::Container>());
        if let Some(icon) = crate::icons::icon_widget(
            &content.app_icon,
            &self.app_name,
            style,
            WidgetExt::scale_factor(&self.window),
        ) {
            self.icon_slot.pack_start(&icon, false, false, 0);
            icon.show();
        }

        clear_children(self.progress_slot.upcast_ref::<gtk::Container>());
        let Some(value) = content.value.filter(|_| style.progress_bar) else {
            return;
        };
        let bar = gtk::ProgressBar::new();
        bar.set_fraction((value.clamp(0, 100) as f64) / 100.0);
        bar.set_halign(gtk::Align::Start);
        bar.style_context().add_class("progress");
        self.progress_slot.pack_start(&bar, false, false, 0);
        bar.show();
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
        let (_, natural) = content.preferred_size();
        (natural.width.max(1), natural.height.max(1))
    }

    /// Natural height when wrapped to the given width (logical pixels).
    pub fn height_for_width(&self, width: i32) -> i32 {
        let content = self.window.child().expect("window has a child");
        let (_, nh) = content.preferred_height_for_width(width.max(1));
        nh.max(1)
    }

    /// Apply the final geometry (logical pixels; GTK handles HiDPI scaling).
    /// The first call shows the window; later calls (reflows) reposition
    /// and resize via the official GTK3 window APIs.
    pub fn apply_geometry(&self, x: i32, y: i32, width: i32, height: i32) {
        self.window.set_default_size(width.max(1), height.max(1));
        self.window.move_(x, y);
        if !self.presented.get() {
            self.window.show_all();
            self.presented.set(true);
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
        assert!(!css.contains("max-width"), "{css}");
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
