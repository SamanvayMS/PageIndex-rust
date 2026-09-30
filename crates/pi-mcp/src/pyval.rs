//! Python semantics over JSON argument values: truthiness, `int(x)`, and `datetime`
//! normalisation for `createdAt`.

use serde_json::Value;

/// `bool(value)`.
pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i != 0
            } else if let Some(u) = n.as_u64() {
                u != 0
            } else {
                n.as_f64().is_some_and(|f| f != 0.0)
            }
        }
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Whitespace `int(str)` strips: Python's `str.isspace` set minus U+001C..U+001F (those stay
/// as-is through CPython's ASCII transform and are not ASCII `isspace`).
fn is_int_space(c: char) -> bool {
    pi_pycompat::pystr::is_py_space(c) && !matches!(c, '\u{1C}'..='\u{1F}')
}

/// Decimal value of a Unicode `Nd` character, for the blocks `int()` meets in practice.
fn decimal_value(c: char) -> Option<u32> {
    if let Some(d) = c.to_digit(10) {
        return Some(d);
    }
    // Zero code points of the contiguous Nd runs in Unicode 14 (each run is ten long).
    const ZEROS: &[u32] = &[
        0x0660, 0x06F0, 0x07C0, 0x0966, 0x09E6, 0x0A66, 0x0AE6, 0x0B66, 0x0BE6, 0x0C66, 0x0CE6,
        0x0D66, 0x0DE6, 0x0E50, 0x0ED0, 0x0F20, 0x1040, 0x1090, 0x17E0, 0x1810, 0x1946, 0x19D0,
        0x1A80, 0x1A90, 0x1B50, 0x1BB0, 0x1C40, 0x1C50, 0xA620, 0xA8D0, 0xA900, 0xA9D0, 0xA9F0,
        0xAA50, 0xABF0, 0xFF10, 0x104A0, 0x10D30, 0x11066, 0x110F0, 0x11136, 0x111D0, 0x112F0,
        0x11450, 0x114D0, 0x11650, 0x116C0, 0x11730, 0x118E0, 0x11950, 0x11C50, 0x11D50, 0x11DA0,
        0x16A60, 0x16AC0, 0x16B50, 0x1D7CE, 0x1D7D8, 0x1D7E2, 0x1D7EC, 0x1D7F6, 0x1E140, 0x1E2F0,
        0x1E950, 0x1FBF0,
    ];
    let cp = c as u32;
    ZEROS
        .iter()
        .find(|&&z| (z..z + 10).contains(&cp))
        .map(|&z| cp - z)
}

/// `int(s)` for a `str`: optional sign, decimal digits (any `Nd`), single underscores
/// between digits, surrounding whitespace. Saturates at `i128` bounds.
pub fn int_from_str(s: &str) -> Option<i128> {
    let s = s.trim_matches(is_int_space);
    let (neg, body) = match s.chars().next()? {
        '+' => (false, &s[1..]),
        '-' => (true, &s[1..]),
        _ => (false, s),
    };
    let mut value: i128 = 0;
    let mut prev_digit = false;
    let mut any = false;
    for c in body.chars() {
        if c == '_' {
            if !prev_digit {
                return None;
            }
            prev_digit = false;
            continue;
        }
        let d = decimal_value(c)?;
        value = value.saturating_mul(10).saturating_add(d as i128);
        prev_digit = true;
        any = true;
    }
    if !any || !prev_digit {
        return None;
    }
    Some(if neg { -value } else { value })
}

/// `int(value)`: `None` stands for the `TypeError`/`ValueError` Python raises.
pub fn py_int(value: &Value) -> Option<i128> {
    match value {
        Value::Bool(b) => Some(*b as i128),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(i as i128)
            } else if let Some(u) = n.as_u64() {
                Some(u as i128)
            } else {
                // Finite JSON floats truncate toward zero (`as` saturates).
                Some(n.as_f64().unwrap_or(0.0).trunc() as i128)
            }
        }
        Value::String(s) => int_from_str(s),
        _ => None,
    }
}

// ── createdAt normalisation (datetime.fromisoformat, CPython 3.11) ──

fn digits(s: &[char]) -> Option<i64> {
    if s.is_empty() || !s.iter().all(char::is_ascii_digit) {
        return None;
    }
    Some(
        s.iter()
            .fold(0, |a, c| a * 10 + c.to_digit(10).unwrap() as i64),
    )
}

fn is_leap(y: i64) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if is_leap(y) => 29,
        _ => 28,
    }
}

