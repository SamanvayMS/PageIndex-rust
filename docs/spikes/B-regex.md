# Spike B: regex and Python-compat audit (reference @619cbd8)

## Result
- The plain `regex` crate is enough: there are **0 lookarounds and 0 backreferences**, so `fancy-regex` isn't needed.
- About 68 pattern call sites (about 64 distinct patterns).
- Third-party `regex` is actually used for 5 patterns (`\p{Number}`, `\p{Lu}`), all Rust-compatible.

## Translation rules (to implement in `pi-pycompat`, each with conformance tests)

| Python | Rust | Sites |
|---|---|---|
| `\Z` | `\z` | span_line.py:35 (x2), merge_rules.py:30, caption_text.py:27, gutters.py:70, classification/keyword_tables.py:101-102 |
| `$`, which also matches before a final `\n` | `(?:\n?\z)`, or strip first | ~21: embedded_toc.py:62, outline/filtering.py (7), numbering.py:33,46-50, cmap_parse.py:39-43, agent_tools.py:253, tree_optimize.py:944 |
| `\w` = isalnum + `_` (includes `No` such as ², ½; excludes marks) | explicit class built from Python's `str.isalnum` table | embedded_toc.py:65 (feeds title normalization), outline/filtering.py:187,189,204 |
| `\s` includes U+001C–001F | `[\s\x1C-\x1F]` | outline/tree.py:45, filtering.py, embedded_toc.py:62, tree_optimize.py:122, agent_tools.py:1298 |
| `re.ASCII \| re.IGNORECASE` | `(?i-u)` (otherwise `ſ` matches `s`) | span_line.py:34-35 (bold/italic font-name regexes) |
| Unicode IGNORECASE `[a-z]` also matches İ ı ſ K | explicit class (Rust folds ſ and K but not İ/ı) | filtering.py:63, embedded_toc.py:62, font_unicode.py:339,341 |
| lone-surrogate class `[\ud800-\udfff]` | not expressible; map to U+FFFD at `char::from_u32` | unicode_apply.py:17 |
| `int(s)` on `\d` matches accepts non-ASCII digits | `to_digit` via Unicode Nd value | filtering.py:142,230,237, font_unicode.py:211,260,314, code_walk.py:43 |
| `DEAD_DIGIT_RE` under stdlib `re` (the literal text `p{Number}`) | keep literal | heading_detection/keyword_tables.py:60 |

## Other Python-semantics hot spots
- **difflib:** `SequenceMatcher.ratio()` with autojunk (embedded_toc.py:283, threshold 0.7), `get_opcodes()` with autojunk=False (unicode_apply.py:151), and `get_close_matches(cutoff=0.5)` (agent_tools.py:410). Port the algorithm exactly.
- **unicodedata:** `category` ×4, `normalize` (NFC/NFD/NFKC/NFKD) ×10, `bidirectional` ×1. Python 3.11 = Unicode 14.0; see Spike D.
- **`round()`:** 7 sites, half-to-even. Also `float(f"{x:.6g}")` (geometry.py:132, char_extract.py:237) and the custom `_round_half_up_to_int`.
- **No-argument `.split()`/`.strip()`:** 37 sites, using Python's whitespace set (includes U+001C–001F and U+0085).
- **Ordering:** `Counter.most_common(1)` ties (embedded_toc.py:192) break by first insertion. Dict iteration order matters at pdf_objects.py:94 and code_walk.py:42-65, so use `IndexMap`. No set iteration feeds ordered output.
