//! Stage 09 (`apply_embedded_toc`) parity.
//!
//! - `fixtures/embedded_toc.json` (recorded from CPython by `gen/embedded_toc_fixture.py`):
//!   title normalization cases; bookmark reads (count + sha256 + validated sha + tier) for
//!   every PDF found locally; two SKELETON-tier (hybrid) FinanceBench docs with their stage-08
//!   tree, page texts and expected stage-09.
//! - Goldens: for each doc under env `PI_GOLDEN` (default
//!   `/home/user/PageIndex-rust/parity/golden`), 08 tree + page texts + the source PDF from
//!   `../corpus.toml` must give `09_tree_bookmarks.json`. Extra golden dirs (same layout, PDF in
//!   `bench/data/pdfs/<doc>`) can be given in env `PI_EXTRA_GOLDEN` (`:`-separated roots).
//!
//! PDF-dependent checks are skipped when PDFium cannot be loaded (set `PDFIUM_LIB`) or a PDF
//! is missing.

use std::path::{Path, PathBuf};

use pi_outline::embedded_toc::{
    Entry, PdfSource, Tier, apply_bookmark_entries, apply_embedded_toc, classify_bookmarks,
    is_generic_title, normalize_title, read_bookmarks, title_template, validate_bookmarks,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/embedded_toc.json")).unwrap()
}

fn pdfium_ok() -> bool {
    match pi_extract::pdfium::bindings() {
        Ok(_) => true,
        Err(e) => {
            eprintln!("PDFium unavailable ({e}); skipping PDF checks");
            false
        }
    }
}

fn entries_json(entries: &[Entry]) -> Value {
    Value::Array(
        entries
            .iter()
            .map(|e| json!({"title": e.title, "level": e.level, "page": e.page}))
            .collect(),
    )
}

fn entries_from(v: &Value) -> Vec<Entry> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|e| Entry {
            title: e["title"].as_str().unwrap().to_string(),
            level: e["level"].as_u64().unwrap() as usize,
            page: e["page"].as_i64().unwrap(),
        })
        .collect()
}