/// Days from 0001-01-01 (ordinal 1), as `date.toordinal()`.
fn ymd_to_ord(y: i64, m: i64, d: i64) -> i64 {
    let py = y - 1;
    let before_year = py * 365 + py / 4 - py / 100 + py / 400;
    let before_month: i64 = (1..m).map(|mm| days_in_month(y, mm)).sum();
    before_year + before_month + d
}

fn ord_to_ymd(mut n: i64) -> (i64, i64, i64) {
    // Walk years and months; ordinals here stay within 1..=3_652_059.
    let mut y = 1 + (n - 1) / 366;
    while ymd_to_ord(y + 1, 1, 1) <= n {
        y += 1;
    }
    n -= ymd_to_ord(y, 1, 1) - 1;
    let mut m = 1;
    while n > days_in_month(y, m) {
        n -= days_in_month(y, m);
        m += 1;
    }
    (y, m, n)
}

fn find_separator(s: &[char]) -> Option<usize> {
    let len = s.len();
    if len == 7 {
        return Some(7);
    }
    if s[4] == '-' {
        if s[5] == 'W' {
            if len > 8 && s[8] == '-' {
                if len == 9 {
                    return None;
                }
                if len > 10 && s[10].is_ascii_digit() {
                    return Some(8);
                }
                return Some(10);
            }
            return Some(8);
        }
        return Some(10);
    }
    if s[4] == 'W' {
        let mut idx = 7;
        while idx < len && s[idx].is_ascii_digit() {
            idx += 1;
        }
        if idx < 9 {
            return Some(idx);
        }
        return Some(if idx % 2 == 0 { 7 } else { 8 });
    }
    Some(8)
}

fn parse_date(s: &[char]) -> Option<(i64, i64, i64)> {
    if ![7, 8, 10].contains(&s.len()) {
        return None;
    }
    let year = digits(&s[0..4])?;
    let has_sep = s[4] == '-';
    let mut pos = 4 + has_sep as usize;
    if s.get(pos) == Some(&'W') {
        pos += 1;
        let week = digits(s.get(pos..pos + 2)?)?;
        pos += 2;
        let mut day = 1;
        if s.len() > pos {
            if (s[pos] == '-') != has_sep {
                return None;
            }
            pos += has_sep as usize;
            day = digits(s.get(pos..pos + 1)?)?;
        }
        if !(1..=9999).contains(&year) {
            return None;
        }
        let first_weekday = ymd_to_ord(year, 1, 1) % 7;
        let week_ok = (1..53).contains(&week)
            || (week == 53 && (first_weekday == 4 || (first_weekday == 3 && is_leap(year))));
        if !week_ok || !(1..8).contains(&day) {
            return None;
        }
        // _isoweek1monday
        let first_day = ymd_to_ord(year, 1, 1);
        let first_wd = (first_day + 6) % 7;
        let mut week1monday = first_day - first_wd;
        if first_wd > 3 {
            week1monday += 7;
        }
        return Some(ord_to_ymd(week1monday + (week - 1) * 7 + (day - 1)));
    }
    let month = digits(s.get(pos..pos + 2)?)?;
    pos += 2;
    if (s.get(pos) == Some(&'-')) != has_sep {
        return None;
    }
    pos += has_sep as usize;
    let day = digits(s.get(pos..pos + 2)?)?;
    if pos + 2 != s.len() {
        return None;
    }
    Some((year, month, day))
}

/// `_parse_hh_mm_ss_ff` → [h, m, s, us].
fn parse_hms(t: &[char]) -> Option<[i64; 4]> {
    let len = t.len();
    let mut comps = [0i64; 4];
    let mut pos = 0;
    let mut has_sep = false;
    for (comp, slot) in comps.iter_mut().take(3).enumerate() {
        if len - pos < 2 {
            return None;
        }
        *slot = digits(&t[pos..pos + 2])?;
        pos += 2;
        let next = t.get(pos).copied();
        if comp == 0 {
            has_sep = next == Some(':');
        }
        if next.is_none() || comp >= 2 {
            break;
        }
        if has_sep && next != Some(':') {
            return None;
        }
        pos += has_sep as usize;
    }
    if pos < len {
        if t[pos] != '.' && t[pos] != ',' {
            return None;
        }
        pos += 1;
        let rem = len - pos;
        let take = rem.min(6);
        let mut us = digits(&t[pos..pos + take])?;
        for _ in take..6 {
            us *= 10;
        }
        comps[3] = us;
        if rem > take && !t[pos + take..].iter().all(char::is_ascii_digit) {
            return None;
        }
    }
    Some(comps)
}

