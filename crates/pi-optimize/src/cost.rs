//! Cost model and search-complexity metrics. ref: pageindex/tree_optimize.py:232-395

use pi_pycompat::pyround::round_nd;
use serde_json::{Map, Value};

use crate::tree::{Nid, Tree};

/// Pages in `[lo, hi]` (empty when `hi < lo`) covered by none of `ranges` (inclusive).
fn uncovered(lo: i64, hi: i64, ranges: &mut [(i64, i64)]) -> i64 {
    if hi < lo {
        return 0;
    }
    ranges.sort_unstable();
    let mut covered = 0i64;
    let mut cur: Option<(i64, i64)> = None;
    for &(a, b) in ranges.iter() {
        let (a, b) = (a.max(lo), b.min(hi));
        if b < a {
            continue;
        }
        match cur {
            Some((ca, cb)) if a <= cb + 1 => cur = Some((ca, cb.max(b))),
            Some((ca, cb)) => {
                covered += cb - ca + 1;
                cur = Some((a, b));
            }
            None => cur = Some((a, b)),
        }
    }
    if let Some((ca, cb)) = cur {
        covered += cb - ca + 1;
    }
    (hi - lo + 1) - covered
}

/// `S(node)`: pages to scan linearly if the node were collapsed. ref: tree_optimize.py:236
pub fn s(t: &Tree, n: Nid) -> i64 {
    t.subtree_end(n) - t.start(n) + 1
}

/// `S_residual(node)`: pages of the node covered by no child. ref: tree_optimize.py:241
pub fn s_residual(t: &Tree, n: Nid) -> i64 {
    let children = t.children(n);
    if children.is_empty() {
        return s(t, n);
    }
    let mut ranges: Vec<(i64, i64)> = children
        .iter()
        .map(|&c| (t.start(c), t.subtree_end(c)))
        .collect();
    uncovered(t.start(n), t.subtree_end(n), &mut ranges)
}

/// `tree_cost(node)`: worst-case search cost of the subtree as it stands.
/// ref: tree_optimize.py:252
pub fn tree_cost(t: &Tree, n: Nid, routing: i64) -> i64 {
    if t.is_frontier(n) {
        return s(t, n);
    }
    let mut best = t
        .children(n)
        .iter()
        .map(|&c| tree_cost(t, c, routing))
        .max()
        .unwrap();
    let residual = s_residual(t, n);
    if residual != 0 {
        best = best.max(residual);
    }
    routing + best
}

/// `frontier_costs(node)`: every branch as (routing distance, scan pages, label).
/// ref: tree_optimize.py:263
pub fn frontier_costs(t: &Tree, n: Nid, distance: i64) -> Vec<(i64, i64, String)> {
    if t.is_frontier(n) {
        return vec![(
            distance,
            s(t, n),
            t.node_id(n).unwrap_or_else(|| "None".into()),
        )];
    }
    let mut entries = Vec::new();
    let residual = s_residual(t, n);
    if residual != 0 {
        let id = t.node_id(n).unwrap_or_else(|| "None".into());
        entries.push((distance + 1, residual, format!("{id}:residual")));
    }
    for &c in t.children(n) {
        entries.extend(frontier_costs(t, c, distance + 1));
    }
    entries
}

/// `tree_cost_via_frontier(node)`. ref: tree_optimize.py:279
pub fn tree_cost_via_frontier(t: &Tree, n: Nid, routing: i64) -> i64 {
    frontier_costs(t, n, 0)
        .iter()
        .map(|(d, s, _)| d * routing + s)
        .max()
        .unwrap_or(0)
}

/// A candidate child as expand proposes it (not yet in the tree).
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub title: String,
    pub start_index: i64,
    pub end_index: i64,
    pub node_id: String,
}

/// `expand_cost(node, children)`: cost after a one-step lookahead, children collapsed.
/// Returns `(cost, residual)`. ref: tree_optimize.py:284
pub fn expand_cost(t: &Tree, n: Nid, children: &[Candidate], routing: i64) -> (i64, i64) {
    let mut ranges: Vec<(i64, i64)> = children
        .iter()
        .map(|c| (c.start_index, c.end_index))
        .collect();
    let residual = uncovered(t.start(n), t.subtree_end(n), &mut ranges);
    let scans = children.iter().map(|c| c.end_index - c.start_index + 1);
    (routing + scans.fold(residual, i64::max), residual)
}

