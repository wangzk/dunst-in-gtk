//! Icon resolution and rendering (ticket 07).
//!
//! dunst semantics (verified against dunst master `src/icon.c`,
//! `src/icon-lookup.c`, `src/dbus.c`):
//! - a non-empty `app_icon` is either a theme icon name or a file path
//!   (paths are recognized by a leading `/`, `~` or by containing `/`)
//! - the `image-path` hint overrides `app_icon` (handled in `dbus.rs`,
//!   exactly like dunst replaces `iconname`)
//! - icons are shown at `[min_icon_size, max_icon_size]` (0 = unbounded),
//!   implemented via GtkImage's `pixel-size` (the effective target is
//!   max_icon_size, min_icon_size as fallback — dunst's behavior)
//! - dunst shows nothing for missing icons; this project shows a
//!   first-letter placeholder instead (ticket requirement)
//!
//! Scaling strategy: icons are displayed through plain GtkImage widgets
//! with a logical `pixel-size`; GTK's scale factor renders them at the
//! correct device resolution (48 logical px = 96 physical px at
//! GDK_SCALE=2) and picks the best theme resource for the target
//! (`pixel_size x scale`), preferring vector icons when the theme
//! provides them. Verified empirically with a standalone probe under
//! GDK_SCALE=1 and =2 (see the icon HiDPI integration test): both
//! `from_icon_name` and `from_pixbuf` images scale 1x -> 2x correctly.
//! No manual scaling is done here.

use gtk4 as gtk;
use gtk4::pango;
use gtk4::prelude::*;

use crate::config::IconPosition;
use crate::window::WindowStyle;

/// Is `icon` a file path rather than a theme icon name?
pub fn is_path(icon: &str) -> bool {
    icon.starts_with('/') || icon.starts_with('~') || icon.contains('/')
}

/// First letter of the app name, for the missing-icon placeholder.
pub fn first_letter(app_name: &str) -> String {
    app_name
        .chars()
        .next()
        .map(|c| c.to_uppercase().collect::<String>())
        .unwrap_or_else(|| "?".to_string())
}

/// Fixed logical pixel size for icons: max wins (dunst's effective
/// behavior), min is the fallback when max is unbounded, None = natural
/// (no pixel-size, icon shows at its intrinsic size).
fn target_size(style: &WindowStyle) -> Option<i32> {
    if style.max_icon_size > 0 {
        Some(style.max_icon_size.max(style.min_icon_size.max(1)))
    } else if style.min_icon_size > 0 {
        Some(style.min_icon_size)
    } else {
        None
    }
}

/// Build the icon widget for a notification, honoring the style.
/// Returns None when no icon should be shown (icons disabled, position Off,
/// or no icon given).
pub fn icon_widget(
    icon: &str,
    app_name: &str,
    style: &WindowStyle,
    display: &gtk::gdk::Display,
) -> Option<gtk::Widget> {
    if !style.icons || style.icon_position == IconPosition::Off || icon.is_empty() {
        return None;
    }
    if is_path(icon) {
        file_icon(icon, style)
    } else {
        theme_icon(icon, app_name, style, display)
    }
}

fn theme_icon(
    name: &str,
    app_name: &str,
    style: &WindowStyle,
    display: &gtk::gdk::Display,
) -> Option<gtk::Widget> {
    let theme = gtk::IconTheme::for_display(display);
    let resolved = if theme.has_icon(name) {
        name
    } else if theme.has_icon("dialog-information") {
        // Generic fallback for unknown icon names.
        log::debug!("icon {name:?} not in theme, using dialog-information");
        "dialog-information"
    } else {
        log::debug!("no themed icon for {name:?}, using letter placeholder");
        return Some(letter_placeholder(app_name, style).upcast());
    };
    let image = gtk::Image::from_icon_name(resolved);
    if let Some(size) = target_size(style) {
        image.set_pixel_size(size);
    }
    image.set_valign(gtk::Align::Center);
    Some(image.upcast())
}

fn file_icon(path: &str, style: &WindowStyle) -> Option<gtk::Widget> {
    let pixbuf = match gtk::gdk_pixbuf::Pixbuf::from_file(path) {
        Ok(p) => p,
        Err(e) => {
            log::warn!("cannot load icon file {path:?}: {e}");
            return None;
        }
    };
    let image = gtk::Image::from_pixbuf(Some(&pixbuf));
    if let Some(size) = target_size(style) {
        image.set_pixel_size(size);
    }
    image.set_valign(gtk::Align::Center);
    Some(image.upcast())
}

/// Placeholder for a missing icon: the app name's first letter, sized like
/// an icon.
fn letter_placeholder(app_name: &str, style: &WindowStyle) -> gtk::Label {
    let label = gtk::Label::new(Some(&first_letter(app_name)));
    let size = target_size(style).unwrap_or(32).max(8);
    let attrs = pango::AttrList::new();
    // Absolute (pixel) size so the letter roughly matches the icon box.
    attrs.insert(pango::AttrSize::new_size_absolute(size * pango::SCALE));
    label.set_attributes(Some(&attrs));
    label.set_valign(gtk::Align::Center);
    label.add_css_class("icon-placeholder");
    label
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_detection() {
        assert!(is_path("/usr/share/icons/x.png"));
        assert!(is_path("~/icons/x.png"));
        assert!(is_path("rel/dir/icon.png"));
        assert!(is_path("./icon.png"));
        assert!(!is_path("dialog-information"));
        assert!(!is_path("firefox"));
        assert!(!is_path(""));
    }

    #[test]
    fn first_letter_upper() {
        assert_eq!(first_letter("firefox"), "F");
        assert_eq!(first_letter(""), "?");
        assert_eq!(first_letter(" irc"), " ");
    }
}
