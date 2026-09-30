//! CPython 3.11 `list.sort` (timsort with the powersort merge policy), driven by a `<` predicate.
//!
//! Any stable sort gives Python's result when the predicate is a strict weak order. The
//! reference also sorts with comparators that are *not* (e.g. `functools.cmp_to_key` over
//! `a - b`, where NaN compares "equal" to everything), and then the exact sequence of comparisons
//! decides the output. This is a line-for-line port of `Objects/listobject.c::list_sort_impl`
//! (count_run, binarysort, powerloop/found_new_run, merge_at, gallop_left/right, merge_lo/hi,
//! merge_force_collapse) so such sorts reproduce CPython bit for bit.

const MIN_GALLOP: usize = 7;

struct Run {
    base: usize,
    len: usize,
    power: i32,
}

struct MergeState<'a, F: FnMut(usize, usize) -> bool> {
    /// Permutation being sorted: `keys[i]` is an index into the caller's slice.
    keys: Vec<usize>,
    lt: &'a mut F,
    min_gallop: usize,
    pending: Vec<Run>,
    tmp: Vec<usize>,
    listlen: usize,
}

/// Sort `v` in place exactly as CPython's `list.sort` would, where `lt(a, b)` is `a < b`.
pub fn sort_by_lt<T, F: FnMut(&T, &T) -> bool>(v: &mut Vec<T>, mut lt: F) {
    let n = v.len();
    if n < 2 {
        return;
    }
    let perm = {
        let data: &Vec<T> = v;
        let mut ilt = |a: usize, b: usize| lt(&data[a], &data[b]);
        sort_perm(n, &mut ilt)
    };
    let mut slots: Vec<Option<T>> = std::mem::take(v).into_iter().map(Some).collect();
    v.extend(perm.into_iter().map(|i| slots[i].take().unwrap()));
}

/// Sort by a key, comparing keys with `lt` (`sorted(xs, key=...)`).
pub fn sort_by_key_lt<T, K, FK: FnMut(&T) -> K, F: FnMut(&K, &K) -> bool>(
    v: &mut Vec<T>,
    mut key: FK,
    mut lt: F,
) {
    let n = v.len();
    if n < 2 {
        return;
    }
    let keys: Vec<K> = v.iter().map(&mut key).collect();
    let mut ilt = |a: usize, b: usize| lt(&keys[a], &keys[b]);
    let perm = sort_perm(n, &mut ilt);
    let mut slots: Vec<Option<T>> = std::mem::take(v).into_iter().map(Some).collect();
    v.extend(perm.into_iter().map(|i| slots[i].take().unwrap()));
}

/// Returns the sorted permutation of `0..n` under `lt(i, j)`.
pub fn sort_perm<F: FnMut(usize, usize) -> bool>(n: usize, lt: &mut F) -> Vec<usize> {
    let mut ms = MergeState {
        keys: (0..n).collect(),
        lt,
        min_gallop: MIN_GALLOP,
        pending: Vec::new(),
        tmp: Vec::new(),
        listlen: n,
    };
    if n < 2 {
        return ms.keys;
    }
    let minrun = merge_compute_minrun(n);
    let mut lo = 0;
    let mut nremaining = n;
    while nremaining > 0 {
        let (mut run, descending) = ms.count_run(lo, lo + nremaining);
        if descending {
            ms.keys[lo..lo + run].reverse();
        }
        if run < minrun {
            let force = nremaining.min(minrun);
            ms.binarysort(lo, lo + force, lo + run);
            run = force;
        }
        ms.found_new_run(run);
        ms.pending.push(Run {
            base: lo,
            len: run,
            power: 0,
        });
        lo += run;
        nremaining -= run;
    }
    ms.merge_force_collapse();
    ms.keys
}

fn merge_compute_minrun(mut n: usize) -> usize {
    let mut r = 0;
    while n >= 64 {
        r |= n & 1;
        n >>= 1;
    }
    n + r
}

fn powerloop(s1: usize, n1: usize, n2: usize, n: usize) -> i32 {
    let mut result = 0;
    let mut a = 2 * s1 + n1;
    let mut b = a + n1 + n2;
    loop {
        result += 1;
        if a >= n {
            a -= n;
            b -= n;
        } else if b >= n {
            break;
        }
        a <<= 1;
        b <<= 1;
    }
    result
}

