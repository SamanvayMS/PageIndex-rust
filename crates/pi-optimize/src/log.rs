//! The per-decision log of `optimize` (the reference keeps dicts; only the fields that feed the
//! run summary and diagnostics are kept here).

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    MergeSamePage {
        node_id: Option<String>,
        pages: (i64, i64),
        dropped: usize,
    },
    Merge {
        node_id: Option<String>,
        s: i64,
        tree_cost: i64,
        removed: usize,
    },
    Expand {
        node_id: Option<String>,
        decision: ExpandDecision,
    },
    Round {
        round: u32,
        same_page: bool,
        merged: bool,
        expanded: bool,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExpandDecision {
    /// A proposal attempt failed with an absorbed error.
    Error {
        attempt: u32,
        detail: String,
    },
    NoChildren {
        s: i64,
        attempts: u32,
    },
    Expand {
        s: i64,
        expand_cost: i64,
        children: usize,
    },
    KeepCollapsed {
        s: i64,
        expand_cost: i64,
    },
}
