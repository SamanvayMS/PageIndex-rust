# Reference field map (PageIndex @619cbd8)

The reference uses obfuscated attribute names. `dump_reference.py` renames them on output as listed below. Rust ports use the right-hand names.

| Class | Reference attr | Dump / Rust name |
|---|---|---|
| Rect | left, right, top, primary_slot | `[left, right, top, bottom]` (PDF coords, y up) |
| Bounded | secondary_slot | bbox |
| Span | primary_slot / measure_slot / previous_slot / state_slot | bold / italic / skew / trimmed_text |
| Line | primary_slot | spans (indices into the page's 01_spans list) |
| Line | alignment_slot | first_letter_span |
| Line | weighted_ratio_primary / _secondary / _tertiary | bold_frac / italic_frac / skew_frac |
| Line | metric_slot / previous_slot | avg_font_size / max_span_height |
| Line | measure_slot | column (-1 unassigned) |
| Line | state_slot / style_slot | numbering_kind (-1 uncomputed, 0 none, 1 digit, 2 upper, 3 lower) / numbering_text |
| Line | cache_slot / marker_slot | ink_density / cached trimmed text |
| Block | primary_slot | lines (indices into the page's 02_lines list) |
| Block | alignment_slot | center_aligned |
| Block | weighted_ratio_tertiary / previous_slot | bold_frac / italic_frac |
| Block | weighted_ratio_primary / weighted_ratio_secondary | density_chars / density_area |
| Block | style_slot | max_line_height |
| Block | measure_slot | caption_claimed |
| Block | marker_slot | caption_label (0, 4 figure, 5 table, 11 chart) |
| Block | state_slot | region_label |
| Block | metric_slot / cache_slot | alignment_code cache (1-5) / block_text cache |
| Block | type | 0 unclassified, 1 header, 2 footer, 3 title, 7 heading, 8 numbered heading, 9 TOC, 12 boilerplate/watermark/dup |
| PageStats | secondary, tertiary, previous, style, cache, option, primary, measure, state, auxiliary | total_line_weight, median_overlap_gap, median_line_width, median_char_count, median_density, median_center_y, median_font_size, avg_char_width, dominant_font, dominant_style |
| DocStats | tertiary, style, cache, state, previous, secondary, option, auxiliary, measure, primary | dominant_script, landscape_pages, total_lines, total_weight, max_page_weight, median_page_weight, median_line_width, p80_density, median_center_y, body_font_size |
| PageView | primary / tertiary / output / secondary | page_stats / columns / blocks (original order) / blocks (reading order) |
| PageView | measure / auxiliary / state / style | has_caption / title_or_refs_page / has_body / body_style_hashes |
| DocumentState | primary / secondary / tertiary | pages / doc_stats / recurring_text_hist |
| HeadingCandidate | group_slot / tertiary_slot | block / anchor (as `[page, orig_index]`) |
| HeadingCandidate | secondary_slot / primary_slot | prefix / title (TokenView → str) |
| HeadingCandidate | state_slot / auxiliary_slot | script / y_frac |
| CharStats | primary / secondary / tertiary / auxiliary | category_counts[12] / first_cat / last_cat / total_chars |

Every block ref in stages 05 to 07 is `[page (1-based), orig_index]`. Non-finite floats are written as the strings `"inf"`, `"-inf"` and `"nan"`.
