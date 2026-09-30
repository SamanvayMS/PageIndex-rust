//! Parity against `parity/golden/<doc>/10_tree_optimized.json`: the page_index_flash
//! post-processing + deterministic merge (optimize="merge", no LLM) on the stage-09 tree.
//!
//! Golden dir: env `PI_GOLDEN`, default `/home/user/PageIndex-rust/parity/golden`; skipped when
//! missing.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

fn golden_dir() -> PathBuf {
    std::env::var_os("PI_GOLDEN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/home/user/PageIndex-rust/parity/golden"))
}

fn load(dir: &Path, name: &str) -> Value {
    let text = std::fs::read_to_string(dir.join(name))
        .unwrap_or_else(|e| panic!("{}: {e}", dir.join(name).display()));
    serde_json::from_str(&text).unwrap()
}

/// The `extract_toc` result as it reaches page_index_flash, rebuilt from the stage dumps.
pub fn extract_toc_result(dir: &Path) -> Map<String, Value> {
    let s09 = load(dir, "09_tree_bookmarks.json");
    let s05 = load(dir, "05_classified.json");
    let s07 = load(dir, "07_outline.json");
    let texts = load(dir, "page_texts.json");
    let mut r = Map::new();
    r.insert("doc_name".into(), s09["doc"].clone());
    r.insert("doc_title".into(), s05["data"]["doc_title"].clone());
    r.insert("structure".into(), s09["data"]["structure"].clone());
    r.insert(
        "has_abstract_or_references_section".into(),
        s07["data"]["has_abstract_or_references"].clone(),
    );
    r.insert("page_texts".into(), texts["data"].clone());
    r.insert("toc_source".into(), s09["data"]["toc_source"].clone());
    r
}

/// JSON equality: key order, types (int vs float) and strings exact; floats within 1e-9.
fn diff(a: &Value, b: &Value, path: &str) -> Option<String> {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            let kx: Vec<_> = x.keys().collect();
            let ky: Vec<_> = y.keys().collect();
            if kx != ky {
                return Some(format!("{path}: keys {kx:?} != {ky:?}"));
            }
            x.iter()
                .find_map(|(k, v)| diff(v, &y[k], &format!("{path}.{k}")))
        }
        (Value::Array(x), Value::Array(y)) => {
            if x.len() != y.len() {
                return Some(format!("{path}: len {} != {}", x.len(), y.len()));
            }
            x.iter()
                .zip(y)
                .enumerate()
                .find_map(|(i, (p, q))| diff(p, q, &format!("{path}[{i}]")))
        }
        (Value::Number(x), Value::Number(y)) => {
            if x.is_f64() != y.is_f64() {
                return Some(format!("{path}: number kind {x} vs {y}"));
            }
            if x.is_f64() {
                let (p, q) = (x.as_f64().unwrap(), y.as_f64().unwrap());
                ((p - q).abs() > 1e-9).then(|| format!("{path}: {p} != {q}"))
            } else {
                (x != y).then(|| format!("{path}: {x} != {y}"))
            }
        }
        _ => (a != b).then(|| format!("{path}: {a} != {b}")),
    }
}

#[test]
fn merge_matches_golden_10() {
    let root = golden_dir();
    if !root.is_dir() {
        eprintln!("golden dir {} missing; skipping", root.display());
        return;
    }
    let mut docs: Vec<PathBuf> = std::fs::read_dir(&root)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.join("10_tree_optimized.json").is_file() && p.join("09_tree_bookmarks.json").is_file()
        })
        .collect();
    docs.sort();
    let mut failures = Vec::new();
    for dir in &docs {
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        let got = pi_optimize::postprocess_merge(extract_toc_result(dir)).unwrap();
        let want = load(dir, "10_tree_optimized.json")["data"].clone();
        match diff(&Value::Object(got), &want, "$") {
            None => eprintln!("PASS {name}"),
            Some(d) => {
                eprintln!("FAIL {name}: {d}");
                failures.push(format!("{name}: {d}"));
            }
        }
    }
    eprintln!("{} docs, {} failures", docs.len(), failures.len());
    assert!(failures.is_empty(), "{failures:#?}");
}
