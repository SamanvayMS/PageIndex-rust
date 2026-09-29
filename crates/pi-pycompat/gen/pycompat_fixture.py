"""Generate crates/pi-pycompat/tests/fixtures/pycompat.json from CPython (run with Python 3.11)."""
import difflib
import json
import random
import sys
import unicodedata
from pathlib import Path

rng = random.Random(1234)
ALPHA = "abcde fghij.,-1234ÅéİıſK中文"


def rstr(lo, hi, alphabet=ALPHA):
    return "".join(rng.choice(alphabet) for _ in range(rng.randint(lo, hi)))


def main():
    out = {"python": sys.version.split()[0], "unicode": unicodedata.unidata_version}
    ratio = []
    for _ in range(300):
        a, b = rstr(0, 40), rstr(0, 40)
        ratio.append([a, b, difflib.SequenceMatcher(None, a, b).ratio()])
    for _ in range(60):  # >= 200 elements: autojunk path
        a, b = rstr(150, 400, "abc de"), rstr(200, 400, "abc de")
        ratio.append([a, b, difflib.SequenceMatcher(None, a, b).ratio()])
    out["ratio"] = ratio
    ops = []
    for _ in range(200):
        a, b = rstr(0, 300, "abcd "), rstr(0, 300, "abcd ")
        ops.append([a, b, [list(o) for o in difflib.SequenceMatcher(None, a, b, autojunk=False).get_opcodes()]])
    out["opcodes_nojunk"] = ops
    close = []
    for _ in range(100):
        word = rstr(3, 12, "abcdefg_")
        poss = [rstr(2, 14, "abcdefg_") for _ in range(rng.randint(0, 25))]
        close.append([word, poss, difflib.get_close_matches(word, poss, n=3, cutoff=0.5)])
    out["close"] = close
    rounds = []
    for _ in range(2000):
        x = rng.choice([rng.uniform(-1000, 1000), rng.randint(-2000, 2000) / 8, rng.randint(-100000, 100000) / 1000])
        nd = rng.choice([0, 1, 2, 3])
        rounds.append([x, nd, round(x, nd), float(round(x)), float(f"{x:.6g}")])
    out["round"] = rounds
    ws = "".join(chr(c) for c in range(0x3001) if chr(c).isspace())
    out["py_whitespace"] = ws
    splits = []
    for _ in range(200):
        s = rstr(0, 30, "ab \t\n\x1c\x1f\x85\xa0 　​")
        splits.append([s, s.split(), s.strip()])
    out["split"] = splits
    p = Path(__file__).resolve().parent.parent / "tests" / "fixtures" / "pycompat.json"
    p.write_text(json.dumps(out, ensure_ascii=False))
    print(p, p.stat().st_size)


main()
