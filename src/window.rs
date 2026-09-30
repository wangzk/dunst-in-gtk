//! A single notification window (GTK3).
//!
//! Each notification gets its own `GtkWindow` (dunst's architecture). The
//! window is made **override-redirect** before mapping so no window manager
//! manages it: i3 otherwise inserts a new window into the *focused*
//! workspace and relocates it when the requested coordinates are on another
//! output, which makes it impossible to show a notification on a
//! non-focused monitor (see `apply_geometry`).
//!
//! GTK3 still ships the classic toplevel hints as first-class APIs, so the
//! hints the GTK4 version set by hand are now official — they remain useful
//! as a description if the window is not override-redirect (non-X11):
//!   - `set_type_hint(WindowTypeHint::Notification)`
//!   - `set_accept_focus(false)` + `set_focus_on_map(false)` — never steal
//!     the keyboard focus (maps to WM_HINTS input=False)
//!   - `set_keep_above(true)` / `set_skip_taskbar_hint(true)` /
//!     `set_skip_pager_hint(true)`
//!   - `move_(x, y)` for corner placement
//!
//! Sizing: GTK sizes a non-resizable toplevel at `max(default_size,
//! natural_size)`, so the content's natural width is capped explicitly
//! (`set_layout_width`) — otherwise a long unwrappable line makes the window
//! wider than the `width` spec and it straddles two monitors. Measurement
//! only works once the widgets are visible, hence the `show_all` on the
//! content in `new()` (the toplevel itself stays hidden until placement).
//!
//! HiDPI is handled by GTK's per-window scale factor (logical coordinates).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use glib::translate::ToGlibPtr;
use gtk::pango;
use gtk::prelude::*;

