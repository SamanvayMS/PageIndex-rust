//! Write stage-01 spans in the parity golden format.
//!
//! Usage: dump_spans <file.pdf> <out_dir/doc_id>   (writes <out>/01_spans.json)

use std::time::Instant;

use anyhow::{Context, Result};
use serde_json::{Value, json};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let (pdf, out) = (
        args.get(1).context("pdf path")?,
        args.get(2).context("out dir")?,
    );
    let t0 = Instant::now();
    let pages = pi_extract::extract_pdf(std::path::Path::new(pdf))?;
    let secs = t0.elapsed().as_secs_f64();
    let mut data = serde_json::to_value(&pages)?;
    // The golden has no `source` field for text-layer spans.
    if let Value::Array(ps) = &mut data {
        for p in ps {
            if let Some(Value::Array(ss)) = p.get_mut("spans") {
                for s in ss {
                    if let Value::Object(m) = s {
                        m.remove("source");
                    }
                }
            }
        }
    }
    let name = std::path::Path::new(pdf)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned());
    let body = json!({"schema": 1, "reference": "619cbd8", "doc": name, "stage": "01_spans", "data": data});
    std::fs::create_dir_all(out)?;
    std::fs::write(
        format!("{out}/01_spans.json"),
        serde_json::to_string(&body)?,
    )?;
    eprintln!(
        "{} pages in {:.3}s ({:.1} ms/page)",
        pages.len(),
        secs,
        1000.0 * secs / pages.len().max(1) as f64
    );
    Ok(())
}
