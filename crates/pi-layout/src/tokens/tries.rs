//! Token trie construction, matching, and token trimming utilities.
//!
//! ref: pageindex/flash/tokens/tries.py
//!
//! An exact port rather than the `aho-corasick` crate: the reference walks tokens (not bytes)
//! with keys normalized per lookup, and its "shortest earliest" match rule reads the
//! dictionary-suffix depth while keeping the current node, which a generic automaton does not
//! reproduce.

use std::cell::RefCell;
use std::collections::HashMap;

use super::token_types::{TokenView, can_extend_token, is_trimmable_token};
use crate::model::block::strip_diacritics;
use crate::model::char_stats::char_category;

struct Node {
    key: String,
    depth: i64,
    children: HashMap<String, usize>,
    fail: Option<usize>,
    dict_suffix: Option<usize>,
    terminal: bool,
}

/// ref: tokens/tries.py::BuiltTrie (+ TrieConfig)
pub struct Trie {
    nodes: Vec<Node>,
    reverse: bool,
    case_fold: bool,
}

thread_local! {
    static NORM_CACHE: RefCell<[HashMap<String, String>; 2]> = RefCell::new([HashMap::new(), HashMap::new()]);
}

/// ref: tokens/tries.py::_de_norm
pub fn de_norm(text: &str, case_fold: bool) -> String {
    if text.is_ascii() {
        return if case_fold {
            text.to_ascii_lowercase()
        } else {
            text.to_string()
        };
    }
    NORM_CACHE.with(|c| {
        let mut c = c.borrow_mut();
        let m = &mut c[case_fold as usize];
        if let Some(v) = m.get(text) {
            return v.clone();
        }
        let v = if case_fold {
            strip_diacritics(&pi_pycompat::unicode::lower(text))
        } else {
            strip_diacritics(text)
        };
        m.insert(text.to_string(), v.clone());
        v
    })
}

