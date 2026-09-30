//! Conformance against CPython 3.11 / Unicode 14 recorded by `gen/unicode_tables.py` and
//! `gen/pysort_fixture.py`.

use pi_pycompat::pysort;
use pi_pycompat::unicode::{self, Form};
use serde_json::Value;

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/unicode.json")).unwrap()
}

/// Expand a `[[value, run_length], ...]` list over the whole code space.
fn expand(v: &Value) -> Vec<Value> {
    let mut out = Vec::with_capacity(0x110000);
    for run in v.as_array().unwrap() {
        let n = run[1].as_u64().unwrap() as usize;
        out.extend(std::iter::repeat_n(run[0].clone(), n));
    }
    assert_eq!(out.len(), 0x110000);
    out
}

#[test]
fn fixture_is_unicode_14() {
    let fx = fixture();
    assert_eq!(fx["unicode"], unicode::UNIDATA_VERSION);
}

#[test]
fn category_every_code_point() {
    let want = expand(&fixture()["category"]);
    for (cp, w) in want.iter().enumerate() {
        assert_eq!(
            unicode::category_of(cp as u32).as_str(),
            w.as_str().unwrap(),
            "U+{cp:04X}"
        );
    }
}

#[test]
fn bidirectional_every_code_point() {
    let want = expand(&fixture()["bidirectional"]);
    for (cp, w) in want.iter().enumerate() {
        let Some(c) = char::from_u32(cp as u32) else {
            continue;
        };
        assert_eq!(
            unicode::bidirectional(c).as_str(),
            w.as_str().unwrap(),
            "U+{cp:04X}"
        );
    }
}

#[test]
fn regex_number_every_code_point() {
    let want = expand(&fixture()["regex_number"]);
    for (cp, w) in want.iter().enumerate() {
        let Some(c) = char::from_u32(cp as u32) else {
            continue;
        };
        assert_eq!(
            unicode::is_regex_number(c),
            w.as_u64().unwrap() == 1,
            "U+{cp:04X}"
        );
    }
}

#[test]
fn lower_upper_mappings() {
    let fx = fixture();
    for (key, f) in [
        ("lower", unicode::lower as fn(&str) -> String),
        ("upper", unicode::upper),
    ] {
        let map: std::collections::HashMap<u32, &str> = fx[key]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| (e[0].as_u64().unwrap() as u32, e[1].as_str().unwrap()))
            .collect();
        for cp in 0..0x110000u32 {
            let Some(c) = char::from_u32(cp) else {
                continue;
            };
            let got = f(&c.to_string());
            let want = map.get(&cp).map(|s| s.to_string()).unwrap_or(c.to_string());
            assert_eq!(got, want, "{key}(U+{cp:04X})");
        }
    }
    // Final sigma context (CPython handle_capital_sigma).
    assert_eq!(unicode::lower("ΟΔΟΣ"), "οδος");
    assert_eq!(unicode::lower("Σ"), "σ");
    assert_eq!(unicode::lower("ΑΣ.Β"), "ασ.β");
    assert_eq!(unicode::lower("ΑΣ Β"), "ας β");
    assert_eq!(unicode::lower("ΑΣ'Β"), "ασ'β");
    assert_eq!(unicode::lower("İ"), "i\u{307}");
}

#[test]
fn normalization_single_code_points() {
    let fx = fixture();
    for (key, form) in [
        ("NFC", Form::Nfc),
        ("NFD", Form::Nfd),
        ("NFKC", Form::Nfkc),
        ("NFKD", Form::Nfkd),
    ] {
        let map: std::collections::HashMap<u32, &str> = fx["normalize"][key]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| (e[0].as_u64().unwrap() as u32, e[1].as_str().unwrap()))
            .collect();
        for cp in 0..0x110000u32 {
            if (0xAC00..=0xD7A3).contains(&cp) {
                continue; // Hangul syllables: sampled in normalize_seqs
            }
            let Some(c) = char::from_u32(cp) else {
                continue;
            };
            let s = c.to_string();
            let want = map.get(&cp).copied().unwrap_or(&s);
            assert_eq!(unicode::normalize(form, &s), want, "{key}(U+{cp:04X})");
        }
    }
}

#[test]
fn normalization_sequences() {
    let fx = fixture();
    let forms = [Form::Nfc, Form::Nfd, Form::Nfkc, Form::Nfkd];
    for case in fx["normalize_seqs"].as_array().unwrap() {
        let s = case[0].as_str().unwrap();
        for (k, form) in forms.iter().enumerate() {
            assert_eq!(
                unicode::normalize(*form, s),
                case[k + 1].as_str().unwrap(),
                "{form:?}({s:?})"
            );
        }
        assert_eq!(unicode::nfkc(s), case[3].as_str().unwrap());
    }
}

fn num(v: &Value) -> f64 {
    match v {
        Value::String(s) if s == "nan" => f64::NAN,
        Value::String(s) if s == "inf" => f64::INFINITY,
        Value::String(s) if s == "-inf" => f64::NEG_INFINITY,
        v => v.as_f64().unwrap(),
    }
}

/// `cmp_to_key(_percentile_sample_cmp)`: `a - b < 0`, so NaN results compare as "not less".
#[test]
fn timsort_matches_cpython_for_nan_comparator() {
    let cases: Value = serde_json::from_str(include_str!("fixtures/pysort.json")).unwrap();
    for (ci, case) in cases.as_array().unwrap().iter().enumerate() {
        let vals: Vec<(f64, f64)> = case["values"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (num(&p[0]), num(&p[1])))
            .collect();
        let want: Vec<usize> = case["perm"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_u64().unwrap() as usize)
            .collect();
        let cmp = |a: &(f64, f64), b: &(f64, f64)| {
            if a.0 != b.0 { a.0 - b.0 } else { a.1 - b.1 }
        };
        let mut idx: Vec<usize> = (0..vals.len()).collect();
        pysort::sort_by_lt(&mut idx, |&i, &j| cmp(&vals[i], &vals[j]) < 0.0);
        assert_eq!(idx, want, "case {ci} (n={})", vals.len());
    }
}
