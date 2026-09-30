//! The `optimize` driver: merge and expand rounds until nothing changes.
//! ref: pageindex/tree_optimize.py:782-864

use std::sync::Mutex;

use pi_llm::{Llm, LlmError};
use serde_json::{Map, Value};

use crate::consts::{
    EMPTY_RETRIES, EXPAND_CONCURRENCY, MAX_ROUNDS, MIN_GAIN_RATIO, ROUTING_COST, TRIGGER_PAGES,
};
use crate::cost::{complexity, s};
use crate::expand::{ExpandArgs, expand};
use crate::log::{Event, ExpandDecision};
use crate::merge::{Frozen, ListRef, merge, merge_same_page, relabel};
use crate::tree::{Nid, Tree};

/// `on_final(nodes)`: hears which nodes will not change any more. Called with the tree lock
/// released; the callee may lock the tree.
pub type OnFinal<'a> = dyn Fn(&[Nid]) + Send + Sync + 'a;

#[derive(Debug, thiserror::Error)]
pub enum OptimizeError {
    /// An unrecoverable model error (`_is_unrecoverable`): every remaining call would fail too.
    #[error(transparent)]
    Llm(#[from] LlmError),
    #[error("expand needs the PDF pages and a model; pass pages and an Llm, or do_expand=false")]
    ExpandNeedsPages,
}

/// Keyword arguments of `optimize` with the reference defaults.
#[derive(Debug, Clone)]
pub struct OptimizeOptions {
    pub routing: i64,
    pub trigger_pages: i64,
    pub min_gain_ratio: f64,
    pub do_merge: bool,
    pub do_expand: bool,
    pub max_rounds: u32,
    /// `page_count`: `None`/0 skips the before/after metrics.
    pub page_count: Option<i64>,
    pub empty_retries: u32,
    pub do_relabel: bool,
    /// `concurrency`; expand runs `min(EXPAND_CONCURRENCY, concurrency or EXPAND_CONCURRENCY)`.
    pub concurrency: Option<usize>,
}

impl Default for OptimizeOptions {
    fn default() -> Self {
        OptimizeOptions {
            routing: ROUTING_COST,
            trigger_pages: TRIGGER_PAGES,
            min_gain_ratio: MIN_GAIN_RATIO,
            do_merge: true,
            do_expand: true,
            max_rounds: MAX_ROUNDS,
            page_count: None,
            empty_retries: EMPTY_RETRIES,
            do_relabel: true,
            concurrency: None,
        }
    }
}

/// The run summary `optimize` returns (without `structure`, which stays in the tree).
#[derive(Debug, Clone, Default)]
pub struct Outcome {
    pub log: Vec<Event>,
    pub rounds: u32,
    pub before: Map<String, Value>,
    pub after: Map<String, Value>,
    pub id_map: Vec<(String, String)>,
    pub merges: usize,
    pub same_page_merges: usize,
    pub same_page_dropped: usize,
    pub expands: usize,
    pub kept_collapsed: usize,
}

impl Outcome {
    /// The `optimize` report of `flash/api.py::_optimize_async` (api.py:98), keys in order.
    pub fn report(&self) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("merges".into(), self.merges.into());
        m.insert("expands".into(), self.expands.into());
        m.insert("same_page_merges".into(), self.same_page_merges.into());
        m.insert("same_page_dropped".into(), self.same_page_dropped.into());
        m.insert("kept_collapsed".into(), self.kept_collapsed.into());
        m.insert("before".into(), Value::Object(self.before.clone()));
        m.insert("after".into(), Value::Object(self.after.clone()));
        m
    }
}

/// `final_nodes(nodes, trigger, frozen)`: everything but a collapsed node over the trigger that
/// expand has not judged yet. ref: tree_optimize.py:782
pub fn final_nodes(t: &Tree, nodes: &[Nid], trigger: i64, frozen: &Frozen) -> Vec<Nid> {
    t.flatten(nodes)
        .into_iter()
        .filter(|&n| !t.is_frontier(n) || s(t, n) <= trigger || frozen.contains(&t.node_id(n)))
        .collect()
}

