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

## Profile `paddleocr-vl` (PaddleOCR-VL behind vLLM)
PaddleOCR-VL is an *element-level* model. It does not follow the JSON prompt above, so it gets its own engine (`pi_ocr::PaddleVlEngine`), selected with `profile = "paddleocr-vl"`. Triage, rendering, span mapping, routing and layout are shared with `spans-json`.

**Request contract**, checked by `crates/pi-ocr/tests/mock_paddle_vl.rs`:
- One user message: `[image_url (PNG data URL), text "<task>:"]`. No system message, no `response_format`, `temperature = 0`.
- Always send `skip_special_tokens: false`. vLLM strips special tokens by default, which would drop the `<|LOC_n|>` location tokens.
- Per page, one `Spotting:` call. The reply is text lines with `<|LOC_0..1000|>` boxes, normalized to 0-1000 on each axis:
  - 4 tokens give `x0 y0 x1 y1`; 8 give a quad, which is reduced to its axis-aligned box.
  - The tokens may come before or after the line text.
  - With no LOC tokens, the engine warns once and falls back to the plain-text parser (`approx_bbox`).
- The page is rendered at a dpi that fits `max_pixels`: `72·sqrt(max_pixels / page_area_pt²)`, clamped to 72..400. The default is 2048·28·28 ≈ 1.6 MP, the Spotting budget from the model card.
- **Tables** (`tables = true`): a grid heuristic over the spotted lines finds candidate regions. A region needs ≥3 consecutive rows with ≥2 cells each, a numeric cell, and gaps of no more than 3 line heights. Each region is cropped from the page PNG and sent as `Table Recognition:`.
  - The OTSL reply (`<fcel> <ecel> <lcel> <ucel> <xcel> <nl>`) is converted to markdown. Merged cells are left empty, and an HTML table reply also works.
  - The markdown is appended to that page's text in `pages.json`.
- PaddleOCR-VL returns no heading levels; page layout in PaddleOCR comes from a separate detector. Headings on OCR'd pages therefore come from the normal layout/outline heuristics, using the font size estimated from line height.

**Serving.** Use the official PaddleOCR genai server with the vLLM backend, e.g. `paddleocr genai_server --model_name PaddleOCR-VL-1.6-0.9B --backend vllm --port 8118`, or plain `vllm serve PaddlePaddle/PaddleOCR-VL-1.6 --trust-remote-code`. Flags change between releases, so check the current PaddleOCR docs. Either way the server exposes `/v1/chat/completions`.

```toml
[ocr]
profile = "paddleocr-vl"
base_url = "http://dgx:8118/v1"
model = "PaddleOCR-VL-1.6-0.9B"   # the served model name
api_key_env = "PI_OCR_KEY"        # omit for an unauthenticated local server
concurrency = 16
max_tokens = 8192
tables = true
max_pixels = 1605632              # 2048*28*28
timeout_s = 180
```

**Verify on the real server first.** `ocr_calibrate --profile paddleocr-vl --dump-raw page.pdf` prints one raw Spotting reply; it should contain `<|LOC_n|>` tokens. Then run without `--dump-raw` on born-digital PDFs to get `font_size_factor`.

## Open items (need an endpoint)
- Which served model: PaddleOCR-VL (profile above), a Qwen-VL class model, or a hosted GPT/Claude vision model. Compare box quality and cost per page on the synthetic scans (`parity/make_scans.py`) against the born-digital goldens.
- Whether the served model supports `response_format`; `json_mode = false` covers the case where it doesn't.