/// Split a phrase into token strings the way the tokenizer would.
// ref: tokens/tries.py::_trie_insert_entry (tokenization part)
fn phrase_tokens(entry: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut prev = 0u8;
    for ch in entry.chars() {
        let cat = char_category(ch);
        // A token ends at whitespace or where the category cannot extend it.
        if !cur.is_empty() && (cat == 10 || !can_extend_token(prev, cat, ch)) {
            out.push(std::mem::take(&mut cur));
        }
        if cat != 10 {
            cur.push(ch);
        }
        prev = cat;
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

impl Trie {
    /// ref: tokens/tries.py::build_trie
    pub fn build<S: AsRef<str>>(
        strings: impl IntoIterator<Item = S>,
        case_fold: bool,
        reverse: bool,
    ) -> Trie {
        let mut t = Trie {
            nodes: vec![Node {
                key: String::new(),
                depth: 0,
                children: HashMap::new(),
                fail: None,
                dict_suffix: None,
                terminal: false,
            }],
            reverse,
            case_fold,
        };
        for s in strings {
            let mut toks = phrase_tokens(s.as_ref());
            if reverse {
                toks.reverse();
            }
            let mut node = 0;
            for tok in toks {
                // ref: tokens/tries.py::trie_insert_step
                let key = de_norm(&tok, case_fold);
                node = match t.nodes[node].children.get(&key) {
                    Some(&c) => c,
                    None => {
                        let depth = t.nodes[node].depth + 1;
                        t.nodes.push(Node {
                            key: key.clone(),
                            depth,
                            children: HashMap::new(),
                            fail: None,
                            dict_suffix: None,
                            terminal: false,
                        });
                        let id = t.nodes.len() - 1;
                        t.nodes[node].children.insert(key, id);
                        id
                    }
                };
            }
            t.nodes[node].terminal = true;
        }
        t.finalize();
        t
    }

    // ref: tokens/tries.py::_trie_finalize
    fn finalize(&mut self) {
        let mut queue = std::collections::VecDeque::from([0usize]);
        while let Some(n) = queue.pop_front() {
            let children: Vec<usize> = self.nodes[n].children.values().copied().collect();
            for child in children {
                queue.push_back(child);
                let mut tr = n;
                let mut fail = None;
                while let Some(f) = self.nodes[tr].fail {
                    let k = de_norm(&self.nodes[child].key, self.case_fold);
                    fail = self.nodes[f].children.get(&k).copied();
                    if fail.is_some() {
                        break;
                    }
                    tr = f;
                }
                let fail = fail.unwrap_or(0);
                self.nodes[child].fail = Some(fail);
                let mut tr = Some(fail);
                while let Some(x) = tr {
                    if self.nodes[x].terminal {
                        self.nodes[child].dict_suffix = Some(x);
                        break;
                    }
                    tr = self.nodes[x].fail;
                }
            }
        }
    }

    // ref: tokens/tries.py::trie_walk_step
    fn walk(&self, mut node: usize, key: &str) -> usize {
        loop {
            if let Some(&c) = self.nodes[node].children.get(key) {
                return c;
            }
            match self.nodes[node].fail {
                Some(f) => node = f,
                None => return node,
            }
        }
    }

    /// Shortest earliest match anywhere in the tokens.
    #[allow(clippy::int_plus_one)] // `index - depth + 1 <= earliest` as in the reference
    // ref: tokens/tries.py::aho_corasick_match / aho_corasick_tokens
    pub fn search(&self, tokens: &TokenView) -> Option<TokenView> {
        let tokens = if self.reverse {
            tokens.reverse()
        } else {
            tokens.clone()
        };
        let mut matched = None;
        let mut earliest: i64 = -1;
        let mut node = 0;
        for (index, tok) in tokens.iter().enumerate() {
            let index = index as i64;
            node = self.walk(node, &de_norm(&tok.text, self.case_fold));
            let depth = if self.nodes[node].terminal {
                self.nodes[node].depth
            } else {
                0
            };
            if depth > 0 && (earliest < 0 || index - depth + 1 <= earliest) {
                earliest = index - depth + 1;
                let m = tokens.slice(earliest, index + 1);
                matched = Some(if self.reverse { m.reverse() } else { m });
            }
            let kb_depth = self.nodes[node]
                .dict_suffix
                .map_or(0, |k| self.nodes[k].depth);
            if kb_depth > 0 && (earliest < 0 || index - kb_depth + 1 <= earliest) {
                earliest = index - kb_depth + 1;
                let m = tokens.slice(earliest, index + 1);
                matched = Some(if self.reverse { m.reverse() } else { m });
            }
            if earliest >= 0 && index - self.nodes[node].depth + 1 > earliest {
                break;
            }
        }
        matched
    }

    /// Longest prefix match.
    // ref: tokens/tries.py::trie_prefix_match
    pub fn prefix_match(&self, tokens: &TokenView) -> Option<TokenView> {
        let tokens = if self.reverse {
            tokens.reverse()
        } else {
            tokens.clone()
        };
        let mut matched = None;
        let mut node = 0;
        for (index, tok) in tokens.iter().enumerate() {
            if self.nodes[node].children.is_empty() {
                break;
            }
            let Some(&next) = self.nodes[node]
                .children
                .get(&de_norm(&tok.text, self.case_fold))
            else {
                break;
            };
            node = next;
            if self.nodes[node].terminal {
                let sv = tokens.slice(0, index as i64 + 1);
                matched = Some(if self.reverse { sv.reverse() } else { sv });
            }
        }
        matched
    }

    // ref: tokens/tries.py::_trie_full_match
    pub fn full_match(&self, tokens: &TokenView) -> bool {
        self.prefix_match(tokens)
            .is_some_and(|m| m.length == tokens.length)
    }

    pub fn is_reverse(&self) -> bool {
        self.reverse
    }
}

// ref: tokens/tries.py::strip_trie_match
pub fn strip_trie_match(tokens: &TokenView, trie: &Trie) -> TokenView {
    match trie.prefix_match(tokens) {
        None => tokens.clone(),
        Some(m) if trie.is_reverse() => tokens.slice(0, tokens.length - m.length),
        Some(m) => tokens.from(m.length),
    }
}

// ref: tokens/tries.py::strip_leading_if_in
pub fn strip_leading_if_in(tokens: &TokenView, set: &[&str]) -> TokenView {
    match tokens.first() {
        Some(f) if set.contains(&f.text.as_str()) => tokens.from(1),
        _ => tokens.clone(),
    }
}

/// ref: tokens/tries.py:312 `COMMA_CHARS`
pub const COMMA_CHARS: [&str; 6] = [
    ",", "\u{FE50}", "\u{FF0C}", "\u{3001}", "\u{FE51}", "\u{FF64}",
];

// ref: tokens/tries.py::is_comma_token
pub fn is_comma_token(t: Option<&super::Token>) -> bool {
    t.is_some_and(|t| COMMA_CHARS.contains(&t.text.as_str()))
}

// ref: tokens/tries.py::trim_trailing_punct
pub fn trim_trailing_punct(tokens: &TokenView) -> TokenView {
    let mut end = tokens.length;
    while end > 0 {
        match tokens.token_at(end - 1) {
            Some(t) if is_trimmable_token(t) => end -= 1,
            _ => break,
        }
    }
    tokens.slice(0, end)
}
