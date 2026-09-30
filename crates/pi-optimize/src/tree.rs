//! The PageIndex tree as an arena of JSON-like nodes.
//!
//! The reference mutates nested dicts in place and identifies nodes by `id(node)`. Here every
//! node lives in an arena slot ([`Nid`]) that stays valid when the node is detached, so
//! concurrent passes (expand, the summary scheduler) can refer to nodes stably.
//!
//! Each node keeps its keys in insertion order, exactly as the Python dict would, including
//! the position of the `"nodes"` key: the map holds a `Null` placeholder under `"nodes"` while
//! the node has that key, and the children live in `children`.

use serde_json::{Map, Value};

pub type Nid = usize;

pub const NODES: &str = "nodes";

#[derive(Debug, Clone, Default)]
pub struct Node {
    /// All keys in dict order; `"nodes"` maps to a `Null` placeholder when present.
    pub fields: Map<String, Value>,
    /// `node["nodes"]`, meaningful only while `fields` has the `"nodes"` key.
    pub children: Vec<Nid>,
}

#[derive(Debug, Clone, Default)]
pub struct Tree {
    pub arena: Vec<Node>,
    /// The top-level `structure` list.
    pub roots: Vec<Nid>,
}

#[derive(Debug, thiserror::Error)]
pub enum TreeError {
    #[error("node is not a JSON object: {0}")]
    NotObject(String),
    #[error("node {0:?} has a non-integer {1}")]
    BadIndex(String, &'static str),
    #[error("structure is not a list")]
    NotList,
}

impl Tree {
    /// Build from a `structure` list.
    pub fn from_structure(structure: &[Value]) -> Result<Tree, TreeError> {
        let mut t = Tree::default();
        let mut roots = Vec::with_capacity(structure.len());
        for v in structure {
            roots.push(t.add_value(v)?);
        }
        t.roots = roots;
        Ok(t)
    }

    pub fn from_value(structure: &Value) -> Result<Tree, TreeError> {
        match structure {
            Value::Array(a) => Tree::from_structure(a),
            _ => Err(TreeError::NotList),
        }
    }

    fn add_value(&mut self, v: &Value) -> Result<Nid, TreeError> {
        let obj = v
            .as_object()
            .ok_or_else(|| TreeError::NotObject(v.to_string()))?;
        let mut fields = Map::new();
        let mut children = Vec::new();
        for (k, val) in obj {
            if k == NODES {
                fields.insert(k.clone(), Value::Null);
                if let Value::Array(items) = val {
                    for c in items {
                        children.push(self.add_value(c)?);
                    }
                }
            } else {
                fields.insert(k.clone(), val.clone());
            }
        }
        for key in ["start_index", "end_index"] {
            if fields.get(key).is_none_or(|v| v.as_i64().is_none()) {
                let title = fields
                    .get("title")
                    .map(|t| t.to_string())
                    .unwrap_or_default();
                return Err(TreeError::BadIndex(title, key));
            }
        }
        Ok(self.push(Node { fields, children }))
    }

    pub fn push(&mut self, node: Node) -> Nid {
        self.arena.push(node);
        self.arena.len() - 1
    }

    /// Back to a `structure` list.
    pub fn to_structure(&self) -> Vec<Value> {
        self.roots.iter().map(|&n| self.node_value(n)).collect()
    }

    pub fn to_value(&self) -> Value {
        Value::Array(self.to_structure())
    }

    pub fn node_value(&self, n: Nid) -> Value {
        let node = &self.arena[n];
        let mut out = Map::new();
        for (k, v) in &node.fields {
            if k == NODES {
                out.insert(
                    k.clone(),
                    Value::Array(node.children.iter().map(|&c| self.node_value(c)).collect()),
                );
            } else {
                out.insert(k.clone(), v.clone());
            }
        }
        Value::Object(out)
    }

    pub fn node(&self, n: Nid) -> &Node {
        &self.arena[n]
    }

    pub fn node_mut(&mut self, n: Nid) -> &mut Node {
        &mut self.arena[n]
    }

    pub fn get(&self, n: Nid, key: &str) -> Option<&Value> {
        self.arena[n].fields.get(key)
    }

    /// `node[key] = value` (keeps the key's position when it exists, appends otherwise).
    pub fn set(&mut self, n: Nid, key: &str, value: Value) {
        debug_assert!(key != NODES);
        self.arena[n].fields.insert(key.to_string(), value);
    }

    /// `node.pop(key, None)`.
    pub fn pop(&mut self, n: Nid, key: &str) -> Option<Value> {
        let v = self.arena[n].fields.shift_remove(key);
        if key == NODES {
            self.arena[n].children.clear();
        }
        v
    }

    /// `node.get("nodes") or []`.
    pub fn children(&self, n: Nid) -> &[Nid] {
        let node = &self.arena[n];
        if node.fields.contains_key(NODES) {
            &node.children
        } else {
            &[]
        }
    }