fn sha(v: &Value) -> String {
    Sha256::digest(serde_json::to_string(v).unwrap().as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Exact equality including key order; on mismatch, the neighbourhood of the first difference.
fn same(got: &Value, want: &Value) -> Result<(), String> {
    let (g, w) = (
        serde_json::to_string(got).unwrap(),
        serde_json::to_string(want).unwrap(),
    );
    if g == w {
        return Ok(());
    }
    let i = g
        .bytes()
        .zip(w.bytes())
        .position(|(a, b)| a != b)
        .unwrap_or(g.len().min(w.len()));
    let lo = g.floor_char_boundary(i.saturating_sub(150));
    let hi_g = g.ceil_char_boundary((i + 150).min(g.len()));
    let hi_w = w.ceil_char_boundary((i + 150).min(w.len()));
    Err(format!(
        "got  ...{}\nwant ...{}",
        &g[lo..hi_g],
        &w[lo.min(w.len())..hi_w]
    ))
}

#[test]
fn titles_match_cpython() {
    for case in fixture()["titles"].as_array().unwrap() {
        let t = case["title"].as_str().unwrap();
        assert_eq!(
            normalize_title(t),
            case["norm"].as_str().unwrap(),
            "normalize {t:?}"
        );
        assert_eq!(
            title_template(t),
            case["template"].as_str().unwrap(),
            "template {t:?}"
        );
        assert_eq!(
            is_generic_title(t),
            case["generic"].as_bool().unwrap(),
            "generic {t:?}"
        );
    }
}

#[test]
fn bookmark_reads_match_pypdfium2() {
    if !pdfium_ok() {
        return;
    }
    let (mut checked, mut tiers) = (0, [0usize; 4]);
    for case in fixture()["bookmarks"].as_array().unwrap() {
        let path = Path::new(case["file"].as_str().unwrap());
        if !path.is_file() {
            continue;
        }
        let n = case["n_pages"].as_i64().unwrap();
        let raw = read_bookmarks(PdfSource::Path(path));
        assert_eq!(
            raw.len() as u64,
            case["count"].as_u64().unwrap(),
            "{}",
            path.display()
        );
        assert_eq!(
            sha(&entries_json(&raw)),
            case["sha"].as_str().unwrap(),
            "{}",
            path.display()
        );
        let val = validate_bookmarks(&raw, n);
        assert_eq!(
            sha(&entries_json(&val)),
            case["validated_sha"].as_str().unwrap(),
            "{}",
            path.display()
        );
        let tier = classify_bookmarks(&val, n);
        assert_eq!(
            tier as u64,
            case["tier"].as_u64().unwrap(),
            "{}",
            path.display()
        );
        tiers[tier as usize] += 1;
        checked += 1;
    }
    eprintln!(
        "bookmark reads checked: {checked} (ignore {}, skeleton {}, full {})",
        tiers[1], tiers[2], tiers[3]
    );
}

#[test]
fn hybrid_docs_match_reference() {
    let f = fixture();
    let hybrid = f["hybrid"].as_array().unwrap();
    assert!(!hybrid.is_empty());
    for case in hybrid {
        let doc = case["doc"].as_str().unwrap();
        let entries = entries_from(&case["entries"]);
        let n = case["n_pages"].as_i64().unwrap();
        assert_eq!(
            classify_bookmarks(&validate_bookmarks(&entries, n), n),
            Tier::Skeleton,
            "{doc}"
        );
        let texts: Vec<String> = case["page_texts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t.as_str().unwrap().to_string())
            .collect();
        let (structure, source) = apply_bookmark_entries(
            case["structure"].as_array().unwrap(),
            &entries,
            n,
            Some(&texts),
        );
        let got = json!({"toc_source": source, "structure": structure});
        if let Err(d) = same(&got, &case["expected"]) {
            panic!("{doc}: {d}");
        }
        eprintln!("PASS hybrid {doc}");
    }
}

fn golden_dir() -> PathBuf {
    std::env::var_os("PI_GOLDEN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/home/user/PageIndex-rust/parity/golden"))
}

/// `id -> pdf path` from parity/corpus.toml (next to the golden dir).
fn corpus(parity: &Path) -> Vec<(String, PathBuf)> {
    let Ok(text) = std::fs::read_to_string(parity.join("corpus.toml")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut id = None;
    for line in text.lines() {
        let line = line.trim();
        let val = |l: &str| {
            l.split_once('=')
                .map(|(_, v)| v.trim().trim_matches('"').to_string())
        };
        if line.starts_with("id =") || line.starts_with("id=") {
            id = val(line);
        } else if (line.starts_with("path =") || line.starts_with("path=")) && id.is_some() {
            let p = PathBuf::from(val(line).unwrap());
            out.push((
                id.take().unwrap(),
                if p.is_absolute() { p } else { parity.join(p) },
            ));
        }
    }
    out
}

fn load(p: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn check_dir(dir: &Path, pdf: &Path) -> Result<String, String> {
    let s08 = load(&dir.join("08_tree_raw.json"));
    let want = load(&dir.join("09_tree_bookmarks.json"))["data"].clone();
    let texts: Vec<String> = load(&dir.join("page_texts.json"))["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t.as_str().unwrap_or("").to_string())
        .collect();
    let (structure, source) = apply_embedded_toc(
        s08["data"].as_array().unwrap(),
        PdfSource::Path(pdf),
        texts.len() as i64,
        Some(&texts),
    );
    same(
        &json!({"toc_source": source, "structure": structure}),
        &want,
    )?;
    Ok(source.to_string())
}

#[test]
fn goldens_match_09() {
    let root = golden_dir();
    if !root.is_dir() || !pdfium_ok() {
        eprintln!("goldens or PDFium missing; skipping");
        return;
    }
    let parity = root.parent().unwrap().to_path_buf();
    let mut jobs: Vec<(String, PathBuf, PathBuf)> = corpus(&parity)
        .into_iter()
        .map(|(id, pdf)| (id.clone(), root.join(&id), pdf))
        .filter(|(_, d, _)| d.join("09_tree_bookmarks.json").is_file())
        .collect();
    if let Some(extra) = std::env::var_os("PI_EXTRA_GOLDEN") {
        let bench = parity.parent().unwrap().join("bench/data/pdfs");
        for r in std::env::split_paths(&extra) {
            for d in std::fs::read_dir(&r).into_iter().flatten().flatten() {
                let name = d.file_name().to_string_lossy().to_string();
                jobs.push((name.clone(), d.path(), bench.join(format!("{name}.pdf"))));
            }
        }
    }
    let mut failures = Vec::new();
    let mut ran = 0;
    for (id, dir, pdf) in jobs {
        if !pdf.is_file() {
            eprintln!("SKIP {id}: {} missing", pdf.display());
            continue;
        }
        ran += 1;
        match check_dir(&dir, &pdf) {
            Ok(source) => eprintln!("PASS {id} ({source})"),
            Err(d) => {
                eprintln!("FAIL {id}: {d}");
                failures.push(id);
            }
        }
    }
    eprintln!("{ran} docs, {} failures", failures.len());
    assert!(failures.is_empty(), "{failures:?}");
}
