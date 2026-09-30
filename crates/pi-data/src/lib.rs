//! Data tables copied verbatim from the reference, `pageindex/flash/data/*.json` @619cbd8
//! (MIT, Copyright (c) 2025 Vectify AI; see NOTICE). Exposed as raw JSON text so each consumer
//! parses exactly the structure it needs.

/// Keyword/section dictionaries used by the tries (`tokens/tries.py`, `title/dicts.py`, ...).
pub const DICTIONARIES_JSON: &str = include_str!("../data/dictionaries.json");
/// Boilerplate phrase list (`classification/toc_boilerplate.py`).
pub const BOILERPLATE_PHRASES_JSON: &str = include_str!("../data/boilerplate_phrases.json");
/// Adobe glyph name -> Unicode table (`parser_pdfium_charlevel/glyph_tables.py`).
pub const GLYPH_NAME_TABLE_JSON: &str = include_str!("../data/glyph_name_table.json");
/// Unicode normalization overrides (`parser_pdfium_charlevel/text_normalize.py`).
pub const NORMALIZED_UNICODES_JSON: &str = include_str!("../data/normalized_unicodes.json");
/// Script bucket table (`stats/scripts.py`).
pub const SCRIPT_BUCKET_TABLE_JSON: &str = include_str!("../data/script_bucket_table.json");
