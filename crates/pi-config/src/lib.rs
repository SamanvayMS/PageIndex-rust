//! Configuration: LLM/OCR endpoints, storage, ingest and indexing options.
//!
//! A TOML file (all sections optional) plus environment overrides:
//!
//! ```toml
//! [llm.summary]            # also [llm.expand], [llm.description], [llm.chat]
//! base_url = "https://api.openai.com/v1"
//! model = "gpt-5.6-luna"
//! api_key_env = "OPENAI_API_KEY"   # the NAME of the variable holding the key
//! concurrency = 64
//! timeout_s = 600
//! max_retries = 10
//! inherit = "summary"              # unset fields come from this role ("none" to stop)
//!
//! [ocr]
//! base_url = "..."; model = "..."; api_key_env = "PI_OCR_KEY"
//! dpi = 200; concurrency = 8; json_mode = true; profile = "spans-json"; timeout_s = 120
//! max_tokens = 8192; tables = true; max_pixels = 1605632   # profile = "paddleocr-vl"
//!
//! [storage]
//! index_root = ".pageindex"
//! mirror = "s3://bucket/prefix"    # or gs://bucket/prefix
//!
//! [ingest]
//! source = "pdfs/"                 # local path, s3:// or gs:// URI
//!
//! [index]
//! use_embedded_toc = true
//! optimize = "full"                # "full" | "merge" | "off"
//! summary_max_words = 150
//! ```
//!
//! Environment (beats the file): `PI_LLM_BASE_URL`, `PI_LLM_MODEL`, `PI_LLM_KEY`,
//! `PI_LLM_CONCURRENCY`, `PI_LLM_TIMEOUT_S`, `PI_LLM_MAX_RETRIES` apply to every role unless
//! the role-specific `PI_LLM_<ROLE>_<FIELD>` is set; `PI_OCR_BASE_URL`, `PI_OCR_MODEL`,
//! `PI_OCR_KEY`, `PI_OCR_DPI`, `PI_OCR_CONCURRENCY`, `PI_OCR_JSON_MODE`, `PI_OCR_PROFILE`,
//! `PI_OCR_TIMEOUT_S`, `PI_OCR_MAX_TOKENS`, `PI_OCR_TABLES`, `PI_OCR_MAX_PIXELS`.
//! A `*_KEY` variable holds the key itself; the resolved config records
//! only which variable to read, never a key value.

pub mod consts;

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use consts::*;

/// A configuration problem, naming the offending field or variable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    /// Dotted TOML path (`llm.expand.inherit`) or environment variable name.
    pub field: String,
    pub message: String,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}

impl std::error::Error for ConfigError {}

fn err(field: impl Into<String>, message: impl Into<String>) -> ConfigError {
    ConfigError {
        field: field.into(),
        message: message.into(),
    }
}