/// `frontier_nodes(structure, root_depth)`. ref: tree_optimize.py:298
pub fn frontier_nodes(t: &Tree, structure: &[Nid], root_depth: i64) -> Vec<(Nid, i64)> {
    fn visit(t: &Tree, n: Nid, depth: i64, found: &mut Vec<(Nid, i64)>) {
        if t.is_frontier(n) {
            found.push((n, depth));
            return;
        }
        for &c in t.children(n) {
            visit(t, c, depth + 1, found);
        }
        if s_residual(t, n) != 0 {
            found.push((n, depth + 1));
        }
    }
    let mut found = Vec::new();
    for &r in structure {
        visit(t, r, root_depth, &mut found);
    }
    found
}

/// `pages(node)`: pages to scan once search arrives at a frontier entry.
/// ref: tree_optimize.py:323
fn pages(t: &Tree, n: Nid) -> i64 {
    if t.is_frontier(n) {
        s(t, n)
    } else {
        s_residual(t, n)
    }
}

/// `worst_case_search_complexity`. ref: tree_optimize.py:328
pub fn worst_case_search_complexity(
    t: &Tree,
    structure: &[Nid],
    root_depth: i64,
    routing: i64,
) -> i64 {
    frontier_nodes(t, structure, root_depth)
        .iter()
        .map(|&(n, d)| d * routing + pages(t, n))
        .max()
        .unwrap_or(0)
}

/// `average_search_complexity`: returns `(total, weight)`. ref: tree_optimize.py:336
pub fn average_search_complexity(
    t: &Tree,
    structure: &[Nid],
    total_pages: i64,
    root_depth: i64,
    routing: i64,
) -> (f64, f64) {
    if total_pages == 0 {
        return (0.0, 0.0);
    }
    let mut total = 0.0f64;
    let mut weight = 0.0f64;
    for (n, depth) in frontier_nodes(t, structure, root_depth) {
        let np = pages(t, n);
        let p = np as f64 / total_pages as f64;
        weight += p;
        total += p * ((depth * routing) as f64 + (np + 1) as f64 / 2.0);
    }
    (total, weight)
}

/// `complexity(structure, total_pages)`: the metrics dict, keys in reference order.
/// ref: tree_optimize.py:376
pub fn complexity(
    t: &Tree,
    structure: &[Nid],
    total_pages: i64,
    routing: i64,
) -> Map<String, Value> {
    let root_depth = 1;
    let entries = frontier_nodes(t, structure, root_depth);
    let worst = worst_case_search_complexity(t, structure, root_depth, routing);
    let (average, weight) =
        average_search_complexity(t, structure, total_pages, root_depth, routing);
    let depths: Vec<i64> = entries.iter().map(|&(_, d)| d).collect();
    let normalized = if total_pages == 0 {
        0.0
    } else {
        worst as f64 / total_pages as f64
    };
    let mut m = Map::new();
    m.insert("total_pages".into(), total_pages.into());
    m.insert("frontier_nodes".into(), entries.len().into());
    m.insert("worst_case_search_complexity".into(), worst.into());
    m.insert(
        "average_search_complexity".into(),
        float(round_nd(average, 3)),
    );
    m.insert(
        "normalized_worst_case_complexity".into(),
        float(round_nd(normalized, 4)),
    );
    m.insert(
        "max_depth".into(),
        depths.iter().copied().max().unwrap_or(0).into(),
    );
    let mean_depth = if depths.is_empty() {
        Value::from(0)
    } else {
        let sum: i64 = depths.iter().sum();
        float(round_nd(sum as f64 / depths.len() as f64, 2))
    };
    m.insert("mean_depth".into(), mean_depth);
    m.insert("probability_mass".into(), float(round_nd(weight, 4)));
    m
}

fn float(x: f64) -> Value {
    serde_json::Number::from_f64(x)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uncovered_counts_gaps() {
        assert_eq!(uncovered(1, 10, &mut [(2, 3), (3, 5), (8, 20)]), 3);
        assert_eq!(uncovered(1, 10, &mut []), 10);
        assert_eq!(uncovered(5, 4, &mut [(1, 9)]), 0);
        assert_eq!(uncovered(1, 3, &mut [(5, 9), (0, 0)]), 3);
        assert_eq!(uncovered(1, 10, &mut [(1, 4), (5, 10)]), 0);
    }
}
