//! Pure layout math for notification placement (ticket 03).
//!
//! Semantics follow dunst: a stack of notifications grows away from the
//! configured origin (9 positions), inset by `offset`, spaced by `gap_size`.
//! Sizes come from the dunstrc `width`/`height` specs (constant, min-max
//! range, or percent of the monitor) clamped around the content's natural
//! size. All coordinates are logical (device-independent) pixels; the X11
//! layer converts by the surface scale factor for HiDPI.
//!
//! This module is pure: no GTK, no D-Bus — the whole geometry contract is
//! unit-testable here.

use crate::config::{Origin, SizeSpec};

/// Logical geometry of a monitor (as reported by GDK).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorGeometry {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// Final geometry of one notification window (logical pixels).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // returned by callers in later tickets
pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// Apply a dunstrc size spec to a natural content size.
///
/// - `Constant(0)` / `Range(0, 0)`: natural size.
/// - `Constant(w)` with w > 0: fixed size.
/// - `Range(min, max)`: natural clamped into [min, max] (0 = unbounded side).
/// - `Percent(p)`: `p * monitor` (0.62 = 62%), clamped into the range when
///   both are given (percent wins as the base, range clamps).
pub fn resolve_size(
    spec: SizeSpec,
    natural: i32,
    monitor_extent: i32,
) -> i32 {
    match spec {
        SizeSpec::Constant(0) => natural,
        SizeSpec::Constant(w) if w < 0 => natural,
        SizeSpec::Constant(w) => w,
        SizeSpec::Percent(p) => {
            let w = (monitor_extent as f64 * p) as i32;
            w.max(1)
        }
        SizeSpec::Range(min, max) => {
            let min = min.max(0);
            let max = if max <= 0 { i32::MAX } else { max };
            natural.clamp(min, max)
        }
    }
}

