//! Writes `06_candidates.json`, `07_outline.json` and `08_tree_raw.json` in the golden file
//! format, so `pageindex-rs diff --stage 6|7|8 --a <golden> --b <out>` can compare them.
//!
//! Usage: `dump_outline [--inject] <golden_root> <out_root> [doc ...]`. Each doc runs the Rust
//! pipeline from its `01_spans.json` (stages 02-05 in `pi_layout`); with `--inject` the
//! stage-05 state is rebuilt from the goldens instead (see `pi_outline::golden`).

use std::path::Path;

use pi_outline::golden::{classified_document, document_from_spans};
use pi_outline::pipeline::{extract_outline, openers_from_layout};
use pi_outline::{Doc, dump};

fn main() -> anyhow::Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let inject = args.first().is_some_and(|a| a == "--inject");
    if inject {
        args.remove(0);
    }
    if args.len() < 2 {
        anyhow::bail!("usage: dump_outline [--inject] <golden_root> <out_root> [doc ...]");
    }
    let (root, out_root) = (Path::new(&args[0]), Path::new(&args[1]));
    let docs: Vec<String> = if args.len() > 2 {
        args[2..].to_vec()
    } else {
        let mut d: Vec<String> = std::fs::read_dir(root)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().join("01_spans.json").exists())
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        d.sort();
        d
    };
    for doc in docs {
        let dir = root.join(&doc);
        let (document, openers) = if inject {
            let (document, openers, notes) =
                classified_document(&dir).map_err(anyhow::Error::msg)?;
            for n in notes {
                eprintln!("{doc}: {n}");
            }
            (document, openers)
        } else {
            let document = document_from_spans(&dir).map_err(anyhow::Error::msg)?;
            let classified = pi_layout::phases::classify_document(&document);
            let openers = openers_from_layout(&classified.section_openers);
            (document, openers)
        };
        let d = Doc::new(&document);
        let out = extract_outline(d, openers);
        let od = out_root.join(&doc);
        std::fs::create_dir_all(&od)?;
        let stages = [
            ("06_candidates", dump::candidates_stage(d, &out)),
            ("07_outline", dump::outline_stage(d, &out)),
            ("08_tree_raw", dump::tree_stage(&out)),
        ];
        for (name, data) in stages {
            let body = dump::wrap(&doc, name, data);
            std::fs::write(
                od.join(format!("{name}.json")),
                serde_json::to_string(&body)?,
            )?;
        }
        println!(
            "{doc}: {} candidates, {} top-level nodes",
            out.candidates.len(),
            out.tree.len()
        );
    }
    Ok(())
}
