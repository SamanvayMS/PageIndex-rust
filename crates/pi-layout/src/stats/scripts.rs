//! Script bucket tables and script histogram helpers.
//!
//! ref: pageindex/flash/stats/scripts.py

use std::sync::LazyLock;

/// ref: stats/scripts.py:13 `SCRIPT_BUCKET_TABLE` (one bucket per 16 BMP code points).
static SCRIPT_BUCKET_TABLE: LazyLock<Vec<u8>> = LazyLock::new(|| {
    serde_json::from_str(pi_data::SCRIPT_BUCKET_TABLE_JSON).expect("script bucket table")
});

// ref: stats/scripts.py::char_script_bucket (for a single character)
pub fn char_script_bucket(c: char) -> u8 {
    let cp = c as u32;
    if cp > 0xFFFF {
        return 10;
    }
    if c.is_ascii_alphabetic() {
        return 3;
    }
    if '\x00' < c && c < ' ' {
        return 2;
    }
    if c < '\u{80}' {
        return 1;
    }
    let idx = (cp >> 4) as usize;
    SCRIPT_BUCKET_TABLE.get(idx).copied().unwrap_or(0)
}

/// ref: stats/scripts.py:39 `SCRIPT_FAMILY_WEIGHTS`
const SCRIPT_FAMILY_WEIGHTS: [(usize, &[(usize, i64)]); 10] = [
    (2, &[(2, 10)]),
    (0, &[(0, 1), (2, 1)]),
    (
        3,
        &[
            (3, 1),
            (4, -3),
            (5, -3),
            (6, -3),
            (7, -3),
            (8, -3),
            (9, -10),
        ],
    ),
    (4, &[(4, 1)]),
    (5, &[(5, 1), (6, -10), (7, -10)]),
    (6, &[(6, 1)]),
    (7, &[(7, 1)]),
    (8, &[(8, 1)]),
    (9, &[(9, 1)]),
    (10, &[(10, 1)]),
];

/// ref: stats/scripts.py::ScriptHistogram
#[derive(Debug, Clone, Default)]
pub struct ScriptHistogram {
    /// ref slot: `secondary_slot`
    pub total_chars: u64,
    /// ref slot: `primary_slot`
    pub buckets: [u64; 11],
}

// ref: stats/scripts.py::tally_scripts
pub fn tally_scripts(h: &mut ScriptHistogram, text: &str) {
    for c in text.chars() {
        h.buckets[char_script_bucket(c) as usize] += 1;
        h.total_chars += 1;
    }
}

// ref: stats/scripts.py::dominant_script_family
pub fn dominant_script_family(h: &ScriptHistogram) -> u8 {
    let mut best = 0usize;
    let mut best_score: i64 = 0;
    for family in 0..11 {
        let Some((_, weights)) = SCRIPT_FAMILY_WEIGHTS.iter().find(|(f, _)| *f == family) else {
            continue;
        };
        let score: i64 = weights.iter().map(|&(b, w)| w * h.buckets[b] as i64).sum();
        if score > best_score {
            best = family;
            best_score = score;
        }
    }
    best as u8
}
