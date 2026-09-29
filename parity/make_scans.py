"""Make synthetic scanned PDFs from born-digital corpus docs (image-only, no text layer).

Ground truth for OCR evaluation is the born-digital golden tree of the source doc. Options add
mild scan artefacts: skew, gaussian noise, JPEG compression, grayscale.

  parity/.venv/bin/python parity/make_scans.py --corpus parity/corpus.toml --ids earthmover --dpi 150 300
"""
from __future__ import annotations

import argparse
import io
import random
import tomllib
from pathlib import Path

import pypdfium2 as pdfium
from PIL import Image

HERE = Path(__file__).resolve().parent


def degrade(img: Image.Image, rng: random.Random, skew: float, noise: float, jpeg: int) -> Image.Image:
    img = img.convert("L")
    if skew:
        img = img.rotate(rng.uniform(-skew, skew), resample=Image.BICUBIC, expand=False, fillcolor=255)
    if noise:
        px = img.load()
        w, h = img.size
        for _ in range(int(noise * w * h)):
            x, y = rng.randrange(w), rng.randrange(h)
            px[x, y] = rng.choice((0, 255))
    if jpeg:
        buf = io.BytesIO()
        img.save(buf, "JPEG", quality=jpeg)
        img = Image.open(io.BytesIO(buf.getvalue()))
    return img


def scan(src: Path, dst: Path, dpi: int, skew: float, noise: float, jpeg: int, seed: int, max_pages: int | None):
    rng = random.Random(seed)
    doc = pdfium.PdfDocument(str(src))
    pages = []
    try:
        for i in range(len(doc) if max_pages is None else min(len(doc), max_pages)):
            page = doc[i]
            bitmap = page.render(scale=dpi / 72)
            pages.append(degrade(bitmap.to_pil(), rng, skew, noise, jpeg))
            page.close()
    finally:
        doc.close()
    dst.parent.mkdir(parents=True, exist_ok=True)
    pages[0].save(dst, "PDF", resolution=dpi, save_all=True, append_images=pages[1:])


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--corpus", type=Path, default=HERE / "corpus.toml")
    ap.add_argument("--ids", nargs="+", required=True)
    ap.add_argument("--dpi", type=int, nargs="+", default=[200])
    ap.add_argument("--skew", type=float, default=0.8, help="max rotation in degrees")
    ap.add_argument("--noise", type=float, default=0.0005, help="salt-and-pepper fraction")
    ap.add_argument("--jpeg", type=int, default=70, help="JPEG quality, 0 = lossless")
    ap.add_argument("--max-pages", type=int)
    ap.add_argument("--out", type=Path, default=HERE / "scans")
    args = ap.parse_args()
    cfg = tomllib.loads(args.corpus.read_text())
    docs = {d["id"]: (args.corpus.parent / d["path"]).resolve() for d in cfg["doc"]}
    for doc_id in args.ids:
        for dpi in args.dpi:
            dst = args.out / f"{doc_id}__scan{dpi}.pdf"
            scan(docs[doc_id], dst, dpi, args.skew, args.noise, args.jpeg, seed=sum(map(ord, doc_id)) * 1000 + dpi,
                 max_pages=args.max_pages)
            print(f"{dst} ({dst.stat().st_size // 1024} KiB)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
