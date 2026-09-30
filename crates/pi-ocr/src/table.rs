//! HTML table → GitHub-flavoured markdown (for `pages.json`).

fn unescape(s: &str) -> String {
    s.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

fn strip_tags(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    unescape(&out)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn attr_span(tag: &str, name: &str) -> usize {
    let lower = tag.to_ascii_lowercase();
    lower
        .find(&format!("{name}="))
        .and_then(|p| {
            let rest = &lower[p + name.len() + 1..];
            let digits: String = rest
                .trim_start_matches(['"', '\''])
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            digits.parse().ok()
        })
        .unwrap_or(1)
        .clamp(1, 50)
}

/// Rows of cell strings (colspan repeated as empty cells).
pub fn html_table_rows(html: &str) -> Vec<Vec<String>> {
    let lower = html.to_ascii_lowercase();
    let mut rows = Vec::new();
    let mut pos = 0;
    while let Some(tr) = lower[pos..].find("<tr") {
        let start = pos + tr;
        let end = lower[start + 3..]
            .find("<tr")
            .map(|e| start + 3 + e)
            .unwrap_or(lower.len());
        let row_html = &html[start..end];
        let row_lower = &lower[start..end];
        let mut cells = Vec::new();
        let mut cp = 0;
        loop {
            let td = row_lower[cp..].find("<td");
            let th = row_lower[cp..].find("<th");
            let Some(open) = [td, th].into_iter().flatten().min() else {
                break;
            };
            let open = cp + open;
            let tag_end = row_lower[open..]
                .find('>')
                .map(|e| open + e + 1)
                .unwrap_or(row_lower.len());
            let close = ["</td", "</th", "<td", "<th"]
                .iter()
                .filter_map(|m| row_lower[tag_end..].find(m))
                .min()
                .map(|e| tag_end + e)
                .unwrap_or(row_lower.len());
            let span = attr_span(&row_html[open..tag_end], "colspan");
            cells.push(strip_tags(&row_html[tag_end..close]));
            cells.extend(std::iter::repeat_n(String::new(), span - 1));
            cp = close.max(tag_end);
            if cp >= row_lower.len() {
                break;
            }
        }
        if !cells.is_empty() {
            rows.push(cells);
        }
        pos = end;
    }
    rows
}

/// Markdown table; the first row is the header.
pub fn html_table_to_markdown(html: &str) -> Option<String> {
    let rows = html_table_rows(html);
    let ncol = rows.iter().map(Vec::len).max()?;
    if ncol == 0 {
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
    fn simple_table() {
        let h = "<table><tr><th>Item</th><th>2023</th></tr><tr><td>Revenue &amp; other</td><td>1,234</td></tr>\
                 <tr><td colspan=2>Total</td></tr></table>";
        let md = html_table_to_markdown(h).unwrap();
        assert_eq!(
            md,
            "| Item | 2023 |\n| --- | --- |\n| Revenue & other | 1,234 |\n| Total |  |"
        );
    }
}
