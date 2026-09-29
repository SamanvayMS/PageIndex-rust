//! Exact port of CPython 3.11 `difflib.SequenceMatcher` (Ratcliff–Obershelp with the
//! "popular element" autojunk heuristic) and `get_close_matches`.
//!
//! Python strings are sequences of code points, so string callers pass `&[char]`
//! (see [`ratio_str`]). Reference users: `embedded_toc.py:283` (`ratio`, autojunk on),
//! `unicode_apply.py:151` (`get_opcodes`, autojunk off), `agent_tools.py:410` (`get_close_matches`).

use std::collections::HashMap;
use std::hash::Hash;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Match {
    pub a: usize,
    pub b: usize,
    pub size: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tag {
    Replace,
    Delete,
    Insert,
    Equal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Opcode {
    pub tag: Tag,
    pub i1: usize,
    pub i2: usize,
    pub j1: usize,
    pub j2: usize,
}

pub struct SequenceMatcher<'s, T: Eq + Hash> {
    a: &'s [T],
    b: &'s [T],
    autojunk: bool,
    b2j: HashMap<&'s T, Vec<usize>>,
    matching_blocks: Option<Vec<Match>>,
}

/// `_calculate_ratio`: `2*M/T`, or 1.0 when both sequences are empty.
fn calculate_ratio(matches: usize, length: usize) -> f64 {
    if length > 0 {
        2.0 * matches as f64 / length as f64
    } else {
        1.0
    }
}

impl<'s, T: Eq + Hash> SequenceMatcher<'s, T> {
    /// `SequenceMatcher(None, a, b, autojunk)`. The reference never passes an `isjunk` callable,
    /// so `bjunk` is always empty and its extension loops in `find_longest_match` are no-ops.
    pub fn new(a: &'s [T], b: &'s [T], autojunk: bool) -> Self {
        let mut m = Self {
            a,
            b: &[],
            autojunk,
            b2j: HashMap::new(),
            matching_blocks: None,
        };
        m.set_seq2(b);
        m
    }

    pub fn set_seq1(&mut self, a: &'s [T]) {
        self.a = a;
        self.matching_blocks = None;
    }

    pub fn set_seq2(&mut self, b: &'s [T]) {
        self.b = b;
        self.matching_blocks = None;
        // __chain_b
        let mut b2j: HashMap<&'s T, Vec<usize>> = HashMap::new();
        for (i, elt) in b.iter().enumerate() {
            b2j.entry(elt).or_default().push(i);
        }
        let n = b.len();
        if self.autojunk && n >= 200 {
            let ntest = n / 100 + 1;
            b2j.retain(|_, idxs| idxs.len() <= ntest);
        }
        self.b2j = b2j;
    }

    pub fn find_longest_match(&self, alo: usize, ahi: usize, blo: usize, bhi: usize) -> Match {
        let (a, b) = (self.a, self.b);
        let (mut besti, mut bestj, mut bestsize) = (alo, blo, 0usize);
        let mut j2len: HashMap<usize, usize> = HashMap::new();
        for (i, ai) in a.iter().enumerate().take(ahi).skip(alo) {
            let mut newj2len: HashMap<usize, usize> = HashMap::new();
            if let Some(js) = self.b2j.get(ai) {
                for &j in js {
                    if j < blo {
                        continue;
                    }
                    if j >= bhi {
                        break;
                    }
                    let k = j
                        .checked_sub(1)
                        .and_then(|p| j2len.get(&p))
                        .copied()
                        .unwrap_or(0)
                        + 1;
                    newj2len.insert(j, k);
                    if k > bestsize {
                        besti = i + 1 - k;
                        bestj = j + 1 - k;
                        bestsize = k;
                    }
                }
            }
            j2len = newj2len;
        }
        // Extend with non-junk equal elements on both sides. Popular (autojunked) elements are
        // not in `bjunk`, so they count as non-junk here — exactly as in CPython.
        while besti > alo && bestj > blo && a[besti - 1] == b[bestj - 1] {
            besti -= 1;
            bestj -= 1;
            bestsize += 1;
        }
        while besti + bestsize < ahi
            && bestj + bestsize < bhi
            && a[besti + bestsize] == b[bestj + bestsize]
        {
            bestsize += 1;
        }
        Match {
            a: besti,
            b: bestj,
            size: bestsize,
        }
    }

    pub fn get_matching_blocks(&mut self) -> &[Match] {
        if self.matching_blocks.is_none() {
            let (la, lb) = (self.a.len(), self.b.len());
            let mut queue = vec![(0, la, 0, lb)];
            let mut blocks: Vec<Match> = Vec::new();
            while let Some((alo, ahi, blo, bhi)) = queue.pop() {
                let x = self.find_longest_match(alo, ahi, blo, bhi);
                let (i, j, k) = (x.a, x.b, x.size);
                if k > 0 {
                    blocks.push(x);
                    if alo < i && blo < j {
                        queue.push((alo, i, blo, j));
                    }
                    if i + k < ahi && j + k < bhi {
                        queue.push((i + k, ahi, j + k, bhi));
                    }
                }
            }
            blocks.sort();
            let (mut i1, mut j1, mut k1) = (0, 0, 0);
            let mut non_adjacent = Vec::new();
            for m in blocks {
                if i1 + k1 == m.a && j1 + k1 == m.b {
                    k1 += m.size;
                } else {
                    if k1 > 0 {
                        non_adjacent.push(Match {
                            a: i1,
                            b: j1,
                            size: k1,
                        });
                    }
                    (i1, j1, k1) = (m.a, m.b, m.size);
                }
            }
            if k1 > 0 {
                non_adjacent.push(Match {
                    a: i1,
                    b: j1,
                    size: k1,
                });
            }
            non_adjacent.push(Match {
                a: la,
                b: lb,
                size: 0,
            });
            self.matching_blocks = Some(non_adjacent);
        }
        self.matching_blocks.as_deref().unwrap_or_default()
    }

    pub fn get_opcodes(&mut self) -> Vec<Opcode> {
        let (mut i, mut j) = (0, 0);
        let mut answer = Vec::new();
        for &Match { a: ai, b: bj, size } in self.get_matching_blocks() {
            let tag = if i < ai && j < bj {
                Some(Tag::Replace)
            } else if i < ai {
                Some(Tag::Delete)
            } else if j < bj {
                Some(Tag::Insert)
            } else {
                None
            };
            if let Some(tag) = tag {
                answer.push(Opcode {
                    tag,
                    i1: i,
                    i2: ai,
                    j1: j,
                    j2: bj,
                });
            }
            (i, j) = (ai + size, bj + size);
            if size > 0 {
                answer.push(Opcode {
                    tag: Tag::Equal,
                    i1: ai,
                    i2: i,
                    j1: bj,
                    j2: j,
                });
            }
        }
        answer
    }

    pub fn ratio(&mut self) -> f64 {
        let len = self.a.len() + self.b.len();
        let matches = self.get_matching_blocks().iter().map(|m| m.size).sum();
        calculate_ratio(matches, len)
    }

    pub fn quick_ratio(&self) -> f64 {
        let mut fullbcount: HashMap<&T, i64> = HashMap::new();
        for elt in self.b {
            *fullbcount.entry(elt).or_default() += 1;
        }
        let mut avail: HashMap<&T, i64> = HashMap::new();
        let mut matches = 0;
        for elt in self.a {
            let numb = match avail.get(elt) {
                Some(&n) => n,
                None => fullbcount.get(elt).copied().unwrap_or(0),
            };
            avail.insert(elt, numb - 1);
            if numb > 0 {
                matches += 1;
            }
        }
        calculate_ratio(matches, self.a.len() + self.b.len())
    }

    pub fn real_quick_ratio(&self) -> f64 {
        let (la, lb) = (self.a.len(), self.b.len());
        calculate_ratio(la.min(lb), la + lb)
    }
}

/// `SequenceMatcher(None, a, b).ratio()` over code points (autojunk on, the Python default).
pub fn ratio_str(a: &str, b: &str) -> f64 {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    SequenceMatcher::new(&a, &b, true).ratio()
}

/// `difflib.get_close_matches(word, possibilities, n, cutoff)`: best `n` by (score, string),
/// both descending, as `heapq.nlargest` over `(score, x)` tuples.
pub fn get_close_matches<'p>(
    word: &str,
    possibilities: &[&'p str],
    n: usize,
    cutoff: f64,
) -> Vec<&'p str> {
    assert!(n > 0, "n must be > 0");
    assert!(
        (0.0..=1.0).contains(&cutoff),
        "cutoff must be in [0.0, 1.0]"
    );
    let w: Vec<char> = word.chars().collect();
    let mut result: Vec<(f64, &'p str)> = Vec::new();
    for &x in possibilities {
        let xs: Vec<char> = x.chars().collect();
        let mut s = SequenceMatcher::new(&xs, &w, true);
        if s.real_quick_ratio() >= cutoff && s.quick_ratio() >= cutoff {
            let r = s.ratio();
            if r >= cutoff {
                result.push((r, x));
            }
        }
    }
    // Python compares str by code point, which is Rust's `str` Ord.
    result.sort_by(|p, q| q.0.total_cmp(&p.0).then_with(|| q.1.cmp(p.1)));
    result.truncate(n);
    result.into_iter().map(|(_, x)| x).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classic_examples() {
        // From the CPython docs.
        let r = ratio_str("abcd", "bcde");
        assert_eq!(r, 0.75);
        assert_eq!(
            get_close_matches("appel", &["ape", "apple", "peach", "puppy"], 3, 0.6),
            vec!["apple", "ape"]
        );
        let a: Vec<char> = "qabxcd".chars().collect();
        let b: Vec<char> = "abycdf".chars().collect();
        let ops = SequenceMatcher::new(&a, &b, true).get_opcodes();
        let tags: Vec<Tag> = ops.iter().map(|o| o.tag).collect();
        assert_eq!(
            tags,
            vec![
                Tag::Delete,
                Tag::Equal,
                Tag::Replace,
                Tag::Equal,
                Tag::Insert
            ]
        );
    }

    #[test]
    fn empty_is_one() {
        assert_eq!(ratio_str("", ""), 1.0);
    }
}
