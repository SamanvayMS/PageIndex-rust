//! Rectangle helpers, geometry predicates and ordering keys.
//!
//! ref: pageindex/flash/model/rects.py

use pi_core::Rect;
use pi_pycompat::pymath;

use super::char_stats::{max_nan, min_nan};

/// Anything with a bounding box (the reference's `Bounded` mixin; `Rect` exposes the same
/// accessors). Accessor names follow the reference.
pub trait Bounded {
    fn rect(&self) -> &Rect;

    fn left_edge(&self) -> f64 {
        self.rect().left
    }
    fn right_edge(&self) -> f64 {
        self.rect().right
    }
    fn top_edge(&self) -> f64 {
        self.rect().top
    }
    fn bottom_edge(&self) -> f64 {
        self.rect().bottom
    }
    // ref: model/rects.py::Rect.bbox_width
    fn bbox_width(&self) -> f64 {
        let r = self.rect();
        max_nan(0.0, r.right - r.left)
    }
    // ref: model/rects.py::Rect.bbox_height
    fn bbox_height(&self) -> f64 {
        let r = self.rect();
        max_nan(0.0, r.top - r.bottom)
    }
    // ref: model/rects.py::Rect.area
    fn area(&self) -> f64 {
        self.bbox_width() * self.bbox_height()
    }
    // ref: model/rects.py::Rect.center_x
    fn center_x(&self) -> f64 {
        let r = self.rect();
        (r.left + r.right) / 2.0
    }
    // ref: model/rects.py::Rect.center_y
    fn center_y(&self) -> f64 {
        let r = self.rect();
        (r.top + r.bottom) / 2.0
    }
}

impl Bounded for Rect {
    fn rect(&self) -> &Rect {
        self
    }
}

/// ref: model/rects.py::EMPTY_RECT
pub const EMPTY_RECT: Rect = Rect::EMPTY;

// ref: model/rects.py::rect_union
pub fn rect_union(a: &Rect, b: &Rect) -> Rect {
    Rect::new(
        min_nan(a.left, b.left),
        max_nan(a.right, b.right),
        max_nan(a.top, b.top),
        min_nan(a.bottom, b.bottom),
    )
}

/// Python tuple `<` on float tuples: the first position where the elements are not `==`
/// decides with `<`; all-equal tuples are not less.
pub fn tuple_lt(a: &[f64], b: &[f64]) -> bool {
    for (x, y) in a.iter().zip(b) {
        if x != y {
            return x < y;
        }
    }
    a.len() < b.len()
}

/// Python tuple `==` on float tuples.
pub fn tuple_eq(a: &[f64], b: &[f64]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x == y)
}

// ref: model/rects.py::left_edge_key
pub fn left_edge_key(b: &impl Bounded) -> [f64; 4] {
    [
        b.left_edge(),
        b.right_edge(),
        -b.top_edge(),
        -b.bottom_edge(),
    ]
}

// ref: model/rects.py::reading_order_key
pub fn reading_order_key(b: &impl Bounded) -> [f64; 4] {
    [
        -b.top_edge(),
        -b.bottom_edge(),
        b.left_edge(),
        b.right_edge(),
    ]
}

// ref: model/rects.py::magnitude_ratio
pub fn magnitude_ratio(value: f64, other: f64) -> f64 {
    let (num, den) = if value.abs() > other.abs() {
        (value, other)
    } else {
        (other, value)
    };
    if den != 0.0 {
        return num / den;
    }
    if num == 0.0 || num.is_nan() {
        return f64::NAN;
    }
    f64::INFINITY.copysign(num) * 1.0f64.copysign(den)
}

// ref: model/rects.py::same_x_extent
pub fn same_x_extent(a: &impl Bounded, b: &impl Bounded, tol: f64) -> bool {
    (a.left_edge() - b.left_edge()).abs() <= tol && (a.right_edge() - b.right_edge()).abs() <= tol
}

// ref: model/rects.py::same_y_extent
pub fn same_y_extent(a: &impl Bounded, b: &impl Bounded, tol: f64) -> bool {
    (a.top_edge() - b.top_edge()).abs() <= tol && (a.bottom_edge() - b.bottom_edge()).abs() <= tol
}

// ref: model/rects.py::left_aligned
pub fn left_aligned(a: &impl Bounded, b: &impl Bounded, tol: f64) -> bool {
    (a.left_edge() - b.left_edge()).abs() <= tol
}

// ref: model/rects.py::right_aligned
pub fn right_aligned(a: &impl Bounded, b: &impl Bounded, tol: f64) -> bool {
    (a.right_edge() - b.right_edge()).abs() <= tol
}

// ref: model/rects.py::center_aligned
pub fn center_aligned(a: &impl Bounded, b: &impl Bounded, tol: f64) -> bool {
    let dl = a.left_edge() - b.left_edge();
    let dr = a.right_edge() - b.right_edge();
    let sign = |d: f64| -> i32 {
        if d > 0.0 {
            1
        } else if d < 0.0 {
            -1
        } else {
            0
        }
    };
    if sign(dl) != -sign(dr) {
        return false;
    }
    (a.center_x() - b.center_x()).abs() <= pymath::max(tol, pymath::min(dl.abs(), dr.abs()) / 2.0)
}

// ref: model/rects.py::x_centers_close
pub fn x_centers_close(a: &impl Bounded, b: &impl Bounded) -> bool {
    (b.center_x() - a.center_x()).abs() <= pymath::max(1.0, b.bbox_width() / 10.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_rect_geometry() {
        assert_eq!(EMPTY_RECT.bbox_width(), 0.0);
        assert_eq!(EMPTY_RECT.area(), 0.0);
        assert!(EMPTY_RECT.center_y().is_nan());
    }

    #[test]
    fn magnitude_ratio_ieee() {
        assert_eq!(magnitude_ratio(5.0, -0.0), f64::NEG_INFINITY);
        assert!(magnitude_ratio(0.0, 0.0).is_nan());
        assert_eq!(magnitude_ratio(2.0, 4.0), 2.0);
    }

    #[test]
    fn tuple_order() {
        assert!(tuple_lt(&[1.0, 2.0], &[1.0, 3.0]));
        assert!(!tuple_lt(&[1.0, 3.0], &[1.0, 3.0]));
        assert!(tuple_eq(&[-0.0], &[0.0]));
    }
}
