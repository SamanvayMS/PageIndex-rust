//! Structural JSON diff for parity stage dumps.
//!
//! Integers, strings, bools and nulls must match exactly; floats match within an absolute or
//! relative tolerance (default 1e-6, for geometry and statistics). Non-finite floats are encoded
//! as the strings "inf"/"-inf"/"nan" and so compare exactly. Object key order is ignored;
//! array order is significant.

use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct Divergence {
    /// JSON-pointer-like path, e.g. `data.pages[3].blocks[5].type`.
    pub path: String,
    pub a: String,
    pub b: String,
}

pub struct Differ {
    pub tol: f64,
    pub max: usize,
    pub found: Vec<Divergence>,
    pub total: usize,
}

impl Differ {
    pub fn new(tol: f64, max: usize) -> Self {
        Self {
            tol,
            max,
            found: Vec::new(),
            total: 0,
        }
    }

    fn report(&mut self, path: &str, a: &Value, b: &Value) {
        self.total += 1;
        if self.found.len() < self.max {
            let clip = |v: &Value| {
                let s = v.to_string();
                if s.chars().count() > 160 {
                    format!("{}…", s.chars().take(160).collect::<String>())
                } else {
                    s
                }
            };
            self.found.push(Divergence {
                path: path.to_string(),
                a: clip(a),
                b: clip(b),
            });
        }
    }

    fn floats_close(&self, x: f64, y: f64) -> bool {
        x == y || (x - y).abs() <= self.tol || (x - y).abs() <= self.tol * x.abs().max(y.abs())
    }

    pub fn diff(&mut self, path: &str, a: &Value, b: &Value) {
        match (a, b) {
            (Value::Number(x), Value::Number(y)) => {
                let exact_ints = (x.is_i64() || x.is_u64()) && (y.is_i64() || y.is_u64());
                let same = if exact_ints {
                    x == y
                } else {
                    match (x.as_f64(), y.as_f64()) {
                        (Some(p), Some(q)) => self.floats_close(p, q),
                        _ => false,
                    }
                };
                if !same {
                    self.report(path, a, b);
                }
            }
            (Value::Array(xs), Value::Array(ys)) => {
                if xs.len() != ys.len() {
                    self.report(&format!("{path}.len()"), &xs.len().into(), &ys.len().into());
                }
                for (i, (x, y)) in xs.iter().zip(ys).enumerate() {
                    self.diff(&format!("{path}[{i}]"), x, y);
                }
            }
            (Value::Object(xs), Value::Object(ys)) => {
                for (k, x) in xs {
                    match ys.get(k) {
                        Some(y) => self.diff(&format!("{path}.{k}"), x, y),
                        None => self.report(
                            &format!("{path}.{k}"),
                            x,
                            &Value::String("<missing>".into()),
                        ),
                    }
                }
                for (k, y) in ys {
                    if !xs.contains_key(k) {
                        self.report(
                            &format!("{path}.{k}"),
                            &Value::String("<missing>".into()),
                            y,
                        );
                    }
                }
            }
            _ => {
                if a != b {
                    self.report(path, a, b);
                }
            }
        }
    }
}

/// Pull `page`/`block` context out of a divergence path for the human summary.
pub fn locate(path: &str, root: &Value) -> Option<String> {
    // Walk the path and remember the last object carrying a "page" key.
    let mut cur = root;
    let mut page = None;
    let mut block = None;
    let mut last_key = "";
    for seg in path.split('.') {
        let (key, idxs) = match seg.find('[') {
            Some(p) => (&seg[..p], &seg[p..]),
            None => (seg, ""),
        };
        if !key.is_empty() {
            cur = cur.get(key)?;
            last_key = key;
        }
        for idx in idxs.split('[').filter(|s| !s.is_empty()) {
            let i: usize = idx.trim_end_matches(']').parse().ok()?;
            cur = cur.get(i)?;
            if last_key == "blocks" {
                block = Some(i);
            }
        }
        if let Some(p) = cur.get("page").and_then(Value::as_u64) {
            page = Some(p);
        }
    }
    match (page, block) {
        (Some(p), Some(b)) => Some(format!("page {p}, block {b}")),
        (Some(p), None) => Some(format!("page {p}")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn run(a: Value, b: Value) -> Differ {
        let mut d = Differ::new(1e-6, 10);
        d.diff("data", &a, &b);
        d
    }

    #[test]
    fn floats_within_tolerance_match() {
        assert_eq!(run(json!([1.0000001, "inf"]), json!([1.0, "inf"])).total, 0);
    }

    #[test]
    fn ints_and_text_are_exact() {
        let d = run(
            json!({"type": 7, "text": "Item 7."}),
            json!({"type": 8, "text": "Item 7"}),
        );
        assert_eq!(d.total, 2);
    }

    #[test]
    fn locates_page_and_block() {
        let root = json!({"data": {"pages": [{"page": 3, "blocks": [{"type": 0}, {"type": 7}]}]}});
        assert_eq!(
            locate("data.pages[0].blocks[1].type", &root).as_deref(),
            Some("page 3, block 1")
        );
    }

    #[test]
    fn length_mismatch_reported() {
        assert_eq!(run(json!([1, 2]), json!([1])).total, 1);
    }
}