    /// `node["nodes"] = children`.
    pub fn set_children(&mut self, n: Nid, children: Vec<Nid>) {
        let node = &mut self.arena[n];
        node.fields.insert(NODES.to_string(), Value::Null);
        node.children = children;
    }

    pub fn start(&self, n: Nid) -> i64 {
        self.get(n, "start_index")
            .and_then(Value::as_i64)
            .unwrap_or(0)
    }

    pub fn end(&self, n: Nid) -> i64 {
        self.get(n, "end_index")
            .and_then(Value::as_i64)
            .unwrap_or(0)
    }

    pub fn title(&self, n: Nid) -> &str {
        self.get(n, "title").and_then(Value::as_str).unwrap_or("")
    }

    /// `node.get("node_id")` as a string (ids are always strings in the pipeline).
    pub fn node_id(&self, n: Nid) -> Option<String> {
        match self.get(n, "node_id") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(other) => Some(other.to_string()),
        }
    }

    /// `node.get("key_items") or []`, string entries only.
    pub fn key_items(&self, n: Nid) -> Vec<String> {
        match self.get(n, "key_items") {
            Some(Value::Array(items)) => items
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| v.to_string())
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// `is_frontier(node)`: no (or an empty) `nodes` list. ref: tree_optimize.py:169
    pub fn is_frontier(&self, n: Nid) -> bool {
        self.children(n).is_empty()
    }

    /// `flatten(nodes)`: pre-order walk. ref: tree_optimize.py:108
    pub fn flatten(&self, nodes: &[Nid]) -> Vec<Nid> {
        let mut out = Vec::new();
        self.flatten_into(nodes, &mut out);
        out
    }

    fn flatten_into(&self, nodes: &[Nid], out: &mut Vec<Nid>) {
        for &n in nodes {
            out.push(n);
            self.flatten_into(self.children(n), out);
        }
    }

    /// `flatten` with each node's parent.
    pub fn flatten_with_parent(
        &self,
        nodes: &[Nid],
        parent: Option<Nid>,
    ) -> Vec<(Nid, Option<Nid>)> {
        let mut out = Vec::new();
        let mut stack: Vec<(Nid, Option<Nid>)> = nodes.iter().rev().map(|&n| (n, parent)).collect();
        while let Some((n, p)) = stack.pop() {
            out.push((n, p));
            for &c in self.children(n).iter().rev() {
                stack.push((c, Some(n)));
            }
        }
        out
    }

    /// Every node reachable from the roots.
    pub fn live(&self) -> Vec<Nid> {
        self.flatten(&self.roots)
    }

    /// `subtree_end(node)`: last page covered by the node or any descendant.
    /// ref: tree_optimize.py:156
    pub fn subtree_end(&self, n: Nid) -> i64 {
        let mut end = self.end(n);
        for c in self.flatten(self.children(n)) {
            end = end.max(self.end(c));
        }
        end
    }

    /// `utils.strip_internal_keys` (utils.py:877): drop `_same_page` everywhere. Like the
    /// reference, it recurses only into non-empty `nodes` lists.
    pub fn strip_internal_keys(&mut self) {
        let roots = self.roots.clone();
        self.strip_from(&roots);
    }

    fn strip_from(&mut self, nodes: &[Nid]) {
        for &n in nodes {
            self.arena[n].fields.shift_remove("_same_page");
            let kids = self.children(n).to_vec();
            if !kids.is_empty() {
                self.strip_from(&kids);
            }
        }
    }
}

/// `utils.strip_internal_keys` on a JSON structure.
pub fn strip_internal_keys_value(structure: &mut Value) {
    match structure {
        Value::Array(items) => {
            for item in items {
                strip_node(item);
            }
        }
        other => strip_node(other),
    }
}

fn strip_node(node: &mut Value) {
    if let Value::Object(map) = node {
        map.shift_remove("_same_page");
        if let Some(Value::Array(kids)) = map.get_mut(NODES) {
            for k in kids {
                strip_node(k);
            }
        }
    }
}

/// `utils.write_node_id(data, node_id=0)` (utils.py:282): pre-order ids `str(n).zfill(4)`,
/// recursing into every key that contains `"nodes"`. Returns the next id.
pub fn write_node_id(data: &mut Value, mut node_id: i64) -> i64 {
    match data {
        Value::Object(map) => {
            map.insert("node_id".to_string(), Value::String(zfill4(node_id)));
            node_id += 1;
            let keys: Vec<String> = map.keys().filter(|k| k.contains(NODES)).cloned().collect();
            for k in keys {
                if let Some(v) = map.get_mut(&k) {
                    node_id = write_node_id(v, node_id);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                node_id = write_node_id(item, node_id);
            }
        }
        _ => {}
    }
    node_id
}

/// `str(n).zfill(4)`.
pub fn zfill4(n: i64) -> String {
    if n < 0 {
        format!("-{:03}", -n)
    } else {
        format!("{n:04}")
    }
}