// ── file schema (every field optional) ──

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    llm: RawLlm,
    #[serde(default)]
    ocr: RawOcr,
    #[serde(default)]
    storage: RawStorage,
    #[serde(default)]
    ingest: RawIngest,
    #[serde(default)]
    index: RawIndex,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLlm {
    summary: Option<RawRole>,
    expand: Option<RawRole>,
    description: Option<RawRole>,
    chat: Option<RawRole>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRole {
    base_url: Option<String>,
    model: Option<String>,
    api_key_env: Option<String>,
    concurrency: Option<i64>,
    timeout_s: Option<f64>,
    max_retries: Option<i64>,
    inherit: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOcr {
    base_url: Option<String>,
    model: Option<String>,
    api_key_env: Option<String>,
    dpi: Option<i64>,
    concurrency: Option<i64>,
    json_mode: Option<bool>,
    profile: Option<String>,
    timeout_s: Option<f64>,
    max_tokens: Option<i64>,
    tables: Option<bool>,
    max_pixels: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStorage {
    index_root: Option<PathBuf>,
    mirror: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawIngest {
    source: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawIndex {
    use_embedded_toc: Option<bool>,
    optimize: Option<String>,
    summary_max_words: Option<i64>,
}

// ── resolved config ──

/// One LLM role's endpoint (OpenAI-compatible).
#[derive(Debug, Clone, PartialEq)]
pub struct LlmRole {
    pub base_url: Option<String>,
    pub model: Option<String>,
    /// Name of the environment variable that holds the API key.
    pub api_key_env: String,
    pub concurrency: usize,
    pub timeout_s: f64,
    pub max_retries: u32,
}

impl LlmRole {
    /// The API key, read from the environment at call time.
    pub fn api_key(&self) -> Option<String> {
        std::env::var(&self.api_key_env)
            .ok()
            .filter(|k| !k.is_empty())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LlmConfig {
    pub summary: LlmRole,
    pub expand: LlmRole,
    pub description: LlmRole,
    pub chat: LlmRole,
}

impl LlmConfig {
    pub fn role(&self, name: &str) -> Option<&LlmRole> {
        match name {
            "summary" => Some(&self.summary),
            "expand" => Some(&self.expand),
            "description" => Some(&self.description),
            "chat" => Some(&self.chat),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct OcrConfig {
    pub base_url: Option<String>,
    pub model: Option<String>,
    /// Name of the environment variable that holds the API key.
    pub api_key_env: String,
    pub dpi: u32,
    pub concurrency: usize,
    pub json_mode: bool,
    /// Engine protocol: `spans-json` (generic vision model asked for a JSON block layout) or
    /// `paddleocr-vl` (PaddleOCR-VL task prompts: `Spotting:` + `Table Recognition:`).
    pub profile: String,
    pub timeout_s: f64,
    /// Completion token cap per OCR request.
    pub max_tokens: u32,
    /// `paddleocr-vl`: also recognise detected table regions into markdown.
    pub tables: bool,
    /// `paddleocr-vl`: pixel budget for the page image (the model's spotting max_pixels).
    pub max_pixels: u64,
}

/// Recognised `[ocr] profile` values.
pub const OCR_PROFILES: [&str; 2] = ["spans-json", "paddleocr-vl"];

impl OcrConfig {
    pub fn api_key(&self) -> Option<String> {
        std::env::var(&self.api_key_env)
            .ok()
            .filter(|k| !k.is_empty())
    }
}

/// Cloud object-store scheme of a mirror or source URI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    S3,
    Gs,
}

/// A parsed `s3://bucket/prefix` or `gs://bucket/prefix` URI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteUri {
    pub scheme: Scheme,
    pub bucket: String,
    /// Key prefix without leading or trailing `/` (may be empty).
    pub prefix: String,
}

impl RemoteUri {
    /// Parse an `s3://` or `gs://` URI; `Ok(None)` for anything else (a local path).
    pub fn parse(uri: &str, field: &str) -> Result<Option<RemoteUri>, ConfigError> {
        let (scheme, rest) = if let Some(rest) = uri.strip_prefix("s3://") {
            (Scheme::S3, rest)
        } else if let Some(rest) = uri.strip_prefix("gs://") {
            (Scheme::Gs, rest)
        } else if uri.contains("://") {
            return Err(err(
                field,
                format!("unsupported URI scheme in {uri:?} (use s3:// or gs://)"),
            ));
        } else {
            return Ok(None);
        };
        let (bucket, prefix) = rest.split_once('/').unwrap_or((rest, ""));
        if bucket.is_empty() {
            return Err(err(field, format!("missing bucket in {uri:?}")));
        }
        Ok(Some(RemoteUri {
            scheme,
            bucket: bucket.to_string(),
            prefix: prefix.trim_matches('/').to_string(),
        }))
    }
}

impl fmt::Display for RemoteUri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let scheme = match self.scheme {
            Scheme::S3 => "s3",
            Scheme::Gs => "gs",
        };
        if self.prefix.is_empty() {
            write!(f, "{scheme}://{}", self.bucket)
        } else {
            write!(f, "{scheme}://{}/{}", self.bucket, self.prefix)
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StorageConfig {
    pub index_root: PathBuf,
    pub mirror: Option<RemoteUri>,
}

/// Where input PDFs come from.
#[derive(Debug, Clone, PartialEq)]
pub enum IngestSource {
    Local(PathBuf),
    Remote(RemoteUri),
}

#[derive(Debug, Clone, PartialEq)]
pub struct IngestConfig {
    pub source: Option<IngestSource>,
}

/// Tree optimisation mode (the reference's `optimize`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Optimize {
    Full,
    Merge,
    Off,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IndexConfig {
    pub use_embedded_toc: bool,
    pub optimize: Optimize,
    pub summary_max_words: u32,
}

/// The resolved configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub llm: LlmConfig,
    pub ocr: OcrConfig,
    pub storage: StorageConfig,
    pub ingest: IngestConfig,
    pub index: IndexConfig,
}

impl Default for Config {
    fn default() -> Self {
        Config::resolve(RawConfig::default(), &|_| None).expect("defaults are valid")
    }
}

impl Config {
    /// Load `path` (or only defaults when `None`) and apply the process environment.
    pub fn load(path: Option<&Path>) -> Result<Config, ConfigError> {
        Self::load_with_env(path, &|k| std::env::var(k).ok())
    }

    /// As [`Config::load`], with an explicit environment lookup.
    pub fn load_with_env(
        path: Option<&Path>,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Config, ConfigError> {
        let raw = match path {
            None => RawConfig::default(),
            Some(p) => {
                let text = std::fs::read_to_string(p)
                    .map_err(|e| err(p.display().to_string(), e.to_string()))?;
                parse(&text)?
            }
        };
        Self::resolve(raw, env)
    }

    /// Parse TOML text and apply `env`.
    pub fn from_toml_str(
        text: &str,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Config, ConfigError> {
        Self::resolve(parse(text)?, env)
    }

    fn resolve(
        raw: RawConfig,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Config, ConfigError> {
        let env = |k: &str| env(k).filter(|v| !v.is_empty());
        Ok(Config {
            llm: resolve_llm(&raw.llm, &env)?,
            ocr: resolve_ocr(&raw.ocr, &env)?,
            storage: StorageConfig {
                index_root: raw
                    .storage
                    .index_root
                    .unwrap_or_else(|| PathBuf::from(INDEX_ROOT)),
                mirror: match &raw.storage.mirror {
                    None => None,
                    Some(uri) => {
                        Some(RemoteUri::parse(uri, "storage.mirror")?.ok_or_else(|| {
                            err(
                                "storage.mirror",
                                format!("{uri:?} is not an s3:// or gs:// URI"),
                            )
                        })?)
                    }
                },
            },
            ingest: IngestConfig {
                source: match &raw.ingest.source {
                    None => None,
                    Some(s) if s.is_empty() => {
                        return Err(err("ingest.source", "must not be empty"));
                    }
                    Some(s) => Some(match RemoteUri::parse(s, "ingest.source")? {
                        Some(uri) => IngestSource::Remote(uri),
                        None => IngestSource::Local(PathBuf::from(s)),
                    }),
                },
            },
            index: IndexConfig {
                use_embedded_toc: raw.index.use_embedded_toc.unwrap_or(true),
                optimize: match raw.index.optimize.as_deref() {
                    None | Some("full") => Optimize::Full,
                    Some("merge") => Optimize::Merge,
                    Some("off") => Optimize::Off,
                    Some(other) => {
                        return Err(err(
                            "index.optimize",
                            format!("{other:?} is not one of \"full\", \"merge\", \"off\""),
                        ));
                    }
                },
                summary_max_words: positive(
                    raw.index.summary_max_words,
                    SUMMARY_MAX_WORDS as i64,
                    "index.summary_max_words",
                )? as u32,
            },
        })
    }
}

fn parse(text: &str) -> Result<RawConfig, ConfigError> {
    toml::from_str(text).map_err(|e| {
        let field = e
            .message()
            .split('`')
            .nth(1)
            .map(str::to_string)
            .unwrap_or_else(|| "config".into());
        err(field, e.to_string().trim().to_string())
    })
}

fn positive(value: Option<i64>, default: i64, field: &str) -> Result<i64, ConfigError> {
    let v = value.unwrap_or(default);
    if v < 1 || v > u32::MAX as i64 {
        return Err(err(field, format!("must be a positive integer, got {v}")));
    }
    Ok(v)
}

fn non_negative(value: Option<i64>, default: i64, field: &str) -> Result<i64, ConfigError> {
    let v = value.unwrap_or(default);
    if v < 0 || v > u32::MAX as i64 {
        return Err(err(
            field,
            format!("must be a non-negative integer, got {v}"),
        ));
    }
    Ok(v)
}

fn seconds(value: Option<f64>, default: f64, field: &str) -> Result<f64, ConfigError> {
    let v = value.unwrap_or(default);
    if !(v.is_finite() && v > 0.0) {
        return Err(err(
            field,
            format!("must be a positive number of seconds, got {v}"),
        ));
    }
    Ok(v)
}

fn env_int(env: &dyn Fn(&str) -> Option<String>, var: &str) -> Result<Option<i64>, ConfigError> {
    env(var)
        .map(|v| {
            v.trim()
                .parse::<i64>()
                .map_err(|_| err(var, format!("expected an integer, got {v:?}")))
        })
        .transpose()
}

fn env_float(env: &dyn Fn(&str) -> Option<String>, var: &str) -> Result<Option<f64>, ConfigError> {
    env(var)
        .map(|v| {
            v.trim()
                .parse::<f64>()
                .map_err(|_| err(var, format!("expected a number, got {v:?}")))
        })
        .transpose()
}

fn env_bool(env: &dyn Fn(&str) -> Option<String>, var: &str) -> Result<Option<bool>, ConfigError> {
    env(var)
        .map(|v| match v.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Ok(true),
            "0" | "false" | "no" | "off" => Ok(false),
            _ => Err(err(var, format!("expected a boolean, got {v:?}"))),
        })
        .transpose()
}

/// Built-in inheritance: every role but `summary` falls back to `summary` (the reference
/// runs expand and the document description on the summary model).
fn default_inherit(role: &str) -> Option<&'static str> {
    (role != "summary").then_some("summary")
}

fn raw_role<'a>(llm: &'a RawLlm, role: &str) -> Option<&'a RawRole> {
    match role {
        "summary" => llm.summary.as_ref(),
        "expand" => llm.expand.as_ref(),
        "description" => llm.description.as_ref(),
        "chat" => llm.chat.as_ref(),
        _ => None,
    }
}

/// A role's file fields merged along its inherit chain (nearest wins).
fn merged_role(llm: &RawLlm, role: &str) -> Result<RawRole, ConfigError> {
    let mut chain: Vec<&str> = Vec::new();
    let mut current = Some(ROLES.iter().copied().find(|r| *r == role).unwrap_or(role));
    while let Some(name) = current {
        if chain.contains(&name) {
            return Err(err(
                format!("llm.{}.inherit", chain[chain.len() - 1]),
                format!("inheritance cycle: {} -> {name}", chain.join(" -> ")),
            ));
        }
        chain.push(name);
        let raw = raw_role(llm, name);
        current = match raw.and_then(|r| r.inherit.as_deref()) {
            Some("none") | Some("") => None,
            Some(target) => Some(ROLES.iter().copied().find(|r| *r == target).ok_or_else(
                || {
                    err(
                        format!("llm.{name}.inherit"),
                        format!("unknown role {target:?} (use one of {ROLES:?} or \"none\")"),
                    )
                },
            )?),
            None => default_inherit(name),
        };
    }
    let mut out = RawRole::default();
    for name in chain {
        let Some(r) = raw_role(llm, name) else {
            continue;
        };
        out.base_url = out.base_url.or_else(|| r.base_url.clone());
        out.model = out.model.or_else(|| r.model.clone());
        out.api_key_env = out.api_key_env.or_else(|| r.api_key_env.clone());
        out.concurrency = out.concurrency.or(r.concurrency);
        out.timeout_s = out.timeout_s.or(r.timeout_s);
        out.max_retries = out.max_retries.or(r.max_retries);
    }
    Ok(out)
}

fn default_concurrency(role: &str) -> usize {
    match role {
        "summary" => SUMMARY_CONCURRENCY,
        "expand" => EXPAND_CONCURRENCY,
        "description" => DESCRIPTION_CONCURRENCY,
        _ => CHAT_CONCURRENCY,
    }
}

fn resolve_llm(
    llm: &RawLlm,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<LlmConfig, ConfigError> {
    let mut roles = BTreeMap::new();
    for role in ROLES {
        let file = merged_role(llm, role)?;
        // Concurrency is per lane: inherit it only when the role's own section sets none
        // and it names its source explicitly.
        let own_concurrency = raw_role(llm, role).and_then(|r| r.concurrency);
        let explicit_inherit = raw_role(llm, role).is_some_and(|r| r.inherit.is_some());
        let file_concurrency = if explicit_inherit {
            file.concurrency
        } else {
            own_concurrency
        };
        let up = role.to_ascii_uppercase();
        let pick = |field: &str| {
            env(&format!("PI_LLM_{up}_{field}")).or_else(|| env(&format!("PI_LLM_{field}")))
        };
        let pick_var = |field: &str| {
            let specific = format!("PI_LLM_{up}_{field}");
            if env(&specific).is_some() {
                return Some(specific);
            }
            let global = format!("PI_LLM_{field}");
            env(&global).is_some().then_some(global)
        };
        let num = |field: &str| -> Result<Option<i64>, ConfigError> {
            match pick_var(field) {
                Some(var) => env_int(env, &var),
                None => Ok(None),
            }
        };
        let float = |field: &str| -> Result<Option<f64>, ConfigError> {
            match pick_var(field) {
                Some(var) => env_float(env, &var),
                None => Ok(None),
            }
        };
        let prefix = format!("llm.{role}");
        let resolved = LlmRole {
            base_url: pick("BASE_URL").or(file.base_url),
            model: pick("MODEL").or(file.model),
            api_key_env: pick_var("KEY")
                .or(file.api_key_env)
                .unwrap_or_else(|| LLM_KEY_ENV.to_string()),
            concurrency: positive(
                num("CONCURRENCY")?.or(file_concurrency),
                default_concurrency(role) as i64,
                &format!("{prefix}.concurrency"),
            )? as usize,
            timeout_s: seconds(
                float("TIMEOUT_S")?.or(file.timeout_s),
                LLM_TIMEOUT_S,
                &format!("{prefix}.timeout_s"),
            )?,
            max_retries: non_negative(
                num("MAX_RETRIES")?.or(file.max_retries),
                LLM_MAX_RETRIES as i64,
                &format!("{prefix}.max_retries"),
            )? as u32,
        };
        if let Some(url) = &resolved.base_url {
            check_url(url, &format!("{prefix}.base_url"))?;
        }
        roles.insert(role, resolved);
    }
    Ok(LlmConfig {
        summary: roles.remove("summary").unwrap(),
        expand: roles.remove("expand").unwrap(),
        description: roles.remove("description").unwrap(),
        chat: roles.remove("chat").unwrap(),
    })
}

fn check_url(url: &str, field: &str) -> Result<(), ConfigError> {
    if url.starts_with("http://") || url.starts_with("https://") {
        Ok(())
    } else {
        Err(err(field, format!("{url:?} is not an http(s) URL")))
    }
}

fn resolve_ocr(
    raw: &RawOcr,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<OcrConfig, ConfigError> {
    let base_url = env("PI_OCR_BASE_URL").or_else(|| raw.base_url.clone());
    if let Some(url) = &base_url {
        check_url(url, "ocr.base_url")?;
    }
    let dpi = positive(
        env_int(env, "PI_OCR_DPI")?.or(raw.dpi),
        OCR_DPI as i64,
        "ocr.dpi",
    )?;
    if !(36..=1200).contains(&dpi) {
        return Err(err(
            "ocr.dpi",
            format!("must be within 36..=1200, got {dpi}"),
        ));
    }
    let profile = env("PI_OCR_PROFILE")
        .or_else(|| raw.profile.clone())
        .unwrap_or_else(|| OCR_PROFILE.to_string());
    if !OCR_PROFILES.contains(&profile.as_str()) {
        return Err(err(
            "ocr.profile",
            format!("must be one of {OCR_PROFILES:?}, got {profile:?}"),
        ));
    }
    Ok(OcrConfig {
        base_url,
        model: env("PI_OCR_MODEL").or_else(|| raw.model.clone()),
        api_key_env: if env("PI_OCR_KEY").is_some() {
            OCR_KEY_ENV.to_string()
        } else {
            raw.api_key_env
                .clone()
                .unwrap_or_else(|| OCR_KEY_ENV.to_string())
        },
        dpi: dpi as u32,
        concurrency: positive(
            env_int(env, "PI_OCR_CONCURRENCY")?.or(raw.concurrency),
            OCR_CONCURRENCY as i64,
            "ocr.concurrency",
        )? as usize,
        json_mode: env_bool(env, "PI_OCR_JSON_MODE")?
            .or(raw.json_mode)
            .unwrap_or(OCR_JSON_MODE),
        profile,
        timeout_s: seconds(
            env_float(env, "PI_OCR_TIMEOUT_S")?.or(raw.timeout_s),
            OCR_TIMEOUT_S,
            "ocr.timeout_s",
        )?,
        max_tokens: positive(
            env_int(env, "PI_OCR_MAX_TOKENS")?.or(raw.max_tokens),
            8192,
            "ocr.max_tokens",
        )? as u32,
        tables: env_bool(env, "PI_OCR_TABLES")?
            .or(raw.tables)
            .unwrap_or(true),
        // PaddleOCR-VL spotting budget: 2048 * 28 * 28 pixels (model card).
        max_pixels: positive(
            env_int(env, "PI_OCR_MAX_PIXELS")?.or(raw.max_pixels),
            2048 * 28 * 28,
            "ocr.max_pixels",
        )? as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| map.get(k).cloned()
    }

    #[test]
    fn defaults() {
        let c = Config::from_toml_str("", &env_of(&[])).unwrap();
        assert_eq!(c.llm.summary.concurrency, 64);
        assert_eq!(c.llm.expand.concurrency, 32);
        assert_eq!(c.llm.summary.max_retries, 10);
        assert_eq!(c.llm.chat.api_key_env, "PI_LLM_KEY");
        assert_eq!(c.ocr.dpi, 200);
        assert_eq!(c.ocr.profile, "spans-json");
        assert_eq!(c.storage.index_root, PathBuf::from(".pageindex"));
        assert_eq!(c.index.optimize, Optimize::Full);
        assert_eq!(c.index.summary_max_words, 150);
        assert!(c.index.use_embedded_toc);
        assert_eq!(c, Config::default());
    }

    #[test]
    fn inheritance_and_env_precedence() {
        let toml = r#"
            [llm.summary]
            base_url = "https://a.example/v1"
            model = "small"
            api_key_env = "A_KEY"
            concurrency = 16
            [llm.expand]
            model = "big"
            [llm.chat]
            inherit = "expand"
            timeout_s = 30
        "#;
        let c = Config::from_toml_str(toml, &env_of(&[])).unwrap();
        assert_eq!(
            c.llm.expand.base_url.as_deref(),
            Some("https://a.example/v1")
        );
        assert_eq!(c.llm.expand.model.as_deref(), Some("big"));
        assert_eq!(
            c.llm.expand.concurrency, 32,
            "lane defaults are not inherited implicitly"
        );
        assert_eq!(c.llm.description.model.as_deref(), Some("small"));
        assert_eq!(c.llm.chat.model.as_deref(), Some("big"));
        assert_eq!(
            c.llm.chat.concurrency, 16,
            "explicit inherit carries concurrency"
        );
        assert_eq!(c.llm.chat.api_key_env, "A_KEY");
        assert_eq!(c.llm.chat.timeout_s, 30.0);

        let env = env_of(&[
            ("PI_LLM_MODEL", "global"),
            ("PI_LLM_CHAT_MODEL", "chatty"),
            ("PI_LLM_KEY", "sk-secret"),
            ("PI_LLM_EXPAND_CONCURRENCY", "4"),
            ("PI_OCR_KEY", "ocr-secret"),
            ("PI_OCR_JSON_MODE", "false"),
        ]);
        let c = Config::from_toml_str(toml, &env).unwrap();
        assert_eq!(c.llm.summary.model.as_deref(), Some("global"));
        assert_eq!(c.llm.chat.model.as_deref(), Some("chatty"));
        assert_eq!(c.llm.summary.api_key_env, "PI_LLM_KEY");
        assert_eq!(c.llm.expand.concurrency, 4);
        assert_eq!(c.ocr.api_key_env, "PI_OCR_KEY");
        assert!(!c.ocr.json_mode);
        // Key values never land in the config.
        assert!(!format!("{c:?}").contains("secret"));
    }

    #[test]
    fn errors_name_the_field() {
        let bad = |toml: &str| Config::from_toml_str(toml, &env_of(&[])).unwrap_err().field;
        assert_eq!(
            bad("[llm.expand]\ninherit = \"nope\""),
            "llm.expand.inherit"
        );
        assert_eq!(
            bad("[llm.summary]\ninherit = \"chat\"\n[llm.chat]\ninherit = \"summary\""),
            "llm.chat.inherit"
        );
        assert_eq!(
            bad("[llm.summary]\nconcurrency = 0"),
            "llm.summary.concurrency"
        );
        assert_eq!(bad("[llm.chat]\ntimeout_s = -1.0"), "llm.chat.timeout_s");
        assert_eq!(
            bad("[llm.chat]\nbase_url = \"ftp://x\""),
            "llm.chat.base_url"
        );
        assert_eq!(bad("[index]\noptimize = \"fast\""), "index.optimize");
        assert_eq!(bad("[storage]\nmirror = \"/local\""), "storage.mirror");
        assert_eq!(bad("[storage]\nmirror = \"s3:///x\""), "storage.mirror");
        assert_eq!(bad("[ingest]\nsource = \"az://c/p\""), "ingest.source");
        assert_eq!(bad("[ocr]\ndpi = 5"), "ocr.dpi");
        assert_eq!(bad("[ocr]\nbogus = 1"), "bogus");
        let e = Config::from_toml_str("", &env_of(&[("PI_OCR_DPI", "high")])).unwrap_err();
        assert_eq!(e.field, "PI_OCR_DPI");
    }

    #[test]
    fn uris() {
        let c = Config::from_toml_str(
            "[storage]\nmirror = \"gs://bkt/some/prefix/\"\n[ingest]\nsource = \"s3://in\"",
            &env_of(&[]),
        )
        .unwrap();
        let m = c.storage.mirror.unwrap();
        assert_eq!(
            (m.scheme, m.bucket.as_str(), m.prefix.as_str()),
            (Scheme::Gs, "bkt", "some/prefix")
        );
        assert_eq!(m.to_string(), "gs://bkt/some/prefix");
        assert_eq!(
            c.ingest.source,
            Some(IngestSource::Remote(RemoteUri {
                scheme: Scheme::S3,
                bucket: "in".into(),
                prefix: String::new()
            }))
        );
        let c = Config::from_toml_str("[ingest]\nsource = \"pdfs\"", &env_of(&[])).unwrap();
        assert_eq!(
            c.ingest.source,
            Some(IngestSource::Local(PathBuf::from("pdfs")))
        );
    }
}
