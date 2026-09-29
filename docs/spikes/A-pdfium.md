# Spike A: pdfium-render + lopdf vs the reference's pypdfium2/PyPDF2 surface

## Result: GO

pdfium-render 0.9.4's raw bindings expose every FPDF function the reference calls. Driven through them, Rust gets **bit-identical raw per-char values** to pypdfium2 when both load the same PDFium build.

## PDFium build pinning
- pypdfium2 5.13.0 (the reference pin) bundles **PDFium 153.0.7999.0**.
- Use `bblanchon/pdfium-binaries` release `chromium/7999` (`pdfium-linux-x64.tgz`, sha256 `c3af580f9df0fef9545b44115bc5ea440f286956b5f231df69fb373b8efc4f69`). "latest" is 8076 and must not be used for parity runs.
- pdfium-render's `pdfium_latest` feature targets API 7881. All functions below exist in that API level, and it runs correctly against 7999.

## Coverage (all present in `PdfiumLibraryBindings`)

| Group | Functions |
|---|---|
| Text page | `FPDFText_CountChars`, `GetUnicode`, `IsGenerated`, `GetCharOrigin`, `GetCharBox`, `GetLooseCharBox`, `GetFontInfo`, `GetFontSize`, `GetMatrix` |
| Fonts | `FPDFFont_GetBaseFontName`, `GetFontName`, `GetFamilyName`, `GetWeight`, `GetGlyphWidth` (the reference passes the **Unicode codepoint** as the glyph arg; reproduce this) |
| Objects | `FPDFPage_CountObjects`/`GetObject`, `FPDFFormObj_CountObjects`/`GetObject`, `FPDFPageObj_GetType`/`GetMatrix`/`GetBounds`, `FPDFTextObj_GetFont`/`GetFontSize` |
| Page | `FPDFPage_GetRotation`, `GetMediaBox`, `GetCropBox` |
| Bookmarks | `FPDFBookmark_GetFirstChild`/`GetNextSibling`/`GetTitle`/`GetDest`/`GetAction`, `FPDFAction_GetDest`, `FPDFDest_GetDestPageIndex` |

## API notes
- In 0.9 the high-level wrappers keep their raw handles (`page_handle`, `text_page_handle`) and the bindings accessor `pub(crate)`. The extractor therefore owns the `Box<dyn PdfiumLibraryBindings>` from `Pdfium::bind_to_library` and drives PDFium entirely through raw calls, including `FPDF_InitLibrary`/`FPDF_DestroyLibrary`. That also fits the reference, which needs all pages to stay open across pass 1 and pass 2, because the FPDF_FONT pointer is its font identity key.
- PDFium is not thread-safe. Use one document per process or per mutex; per-page layout after extraction parallelizes freely.
- MediaBox/CropBox `/Parent` inheritance and the content-stream, font and CMap work come from `lopdf` (objects plus decoded streams only). The tokenizers are hand-written in the reference and get ported as-is.

## Parity check (`spikes/pdfium-spike` vs `spikes/pdfium-spike/dump_chars.py`)

Per char: unicode, generated flag, origin, char box, loose box, font size, font name and flags. Per page: rotation and the top-level text-object count.

| PDF | pages | chars | identical | Rust raw walk | Python raw walk |
|---|---|---|---|---|---|
| earthmover.pdf | 12 | 78,170 | yes | 0.33 s | 1.11 s |
| 3M_2018_10K.pdf | 160 | 586,547 | yes | 6.66 s | 13.37 s |
| ja_report.pdf | 4 | 863 | yes | 0.09 s | 0.10 s |

## Performance implication
On the 3M 10-K, the raw PDFium char walk costs **~42 ms/page**, and PDFium itself is most of that. The reference's full `_page_pass1` costs ~540 ms/page. The remaining ~90%+ is Python-side work (content-stream tokenizing, the O(chars × objects) text-object lookup `_find_obj_for_char`, Unicode patching), which is what the Rust port removes. A realistic Rust target for dense 10-Ks is roughly 50–80 ms/page, bounded below by PDFium.

Reproduce:
```bash
cargo build --release -p pdfium-spike
target/release/pdfium-spike <pdfium-7999>/lib/libpdfium.so file.pdf > r.json
parity/.venv/bin/python spikes/pdfium-spike/dump_chars.py file.pdf > p.json   # compare r.json == p.json
```
