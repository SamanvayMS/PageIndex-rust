//! Python builtin `max`/`min` on floats.
//!
//! `max(a, b)` keeps the first argument unless a later one compares strictly greater, so a NaN
//! is kept when it comes first and ignored when it comes later. `f64::max`/`f64::min` always
//! ignore NaN, which differs.

/// `max(a, b)`.
#[inline]
pub fn max(a: f64, b: f64) -> f64 {
    if b > a { b } else { a }
}

/// `min(a, b)`.
#[inline]
pub fn min(a: f64, b: f64) -> f64 {
    if b < a { b } else { a }
}

/// `max(a, b, c)`.
#[inline]
pub fn max3(a: f64, b: f64, c: f64) -> f64 {
    max(max(a, b), c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nan_position_matters() {
        assert!(max(f64::NAN, 1.0).is_nan());
        assert_eq!(max(1.0, f64::NAN), 1.0);
        assert!(min(f64::NAN, 1.0).is_nan());
        assert_eq!(min(1.0, f64::NAN), 1.0);
        // Ties keep the first argument (visible through the sign of zero).
        assert!(max(-0.0, 0.0).is_sign_negative());
    }
}
