//! `pageindex-rs index`: index PDFs into a Python-SDK-compatible `.pageindex/` store.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use pi_config::{Config, IngestSource, Optimize, RemoteUri};
use pi_index::{IndexOptions, IndexedDoc, LlmRoles, OcrMode, index_document};
use pi_storage::{AnySource, Mirror, Source};
use pi_store::LocalApi;
use pi_store::naming::sanitize_filename;
use serde_json::{Map, Value, json};

pub struct IndexArgs {
    pub config: Option<PathBuf>,
    pub storage: Option<PathBuf>,
    pub summary: bool,
    pub optimize: Option<Optimize>,
    pub ocr: OcrMode,
    pub report: Option<PathBuf>,
    pub tree_out: Option<PathBuf>,
    pub accept_flat: bool,
    pub inputs: Vec<String>,
}

fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

fn source_for(input: &str) -> Result<AnySource> {
    let src = match RemoteUri::parse(input, "input").map_err(|e| anyhow::anyhow!("{e}"))? {
        Some(uri) => IngestSource::Remote(uri),
        None => IngestSource::Local(PathBuf::from(input)),
    };
    Ok(AnySource::from_config(&src)?)
}

/// What one document produced: the report line.
struct Outcome {
    line: Map<String, Value>,
    ok: bool,
}

async fn index_one(
    bytes: Vec<u8>,
    name: &str,
    args: &IndexArgs,
    opts: &IndexOptions,
    roles: &LlmRoles,
    api: &LocalApi,
    mirror: Option<&Mirror>,
) -> Result<(IndexedDoc, Option<String>)> {
    let doc_name = sanitize_filename(name);
    if !doc_name.to_lowercase().ends_with(".pdf") {
        bail!("only PDF files are supported: {name}");
    }
    let doc = index_document(bytes, &doc_name, opts, Some(roles.clone())).await?;
    if let Some(dir) = &args.tree_out {
        let stem = Path::new(name)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "document".into());
        std::fs::create_dir_all(dir)?;
        std::fs::write(
            dir.join(format!("{stem}.json")),
            serde_json::to_string_pretty(&Value::Object(doc.result.clone()))?,
        )?;
    }
    let Some((doc_id, _)) = doc.commit(api, &doc_name, args.accept_flat)? else {
        return Ok((doc, None));
    };
    if let Some(m) = mirror {
        m.push_document(api.store().root(), &doc_id).await?;
    }
    Ok((doc, Some(doc_id)))
}

pub async fn run(args: IndexArgs) -> Result<bool> {
    let cfg = Config::load(args.config.as_deref()).map_err(|e| anyhow::anyhow!("config: {e}"))?;
    let mut opts = IndexOptions::from_config(&cfg);
    opts.summary = args.summary;
    if let Some(o) = args.optimize {
        opts.optimize = o;
    }
    opts.ocr = args.ocr;
    let roles = LlmRoles::from_config(&cfg.llm)?;
    if opts.summary && roles.summary.is_none() {
        bail!(
            "--summary needs a summary model: set [llm.summary] base_url and model (or \
             PI_LLM_BASE_URL / PI_LLM_MODEL), or pass --no-summary"
        );
    }
    if opts.optimize == Optimize::Full && roles.expand.is_none() && roles.summary.is_none() {
        bail!(
            "--optimize full needs a model for expand: set [llm.expand] or [llm.summary] \
             (or PI_LLM_BASE_URL / PI_LLM_MODEL), or pass --optimize merge"
        );
    }
    let storage = args
        .storage
        .clone()
        .unwrap_or_else(|| cfg.storage.index_root.clone());
    let api = LocalApi::new(&storage);
    let mirror = match &cfg.storage.mirror {
        Some(uri) => Some(Mirror::from_uri(uri)?),
        None => None,
    };
    let mut report = match &args.report {
        Some(p) => {
            if let Some(dir) = p.parent().filter(|d| !d.as_os_str().is_empty()) {
                std::fs::create_dir_all(dir)?;
            }
            Some(std::fs::File::create(p).with_context(|| format!("creating {}", p.display()))?)
        }
        None => None,
    };
    let (mut n_ok, mut n_err) = (0usize, 0usize);
    for input in &args.inputs {
        let source = source_for(input)?;
        let entries = match source.list().await {
            Ok(e) => e,
            Err(e) => {
                let line = json!({"doc": input, "error": e.to_string()});
                eprintln!("{input}: {e}");
                if let Some(f) = report.as_mut() {
                    writeln!(f, "{line}")?;
                }
                n_err += 1;
                continue;
            }
        };
        for entry in entries {
            let outcome = match source.fetch(&entry).await {
                Ok(fetched) => {
                    let bytes = std::fs::read(fetched.path())?;
                    let res = index_one(
                        bytes,
                        &entry.name,
                        &args,
                        &opts,
                        &roles,
                        &api,
                        mirror.as_ref(),
                    )
                    .await;
                    drop(fetched);
                    outcome_line(&entry.name, res)
                }
                Err(e) => outcome_line(&entry.name, Err(e.into())),
            };
            if outcome.ok {
                n_ok += 1;
            } else {
                n_err += 1;
            }
            let line = Value::Object(outcome.line);
            eprintln!("{line}");
            if let Some(f) = report.as_mut() {
                writeln!(f, "{line}")?;
                f.flush()?;
            }
        }
    }
    eprintln!(
        "indexed {n_ok} document(s), {n_err} failed -> {}",
        storage.display()
    );
    // Per-document failures are in the report; fail the run only when nothing indexed.
    Ok(n_ok > 0 || n_err == 0)
}

fn outcome_line(name: &str, res: Result<(IndexedDoc, Option<String>)>) -> Outcome {
    let mut line = Map::new();
    line.insert("doc".into(), json!(name));
    match res {
        Ok((doc, doc_id)) => {
            let t = &doc.timings;
            line.insert("pages".into(), json!(doc.page_texts.len()));
            line.insert("extract_s".into(), json!(round3(t.extract_s)));
            line.insert("ocr_s".into(), json!(round3(t.ocr_s)));
            line.insert("layout_s".into(), json!(round3(t.layout_s)));
            line.insert("structure_s".into(), json!(round3(t.structure_s)));
            line.insert("post_s".into(), json!(round3(t.post_s)));
            line.insert("description_s".into(), json!(round3(t.description_s)));
            line.insert("total_s".into(), json!(round3(t.total_s)));
            line.insert("toc_source".into(), json!(doc.toc_source()));
            line.insert("nodes".into(), json!(doc.node_count()));
            if let Some(m) = doc.metadata() {
                line.insert("page_labels".into(), m["page_labels"].clone());
            }
            if let Some(r) = &doc.rejection {
                line.insert("rejected".into(), json!(r));
            }
            line.insert("doc_id".into(), json!(doc_id));
            Outcome { line, ok: true }
        }
        Err(e) => {
            line.insert("error".into(), json!(format!("{e:#}")));
            Outcome { line, ok: false }
        }
    }
}