/// `optimize(structure, pages, lines, ...)`: mutates the tree in place.
/// ref: tree_optimize.py:790
pub async fn optimize(
    tree: &Mutex<Tree>,
    pages: Option<&[String]>,
    lines: Option<&[Vec<String>]>,
    llm: Option<&dyn Llm>,
    opts: &OptimizeOptions,
    on_final: Option<&OnFinal<'_>>,
) -> Result<Outcome, OptimizeError> {
    if opts.do_expand && (pages.is_none() || llm.is_none()) {
        return Err(OptimizeError::ExpandNeedsPages);
    }
    let log = Mutex::new(Vec::<Event>::new());
    let frozen = Mutex::new(Frozen::new());
    let page_count = opts.page_count.filter(|&p| p != 0);
    let settled_all = || {
        if let Some(cb) = on_final {
            let nodes = {
                let t = tree.lock().unwrap();
                let frozen = frozen.lock().unwrap();
                final_nodes(&t, &t.roots, opts.trigger_pages, &frozen)
            };
            cb(&nodes);
        }
    };
    let before = match page_count {
        Some(pc) => {
            let t = tree.lock().unwrap();
            complexity(&t, &t.roots, pc, opts.routing)
        }
        None => Map::new(),
    };
    let concurrency = EXPAND_CONCURRENCY.min(
        opts.concurrency
            .filter(|&c| c != 0)
            .unwrap_or(EXPAND_CONCURRENCY),
    );
    let no_lines: Vec<Vec<String>> = Vec::new();

    let mut rounds = 0;
    for round_no in 1..=opts.max_rounds {
        rounds = round_no;
        let round_start = log.lock().unwrap().len();
        let merged = {
            let mut t = tree.lock().unwrap();
            let mut log = log.lock().unwrap();
            let mut frozen = frozen.lock().unwrap();
            if opts.do_merge {
                merge_same_page(&mut t, ListRef::Roots, &mut log);
            }
            let merged = opts.do_merge && merge(&mut t, opts.routing, &mut log, &mut frozen);
            if merged {
                // a collapsed subtree can land on a sibling's exact pages
                merge_same_page(&mut t, ListRef::Roots, &mut log);
            }
            merged
        };
        settled_all();
        let expanded = if opts.do_expand {
            let args = ExpandArgs {
                llm: llm.expect("checked above"),
                routing: opts.routing,
                trigger_pages: opts.trigger_pages,
                min_gain_ratio: opts.min_gain_ratio,
                empty_retries: opts.empty_retries,
                do_merge: opts.do_merge,
                concurrency,
                on_final,
            };
            expand(
                tree,
                pages.expect("checked above"),
                lines.unwrap_or(&no_lines),
                &args,
                &log,
                &frozen,
            )
            .await?
        } else {
            false
        };
        let mut log = log.lock().unwrap();
        let same_page = log[round_start..]
            .iter()
            .any(|e| matches!(e, Event::MergeSamePage { .. }));
        log.push(Event::Round {
            round: round_no,
            same_page,
            merged,
            expanded,
        });
        if !(same_page || merged || expanded) {
            break;
        }
    }
    if let Some(cb) = on_final {
        let all = tree.lock().unwrap().live();
        cb(&all);
    }

    let mut t = tree.lock().unwrap();
    let id_map = if opts.do_relabel {
        relabel(&mut t)
    } else {
        Vec::new()
    };
    let after = match page_count {
        Some(pc) => complexity(&t, &t.roots, pc, opts.routing),
        None => Map::new(),
    };
    let log = log.into_inner().unwrap();
    let count = |f: &dyn Fn(&Event) -> bool| log.iter().filter(|e| f(e)).count();
    let merges = count(&|e| matches!(e, Event::Merge { .. }));
    let same_page_merges = count(&|e| matches!(e, Event::MergeSamePage { .. }));
    let same_page_dropped = log
        .iter()
        .map(|e| match e {
            Event::MergeSamePage { dropped, .. } => *dropped,
            _ => 0,
        })
        .sum();
    let expands = count(&|e| {
        matches!(
            e,
            Event::Expand {
                decision: ExpandDecision::Expand { .. },
                ..
            }
        )
    });
    let kept_collapsed = count(&|e| {
        matches!(
            e,
            Event::Expand {
                decision: ExpandDecision::KeepCollapsed { .. },
                ..
            }
        )
    });
    Ok(Outcome {
        log,
        rounds,
        before,
        after,
        id_map,
        merges,
        same_page_merges,
        same_page_dropped,
        expands,
        kept_collapsed,
    })
}

/// Merge-only `optimize` (no model), run to completion synchronously.
pub fn optimize_merge_only(tree: &mut Tree, page_count: Option<i64>) -> Outcome {
    let opts = OptimizeOptions {
        do_expand: false,
        page_count,
        ..Default::default()
    };
    let shared = Mutex::new(std::mem::take(tree));
    let out = futures::executor::block_on(optimize(&shared, None, None, None, &opts, None))
        .expect("merge-only optimize cannot fail");
    *tree = shared.into_inner().unwrap();
    out
}
