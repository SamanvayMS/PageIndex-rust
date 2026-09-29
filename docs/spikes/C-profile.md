# Spike C: per-stage cost of the reference Flash pipeline

Command: `parity/.venv/bin/python parity/profile_stages.py --ids prml 2023_annual_report earthmover fb_3m_2018_10k fb_pepsico_2022_10k`. It runs `extract_toc(workers=1)` under cProfile (cumulative time per stage function) on a 4-core container. cProfile roughly doubles wall time, so read the table as **shares**, not absolute speed. The unprofiled figures are PRML ~75 ms/page and the 3M 10-K ~370 ms/page.

| stage | prml | 2023 annual report | earthmover | 3M 10-K 2018 | PepsiCo 10-K 2022 |
|---|---|---|---|---|---|
| **wall (profiled), ms/page** | **176.7** | **206.7** | **497.8** | **887.9** | **166.1** |
| parse (chars → spans) | 115.0 | 124.4 | 314.2 | 652.5 | 121.6 |
| · pass1 | 78.5 | 86.6 | 217.0 | 518.8 | 89.1 |
| · · raw chars (pdfium walk) | 36.0 | 45.7 | 100.5 | 254.7 | 43.0 |
| · · content-stream tokenize | 19.2 | 24.5 | 45.3 | 181.6 | 23.0 |
| · · text-object index | 4.7 | 4.6 | 10.7 | 99.8 | 6.0 |
| · · char → object lookup | 5.4 | 14.0 | 18.9 | 78.6 | 8.6 |
| · · font unicode | 17.1 | 11.5 | 51.2 | 68.4 | 18.6 |
| · pass2 | 26.4 | 27.6 | 72.7 | 67.2 | 25.0 |
| · spans (merge/remerge) | 9.2 | 9.4 | 22.4 | 64.6 | 6.5 |
| lines + columns (process_page) | 27.6 | 40.0 | 91.1 | 162.7 | 20.5 |
| blocks | 10.8 | 17.8 | 29.3 | 36.2 | 7.3 |
| header/footer + TOC + body | 16.5 | 13.9 | 56.6 | 21.1 | 12.1 |
| title, captions, openers, outline, tree, bookmarks | 7.4 | 12.3 | 17.9 | 6.3 | 6.2 |

## Takeaways
- **Char extraction is 65–75% of the time** on every doc, and the rest of the layout pipeline is under 30%. Phase 1 (`pi-extract`) is where the speedup is. Layout, outline and tree stages are cheap in Python and will be negligible in Rust.
- The 3M 10-K is slow for a structural reason: dense table pages with many text objects. The text-object index plus char→object lookup (`_find_obj_for_char` is O(chars × objects) per page) and the content-stream tokenizer grow superlinearly. In Rust the lookup should use a spatial index. It must still return **the same object** the reference's linear scan picks (first match in paint order, with the matrix/size tie-break), so the port keeps the scan order and only prunes candidates.
- The PDFium floor measured in Spike A is ~42 ms/page (unprofiled) on the 3M 10-K. Everything above it is Python overhead.
