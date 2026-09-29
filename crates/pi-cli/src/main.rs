//! `pageindex-rs`: command-line entry point.

mod diff;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use serde_json::Value;

#[derive(Parser)]
#[command(
    name = "pageindex-rs",
    version,
    about = "Rust PageIndex: vectorless document indexing"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Diff parity stage dumps between two golden directories (e.g. Python golden vs Rust output).
    Diff {
        /// Stage number (1-10), stage file stem (e.g. 04_blocks), or "all".
        #[arg(long, default_value = "all")]
        stage: String,
        /// Reference directory (parity/golden).
        #[arg(long, default_value = "parity/golden")]
        a: PathBuf,
        /// Candidate directory.
        #[arg(long)]
        b: PathBuf,
        /// Absolute/relative float tolerance.
        #[arg(long, default_value_t = 1e-6)]
        tol: f64,
        /// Divergences to print per stage.
        #[arg(long, default_value_t = 5)]
        show: usize,
        /// Document ids (subdirectories); all present in --a when omitted.
        docs: Vec<String>,
    },
}

fn stage_files(dir: &Path, stage: &str) -> Result<Vec<String>> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.ends_with(".json") && n.as_bytes().first().is_some_and(u8::is_ascii_digit))
        .map(|n| n.trim_end_matches(".json").to_string())
        .collect();
    names.sort();
    if stage == "all" {
        return Ok(names);
    }
    let want = match stage.parse::<u32>() {
        Ok(n) => format!("{n:02}_"),
        Err(_) => stage.to_string(),
    };
    let hit: Vec<String> = names.into_iter().filter(|n| n.starts_with(&want)).collect();
    if hit.is_empty() {
        bail!("no stage matching {stage:?} in {}", dir.display());
    }
    Ok(hit)
}

fn load(path: &Path) -> Result<Value> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

fn run_diff(
    stage: &str,
    a: &Path,
    b: &Path,
    tol: f64,
    show: usize,
    docs: Vec<String>,
) -> Result<bool> {
    let docs = if docs.is_empty() {
        let mut d: Vec<String> = std::fs::read_dir(a)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        d.sort();
        d
    } else {
        docs
    };
    let mut clean = true;
    for doc in &docs {
        for name in stage_files(&a.join(doc), stage)? {
            let pb = b.join(doc).join(format!("{name}.json"));
            if !pb.exists() {
                println!("{doc} {name}: MISSING in {}", b.display());
                clean = false;
                continue;
            }
            let (va, vb) = (load(&a.join(doc).join(format!("{name}.json")))?, load(&pb)?);
            let (da, db) = (va.get("data").unwrap_or(&va), vb.get("data").unwrap_or(&vb));
            let mut d = diff::Differ::new(tol, show);
            d.diff("data", da, db);
            if d.total == 0 {
                println!("{doc} {name}: clean");
                continue;
            }
            clean = false;
            println!("{doc} {name}: {} divergence(s)", d.total);
            let root = serde_json::json!({ "data": da });
            if let Some(first) = d.found.first()
                && let Some(loc) = diff::locate(&first.path, &root)
            {
                println!("  first divergence at {loc}");
            }
            for dv in &d.found {
                println!("  {}\n    a: {}\n    b: {}", dv.path, dv.a, dv.b);
            }
        }
    }
    Ok(clean)
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Diff {
            stage,
            a,
            b,
            tol,
            show,
            docs,
        } => {
            if !run_diff(&stage, &a, &b, tol, show, docs)? {
                std::process::exit(1);
            }
        }
    }
    Ok(())
}
