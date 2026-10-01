//! OTSL table output (PaddleOCR-VL `Table Recognition:`) → markdown.
//!
//! OTSL encodes a grid row by row: `<fcel>` a cell with content (text follows the tag),
//! `<ecel>` an empty cell, `<lcel>` merged with the cell to its left, `<ucel>` merged with the
//! cell above, `<xcel>` merged both ways, `<nl>` end of row. Merged cells render empty in
//! markdown (which has no spans). HTML replies fall back to the HTML converter.

use crate::table::html_table_to_markdown;

/// Parse OTSL into rows of cell strings.
pub fn otsl_rows(s: &str) -> Vec<Vec<String>> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut rest = s;
    let mut cur: Option<String> = None; // open <fcel> content
    let flush = |cur: &mut Option<String>, row: &mut Vec<String>| {
        if let Some(c) = cur.take() {
            row.push(c.split_whitespace().collect::<Vec<_>>().join(" "));
        }
    };
    while !rest.is_empty() {
        if let Some(pos) = rest.find('<') {
            if let Some(c) = cur.as_mut() {
                c.push_str(&rest[..pos]);
            }
            let tail = &rest[pos..];
            let tag = ["<fcel>", "<ecel>", "<lcel>", "<ucel>", "<xcel>", "<nl>"]
                .into_iter()
                .find(|t| tail.starts_with(t));
            match tag {
                Some(t) => {
                    flush(&mut cur, &mut row);
                    match t {
                        "<fcel>" => cur = Some(String::new()),
                        "<nl>" => {
                            if !row.is_empty() {
                                rows.push(std::mem::take(&mut row));
                            }
                        }
                        _ => row.push(String::new()),
                    }
                    rest = &tail[t.len()..];
                }
                None => {
                    // not an OTSL tag: keep the '<' as text
                    if let Some(c) = cur.as_mut() {
                        c.push('<');
                    }
                    rest = &tail[1..];
                }
            }
        } else {
            if let Some(c) = cur.as_mut() {
                c.push_str(rest);
            }
            rest = "";
        }
    }
    flush(&mut cur, &mut row);
    if !row.is_empty() {
        rows.push(row);
    }
    rows
}

/// OTSL (or HTML) table reply → markdown with the first row as header.
pub fn table_reply_to_markdown(reply: &str) -> Option<String> {
    if reply.to_ascii_lowercase().contains("<table") {
        return html_table_to_markdown(reply);
    }
    let rows = otsl_rows(reply);
    let ncol = rows.iter().map(Vec::len).max()?;
    if ncol == 0 || rows.iter().all(|r| r.iter().all(String::is_empty)) {
        return None;
    }
    let esc = |s: &str| s.replace('|', "\\|");
    let line = |r: &Vec<String>| {
        let mut cells: Vec<String> = r.iter().map(|c| esc(c)).collect();
        cells.resize(ncol, String::new());
        format!("| {} |", cells.join(" | "))
    };
    let mut out = vec![
        line(&rows[0]),
        format!("|{}|", vec![" --- "; ncol].join("|")),
    ];
    out.extend(rows[1..].iter().map(line));
    Some(out.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn otsl_basic_and_merged() {
        let r = "<fcel>Item<fcel>2023<fcel>2022<nl><fcel>Revenue<fcel>1,234<fcel>1,100<nl>\
                 <fcel>Total assets<lcel><fcel>9<nl><ecel><ucel><fcel>x<nl>";
        let md = table_reply_to_markdown(r).unwrap();
        assert_eq!(
            md,
            "| Item | 2023 | 2022 |\n| --- | --- | --- |\n| Revenue | 1,234 | 1,100 |\n\
             | Total assets |  | 9 |\n|  |  | x |"
        );
    }

    #[test]
    fn html_fallback_and_empty() {
        assert!(table_reply_to_markdown("<table><tr><td>a</td></tr></table>").is_some());
        assert!(table_reply_to_markdown("").is_none());
        assert_eq!(otsl_rows("<fcel>a < b<nl>")[0][0], "a < b");
    }
}
