//! Stage 02/03 parity against the Python reference goldens.
//!
//! For each doc under `$PI_GOLDEN` (default `/home/user/PageIndex-rust/parity/golden`), runs
//! `process_page` on every page of `01_spans.json` and compares with `02_lines.json` /
//! `03_columns.json`: ints, strings and bools exactly, floats within 1e-9 relative
//! (`PI_PARITY_TOL` overrides). Also checks
//! `compute_doc_stats` against `04_blocks.json`'s `doc_stats`. Skips when the goldens are absent.
//! `PI_PARITY_DOCS=a,b` restricts the run to some docs.

use std::path::{Path, PathBuf};

use pi_core::PageSpans;
use pi_layout::dump;
use serde_json::Value;

/// Relative float tolerance; `PI_PARITY_TOL` overrides (0 = bit-exact).
fn rel_tol() -> f64 {
    std::env::var("PI_PARITY_TOL")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1e-9)
}

fn golden_root() -> PathBuf {
    std::env::var_os("PI_GOLDEN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/home/user/PageIndex-rust/parity/golden"))
}

fn load(p: &Path) -> Value {
    let text = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn close(a: f64, b: f64) -> bool {
    a == b || (a - b).abs() <= rel_tol() * a.abs().max(b.abs())
}

/// First divergence between `want` (golden) and `got`, as a path + values.
fn first_diff(path: &str, want: &Value, got: &Value) -> Option<String> {
    match (want, got) {
        (Value::Number(x), Value::Number(y)) => {
            let ints = x.is_i64() && y.is_i64();
            let same = if ints {
                x == y
            } else {
                close(x.as_f64().unwrap(), y.as_f64().unwrap())
            };
            (!same).then(|| format!("{path}: want {x} got {y}"))
        }
        (Value::Array(xs), Value::Array(ys)) => {
            for (i, (x, y)) in xs.iter().zip(ys).enumerate() {
                if let Some(d) = first_diff(&format!("{path}[{i}]"), x, y) {
                    return Some(d);
                }
            }
            (xs.len() != ys.len())
                .then(|| format!("{path}: length want {} got {}", xs.len(), ys.len()))
        }
        (Value::Object(xs), Value::Object(ys)) => {
            for (k, x) in xs {
                match ys.get(k) {
                    Some(y) => {
                        if let Some(d) = first_diff(&format!("{path}.{k}"), x, y) {
                            return Some(d);
                        }
                    }
                    None => return Some(format!("{path}.{k}: missing")),
                }
            }
            ys.keys()
                .find(|k| !xs.contains_key(*k))
                .map(|k| format!("{path}.{k}: unexpected"))
        }
        _ => (want != got).then(|| format!("{path}: want {want} got {got}")),
    }
}

/// Compares page by page so the report names the first divergent page (and line).
fn compare_pages(stage: &str, want: &Value, got: &[Value]) -> Result<(), String> {
    let want = want["data"].as_array().expect("data array");
    if want.len() != got.len() {
        return Err(format!(
            "{stage}: page count want {} got {}",
            want.len(),
            got.len()
        ));
    }
    for (w, g) in want.iter().zip(got) {
        if let Some(d) = first_diff("", w, g) {
            let page = w["page"].as_u64().unwrap_or(0);
            let ctx = d
                .strip_prefix(".lines[")
                .and_then(|s| s.split(']').next())
                .and_then(|i| i.parse::<usize>().ok())
                .map(|i| {
                    let t = |v: &Value| v["lines"][i]["text"].to_string();
                    format!(
                        "\n    golden line text: {}\n    ours:             {}",
                        t(w),
                        t(g)
                    )
                })
                .unwrap_or_default();
            return Err(format!("{stage}: page {page}: {d}{ctx}"));
        }
    }
    Ok(())
}

fn run_doc(dir: &Path) -> Result<(), String> {
    let spans = load(&dir.join("01_spans.json"));
    let pages: Vec<PageSpans> =
        serde_json::from_value(spans["data"].clone()).map_err(|e| format!("01_spans: {e}"))?;
    let layouts = dump::process_document(&pages);
    let mut errs = Vec::new();
    let lines: Vec<Value> = layouts.iter().map(dump::page_lines).collect();
    if let Err(e) = compare_pages("02_lines", &load(&dir.join("02_lines.json")), &lines) {
        errs.push(e);
    }
    let cols: Vec<Value> = layouts.iter().map(dump::page_columns).collect();
    if let Err(e) = compare_pages("03_columns", &load(&dir.join("03_columns.json")), &cols) {
        errs.push(e);
    }
    let blocks = dir.join("04_blocks.json");
    if blocks.exists() {
        let want = &load(&blocks)["data"]["doc_stats"];
        let got = serde_json::to_value(pi_layout::compute_doc_stats(&layouts)).unwrap();
        if let Some(d) = first_diff("doc_stats", want, &got) {
            errs.push(format!("04_blocks: {d}"));
        }
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs.join("\n  "))
    }
}

#[test]
fn stage_02_03_parity() {
    let root = golden_root();
    let Ok(entries) = std::fs::read_dir(&root) else {
        eprintln!("goldens not found at {}; skipping", root.display());
        return;
    };
    let only: Option<Vec<String>> = std::env::var("PI_PARITY_DOCS")
        .ok()
        .map(|s| s.split(',').map(str::to_string).collect());
    let mut docs: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.join("01_spans.json").exists() && p.join("02_lines.json").exists())
        .filter(|p| {
            only.as_ref().is_none_or(|o| {
                o.iter()
                    .any(|d| p.file_name().is_some_and(|n| n == d.as_str()))
            })
        })
        .collect();
    docs.sort();
    // Docs are independent; run them concurrently.
    let results: Vec<(String, Result<(), String>)> = std::thread::scope(|s| {
        let handles: Vec<_> = docs
            .iter()
            .map(|d| {
                let name = d.file_name().unwrap().to_string_lossy().to_string();
                (name, s.spawn(move || run_doc(d)))
            })
            .collect();
        handles
            .into_iter()
            .map(|(n, h)| {
                let r = h.join().unwrap_or_else(|_| Err("panicked".to_string()));
                (n, r)
            })
            .collect()
    });
    let mut failed = 0;
    for (name, r) in &results {
        match r {
            Ok(()) => eprintln!("{name}: clean"),
            Err(e) => {
                failed += 1;
                eprintln!("{name}: DIVERGED\n  {e}");
            }
        }
    }
    assert_eq!(failed, 0, "{failed}/{} docs diverged", results.len());
}
