# Spike E: OCR engine contract (OpenAI-compatible vision endpoint)

The owner's decision: OCR, like the LLM roles, is an **OpenAI-compatible endpoint** (hosted, or the DGX Spark behind the same API). `pi-ocr` defines an `OcrEngine` trait; the first implementation calls `POST {base_url}/chat/completions` with page images.

## Why it matters (measured)
- On `parity/scans/earthmover__scan150.pdf` (an image-only render of a born-digital paper), the reference Flash returns `toc_source = "unreadable"` with an **empty tree**. Scanned documents get nothing from the Python pipeline today.
- In the FinanceBench set, 15 of 84 filings contain text-less pages (Walmart 2019: 29 of 155).

## Request (one page per request; batching is concurrency, not multi-image prompts)
```json
{
  "model": "<ocr.model>",
  "temperature": 0,
  "response_format": {"type": "json_object"},          // omitted when ocr.json_mode = false
  "messages": [
    {"role": "system", "content": "<PROFILE_PROMPT>"},
    {"role": "user", "content": [
      {"type": "text", "text": "Page 12 of 155. Image is 1700x2200 px."},
      {"type": "image_url", "image_url": {"url": "data:image/png;base64,..."}}
    ]}
  ]
}
```
- Render: pdfium at `ocr.dpi` (default 200); grayscale PNG, or JPEG above a size cap.
- `PROFILE_PROMPT` (`spans-json`) asks for:
```json
{"blocks": [{"bbox": [x0, y0, x1, y1],               // image pixels, origin top-left
             "kind": "text" | "title" | "table" | "header" | "footer" | "caption",
             "level": 1,                             // titles only, optional
             "lines": [{"bbox": [...], "text": "..."}],   // optional; else block text
             "text": "...",
             "html": "<table>...</table>",           // tables only
             "confidence": 0.93}]}                   // optional
```

## Response handling → `PageSpans`
1. Parse leniently: strip code fences, take the outermost `{...}`, and accept a bare list as `blocks`.
2. Map pixel boxes to PDF user space: `x_pdf = x_px * 72/dpi`, `y_pdf = page_h - y_px * 72/dpi`. Then apply the viewbox/rotation the same way the text layer does.
3. Emit one `Span` per line (or per block when there are no lines):
   - `font_size = line_height_pt * k_ocr`, with `k_ocr` calibrated on born-digital pages rendered to images (OCR line box vs text-layer font size, median ratio).
   - `bold = false`, `italic = false`, `skew = 0`, `font_name = None`, `source = Ocr { confidence }`.
4. **Layout hints** (`kind = title` with `level`) are kept in a side table as `HeadingKind::LayoutTitle` candidates (Track B step 3). Tables keep `html`, converted to markdown for `pages.json`.
5. **Fallback when the model returns plain text or markdown without boxes:** split it into lines and distribute them evenly down the page. Mark them `approx_bbox = true` so layout statistics that depend on geometry (columns, gaps) can down-weight them. Markdown `#` headings become `LayoutTitle` hints.

## Config
```toml
[ocr]
base_url = "https://.../v1"
model = "..."
api_key_env = "PI_OCR_KEY"
dpi = 200
concurrency = 8
json_mode = true
profile = "spans-json"
timeout_s = 120
```

## Open items (need an endpoint)
- Which served model: a Paddle-family VL model, Qwen-VL class, or a hosted GPT/Claude vision model. Compare box quality and cost per page on the synthetic scans (`parity/make_scans.py`) against the born-digital goldens.
- Whether the served model supports `response_format`; `json_mode = false` covers the case where it doesn't.
