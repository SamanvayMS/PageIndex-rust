//! Python-compatible helpers: behaviour the reference relies on that Rust's standard library
//! does not reproduce exactly.

pub mod difflib;
pub mod pymath;
pub mod pyround;
pub mod pysort;
pub mod pystr;
pub mod unicode;
#[rustfmt::skip]
mod unicode_tables;