/// Stack placement: position of the notification at `index` in `stack`
/// (outermost first). `stack` holds the sizes of every displayed
/// notification in stacking order, including the one being placed.
///
/// The stack grows away from the origin: a top origin stacks downward,
/// a bottom origin upward, a left origin rightward, a right origin leftward.
pub fn stack_position(
    origin: Origin,
    offset: (i32, i32),
    gap: i32,
    monitor: MonitorGeometry,
    stack: &[(i32, i32)],
    index: usize,
) -> (i32, i32) {
    let (w, h) = stack[index];
    let gap = gap.max(0);
    let (ox, oy) = offset;
    let n = stack.len().max(1);

    // Totals over the whole stack (for center origins, which center the
    // entire stack) and the extents of everything before `index`.
    let total_w: i32 = stack.iter().map(|s| s.0).sum::<i32>() + gap * (n - 1) as i32;
    let total_h: i32 = stack.iter().map(|s| s.1).sum::<i32>() + gap * (n - 1) as i32;
    let before_w: i32 =
        stack[..index].iter().map(|s| s.0).sum::<i32>() + gap * index as i32;
    let before_h: i32 =
        stack[..index].iter().map(|s| s.1).sum::<i32>() + gap * index as i32;

    let x = match origin {
        // Corners: fixed edge alignment, the stack never grows horizontally.
        Origin::TopLeft | Origin::BottomLeft => monitor.x + ox,
        Origin::TopRight | Origin::BottomRight => monitor.x + monitor.width - ox - w,
        // Top/bottom centers: each window horizontally centered.
        Origin::TopCenter | Origin::BottomCenter => monitor.x + (monitor.width - w) / 2,
        // Middle left/right: the stack grows horizontally.
        Origin::Left => monitor.x + ox + before_w,
        Origin::Right => monitor.x + monitor.width - ox - w - before_w,
        // Center: the whole stack is horizontally centered.
        Origin::Center => monitor.x + (monitor.width - total_w) / 2 + before_w,
    };

    let y = match origin {
        Origin::TopLeft | Origin::TopCenter | Origin::TopRight => {
            monitor.y + oy + before_h
        }
        Origin::BottomLeft | Origin::BottomCenter | Origin::BottomRight => {
            monitor.y + monitor.height - oy - h - before_h
        }
        // Middle and center origins: the whole stack is vertically centered.
        Origin::Left | Origin::Right | Origin::Center => {
            monitor.y + (monitor.height - total_h) / 2 + before_h
        }
    };

    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MON: MonitorGeometry = MonitorGeometry { x: 0, y: 0, width: 1920, height: 1080 };
    // A monitor that is not at the origin (multi-monitor).
    const MON2: MonitorGeometry = MonitorGeometry { x: 1920, y: 0, width: 1280, height: 1024 };

    #[test]
    fn top_right_stacks_downward_right_aligned() {
        let stack = [(300, 80), (300, 60), (300, 40)];
        let p0 = stack_position(Origin::TopRight, (10, 10), 0, MON, &stack, 0);
        let p1 = stack_position(Origin::TopRight, (10, 10), 0, MON, &stack, 1);
        let p2 = stack_position(Origin::TopRight, (10, 10), 0, MON, &stack, 2);
        assert_eq!(p0, (1920 - 10 - 300, 10));
        assert_eq!(p1, (1920 - 10 - 300, 10 + 80));
        assert_eq!(p2, (1920 - 10 - 300, 10 + 80 + 60));
        // Right-aligned: same x for all.
        assert_eq!(p0.0, p1.0);
        assert_eq!(p1.0, p2.0);
    }

    #[test]
    fn bottom_right_stacks_upward() {
        let stack = [(300, 80), (300, 60)];
        let p0 = stack_position(Origin::BottomRight, (10, 10), 0, MON, &stack, 0);
        let p1 = stack_position(Origin::BottomRight, (10, 10), 0, MON, &stack, 1);
        assert_eq!(p0, (1920 - 10 - 300, 1080 - 10 - 80));
        assert_eq!(p1, (1920 - 10 - 300, 1080 - 10 - 80 - 60));
    }

    #[test]
    fn top_left_stacks_downward_left_aligned() {
        let stack = [(200, 50)];
        let p = stack_position(Origin::TopLeft, (20, 30), 0, MON, &stack, 0);
        assert_eq!(p, (20, 30));
    }

    #[test]
    fn gap_is_added_between_notifications() {
        let stack = [(300, 80), (300, 60), (300, 40)];
        let p2 = stack_position(Origin::TopRight, (10, 10), 5, MON, &stack, 2);
        assert_eq!(p2.1, 10 + 80 + 5 + 60 + 5); // two gaps before the third
    }

    #[test]
    fn center_origins_center_on_the_other_axis() {
        let stack = [(300, 80)];
        let p = stack_position(Origin::TopCenter, (0, 10), 0, MON, &stack, 0);
        assert_eq!(p.0, (1920 - 300) / 2);
        assert_eq!(p.1, 10);
        let p = stack_position(Origin::Center, (0, 0), 0, MON, &stack, 0);
        assert_eq!(p.0, (1920 - 300) / 2);
        assert_eq!(p.1, (1080 - 80) / 2);
        let p = stack_position(Origin::Left, (5, 0), 0, MON, &stack, 0);
        assert_eq!(p.0, 5);
        assert_eq!(p.1, (1080 - 80) / 2);
    }

    #[test]
    fn center_origin_centers_the_whole_stack() {
        // Two notifications at Center: the pair is centered as a unit.
        let stack = [(300, 80), (300, 60)];
        let total_h = 80 + 60;
        let p0 = stack_position(Origin::Center, (0, 0), 0, MON, &stack, 0);
        let p1 = stack_position(Origin::Center, (0, 0), 0, MON, &stack, 1);
        assert_eq!(p0.1, (1080 - total_h) / 2);
        assert_eq!(p1.1, (1080 - total_h) / 2 + 80);
    }

    #[test]
    fn left_origin_stacks_rightward_vertically_centered() {
        let stack = [(200, 50), (200, 50)];
        let p0 = stack_position(Origin::Left, (10, 0), 4, MON, &stack, 0);
        let p1 = stack_position(Origin::Left, (10, 0), 4, MON, &stack, 1);
        assert_eq!(p0.0, 10);
        assert_eq!(p1.0, 10 + 200 + 4);
        let total_h = 50 + 4 + 50;
        assert_eq!(p0.1, (1080 - total_h) / 2);
        assert_eq!(p1.1, (1080 - total_h) / 2 + 50 + 4);
    }

    #[test]
    fn offsets_on_secondary_monitor() {
        let stack = [(400, 100)];
        let p = stack_position(Origin::TopLeft, (0, 0), 0, MON2, &stack, 0);
        assert_eq!(p, (1920, 0));
        let p = stack_position(Origin::TopRight, (8, 8), 0, MON2, &stack, 0);
        assert_eq!(p, (1920 + 1280 - 8 - 400, 8));
    }

    #[test]
    fn size_specs() {
        assert_eq!(resolve_size(SizeSpec::Constant(0), 120, 1920), 120);
        assert_eq!(resolve_size(SizeSpec::Constant(300), 120, 1920), 300);
        assert_eq!(resolve_size(SizeSpec::Range(0, 300), 120, 1920), 120);
        assert_eq!(resolve_size(SizeSpec::Range(0, 300), 500, 1920), 300);
        assert_eq!(resolve_size(SizeSpec::Range(200, 0), 120, 1920), 200);
        assert_eq!(resolve_size(SizeSpec::Percent(0.5), 120, 1920), 960);
        // Percent clamped by range when combined via parse? percent base:
        assert_eq!(resolve_size(SizeSpec::Percent(0.05), 120, 1920), 96);
    }
}
