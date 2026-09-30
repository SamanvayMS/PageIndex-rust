//! Python `json.dumps(..., ensure_ascii=False)` output.
//!
//! String escaping in CPython's encoder (`json.encoder.ESCAPE`/`ESCAPE_DCT`) is: `"` -> `\"`,
//! `\` -> `\\`, `\n` `\r` `\t` `\b` `\f` as short escapes, every other code point below U+0020
//! as `\u00xx` (lowercase hex); everything else, `/` and non-ASCII included, raw. That is
//! exactly serde_json's escaping, so only the separators need handling here.

use std::io;

use serde::Serialize;
use serde_json::ser::{CompactFormatter, Formatter, Serializer};

/// `json.dumps(v, ensure_ascii=False, separators=(",", ":"))`.
pub fn dumps_compact<T: Serialize + ?Sized>(value: &T) -> String {
    let mut out = Vec::new();
    let mut ser = Serializer::with_formatter(&mut out, CompactFormatter);
    value.serialize(&mut ser).expect("serializing to memory");
    String::from_utf8(out).expect("serde_json writes UTF-8")
}

/// `json.dumps(v, ensure_ascii=False)`: default separators `", "` and `": "`.
pub fn dumps<T: Serialize + ?Sized>(value: &T) -> String {
    let mut out = Vec::new();
    let mut ser = Serializer::with_formatter(&mut out, PyDefaultFormatter);
    value.serialize(&mut ser).expect("serializing to memory");
    String::from_utf8(out).expect("serde_json writes UTF-8")
}

/// Compact formatter with Python's default item/key separators.
#[derive(Clone, Copy, Debug, Default)]
pub struct PyDefaultFormatter;

impl Formatter for PyDefaultFormatter {
    fn begin_array_value<W: ?Sized + io::Write>(
        &mut self,
        w: &mut W,
        first: bool,
    ) -> io::Result<()> {
        if first { Ok(()) } else { w.write_all(b", ") }
    }

    fn begin_object_key<W: ?Sized + io::Write>(
        &mut self,
        w: &mut W,
        first: bool,
    ) -> io::Result<()> {
        if first { Ok(()) } else { w.write_all(b", ") }
    }

    fn begin_object_value<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
        w.write_all(b": ")
    }
}
