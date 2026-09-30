//! Writes stages 02-05 (`02_lines.json`, `03_columns.json`, `04_blocks.json`,
//! `05_classified.json`) for one document in the parity file format, so
//! `pageindex-rs diff --stage N --a <golden> --b <out>` can compare.
//!
//! Usage: `cargo run --release -p pi-layout --example dump_lines -- <doc_dir> <out_root>`
//! where `<doc_dir>` holds `01_spans.json` (e.g. `parity/golden/earthmover`); output goes to
//! `<out_root>/<doc_dir name>/`.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use pi_core::PageSpans;
use pi_layout::dump;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
struct SpansFile {
    doc: Option<String>,
    data: Vec<PageSpans>,
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [doc_dir, out_root] = args.as_slice() else {
        bail!("usage: dump_lines <doc_dir containing 01_spans.json> <out_root>");
    };
    let doc_dir = PathBuf::from(doc_dir);
    let doc_id = doc_dir
        .file_name()
        .context("doc dir has no name")?
        .to_string_lossy()
        .to_string();
    let path = doc_dir.join("01_spans.json");
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let spans: SpansFile =
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    let doc_name = spans.doc.unwrap_or_else(|| doc_id.clone());

    let t0 = std::time::Instant::now();
    let pages = dump::process_document(&spans.data);
    let npages = pages.len();
    let lines = Value::Array(pages.iter().map(dump::page_lines).collect());
    let columns = Value::Array(pages.iter().map(dump::page_columns).collect());
    let doc = pi_layout::phases::build_document(pages);
    let blocks = dump::blocks_stage(&doc);
    let classified = pi_layout::phases::classify_document(&doc);
    let classified = dump::classified_stage(&doc, &classified);
    let elapsed = t0.elapsed();

    let out = PathBuf::from(out_root).join(&doc_id);
    std::fs::create_dir_all(&out)?;
    let stages: [(&str, Value); 4] = [
        ("02_lines", lines),
        ("03_columns", columns),
        ("04_blocks", blocks),
        ("05_classified", classified),
    ];
    for (stage, data) in stages {
        let p = out.join(format!("{stage}.json"));
        std::fs::write(
            &p,
            serde_json::to_string(&dump::wrap(&doc_name, stage, data))?,
        )?;
        println!("{}", p.display());
    }
    eprintln!("{doc_id}: {npages} pages in {:.3}s", elapsed.as_secs_f64());
    Ok(())
}
