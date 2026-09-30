//! Writes stages 02 (`02_lines.json`) and 03 (`03_columns.json`) for one document, in the
//! parity file format, so `pageindex-rs diff --stage 2 --a <golden> --b <out>` can compare.
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
    let elapsed = t0.elapsed();

    let out = PathBuf::from(out_root).join(&doc_id);
    std::fs::create_dir_all(&out)?;
    let stages: [(&str, Value); 2] = [
        (
            "02_lines",
            Value::Array(pages.iter().map(dump::page_lines).collect()),
        ),
        (
            "03_columns",
            Value::Array(pages.iter().map(dump::page_columns).collect()),
        ),
    ];
    for (stage, data) in stages {
        let p = out.join(format!("{stage}.json"));
        std::fs::write(
            &p,
            serde_json::to_string(&dump::wrap(&doc_name, stage, data))?,
        )?;
        println!("{}", p.display());
    }
    eprintln!(
        "{doc_id}: {} pages in {:.3}s",
        pages.len(),
        elapsed.as_secs_f64()
    );
    Ok(())
}
