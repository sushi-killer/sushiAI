use super::*;

#[test]
fn a_tool_without_a_hint_or_override_is_a_write() {
    let none = BTreeMap::new();
    for name in ["run_query", "query", "search_and_replace", "list_watches"] {
        assert_eq!(classify(name, None, &none), Kind::Write, "{name}");
    }
    assert_eq!(classify("run_query", Some(true), &none), Kind::Read);
}

#[test]
fn a_hint_beats_the_name_and_an_override_beats_both() {
    let mut overrides = BTreeMap::new();
    assert_eq!(classify("send_report", Some(true), &overrides), Kind::Read);
    assert_eq!(classify("read_all", Some(false), &overrides), Kind::Write);
    overrides.insert("read_all".to_string(), "read".to_string());
    assert_eq!(classify("read_all", Some(false), &overrides), Kind::Read);
}
