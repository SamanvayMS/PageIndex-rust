# Spike D: Unicode version parity

## Facts
- Goldens are produced with **Python 3.11.15**, whose `unicodedata.unidata_version` is **14.0.0**. Every stage file records `python` and `unicode` in its header, and the diff tool should refuse to compare goldens made under different Unicode versions.
- The Rust Unicode crates track newer Unicode (15.x/16.x): `unicode-normalization 0.1.25`, `unicode-general-category 1.1`, `unicode-script 0.5.8`, `unicode-bidi 0.3.18`. Codepoints assigned after 14.0 would classify differently than in the reference; for example, Python reports `Cn` (unassigned) where Rust reports a real category.

## Decision
1. **General category and bidi class** feed the core heuristics: `char_stats.py` category counts drive caps-heavy, letter weights and heading scores. `pi-pycompat` gets them from **tables generated from Python 3.11's `unicodedata`** by a codegen script (`pi-pycompat/gen/unicode_tables.py` → range-compressed `const` arrays, a few KB). This gives exact parity by construction; a later reference Python upgrade means regenerating the tables.
2. **Normalization (NFC/NFD/NFKC/NFKD)** uses `unicode-normalization`, guarded so any codepoint unassigned in 14.0 (per the generated category table) passes through unchanged, as Python does. A conformance test runs all 1.1M codepoints plus composed sequences through both implementations: Python writes a fixture, and the Rust test compares against it.
3. **Casing:** the reference uses only `.lower()` (25 sites) and `.upper()` (2 sites). Rust's `to_lowercase` is Unicode 15+, so the same guard applies. `İ`.lower() = `i̇` in both.
4. **`str.split()`/`strip()` whitespace:** use Python's set (`\t\n\x0b\x0c\r\x1c-\x1f \x85\xa0  -     　`) as a const in `pi-pycompat`.

## Conformance fixtures (Phase 1 prerequisite)
`parity/gen_unicode_fixture.py` writes `category`, `bidirectional`, `lower`, `upper`, and the four normalizations for every codepoint whose results differ from identity, plus Python-whitespace and `isalnum` sets. `pi-pycompat` tests compare against the fixture.
