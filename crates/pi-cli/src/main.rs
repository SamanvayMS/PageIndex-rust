//! `pageindex-rs`: command-line entry point.

mod diff;
mod index;

use std::io::Write;
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
    /// Index PDFs (local paths, directories, or s3:// / gs:// URIs) into a .pageindex store
    /// readable by the Python SDK.
    Index {
        /// TOML config (see pi-config); defaults plus PI_* environment when omitted.
        #[arg(long)]
        config: Option<PathBuf>,
        /// Store directory (default: [storage] index_root, ".pageindex").
        #[arg(long)]
        storage: Option<PathBuf>,
        /// Model-written node summaries (needs [llm.summary]; the default).
        #[arg(long, overrides_with = "no_summary")]
        summary: bool,
        /// Skip node summaries.
        #[arg(long, overrides_with = "summary")]
        no_summary: bool,
        /// Tree optimisation (default: [index] optimize, "full").
        #[arg(long, value_parser = ["full", "merge", "off"])]
        optimize: Option<String>,
        /// Triage pages and OCR the scanned ones through [ocr] ("off": text layer only).
        #[arg(long, default_value = "off", value_parser = ["auto", "off"])]
        ocr: String,
        /// Write one JSON line per document (timings, toc_source, nodes, error).
        #[arg(long)]
        report: Option<PathBuf>,
        /// Write each page_index_flash result to <DIR>/<stem>.json.
        #[arg(long)]
        tree_out: Option<PathBuf>,
        /// Also store flat one-node-per-page trees longer than 10 pages (the SDK refuses them).
        #[arg(long)]
        accept_flat: bool,
        /// PDF files, directories, or s3:// / gs:// URIs.
        #[arg(required = true)]
        inputs: Vec<String>,
    },
    /// Serve the retrieval tools over MCP (stdio by default).
    ServeMcp {
        /// Store directory.
        #[arg(long, default_value = ".pageindex")]
        storage: PathBuf,
        /// Serve streamable HTTP on this address (e.g. 127.0.0.1:8765) at /mcp instead of stdio.
        #[arg(long)]
        http: Option<std::net::SocketAddr>,
        /// Also register remove_document.
        #[arg(long)]
        management: bool,
    },
    /// Page triage (text / scanned / garbled / mixed / graphic) as JSON lines, one per page.
    Triage {
        #[arg(required = true)]
        pdfs: Vec<PathBuf>,
    },
    /// Stage-01 spans in the parity golden format: <OUT>/<stem>/01_spans.json.
    Extract {
        #[arg(long)]
        out: PathBuf,
        #[arg(required = true)]
        pdfs: Vec<PathBuf>,
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

fn run_triage(pdfs: &[PathBuf]) -> Result<()> {
    let mut out = std::io::stdout().lock();
    for pdf in pdfs {
        let doc = pdf.file_name().map(|n| n.to_string_lossy().into_owned());
        let pages = pi_triage::triage_pdf(pdf, &pi_triage::Thresholds::default())
            .with_context(|| format!("triage {}", pdf.display()))?;
        for p in pages {
            let line = serde_json::json!({
                "doc": doc, "page": p.signals.page, "label": p.label, "signals": p.signals,
            });
            writeln!(out, "{line}")?;
        }
    }
    Ok(())
}

fn run_extract(out: &Path, pdfs: &[PathBuf]) -> Result<()> {
    for pdf in pdfs {
        let t0 = std::time::Instant::now();
        let pages =
            pi_extract::extract_pdf(pdf).with_context(|| format!("extract {}", pdf.display()))?;
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
        let name = pdf.file_name().map(|s| s.to_string_lossy().into_owned());
        let stem = pdf
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "document".into());
        let body = serde_json::json!({"schema": 1, "reference": "619cbd8", "doc": name,
                                      "stage": "01_spans", "data": data});
        let dir = out.join(&stem);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("01_spans.json"), serde_json::to_string(&body)?)?;
        eprintln!(
            "{}: {} pages in {secs:.3}s ({:.1} ms/page) -> {}",
            pdf.display(),
            pages.len(),
            1000.0 * secs / pages.len().max(1) as f64,
            dir.display()
        );
    }
    Ok(())
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?)
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Index {
            config,
            storage,
            summary: _,
            no_summary,
            optimize,
            ocr,
            report,
            tree_out,
            accept_flat,
            inputs,
        } => {
            let args = index::IndexArgs {
                config,
                storage,
                summary: !no_summary,
                optimize: optimize.map(|o| match o.as_str() {
                    "full" => pi_config::Optimize::Full,
                    "merge" => pi_config::Optimize::Merge,
                    _ => pi_config::Optimize::Off,
                }),
                ocr: if ocr == "auto" {
                    pi_index::OcrMode::Auto
                } else {
                    pi_index::OcrMode::Off
                },
                report,
                tree_out,
                accept_flat,
                inputs,
            };
            if !runtime()?.block_on(index::run(args))? {
                std::process::exit(1);
            }
        }
        Cmd::ServeMcp {
            storage,
            http,
            management,
        } => {
            let server = pi_mcp::PageIndexServer::new(
                pi_store::LocalApi::new(&storage),
                pi_mcp::ServerOptions {
                    include_management: management,
                    doc_ids: None,
                },
            );
            let rt = runtime()?;
            match http {
                Some(addr) => rt.block_on(pi_mcp::server::serve_http(server, addr))?,
                None => rt.block_on(pi_mcp::serve_stdio(server))?,
            }
        }
        Cmd::Triage { pdfs } => run_triage(&pdfs)?,
        Cmd::Extract { out, pdfs } => run_extract(&out, &pdfs)?,
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