use crate::config::{Alignment, Color, Config, Ellipsize, IconPosition, Markup, VerticalAlignment};

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
    // NOTE: never set a background on the `window.notification` node —
    // GTK3 then skips redrawing the whole window (contents stay black on
    // a real WM). The window background is the theme's; the inner box
    // carries the notification background/border/radius. Padding on the
    // box keeps the background/border filling the whole window (content
    // inset), matching dunst's look.
    let padding = style.padding.max(0);
    let hpad = style.h_padding.max(0);
    format!(
        r#"
window.notification box.notification {{
    background-color: {bg};
    border: {fw}px solid {frame};
    border-radius: {radius}px;
    padding: {padding}px {hpad}px;
}}
window.notification label {{
    color: {fg};
}}
window.notification label.time {{
    color: #888888;
    font-weight: bold;
}}
window.notification label.app {{
    color: #888888;
    font-weight: bold;
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
    /// When the notification was generated (unix seconds); rendered as a
    /// small light-gray time label in the window's top-right corner.
    pub timestamp: u64,
}

/// Remove every child of a container widget (GTK3 has `get_children`).
fn clear_children(container: &gtk::Container) {
    for child in container.children() {
        container.remove(&child);
    }
}

/// Format a unix timestamp as a local `HH:MM` string; None when the
/// timestamp is missing or the local timezone is unavailable.
fn format_time(timestamp: u64) -> Option<String> {
    if timestamp == 0 {
        return None;
    }
    let dt = glib::DateTime::from_unix_local(timestamp as i64).ok()?;
    Some(dt.format("%H:%M").ok()?.to_string())
}

/// Set the time label text for `timestamp`, hiding it when unrenderable.
fn apply_timestamp(label: &gtk::Label, timestamp: u64) {
    match format_time(timestamp) {
        Some(t) => {
            label.set_text(&t);
            label.show();
        }
        None => label.hide(),
    }
}

/// Maximum number of text lines per label (summary/body) when word wrap is
/// on; text beyond this is ellipsized on the last visible line.
///
/// GTK3 quirk (verified empirically, see `examples/label_probe.rs`): setting
/// both `wrap` and `ellipsize` on a GtkLabel *without* `lines` degrades to a
/// single ellipsized line — the label never wraps, so long notification
/// bodies were shown as "...content...". `lines` restores real wrapping with
/// a cap: the label wraps up to N lines and ellipsizes the Nth. The window
/// height then grows with the wrapped content (height specs `(min, max)` /
/// natural let it).
const MAX_WRAP_LINES: i32 = 5;

// ---- Fade animation (official APIs: set_opacity + add_tick_callback) ----

/// Fade-in duration, milliseconds.
const FADE_IN_MS: i64 = 450;
/// Fade-out duration, milliseconds.
const FADE_OUT_MS: i64 = 350;

/// Cubic ease-out: fast start, gentle landing (fade-in).
fn ease_out_cubic(t: f64) -> f64 {
    1.0 - (1.0 - t).powi(3)
}

/// Cubic ease-in: slow start, accelerating exit (fade-out).
fn ease_in_cubic(t: f64) -> f64 {
    t.powi(3)
}

pub struct NotificationWindow {
    window: gtk::Window,
    summary_label: gtk::Label,
    body_label: gtk::Label,
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
    /// Whether an ARGB visual was available (compositor present): opacity
    /// animation only has a visual effect on such windows.
    has_argb: Cell<bool>,
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
    /// The time label floating at the window's top-right corner.
    time_label: gtk::Label,
    /// The app-name label floating at the window's top-left corner
    /// (gray italic). Hidden when the app name is empty. Configured once
    /// at construction: the app name never changes for a window
    /// (replaces_id keeps the same client).
    #[allow(dead_code)]
    app_label: gtk::Label,
    /// The screen this window's CSS provider is registered on (removed on
    /// destroy; see `css_provider`).
    screen: gtk::gdk::Screen,
    /// The screen-level CSS provider for this window's style. Kept here so
    /// it can be replaced on style updates and removed on destroy — screen
    /// providers are never freed automatically and would accumulate over
    /// the daemon's lifetime.
    css_provider: RefCell<gtk::CssProvider>,
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
        // Real translucency needs an ARGB visual, which requires a
        // compositor (picom). With one, the rgba background alpha shows
        // through to the desktop (picom applies blur/shadow); without
        // one, gdk returns no rgba visual and the theme background is
        // used instead (GTK pre-mixes the rgba onto it — still readable).
        // app_paintable tells GTK not to paint the window background so
        // the alpha is not flattened by a theme background fill.
        let mut has_argb = false;
        if let Some(screen) = gtk::gdk::Screen::default() {
            if let Some(visual) = screen.rgba_visual() {
                window.set_visual(Some(&visual));
                window.set_app_paintable(true);
                has_argb = true;
                log::debug!("window uses ARGB visual (compositor present)");
            }
        }
        window.style_context().add_class("notification");

        // NOTE: widget-level add_provider() does not apply CSS on this
        // system's GTK3 (3.24.52) — styles were silently ignored (verified
        // with a minimal C program). Screen-level providers work; a fresh
        // provider per notification wins over earlier ones (same priority,
        // later addition wins in GTK3), so style updates apply.
        let screen = gtk::gdk::Screen::default().expect("default screen at window creation");
        let css_provider = gtk::CssProvider::new();
        if let Err(e) = css_provider.load_from_data(style_css(style).as_bytes()) {
            log::warn!("CSS load error: {e}");
        }
        gtk::StyleContext::add_provider_for_screen(
            &screen,
            &css_provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        let font_attrs = font_attr_list(&style.font);

        // The summary is always bold, via a Pango attribute (not the
        // markup <b> tag, which would be escaped when markup=no).
        let summary_attrs = font_attr_list(&style.font);
        summary_attrs.insert(pango::AttrInt::new_weight(pango::Weight::Bold));
        let summary_label = gtk::Label::new(None);
        summary_label.set_markup(&render_text(style.markup, &content.summary));
        summary_label.set_halign(align_of(style.alignment));
        summary_label.set_attributes(Some(&summary_attrs));
        // Wrap + ellipsize like the body: an unwrapped single-line summary
        // drives the natural width past the configured `width` spec, and a
        // non-resizable GTK3 window then renders at its (much wider)
        // natural size, ignoring the geometry the layout computed — the
        // window ends up straddling monitors instead of sitting inside one.
        summary_label.set_wrap(style.word_wrap);
        summary_label.set_ellipsize(ellipsize_of(style.ellipsize));
        if style.word_wrap {
            summary_label.set_lines(MAX_WRAP_LINES);
        }

        let body_label = gtk::Label::new(None);
        body_label.set_markup(&render_text(style.markup, &content.body));
        body_label.set_halign(align_of(style.alignment));
        body_label.set_wrap(style.word_wrap);
        body_label.set_ellipsize(ellipsize_of(style.ellipsize));
        if style.word_wrap {
            body_label.set_lines(MAX_WRAP_LINES);
        }
        body_label.set_attributes(Some(&font_attrs));

        // Header row: the app name (gray italic, left) and the timestamp
        // (gray, right). A real row in the layout — not overlay children —
        // so the labels can never overlap the summary text; a floating
        // overlay pinned to the top corners sat on top of the summary.
        let time_label = gtk::Label::new(None);
        time_label.style_context().add_class("time");
        let time_attrs = font_attr_list(&style.font);
        time_attrs.insert(pango::AttrInt::new_weight(pango::Weight::Bold));
        time_label.set_attributes(Some(&time_attrs));
        time_label.set_halign(gtk::Align::End);
        apply_timestamp(&time_label, content.timestamp);

        let app_label = gtk::Label::new(None);
        app_label.style_context().add_class("app");
        let app_attrs = font_attr_list(&style.font);
        // Bold via a Pango attribute like the summary label: it wins over
        // the theme and works even where CSS font-weight is ignored.
        app_attrs.insert(pango::AttrInt::new_weight(pango::Weight::Bold));
        app_label.set_attributes(Some(&app_attrs));
        app_label.set_halign(gtk::Align::Start);
        app_label.set_ellipsize(pango::EllipsizeMode::End);
        app_label.set_single_line_mode(true);
        if app_name.is_empty() {
            app_label.hide();
        } else {
            app_label.set_text(app_name);
        }

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header.pack_start(&app_label, true, false, 0);
        header.pack_end(&time_label, false, false, 0);

        // Text column: summary, body, then the progress-bar slot. It stays
        // non-expanding (natural width) inside the icon row — expanding it
        // would make the labels' configured halign apply across the whole
        // window width (everything visibly centered).
        let text_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        text_box.pack_start(&summary_label, false, false, 0);
        text_box.pack_start(&body_label, false, false, 0);
        let progress_slot = gtk::Box::new(gtk::Orientation::Vertical, 0);
        text_box.pack_start(&progress_slot, false, false, 0);

        // Icon slot next to (or above) the text, per `icon_position`.
        let icon_slot = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let inner: gtk::Widget = if style.icons && style.icon_position != IconPosition::Off {
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
            content_box.upcast()
        } else {
            text_box.upcast()
        };
        // Top-level vertical layout: the header row first, then the icon +
        // text content. The outer box is the window child and fills the
        // window width (default halign fill), so the header spans edge to
        // edge (app name left, time right) while `inner` keeps its original
        // natural-width layout below it. Both header labels hidden -> the
        // header gets no allocation (0 height, no spacing gap).
        let main_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        main_box.pack_start(&header, false, false, 0);
        main_box.pack_start(&inner, false, false, 0);
        main_box.style_context().add_class("notification");
        let child: gtk::Widget = main_box.upcast();
        // Make the content measurable without mapping the window:
        // preferred_size() only returns real values once the widgets are
        // marked visible (it reports 0x0 while they are hidden, which used
        // to make relayout measure 1x1 and fall back to pure guesswork).
        // The toplevel stays hidden until apply_geometry maps it at its
        // final position.
        child.show_all();
        // show_all() forces both header labels visible again; re-apply
        // their intended visibility (hidden when the timestamp is
        // unrenderable or the app name empty).
        apply_timestamp(&time_label, content.timestamp);
        if app_name.is_empty() {
            app_label.hide();
        }
        window.add(&child);
        let content_widget: gtk::Widget = child.clone().upcast();

        let hovered = Rc::new(Cell::new(false));
        let nw = Self {
            window,
            summary_label,
            body_label,
            app_name: app_name.to_string(),
            icon_slot,
            progress_slot,
            id,
            client,
            actions,
            on_event,
            presented: Cell::new(false),
            has_argb: Cell::new(has_argb),
            hovered: Rc::clone(&hovered),
            popover: RefCell::new(None),
            content: content_widget,
            time_label,
            app_label,
            screen,
            css_provider: RefCell::new(css_provider),
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
            item.connect_activate(move |_| {
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

    /// Animate the window opacity from `from` to `to` over `duration_ms`
    /// using a tick callback (official GTK3 frame-clock API: the callback
    /// fires in sync with the frame clock, i.e. once per displayed frame —
    /// 60fps on a 60Hz monitor). Non-linear easing: cubic ease-out when
    /// fading in, cubic ease-in when fading out. `on_done` runs once at the
    /// end (always, even when animation is skipped or fails to start).
    fn start_fade(
        &self,
        from: f64,
        to: f64,
        duration_ms: i64,
        on_done: Option<Box<dyn Fn() + 'static>>,
    ) {
        let done = RefCell::new(on_done);
        let finish = move |done: &RefCell<Option<Box<dyn Fn() + 'static>>>| {
            if let Some(f) = done.borrow_mut().take() {
                f();
            }
        };
        // Without an ARGB visual (no compositor) opacity has no visible
        // effect — skip straight to the end state.
        if !self.has_argb.get() {
            self.window.set_opacity(to);
            finish(&done);
            return;
        }
        let Some(clock) = self.window.frame_clock() else {
            // No frame clock yet: cannot animate; jump to the target.
            self.window.set_opacity(to);
            finish(&done);
            return;
        };
        let start = clock.frame_time(); // microseconds
        let window = self.window.clone();
        self.window.add_tick_callback(move |_, clock| {
            let elapsed_us = (clock.frame_time() - start).max(0) as f64;
            let t = (elapsed_us / (duration_ms as f64 * 1000.0)).clamp(0.0, 1.0);
            let eased = if to > from {
                ease_out_cubic(t)
            } else {
                ease_in_cubic(t)
            };
            window.set_opacity(from + (to - from) * eased);
            if t >= 1.0 {
                finish(&done);
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }

    /// Fade the window out (cubic ease-in), then destroy it. The daemon
    /// calls this instead of `destroy()` so notifications disappear with
    /// the same animation they appear with. Bookkeeping stays with the
    /// caller; the window dies on its own once the animation completes.
    pub fn fade_out_destroy(&self) {
        let window = self.window.clone();
        let screen = self.screen.clone();
        let provider = self.css_provider.borrow().clone();
        let on_done: Box<dyn Fn() + 'static> = Box::new(move || {
            gtk::StyleContext::remove_provider_for_screen(&screen, &provider);
            unsafe { window.destroy() };
        });
        // Fade out from the *current* opacity so a close during fade-in
        // does not jump to full opacity first.
        let from = self.window.opacity();
        self.start_fade(from, 0.0, FADE_OUT_MS, Some(on_done));
    }

    /// Update the content in place (replaces_id) and re-apply the style.
    pub fn update_content(&self, content: &NotificationContent, style: &WindowStyle) {
        // Replace the screen-level provider (the only mechanism that works
        // on this GTK3, see `new`): remove the old one, then add a fresh
        // provider with the new CSS. Keeps exactly one provider per live
        // window.
        let new_css = gtk::CssProvider::new();
        if let Err(e) = new_css.load_from_data(style_css(style).as_bytes()) {
            log::warn!("CSS load error on update: {e}");
        }
        gtk::StyleContext::add_provider_for_screen(
            &self.screen,
            &new_css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        gtk::StyleContext::remove_provider_for_screen(&self.screen, &*self.css_provider.borrow());
        // Swap in the new provider as this window's active one.
        *self.css_provider.borrow_mut() = new_css;

        // Fresh attribute lists per label: `font_attrs.clone()` would share
        // the underlying list, so inserting the summary's bold weight would
        // also make the body and time labels bold.
        let font_attrs = font_attr_list(&style.font);
        let summary_attrs = font_attr_list(&style.font);
        summary_attrs.insert(pango::AttrInt::new_weight(pango::Weight::Bold));
        self.summary_label.set_attributes(Some(&summary_attrs));
        self.body_label.set_attributes(Some(&font_attrs));
        self.summary_label
            .set_markup(&render_text(style.markup, &content.summary));
        self.summary_label.set_halign(align_of(style.alignment));
        self.summary_label.set_wrap(style.word_wrap);
        self.summary_label
            .set_ellipsize(ellipsize_of(style.ellipsize));
        self.summary_label.set_lines(if style.word_wrap {
            MAX_WRAP_LINES
        } else {
            -1
        });
        self.body_label
            .set_markup(&render_text(style.markup, &content.body));
        self.body_label.set_halign(align_of(style.alignment));
        self.body_label.set_wrap(style.word_wrap);
        self.body_label.set_ellipsize(ellipsize_of(style.ellipsize));
        self.body_label.set_lines(if style.word_wrap {
            MAX_WRAP_LINES
        } else {
            -1
        });
        // The time label stays bold across style updates (own attribute
        // list, never shared with the regular-weight body font attrs).
        let time_attrs = font_attr_list(&style.font);
        time_attrs.insert(pango::AttrInt::new_weight(pango::Weight::Bold));
        self.time_label.set_attributes(Some(&time_attrs));
        apply_timestamp(&self.time_label, content.timestamp);
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

    /// Cap the labels' natural width so the window really renders at the
    /// layout-computed width. GTK3 sizes a non-resizable window at
    /// `max(default_size, natural_size)`, so a long unwrappable line would
    /// otherwise make the window render wider than the width spec — on a
    /// multi-monitor setup the window then straddles two screens instead of
    /// sitting inside one (verified on i3 with a rotated secondary monitor).
    /// `max_width_chars` is the only mechanism that caps the natural size
    /// (default size, size request and geometry hints are all overridden by
    /// it).
    ///
    /// The char->pixel factor must come from the *configured* font (the
    /// Pango attributes on the labels), not from the context's default font;
    /// since the remaining error is a few percent (CJK glyphs, icon slot,
    /// CSS box), the cap is then corrected iteratively against the measured
    /// natural width.
    pub fn set_layout_width(&self, width: i32, style: &WindowStyle) {
        // Space the labels cannot use: horizontal CSS padding + border on
        // both sides, plus the icon column when it sits left/right.
        let overhead = 2 * (style.h_padding.max(0) + style.frame_width.max(0))
            + if style.icons
                && matches!(
                    style.icon_position,
                    IconPosition::Left | IconPosition::Right
                )
            {
                style.max_icon_size.max(0) + style.text_icon_padding.max(0)
            } else {
                0
            };
        let budget = (width - overhead).max(1);
        let desc = pango::FontDescription::from_string(&style.font);
        let ctx = self.body_label.pango_context();
        let metrics = ctx.metrics(Some(&desc), None);
        // Pango returns 1/1024 px units; this is the font's average glyph
        // width, which is what max-width-chars is multiplied by.
        let avg = (metrics.approximate_char_width() as f64 / pango::SCALE as f64).max(1.0);

        let mut chars = ((budget as f64 / avg).floor() as i32).max(1);
        for _ in 0..4 {
            self.summary_label.set_max_width_chars(chars);
            self.body_label.set_max_width_chars(chars);
            let natural = self.natural_size().0;
            if natural <= width {
                break;
            }
            // Shrink by the measured excess (plus one char of slack) and
            // re-measure; converges in one or two rounds.
            let reduce = (((natural - width) as f64 / avg).ceil() as i32) + 1;
            let next = (chars - reduce).max(1);
            if next == chars {
                break;
            }
            chars = next;
        }
        log::debug!("set_layout_width({width}): overhead={overhead} avg={avg:.2} chars={chars}");
    }

    /// Apply the final geometry (logical pixels; GTK handles HiDPI scaling).
    /// The first call shows the window; later calls (reflows) reposition
    /// and resize via the official GTK3 window APIs. `resize` is required
    /// for reflows: `set_default_size` only affects a window that has not
    /// been mapped yet, so an already-shown window would keep its old size.
    pub fn apply_geometry(&self, x: i32, y: i32, width: i32, height: i32) {
        let (w, h) = (width.max(1), height.max(1));
        self.window.set_default_size(w, h);
        self.window.move_(x, y);
        if !self.presented.get() {
            // Make the toplevel override-redirect before mapping: the WM
            // must not manage notification windows. Without this, i3
            // inserts the window into the *focused* workspace and, when the
            // requested coordinates are on a different output, moves it
            // there (manage.c stores the client geometry, then
            // floating_enable -> floating_fix_coordinates remaps it onto
            // the focused output), so notifications could never appear on a
            // non-focused monitor. i3 explicitly skips override-redirect
            // windows in manage_window() (attr->override_redirect), and GTK3
            // itself uses OR windows for menus/tooltips, so input/redraw
            // keep working. dunst does the same on X11.
            //
            // Ordering matters: realize() creates the GdkWindow (which is
            // the only moment the X override-redirect attribute can be set),
            // then the position is re-asserted (after realize it is a plain
            // XMoveWindow, with no WM in between) and only then mapped.
            self.window.realize();
            // gdk_window_set_override_redirect() is X11-only; on other
            // backends GTK ignores it (and cannot position windows anyway).
            // NOTE: gdk_display_get_name() returns the *display string*
            // (":0"), not the backend — the backend is the GType name, which
            // is exactly how the gdk crate itself detects it.
            let is_x11 = gtk::gdk::Display::default()
                .map(|d| d.type_().name() == "GdkX11Display")
                .unwrap_or(false);
            if is_x11 {
                if let Some(gdkwin) = self.window.window() {
                    unsafe {
                        // gboolean TRUE == 1 (glib-sys has no exported const).
                        gdk_sys::gdk_window_set_override_redirect(gdkwin.to_glib_none().0, 1)
                    };
                }
            } else {
                log::debug!("non-X11 backend: window stays WM-managed");
            }
            self.window.move_(x, y);
            // Fade in: start fully transparent *before* mapping so the
            // first presented frame is invisible, then animate to opaque.
            self.window.set_opacity(0.0);
            self.window.show_all();
            self.presented.set(true);
            // show_all maps the window (creating its frame clock), so the
            // tick callback can start syncing to frames right away.
            self.start_fade(0.0, 1.0, FADE_IN_MS, None);
        } else {
            self.window.resize(w, h);
        }
    }
}

/// Build a Pango attribute list applying `font` at regular weight.
/// Every label needs its own list: `AttrList` clones are shallow (they share
/// the underlying PangoAttrList), so a fresh list must be built per label —
/// otherwise inserting the summary's bold weight would also bold the body
/// and time labels.
fn font_attr_list(font: &str) -> pango::AttrList {
    let desc = pango::FontDescription::from_string(font);
    let attrs = pango::AttrList::new();
    attrs.insert(pango::AttrFontDesc::new(&desc));
    attrs
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
            background: Color {
                r: 0,
                g: 0,
                b: 0,
                a: 0xcc,
            },
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

    #[test]
    fn header_labels_css_is_gray_bold() {
        let css = style_css(&style());
        assert!(css.contains("label.time"), "{css}");
        assert!(css.contains("label.app"), "{css}");
        assert!(css.contains("#888888"), "{css}");
        assert!(css.contains("font-weight: bold"), "{css}");
        assert!(!css.contains("font-style"), "{css}");
    }

    #[test]
    fn format_time_hides_missing_timestamp() {
        assert_eq!(format_time(0), None);
    }

    #[test]
    fn easing_is_non_linear() {
        // Halfway through the timeline, ease-out is already past 80%
        // (fast start) and ease-in is under 20% (slow start) — neither is
        // the linear 0.5.
        assert!((ease_out_cubic(0.5) - 0.875).abs() < 1e-9);
        assert!((ease_in_cubic(0.5) - 0.125).abs() < 1e-9);
        // Endpoints are exact.
        assert_eq!(ease_out_cubic(0.0), 0.0);
        assert_eq!(ease_out_cubic(1.0), 1.0);
        assert_eq!(ease_in_cubic(0.0), 0.0);
        assert_eq!(ease_in_cubic(1.0), 1.0);
        // Monotonic over the timeline.
        let mut prev = 0.0;
        for i in 1..=100 {
            let t = i as f64 / 100.0;
            let v = ease_out_cubic(t);
            assert!(v >= prev);
            prev = v;
        }
    }
}
