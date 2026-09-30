"""Generate crates/pi-pycompat/tests/fixtures/pysort.json (run with Python 3.11).

Records the permutation CPython's list.sort produces for comparators that are not strict weak
orders: `functools.cmp_to_key` over `a - b` (the reference's `_percentile_sample_cmp`), where NaN
and inf - inf compare "equal" to everything.
"""
import functools
import json
import math
import random
import sys
from pathlib import Path

assert sys.version_info[:2] == (3, 11), sys.version


def sample_cmp(a, b):
    # stats/aggregates.py::_percentile_sample_cmp
    if a[0] != b[0]:
        return a[0] - b[0]
    return a[1] - b[1]


def enc(x):
    if math.isnan(x):
        return "nan"
    if math.isinf(x):
        return "inf" if x > 0 else "-inf"
    return x


def main():
    rng = random.Random(3)
    cases = []
    for n in [0, 1, 2, 3, 5, 10, 40, 63, 64, 65, 100, 300, 1000, 3000]:
        for trial in range(4):
            vals = []
            for _ in range(n):
                r = rng.random()
                if r < 0.08:
                    v = math.nan
                elif r < 0.11:
                    v = rng.choice([math.inf, -math.inf])
                elif r < 0.5:
                    v = float(rng.randint(0, 20))
                else:
                    v = rng.uniform(-50, 50)
                w = rng.choice([0.0, 1.0, rng.uniform(0, 5), math.nan if trial == 3 else 2.0])
                vals.append((v, w))
            if trial == 2 and n > 20:  # long presorted / reversed stretches -> galloping
                head = sorted(vals[: n // 2], key=lambda t: (t[0] if t[0] == t[0] else 0.0))
                vals = head + list(reversed(head)) + vals[n // 2:]
            idx = list(range(len(vals)))
            perm = sorted(idx, key=functools.cmp_to_key(lambda i, j: sample_cmp(vals[i], vals[j])))
            cases.append({"values": [[enc(v), enc(w)] for v, w in vals], "perm": perm})
    p = Path(__file__).resolve().parent.parent / "tests" / "fixtures" / "pysort.json"
    p.write_text(json.dumps(cases, separators=(",", ":")))
    print(p, p.stat().st_size)


main()
