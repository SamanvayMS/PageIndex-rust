//! Conformance against CPython 3.11 outputs recorded by `gen/pycompat_fixture.py`.

use pi_pycompat::{difflib, pyround, pystr};
use serde_json::Value;

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/pycompat.json")).unwrap()
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap()
}

#[test]
fn sequence_matcher_ratio() {
    for case in fixture()["ratio"].as_array().unwrap() {
        let got = difflib::ratio_str(s(&case[0]), s(&case[1]));
        assert_eq!(
            got,
            case[2].as_f64().unwrap(),
            "ratio({:?}, {:?})",
            case[0],
            case[1]
        );
    }
}

#[test]
fn sequence_matcher_opcodes_without_autojunk() {
    for case in fixture()["opcodes_nojunk"].as_array().unwrap() {
        let a: Vec<char> = s(&case[0]).chars().collect();
        let b: Vec<char> = s(&case[1]).chars().collect();
        let got: Vec<Value> = difflib::SequenceMatcher::new(&a, &b, false)
            .get_opcodes()
            .into_iter()
            .map(|o| {
                let tag = match o.tag {
                    difflib::Tag::Replace => "replace",
                    difflib::Tag::Delete => "delete",
                    difflib::Tag::Insert => "insert",
                    difflib::Tag::Equal => "equal",
                };
                serde_json::json!([tag, o.i1, o.i2, o.j1, o.j2])
            })
            .collect();
        assert_eq!(
            Value::Array(got),
            case[2],
            "opcodes({:?}, {:?})",
            case[0],
            case[1]
        );
    }
}

#[test]
fn close_matches() {
    for case in fixture()["close"].as_array().unwrap() {
        let poss: Vec<&str> = case[1].as_array().unwrap().iter().map(s).collect();
        let got = difflib::get_close_matches(s(&case[0]), &poss, 3, 0.5);
        let want: Vec<&str> = case[2].as_array().unwrap().iter().map(s).collect();
        assert_eq!(got, want, "get_close_matches({:?})", case[0]);
    }
}

#[test]
fn rounding() {
    for case in fixture()["round"].as_array().unwrap() {
        let x = case[0].as_f64().unwrap();
        let nd = case[1].as_i64().unwrap() as i32;
        assert_eq!(
            pyround::round_nd(x, nd),
            case[2].as_f64().unwrap(),
            "round({x}, {nd})"
        );
        assert_eq!(pyround::round0(x), case[3].as_f64().unwrap(), "round({x})");
        assert_eq!(pyround::g6(x), case[4].as_f64().unwrap(), "{x}:.6g");
    }
}

#[test]
fn whitespace() {
    let f = fixture();
    let ws: Vec<char> = s(&f["py_whitespace"]).chars().collect();
    for c in (0u32..0x3001).filter_map(char::from_u32) {
        assert_eq!(
            pystr::is_py_space(c),
            ws.contains(&c),
            "isspace(U+{:04X})",
            c as u32
        );
    }
    for case in f["split"].as_array().unwrap() {
        let want: Vec<&str> = case[1].as_array().unwrap().iter().map(s).collect();
        assert_eq!(pystr::split(s(&case[0])), want);
        assert_eq!(pystr::strip(s(&case[0])), s(&case[2]));
    }
}
