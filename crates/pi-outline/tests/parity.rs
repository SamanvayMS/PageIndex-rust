//! Stage 06/07/08 parity against the Python reference goldens.
//!
//! For each doc under `$PI_GOLDEN` (default `/home/user/PageIndex-rust/parity/golden`), runs the
//! Rust pipeline from `01_spans.json` (stages 02-05 in `pi_layout`, then 06-08 here) and compares
//! the section openers with `05_classified.json` and stages 06-08 with `06_candidates.json`,
//! `07_outline.json` and `08_tree_raw.json`: ints, strings and bools exactly, floats within 1e-9
//! relative (`PI_PARITY_TOL` overrides). Skips when the goldens are absent.
//! `PI_PARITY_DOCS=a,b` restricts the run to some docs. `PI_OUTLINE_INJECT=1` instead injects
//! the stage-05 state from the goldens (see `pi_outline::golden`), isolating stages 06-08.

use std::path::{Path, PathBuf};

use pi_outline::golden::{classified_document, document_from_spans, first_diff, load};
use pi_outline::pipeline::{extract_outline, openers_from_layout};
use pi_outline::{Doc, dump};

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

fn run_doc(dir: &Path) -> Result<Vec<String>, String> {
    let inject = std::env::var("PI_OUTLINE_INJECT").is_ok_and(|v| v == "1");
    let (doc, openers, mut notes) = if inject {
        classified_document(dir)?
    } else {
        let doc = document_from_spans(dir)?;
        let classified = pi_layout::phases::classify_document(&doc);
        let openers = openers_from_layout(&classified.section_openers);
        let want = load(&dir.join("05_classified.json"))?;
        let got = dump::outline(Doc::new(&doc), &openers);
        let notes: Vec<String> = first_diff(
            "05 section_openers",
            &want["data"]["section_openers"],
            &got,
            rel_tol(),
        )
        .into_iter()
        .collect();
        (doc, openers, notes)
    };
    let d = Doc::new(&doc);
    let out = extract_outline(d, openers);
    let mut errs = Vec::new();
    let stages = [
        ("06_candidates", dump::candidates_stage(d, &out)),
        ("07_outline", dump::outline_stage(d, &out)),
        ("08_tree_raw", dump::tree_stage(&out)),
    ];
    for (name, got) in stages {
        let want = load(&dir.join(format!("{name}.json")))?;
        if let Some(diff) = first_diff(name, &want["data"], &got, rel_tol()) {
            errs.push(diff);
        }
    }
    notes.retain(|n| !n.starts_with("no 05a"));
    errs.extend(notes);
    Ok(errs)
}

#[test]
fn stages_06_to_08_match_goldens() {
    let root = golden_root();
    if !root.is_dir() {
        eprintln!("skipping: no goldens at {}", root.display());
        return;
    }
    let only: Option<Vec<String>> = std::env::var("PI_PARITY_DOCS")
        .ok()
        .map(|s| s.split(',').map(str::to_string).collect());
    let mut docs: Vec<PathBuf> = std::fs::read_dir(&root)
        .expect("golden root")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.join("01_spans.json").exists() && p.join("06_candidates.json").exists())
        .filter(|p| {
            only.as_ref().is_none_or(|o| {
                o.iter()
                    .any(|d| p.file_name().is_some_and(|n| n == d.as_str()))
            })
        })
        .collect();
    docs.sort();
    if docs.is_empty() {
        eprintln!("skipping: no golden docs under {}", root.display());
        return;
    }
    let mut failures = Vec::new();
    for dir in &docs {
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        match run_doc(dir) {
            Ok(errs) if errs.is_empty() => eprintln!("{name}: clean"),
            Ok(errs) => {
                eprintln!("{name}: {}", errs.join("\n  "));
                failures.push(name);
            }
            Err(e) => {
                eprintln!("{name}: error {e}");
                failures.push(name);
            }
        }
    }
    assert!(failures.is_empty(), "stage 06-08 divergences: {failures:?}");
}
