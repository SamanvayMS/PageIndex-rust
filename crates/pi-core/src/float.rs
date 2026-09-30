//! Serde helpers for floats that may be non-finite.
//!
//! The reference uses `float("inf")` (e.g. cardinal-rotation skew and empty rects). JSON cannot
//! carry those, so parity dumps encode them as the strings `"inf"`, `"-inf"` and `"nan"`.

use serde::{Deserialize, Deserializer, Serializer, de};

pub fn serialize<S: Serializer>(v: &f64, s: S) -> Result<S::Ok, S::Error> {
    if v.is_nan() {
        s.serialize_str("nan")
    } else if v.is_infinite() {
        s.serialize_str(if *v > 0.0 { "inf" } else { "-inf" })
    } else {
        s.serialize_f64(*v)
    }
}

pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Repr {
        Num(f64),
        Str(String),
    }
    match Repr::deserialize(d)? {
        Repr::Num(x) => Ok(x),
        Repr::Str(s) if s == "inf" => Ok(f64::INFINITY),
        Repr::Str(s) if s == "-inf" => Ok(f64::NEG_INFINITY),
        Repr::Str(s) if s == "nan" => Ok(f64::NAN),
        Repr::Str(other) => Err(de::Error::custom(format!("not a float: {other:?}"))),
    }
}
