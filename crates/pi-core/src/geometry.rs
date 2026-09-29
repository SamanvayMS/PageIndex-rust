//! Axis-aligned rectangles in PDF user space (y grows upward).
//!
//! Reference: `pageindex/flash/model/rects.py::Rect` — constructor order `(left, right, top, bottom)`,
//! with `bottom` stored in the obfuscated `primary_slot`. Serialized as `[left, right, top, bottom]`.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub left: f64,
    pub right: f64,
    pub top: f64,
    pub bottom: f64,
}

impl Rect {
    /// The reference's `EMPTY_RECT = Rect(inf, -inf, -inf, inf)`: the identity for `union`.
    pub const EMPTY: Rect = Rect {
        left: f64::INFINITY,
        right: f64::NEG_INFINITY,
        top: f64::NEG_INFINITY,
        bottom: f64::INFINITY,
    };

    pub const fn new(left: f64, right: f64, top: f64, bottom: f64) -> Self {
        Self {
            left,
            right,
            top,
            bottom,
        }
    }

    pub fn width(&self) -> f64 {
        self.right - self.left
    }

    pub fn height(&self) -> f64 {
        self.top - self.bottom
    }

    pub fn union(&self, o: &Rect) -> Rect {
        Rect::new(
            self.left.min(o.left),
            self.right.max(o.right),
            self.top.max(o.top),
            self.bottom.min(o.bottom),
        )
    }
}

impl Serialize for Rect {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeTuple;
        struct F(f64);
        impl Serialize for F {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                crate::float::serialize(&self.0, s)
            }
        }
        let mut t = s.serialize_tuple(4)?;
        for v in [self.left, self.right, self.top, self.bottom] {
            t.serialize_element(&F(v))?;
        }
        t.end()
    }
}

impl<'de> Deserialize<'de> for Rect {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct F(#[serde(with = "crate::float")] f64);
        let [l, r, t, b] = <[F; 4]>::deserialize(d)?;
        Ok(Rect::new(l.0, r.0, t.0, b.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_non_finite() {
        let js = serde_json::to_string(&Rect::EMPTY).unwrap();
        assert_eq!(js, r#"["inf","-inf","-inf","inf"]"#);
        let back: Rect = serde_json::from_str(&js).unwrap();
        assert_eq!(back, Rect::EMPTY);
    }

    #[test]
    fn union_with_empty_is_identity() {
        let r = Rect::new(1.0, 2.0, 4.0, 3.0);
        assert_eq!(Rect::EMPTY.union(&r), r);
    }
}
