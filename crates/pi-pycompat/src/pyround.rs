//! Python numeric formatting/rounding semantics.

/// `round(x)` for a float: round half to even, returning a float (callers cast as needed).
pub fn round0(x: f64) -> f64 {
    x.round_ties_even()
}

/// `round(x, ndigits)`: correctly rounded decimal (the result is the float nearest to the
/// decimal string that Python's `_Py_dg_dtoa` mode 3 produces). Rust's fixed-precision
/// formatting is also exact decimal rounding of the binary value, then parsed back.
pub fn round_nd(x: f64, ndigits: i32) -> f64 {
    if !x.is_finite() {
        return x;
    }
    if ndigits >= 0 {
        let s = format!("{:.*}", ndigits as usize, x);
        s.parse().unwrap_or(x)
    } else {
        let p = 10f64.powi(-ndigits);
        (x / p).round_ties_even() * p
    }
}

/// `float(f"{x:.6g}")`: round to 6 significant digits (`geometry.py:132`, `char_extract.py:237`).
pub fn g6(x: f64) -> f64 {
    sig(x, 6)
}

/// `float(f"{x:.{n}g}")`.
pub fn sig(x: f64, n: usize) -> f64 {
    if !x.is_finite() || x == 0.0 {
        return x;
    }
    format!("{:.*e}", n.saturating_sub(1), x)
        .parse()
        .unwrap_or(x)
}