impl<F: FnMut(usize, usize) -> bool> MergeState<'_, F> {
    #[inline]
    fn islt(&mut self, x: usize, y: usize) -> bool {
        (self.lt)(x, y)
    }

    /// Returns (run length, descending). `hi` is exclusive.
    fn count_run(&mut self, lo: usize, hi: usize) -> (usize, bool) {
        if lo + 1 == hi {
            return (1, false);
        }
        let mut n = 2;
        let mut p = lo + 2;
        let (a, b) = (self.keys[lo + 1], self.keys[lo]);
        if self.islt(a, b) {
            while p < hi {
                let (x, y) = (self.keys[p], self.keys[p - 1]);
                if !self.islt(x, y) {
                    break;
                }
                p += 1;
                n += 1;
            }
            (n, true)
        } else {
            while p < hi {
                let (x, y) = (self.keys[p], self.keys[p - 1]);
                if self.islt(x, y) {
                    break;
                }
                p += 1;
                n += 1;
            }
            (n, false)
        }
    }

    fn binarysort(&mut self, lo: usize, hi: usize, mut start: usize) {
        if lo == start {
            start += 1;
        }
        while start < hi {
            let pivot = self.keys[start];
            let mut l = lo;
            let mut r = start;
            loop {
                let p = l + ((r - l) >> 1);
                let kp = self.keys[p];
                if self.islt(pivot, kp) {
                    r = p;
                } else {
                    l = p + 1;
                }
                if l >= r {
                    break;
                }
            }
            self.keys.copy_within(l..start, l + 1);
            self.keys[l] = pivot;
            start += 1;
        }
    }

    fn found_new_run(&mut self, n2: usize) {
        if let Some(last) = self.pending.last() {
            let s1 = last.base;
            let n1 = last.len;
            let power = powerloop(s1, n1, n2, self.listlen);
            while self.pending.len() > 1 && self.pending[self.pending.len() - 2].power > power {
                let i = self.pending.len() - 2;
                self.merge_at(i);
            }
            let k = self.pending.len() - 1;
            self.pending[k].power = power;
        }
    }

    fn merge_force_collapse(&mut self) {
        while self.pending.len() > 1 {
            let mut n = self.pending.len() - 2;
            if n > 0 && self.pending[n - 1].len < self.pending[n + 1].len {
                n -= 1;
            }
            self.merge_at(n);
        }
    }

    fn merge_at(&mut self, i: usize) {
        let mut ssa = self.pending[i].base;
        let mut na = self.pending[i].len;
        let ssb = self.pending[i + 1].base;
        let mut nb = self.pending[i + 1].len;
        self.pending[i].len = na + nb;
        // `if (i == ms->n - 3) p[i+1] = p[i+2]; --ms->n;`
        self.pending.remove(i + 1);

        let kb = self.keys[ssb];
        let k = self.gallop_right(kb, ssa, na, 0);
        ssa += k;
        na -= k;
        if na == 0 {
            return;
        }
        let ka = self.keys[ssa + na - 1];
        nb = self.gallop_left(ka, ssb, nb, nb - 1);
        if nb == 0 {
            return;
        }
        if na <= nb {
            self.merge_lo(ssa, na, ssb, nb);
        } else {
            self.merge_hi(ssa, na, ssb, nb);
        }
    }

    /// `gallop_left(key, a, n, hint)` over `keys[a..a+n]`.
    fn gallop_left(&mut self, key: usize, a: usize, n: usize, hint: usize) -> usize {
        let mut lastofs: usize;
        let mut ofs: usize;
        let ah = self.keys[a + hint];
        if self.islt(ah, key) {
            let maxofs = n - hint;
            lastofs = 0;
            ofs = 1;
            while ofs < maxofs {
                let x = self.keys[a + hint + ofs];
                if self.islt(x, key) {
                    lastofs = ofs;
                    ofs = (ofs << 1) + 1;
                } else {
                    break;
                }
            }
            if ofs > maxofs {
                ofs = maxofs;
            }
            lastofs += hint;
            ofs += hint;
        } else {
            let maxofs = hint + 1;
            lastofs = 0;
            ofs = 1;
            while ofs < maxofs {
                let x = self.keys[a + hint - ofs];
                if self.islt(x, key) {
                    break;
                }
                lastofs = ofs;
                ofs = (ofs << 1) + 1;
            }
            if ofs > maxofs {
                ofs = maxofs;
            }
            let k = lastofs;
            // lastofs = hint - ofs (may be -1); keep it shifted by one to stay unsigned.
            let lastofs_p1 = hint + 1 - ofs;
            ofs = hint - k;
            return self.gallop_left_finish(key, a, lastofs_p1, ofs);
        }
        self.gallop_left_finish(key, a, lastofs + 1, ofs)
    }

    fn gallop_left_finish(&mut self, key: usize, a: usize, mut lo: usize, mut ofs: usize) -> usize {
        while lo < ofs {
            let m = lo + ((ofs - lo) >> 1);
            let x = self.keys[a + m];
            if self.islt(x, key) {
                lo = m + 1;
            } else {
                ofs = m;
            }
        }
        ofs
    }

    /// `gallop_right(key, a, n, hint)` over `keys[a..a+n]`.
    fn gallop_right(&mut self, key: usize, a: usize, n: usize, hint: usize) -> usize {
        let mut lastofs: usize = 0;
        let mut ofs: usize = 1;
        let ah = self.keys[a + hint];
        if self.islt(key, ah) {
            let maxofs = hint + 1;
            while ofs < maxofs {
                let x = self.keys[a + hint - ofs];
                if self.islt(key, x) {
                    lastofs = ofs;
                    ofs = (ofs << 1) + 1;
                } else {
                    break;
                }
            }
            if ofs > maxofs {
                ofs = maxofs;
            }
            let k = lastofs;
            let lastofs_p1 = hint + 1 - ofs;
            ofs = hint - k;
            return self.gallop_right_finish(key, a, lastofs_p1, ofs);
        }
        let maxofs = n - hint;
        while ofs < maxofs {
            let x = self.keys[a + hint + ofs];
            if self.islt(key, x) {
                break;
            }
            lastofs = ofs;
            ofs = (ofs << 1) + 1;
        }
        if ofs > maxofs {
            ofs = maxofs;
        }
        lastofs += hint;
        ofs += hint;
        self.gallop_right_finish(key, a, lastofs + 1, ofs)
    }

    fn gallop_right_finish(
        &mut self,
        key: usize,
        a: usize,
        mut lo: usize,
        mut ofs: usize,
    ) -> usize {
        while lo < ofs {
            let m = lo + ((ofs - lo) >> 1);
            let x = self.keys[a + m];
            if self.islt(key, x) {
                ofs = m;
            } else {
                lo = m + 1;
            }
        }
        ofs
    }

    fn merge_lo(&mut self, ssa0: usize, mut na: usize, ssb0: usize, mut nb: usize) {
        // Copy run A to temp; `ssa` indexes tmp, `ssb`/`dest` index keys.
        self.tmp.clear();
        self.tmp.extend_from_slice(&self.keys[ssa0..ssa0 + na]);
        let mut ssa = 0usize;
        let mut ssb = ssb0;
        let mut dest = ssa0;
        let mut min_gallop = self.min_gallop;

        self.keys[dest] = self.keys[ssb];
        dest += 1;
        ssb += 1;
        nb -= 1;
        'outer: {
            if nb == 0 {
                break 'outer; // Succeed
            }
            if na == 1 {
                return self.lo_copy_b(ssa, dest, ssb, nb);
            }
            loop {
                let mut acount = 0usize;
                let mut bcount = 0usize;
                loop {
                    let (kb, ka) = (self.keys[ssb], self.tmp[ssa]);
                    if self.islt(kb, ka) {
                        self.keys[dest] = kb;
                        dest += 1;
                        ssb += 1;
                        bcount += 1;
                        acount = 0;
                        nb -= 1;
                        if nb == 0 {
                            break 'outer;
                        }
                        if bcount >= min_gallop {
                            break;
                        }
                    } else {
                        self.keys[dest] = ka;
                        dest += 1;
                        ssa += 1;
                        acount += 1;
                        bcount = 0;
                        na -= 1;
                        if na == 1 {
                            return self.lo_copy_b(ssa, dest, ssb, nb);
                        }
                        if acount >= min_gallop {
                            break;
                        }
                    }
                }
                min_gallop += 1;
                loop {
                    min_gallop -= (min_gallop > 1) as usize;
                    self.min_gallop = min_gallop;
                    let kb = self.keys[ssb];
                    let k = self.gallop_right_tmp(kb, ssa, na, 0);
                    acount = k;
                    if k > 0 {
                        for j in 0..k {
                            self.keys[dest + j] = self.tmp[ssa + j];
                        }
                        dest += k;
                        ssa += k;
                        na -= k;
                        if na == 1 {
                            return self.lo_copy_b(ssa, dest, ssb, nb);
                        }
                        if na == 0 {
                            break 'outer;
                        }
                    }
                    self.keys[dest] = self.keys[ssb];
                    dest += 1;
                    ssb += 1;
                    nb -= 1;
                    if nb == 0 {
                        break 'outer;
                    }

                    let ka = self.tmp[ssa];
                    let k = self.gallop_left(ka, ssb, nb, 0);
                    bcount = k;
                    if k > 0 {
                        self.keys.copy_within(ssb..ssb + k, dest);
                        dest += k;
                        ssb += k;
                        nb -= k;
                        if nb == 0 {
                            break 'outer;
                        }
                    }
                    self.keys[dest] = self.tmp[ssa];
                    dest += 1;
                    ssa += 1;
                    na -= 1;
                    if na == 1 {
                        return self.lo_copy_b(ssa, dest, ssb, nb);
                    }
                    if !(acount >= MIN_GALLOP || bcount >= MIN_GALLOP) {
                        break;
                    }
                }
                min_gallop += 1;
                self.min_gallop = min_gallop;
            }
        }
        // Succeed: copy what's left of A.
        for j in 0..na {
            self.keys[dest + j] = self.tmp[ssa + j];
        }
    }

    fn lo_copy_b(&mut self, ssa: usize, dest: usize, ssb: usize, nb: usize) {
        self.keys.copy_within(ssb..ssb + nb, dest);
        self.keys[dest + nb] = self.tmp[ssa];
    }

    /// gallop_right with the run living in `tmp`.
    fn gallop_right_tmp(&mut self, key: usize, a: usize, n: usize, hint: usize) -> usize {
        // Temporarily view tmp through the same algorithm.
        let saved = std::mem::take(&mut self.keys);
        let tmp = std::mem::take(&mut self.tmp);
        self.keys = tmp;
        let r = self.gallop_right(key, a, n, hint);
        self.tmp = std::mem::replace(&mut self.keys, saved);
        r
    }

    fn gallop_left_tmp(&mut self, key: usize, a: usize, n: usize, hint: usize) -> usize {
        let saved = std::mem::take(&mut self.keys);
        let tmp = std::mem::take(&mut self.tmp);
        self.keys = tmp;
        let r = self.gallop_left(key, a, n, hint);
        self.tmp = std::mem::replace(&mut self.keys, saved);
        r
    }

    fn merge_hi(&mut self, ssa0: usize, mut na: usize, ssb0: usize, mut nb: usize) {
        // Copy run B to temp. Indices are "one past" positions to stay unsigned:
        // `ssa`/`dest` point one past the current keys slot, `ssb` one past the tmp slot.
        self.tmp.clear();
        self.tmp.extend_from_slice(&self.keys[ssb0..ssb0 + nb]);
        let basea = ssa0;
        let mut dest = ssb0 + nb; // one past
        let mut ssa = ssa0 + na; // one past
        let mut ssb = nb; // one past, in tmp
        let mut min_gallop = self.min_gallop;

        dest -= 1;
        ssa -= 1;
        self.keys[dest] = self.keys[ssa];
        na -= 1;
        'outer: {
            if na == 0 {
                break 'outer;
            }
            if nb == 1 {
                return self.hi_copy_a(dest, ssa, ssb, na);
            }
            loop {
                let mut acount = 0usize;
                let mut bcount = 0usize;
                loop {
                    let (kb, ka) = (self.tmp[ssb - 1], self.keys[ssa - 1]);
                    if self.islt(kb, ka) {
                        dest -= 1;
                        ssa -= 1;
                        self.keys[dest] = ka;
                        acount += 1;
                        bcount = 0;
                        na -= 1;
                        if na == 0 {
                            break 'outer;
                        }
                        if acount >= min_gallop {
                            break;
                        }
                    } else {
                        dest -= 1;
                        ssb -= 1;
                        self.keys[dest] = kb;
                        bcount += 1;
                        acount = 0;
                        nb -= 1;
                        if nb == 1 {
                            return self.hi_copy_a(dest, ssa, ssb, na);
                        }
                        if bcount >= min_gallop {
                            break;
                        }
                    }
                }
                min_gallop += 1;
                loop {
                    min_gallop -= (min_gallop > 1) as usize;
                    self.min_gallop = min_gallop;
                    let kb = self.tmp[ssb - 1];
                    let k = self.gallop_right(kb, basea, na, na - 1);
                    let k = na - k;
                    acount = k;
                    if k > 0 {
                        dest -= k;
                        ssa -= k;
                        self.keys.copy_within(ssa..ssa + k, dest);
                        na -= k;
                        if na == 0 {
                            break 'outer;
                        }
                    }
                    dest -= 1;
                    ssb -= 1;
                    self.keys[dest] = self.tmp[ssb];
                    nb -= 1;
                    if nb == 1 {
                        return self.hi_copy_a(dest, ssa, ssb, na);
                    }

                    let ka = self.keys[ssa - 1];
                    let k = self.gallop_left_tmp(ka, 0, nb, nb - 1);
                    let k = nb - k;
                    bcount = k;
                    if k > 0 {
                        dest -= k;
                        ssb -= k;
                        for j in 0..k {
                            self.keys[dest + j] = self.tmp[ssb + j];
                        }
                        nb -= k;
                        if nb == 1 {
                            return self.hi_copy_a(dest, ssa, ssb, na);
                        }
                        if nb == 0 {
                            break 'outer;
                        }
                    }
                    dest -= 1;
                    ssa -= 1;
                    self.keys[dest] = self.keys[ssa];
                    na -= 1;
                    if na == 0 {
                        break 'outer;
                    }
                    if !(acount >= MIN_GALLOP || bcount >= MIN_GALLOP) {
                        break;
                    }
                }
                min_gallop += 1;
                self.min_gallop = min_gallop;
            }
        }
        // Succeed: copy what's left of B (tmp[0..nb]) to keys[dest-nb..dest].
        for j in 0..nb {
            self.keys[dest - nb + j] = self.tmp[j];
        }
    }

    /// CopyA: nb == 1; the first element of B belongs at the front of the merge.
    fn hi_copy_a(&mut self, dest: usize, ssa: usize, ssb: usize, na: usize) {
        // C: dest -= na; ssa -= na; memmove(dest+1, ssa+1, na); *dest = *ssb;
        // Here dest/ssa are one past the C pointers.
        // (C's `ssa` may point one before the run here, so work with `ssa + 1` directly.)
        let d = dest - 1 - na;
        let src = ssa - na; // C: ssa + 1
        self.keys.copy_within(src..src + na, d + 1);
        self.keys[d] = self.tmp[ssb - 1];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u64) -> u64 {
        *seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        *seed >> 33
    }

    #[test]
    fn matches_stable_sort_on_total_orders() {
        let mut seed = 7;
        for n in [0usize, 1, 2, 3, 10, 63, 64, 65, 200, 1000, 5000] {
            for modulus in [2u64, 10, 1000, 1 << 30] {
                let mut v: Vec<(u64, usize)> =
                    (0..n).map(|i| (lcg(&mut seed) % modulus, i)).collect();
                // Mix in presorted and reversed stretches to exercise runs and galloping.
                if n > 100 {
                    v[10..60].sort();
                    v[60..100].sort_by(|a, b| b.cmp(a));
                }
                let mut want = v.clone();
                want.sort_by_key(|x| x.0);
                sort_by_lt(&mut v, |a, b| a.0 < b.0);
                assert_eq!(v, want, "n={n} mod={modulus}");
            }
        }
    }
}
