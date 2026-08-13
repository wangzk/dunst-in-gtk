//! Icon resolution and rendering (ticket 07).
//!
//! dunst semantics (verified against dunst master `src/icon.c`,
//! `src/icon-lookup.c`, `src/dbus.c`):
//! - a non-empty `app_icon` is either a theme icon name or a file path
//!   (paths are recognized by a leading `/`, `~` or by containing `/`)
//! - the `image-path` hint overrides `app_icon` (handled in `dbus.rs`,
//!   exactly like dunst replaces `iconname`)
//! - icons are shown at `[min_icon_size, max_icon_size]` (0 = unbounded),
//!   implemented via GtkImage's `pixel-size` for theme icons (the effective
//!   target is max_icon_size, min_icon_size as fallback — dunst's behavior)
//! - dunst shows nothing for missing icons; this project shows a
//!   first-letter placeholder instead (ticket requirement)
//!
//! Scaling strategy:
//! - theme icons: `GtkImage::from_icon_name` + logical `pixel-size` — GTK3
//!   loads the icon at `pixel_size x scale_factor`, so HiDPI is automatic
//!   and vector icons stay crisp
//! - file icons (`image-path`): GtkImage's pixel-size does not apply to
//!   pixbufs, so we scale the pixbuf ourselves to the physical target
//!   (`target x scale`, dunst does the same)

use gtk::pango;
use gtk::prelude::*;

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
/// behavior), min is the fallback when max is unbounded, None = natural.
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
/// `scale` is the window's scale factor (1, 2, ...), used for file icons
/// (theme icons scale inside GTK).
/// Returns None when no icon should be shown (icons disabled, position Off,
/// or no icon given).
pub fn icon_widget(
    icon: &str,
    app_name: &str,
    style: &WindowStyle,
    scale: i32,
) -> Option<gtk::Widget> {
    if !style.icons || style.icon_position == IconPosition::Off || icon.is_empty() {
        return None;
    }
    if is_path(icon) {
        file_icon(icon, style, scale.max(1))
    } else {
        theme_icon(icon, app_name, style)
    }
}

fn theme_icon(name: &str, app_name: &str, style: &WindowStyle) -> Option<gtk::Widget> {
    let theme = gtk::IconTheme::new();
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
    let image = gtk::Image::from_icon_name(Some(resolved), gtk::IconSize::Invalid);
    if let Some(size) = target_size(style) {
        image.set_pixel_size(size);
    }
    image.set_valign(gtk::Align::Center);
    Some(image.upcast())
}

/// Load an icon from a file path. GtkImage's pixel-size does not apply to
/// pixbufs, so scale the pixbuf to the physical target ourselves.
fn file_icon(path: &str, style: &WindowStyle, scale: i32) -> Option<gtk::Widget> {
    let pixbuf = match gtk::gdk_pixbuf::Pixbuf::from_file(path) {
        Ok(p) => p,
        Err(e) => {
            log::warn!("cannot load icon file {path:?}: {e}");
            return None;
        }
    };
    let Some(target) = target_size(style) else {
        // No configured size: show the pixbuf at its natural size.
        let image = gtk::Image::from_pixbuf(Some(&pixbuf));
        image.set_valign(gtk::Align::Center);
        return Some(image.upcast());
    };
    let (w, h) = clamp_size(pixbuf.width(), pixbuf.height(), target * scale, target * scale);
    let scaled = match pixbuf.scale_simple(w, h, gtk::gdk_pixbuf::InterpType::Bilinear) {
        Some(s) => s,
        None => {
            log::warn!("cannot scale icon file {path:?}");
            return None;
        }
    };
    let image = gtk::Image::from_pixbuf(Some(&scaled));
    image.set_valign(gtk::Align::Center);
    Some(image.upcast())
}

/// Clamp a `(w, h)` size so the largest side stays within `[min, max]`;
/// 0 (or negative) means unbounded on that side. Aspect ratio preserved.
/// Both dimensions of 0 are returned as (1, 1) so callers never divide
/// by zero.
fn clamp_size(w: i32, h: i32, min: i32, max: i32) -> (i32, i32) {
    if w <= 0 && h <= 0 {
        return (1, 1);
    }
    let (w, h) = (w.max(1), h.max(1));
    let largest = w.max(h);
    let target = if max > 0 && largest > max {
        max
    } else if min > 0 && largest < min {
        min
    } else {
        return (w, h);
    };
    let scale = target as f64 / largest as f64;
    (
        ((w as f64) * scale).round().max(1.0) as i32,
        ((h as f64) * scale).round().max(1.0) as i32,
    )
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
    label.style_context().add_class("icon-placeholder");
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
    fn clamp_scales_down_to_max() {
        // 128x64 with max 64: largest side -> 64, aspect kept 2:1
        assert_eq!(clamp_size(128, 64, 0, 64), (64, 32));
        assert_eq!(clamp_size(64, 128, 0, 64), (32, 64));
        // already within bounds: untouched
        assert_eq!(clamp_size(48, 32, 32, 64), (48, 32));
    }

    #[test]
    fn clamp_scales_up_to_min() {
        assert_eq!(clamp_size(16, 16, 32, 64), (32, 32));
        assert_eq!(clamp_size(8, 32, 32, 64), (8, 32)); // largest side = 32 already
        assert_eq!(clamp_size(16, 8, 32, 0), (32, 16)); // unbounded max
    }

    #[test]
    fn clamp_handles_zero_and_negative() {
        assert_eq!(clamp_size(0, 0, 32, 64), (1, 1));
        assert_eq!(clamp_size(50, 50, 0, 0), (50, 50)); // no clamping at all
        assert_eq!(clamp_size(-5, 40, 0, 64), (1, 40));
    }

    #[test]
    fn clamp_scales_by_physical_target() {
        // scale=2: logical [48, 48] becomes physical [96, 96]
        assert_eq!(clamp_size(64, 64, 48 * 2, 48 * 2), (96, 96));
        assert_eq!(clamp_size(64, 64, 48, 48), (48, 48));
        // unbounded max stays unbounded when multiplied by scale
        assert_eq!(clamp_size(16, 8, 32 * 2, 0), (64, 32));
    }

    #[test]
    fn first_letter_upper() {
        assert_eq!(first_letter("firefox"), "F");
        assert_eq!(first_letter(""), "?");
        assert_eq!(first_letter(" irc"), " ");
    }
}