/// `_normalize_created_at`: ISO-8601 UTC with millisecond precision and a `Z`, or the
/// original string when it does not parse; `""` for a non-string or empty value.
/// // ref: pageindex/agent_tools.py:364
pub fn normalize_created_at(value: Option<&Value>) -> String {
    let Some(Value::String(raw)) = value else {
        return String::new();
    };
    if raw.is_empty() {
        return String::new();
    }
    parse_to_utc_ms(&raw.replace('Z', "+00:00")).unwrap_or_else(|| raw.clone())
}

fn parse_to_utc_ms(s: &str) -> Option<String> {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() < 7 {
        return None;
    }
    let sep = find_separator(&chars)?;
    let dstr = &chars[..sep.min(chars.len())];
    let tstr: &[char] = if sep < chars.len() {
        &chars[sep + 1..]
    } else {
        &[]
    };
    let (y, mo, d) = parse_date(dstr)?;
    let (mut hms, mut offset_us) = ([0i64; 4], 0i64);
    if !tstr.is_empty() {
        if tstr.len() < 2 {
            return None;
        }
        let tz_pos = tstr
            .iter()
            .position(|&c| c == '-')
            .or_else(|| tstr.iter().position(|&c| c == '+'))
            .or_else(|| tstr.iter().position(|&c| c == 'Z'))
            .map(|p| p + 1)
            .unwrap_or(0);
        let timestr = if tz_pos > 0 {
            &tstr[..tz_pos - 1]
        } else {
            tstr
        };
        hms = parse_hms(timestr)?;
        if tz_pos > 0 && !(tz_pos == tstr.len() && tstr[tstr.len() - 1] == 'Z') {
            let tz = &tstr[tz_pos..];
            if [0, 1, 3].contains(&tz.len()) {
                return None;
            }
            let c = parse_hms(tz)?;
            let td = ((c[0] * 60 + c[1]) * 60 + c[2]) * 1_000_000 + c[3];
            if td >= 86_400_000_000 {
                return None;
            }
            offset_us = if tstr[tz_pos - 1] == '-' { -td } else { td };
        }
    }
    // Field validation, as the datetime constructor does.
    if !(1..=9999).contains(&y)
        || !(1..=12).contains(&mo)
        || !(1..=days_in_month(y, mo)).contains(&d)
        || !(0..24).contains(&hms[0])
        || !(0..60).contains(&hms[1])
        || !(0..60).contains(&hms[2])
    {
        return None;
    }
    let total_us = ((ymd_to_ord(y, mo, d) * 86_400 + hms[0] * 3600 + hms[1] * 60 + hms[2])
        * 1_000_000
        + hms[3])
        - offset_us;
    let day_us = 86_400_000_000i64;
    let ord = total_us.div_euclid(day_us);
    let rem = total_us.rem_euclid(day_us);
    if !(1..=3_652_059).contains(&ord) {
        return None;
    }
    let (y, mo, d) = ord_to_ymd(ord);
    let secs = rem / 1_000_000;
    let ms = rem % 1_000_000 / 1000;
    Some(format!(
        "{y:04}-{mo:02}-{d:02}T{:02}:{:02}:{:02}.{ms:03}Z",
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn int_parsing_matches_python() {
        for (s, want) in [
            (" 5 ", Some(5)),
            ("5_0", Some(50)),
            ("٥", Some(5)),
            ("+5", Some(5)),
            ("-0", Some(0)),
            ("0x5", None),
            ("1e3", None),
            ("5.0", None),
            ("", None),
            ("_5", None),
            ("5__0", None),
            ("\u{2003} 7\u{2003}", Some(7)),
            ("１", Some(1)),
            ("5_", None),
            ("\u{1c}5", None),
        ] {
            assert_eq!(int_from_str(s), want, "{s:?}");
        }
    }

    #[test]
    fn created_at_matches_python() {
        let n = |s: &str| normalize_created_at(Some(&json!(s)));
        assert_eq!(n("2026-08-01T10:00:00.123000"), "2026-08-01T10:00:00.123Z");
        assert_eq!(n("2026-08-01T10:00:00.123Z"), "2026-08-01T10:00:00.123Z");
        assert_eq!(n("2026-08-01"), "2026-08-01T00:00:00.000Z");
        assert_eq!(n("20260801T100000"), "2026-08-01T10:00:00.000Z");
        assert_eq!(n("2026-08-01T10:00:00+05:30"), "2026-08-01T04:30:00.000Z");
        assert_eq!(n("2026-W31-1"), "2026-07-27T00:00:00.000Z");
        assert_eq!(n("2026-08-01T10:00:00.9999999"), "2026-08-01T10:00:00.999Z");
        assert_eq!(n("2026-08-01T24:00:00"), "2026-08-01T24:00:00");
        assert_eq!(n("garbage"), "garbage");
        assert_eq!(normalize_created_at(Some(&json!(5))), "");
    }
}
