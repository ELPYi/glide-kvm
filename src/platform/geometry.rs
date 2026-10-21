//! Screen geometry, OS-independent: which outer edge the cursor is pushing
//! against, and where to put the cursor when entering through an edge.
//!
//! Each backend snapshots its monitor rectangles into a [`ScreenLayout`]
//! (refreshed when displays change) so the per-mouse-event edge check is a
//! few comparisons rather than several OS calls.

use super::EdgeHit;
use crate::input::Side;

/// A monitor in desktop pixels; `right`/`bottom` are exclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }

    /// Squared distance from (x, y) to this rectangle (0 inside).
    fn distance2(&self, x: i32, y: i32) -> i64 {
        let dx = (self.left - x).max(0).max(x - (self.right - 1)) as i64;
        let dy = (self.top - y).max(0).max(y - (self.bottom - 1)) as i64;
        dx * dx + dy * dy
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScreenLayout {
    monitors: Vec<Rect>,
}

impl ScreenLayout {
    pub fn new(monitors: Vec<Rect>) -> Self {
        ScreenLayout { monitors }
    }

    /// Bounding box of all monitors.
    pub fn bounds(&self) -> Rect {
        let mut it = self.monitors.iter();
        let Some(first) = it.next() else {
            return Rect {
                left: 0,
                top: 0,
                right: 1,
                bottom: 1,
            };
        };
        it.fold(*first, |b, m| Rect {
            left: b.left.min(m.left),
            top: b.top.min(m.top),
            right: b.right.max(m.right),
            bottom: b.bottom.max(m.bottom),
        })
    }

    fn monitor_at(&self, x: i32, y: i32) -> Option<&Rect> {
        self.monitors.iter().find(|m| m.contains(x, y))
    }

    fn nearest(&self, x: i32, y: i32) -> Option<&Rect> {
        self.monitors.iter().min_by_key(|m| m.distance2(x, y))
    }

    /// If the cursor at (x, y) sits on an outer desktop edge (no monitor
    /// beyond it) and the movement (dx, dy) pushes past it.
    pub fn edge_hit(&self, x: i32, y: i32, dx: i32, dy: i32) -> Option<EdgeHit> {
        let r = self.monitor_at(x, y).or_else(|| self.nearest(x, y))?;
        let free = |x: i32, y: i32| self.monitor_at(x, y).is_none();
        let side = if dx > 0 && x >= r.right - 1 && free(r.right, y) {
            Side::Right
        } else if dx < 0 && x <= r.left && free(r.left - 1, y) {
            Side::Left
        } else if dy > 0 && y >= r.bottom - 1 && free(x, r.bottom) {
            Side::Bottom
        } else if dy < 0 && y <= r.top && free(x, r.top - 1) {
            Side::Top
        } else {
            return None;
        };
        let b = self.bounds();
        let frac = match side {
            Side::Left | Side::Right => (y - b.top) as f32 / (b.bottom - b.top - 1).max(1) as f32,
            Side::Top | Side::Bottom => (x - b.left) as f32 / (b.right - b.left - 1).max(1) as f32,
        };
        Some(EdgeHit { side, frac })
    }

    /// Where to put the cursor entering through `side`, `frac` of the way along.
    /// Lands on the edge of a real monitor even in L-shaped layouts.
    pub fn entry_point(&self, side: Side, frac: f32) -> (i32, i32) {
        let b = self.bounds();
        let frac = frac.clamp(0.0, 1.0);
        let along_x = b.left + ((b.right - b.left - 1) as f32 * frac) as i32;
        let along_y = b.top + ((b.bottom - b.top - 1) as f32 * frac) as i32;
        let (tx, ty) = match side {
            Side::Left => (b.left, along_y),
            Side::Right => (b.right - 1, along_y),
            Side::Top => (along_x, b.top),
            Side::Bottom => (along_x, b.bottom - 1),
        };
        let Some(r) = self.nearest(tx, ty) else { return (tx, ty) };
        let x = match side {
            Side::Left => r.left,
            Side::Right => r.right - 1,
            _ => tx.clamp(r.left, r.right - 1),
        };
        let y = match side {
            Side::Top => r.top,
            Side::Bottom => r.bottom - 1,
            _ => ty.clamp(r.top, r.bottom - 1),
        };
        (x, y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, w: i32, h: i32) -> Rect {
        Rect {
            left,
            top,
            right: left + w,
            bottom: top + h,
        }
    }

    fn single() -> ScreenLayout {
        ScreenLayout::new(vec![rect(0, 0, 2560, 1440)])
    }

    #[test]
    fn single_monitor_edges() {
        let l = single();
        assert_eq!(l.edge_hit(2559, 720, 3, 0).map(|h| h.side), Some(Side::Right));
        assert_eq!(l.edge_hit(0, 720, -3, 0).map(|h| h.side), Some(Side::Left));
        assert_eq!(l.edge_hit(1280, 0, 0, -1).map(|h| h.side), Some(Side::Top));
        assert_eq!(l.edge_hit(1280, 1439, 0, 1).map(|h| h.side), Some(Side::Bottom));
        assert_eq!(l.edge_hit(2559, 720, -3, 0), None, "moving away from the edge");
        assert_eq!(l.edge_hit(2000, 720, 3, 0), None, "not at the edge yet");
        let frac = l.edge_hit(2559, 0, 1, 0).unwrap().frac;
        assert_eq!(frac, 0.0);
    }

    #[test]
    fn edges_between_monitors_are_not_outer_edges() {
        // Two side by side: the seam isn't an edge; the far sides are.
        let l = ScreenLayout::new(vec![rect(0, 0, 1920, 1080), rect(1920, 0, 1920, 1080)]);
        assert_eq!(l.edge_hit(1919, 500, 5, 0), None);
        assert_eq!(l.edge_hit(3839, 500, 5, 0).map(|h| h.side), Some(Side::Right));
    }

    #[test]
    fn l_shaped_layouts() {
        // A taller monitor on the left, a shorter one on the right, top-aligned.
        let l = ScreenLayout::new(vec![rect(0, 0, 1920, 1440), rect(1920, 0, 1920, 1080)]);
        // Below the short monitor, the tall one's right side is an outer edge.
        assert_eq!(l.edge_hit(1919, 1300, 5, 0).map(|h| h.side), Some(Side::Right));
        // Entering from the right at the bottom lands on a real monitor.
        let (x, y) = l.entry_point(Side::Right, 1.0);
        assert!(l.monitor_at(x, y).is_some(), "({x}, {y}) is off-screen");
    }

    #[test]
    fn entry_points() {
        let l = single();
        assert_eq!(l.entry_point(Side::Left, 0.5), (0, 719));
        assert_eq!(l.entry_point(Side::Right, 0.0), (2559, 0));
        assert_eq!(l.entry_point(Side::Bottom, 1.0), (2559, 1439));
        assert_eq!(l.entry_point(Side::Top, 2.0), (2559, 0), "frac is clamped");
    }

    #[test]
    fn empty_layout_is_harmless() {
        let l = ScreenLayout::default();
        assert_eq!(l.edge_hit(0, 0, 1, 0), None);
    }
}
