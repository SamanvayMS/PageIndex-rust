//! Structural invariants of merge + relabel over random well-formed trees (union semantics:
//! a node's end_index covers its whole subtree, children lie inside their parent).

use std::collections::{HashMap, HashSet};

use pi_optimize::merge::{Frozen, merge, relabel};
use pi_optimize::tree::{Nid, Tree};
use pi_optimize::{consts::ROUTING_COST, optimize_merge_only};
use proptest::prelude::*;
use serde_json::{Map, Value, json};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next() % n }
    }
}

/// A node spanning [start, end] with random children inside it.
fn gen_node(rng: &mut Rng, start: i64, end: i64, depth: u32, counter: &mut usize) -> Value {
    let id = *counter;
    *counter += 1;
    let mut m = Map::new();
    m.insert("title".into(), json!(format!("T{id}")));
    m.insert("node_id".into(), json!(format!("orig{id}")));
    m.insert("start_index".into(), json!(start));
    m.insert("end_index".into(), json!(end));
    let span = (end - start + 1) as u64;
    if depth < 4 && rng.below(3) != 0 {
        let k = 1 + rng.below(4);
        let mut starts: Vec<i64> = (0..k).map(|_| start + rng.below(span) as i64).collect();
        starts.sort();
        let kids: Vec<Value> = starts
            .iter()
            .enumerate()
            .map(|(i, &s)| {
                // end at the next sibling's start (or one before), or anywhere up to the parent's end
                let e = match starts.get(i + 1) {
                    Some(&nx) if rng.below(2) == 0 => (nx - 1).max(s),
                    Some(&nx) => nx,
                    None => s + rng.below((end - s + 1) as u64) as i64,
                };
                gen_node(rng, s, e.min(end), depth + 1, counter)
            })
            .collect();
        m.insert("nodes".into(), Value::Array(kids));
    }
    Value::Object(m)
}

fn gen_structure(seed: u64) -> Vec<Value> {
    let mut rng = Rng(seed | 1);
    let pages = 1 + rng.below(60) as i64;
    let mut counter = 0;
    let n = 1 + rng.below(5);
    let mut starts: Vec<i64> = (0..n).map(|_| 1 + rng.below(pages as u64) as i64).collect();
    starts.sort();
    starts
        .iter()
        .enumerate()
        .map(|(i, &s)| {
            let e = starts.get(i + 1).map_or(pages, |&nx| nx.max(s));
            gen_node(&mut rng, s, e, 1, &mut counter)
        })
        .collect()
}

fn check_shape(t: &Tree) -> Result<(), TestCaseError> {
    let v = t.to_value();
    fn walk(v: &Value, parent: Option<(i64, i64)>) -> Result<i64, TestCaseError> {
        let (s, e) = (
            v["start_index"].as_i64().unwrap(),
            v["end_index"].as_i64().unwrap(),
        );
        prop_assert!(s <= e, "start {} > end {}", s, e);
        if let Some((ps, pe)) = parent {
            prop_assert!(
                ps <= s && e <= pe,
                "child [{},{}] outside parent [{},{}]",
                s,
                e,
                ps,
                pe
            );
        }
        let mut max_end = e;
        if let Some(kids) = v.get("nodes") {
            let kids = kids.as_array().unwrap();
            prop_assert!(!kids.is_empty(), "empty nodes array");
            for k in kids {
                max_end = max_end.max(walk(k, Some((s, e)))?);
            }
        }
        prop_assert_eq!(e, max_end, "end_index is not the subtree max");
        Ok(max_end)
    }
    for r in v.as_array().unwrap() {
        walk(r, None)?;
    }
    // ids contiguous, zero-padded, pre-order
    for (i, n) in t.live().into_iter().enumerate() {
        prop_assert_eq!(t.node_id(n).unwrap(), format!("{i:04}"));
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn merge_keeps_tree_well_formed(seed in any::<u64>()) {
        let structure = gen_structure(seed);
        let mut t = Tree::from_structure(&structure).unwrap();

        // original parent links and titles, by original id
        let mut parent_of: HashMap<Nid, Option<Nid>> = HashMap::new();
        for (n, p) in t.flatten_with_parent(&t.roots.clone(), None) {
            parent_of.insert(n, p);
        }
        let original: Vec<Nid> = t.live();

        merge(&mut t, ROUTING_COST, &mut Vec::new(), &mut Frozen::new());
        let live: HashSet<Nid> = t.live().into_iter().collect();
        for &n in &original {
            if live.contains(&n) {
                continue;
            }
            let title = t.title(n).to_string();
            let mut a = parent_of[&n];
            let mut found = false;
            while let Some(p) = a {
                if live.contains(&p) && t.key_items(p).contains(&title) {
                    found = true;
                    break;
                }
                a = parent_of[&p];
            }
            prop_assert!(found, "removed title {} not kept by any ancestor", title);
        }
        relabel(&mut t);
        check_shape(&t)?;
    }

    #[test]
    fn merge_only_optimize_keeps_tree_well_formed(seed in any::<u64>()) {
        let structure = gen_structure(seed);
        let mut t = Tree::from_structure(&structure).unwrap();
        let before = t.live().len();
        let out = optimize_merge_only(&mut t, Some(60));
        check_shape(&t)?;
        prop_assert!(t.live().len() <= before);
        prop_assert_eq!(out.expands, 0);
        // merged nodes are frontier nodes carrying key_items
        prop_assert!(out.merges == 0 || t.live().iter().any(|&n| !t.key_items(n).is_empty()));
    }
}
