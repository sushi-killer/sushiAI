use crate::policy::settings::Settings;

#[test]
fn pr_on_done_follows_the_repo_entry_then_the_global_switch() {
    let mut s = Settings::default();
    assert!(s.may_open_pr_on_done("/r/a"));
    s.pr_on_done_repos.insert("/r/a".into(), false);
    assert!(!s.may_open_pr_on_done("/r/a"));
    assert!(s.may_open_pr_on_done("/r/b"));
    s.pr_on_done_off = true;
    assert!(!s.may_open_pr_on_done("/r/b"));
    s.pr_on_done_repos.insert("/r/b/".into(), true);
    assert!(s.may_open_pr_on_done("/r/b"));
}

#[test]
fn gate_at_defaults_to_empty_and_an_unlisted_repo_gates_at_the_parent() {
    let mut v = serde_json::to_value(Settings::default()).unwrap();
    v.as_object_mut().unwrap().remove("gateAt");
    let s: Settings = serde_json::from_value(v.clone()).unwrap();
    assert!(s.gate_at.is_empty());
    assert!(s.gates_at_parent("/some/repo"));
    v["gateAt"] = serde_json::json!({"/a/b": "every", "/c/d": "parent"});
    let s: Settings = serde_json::from_value(v).unwrap();
    assert!(!s.gates_at_parent("/a/b"));
    assert!(!s.gates_at_parent("/a/b/"));
    assert!(s.gates_at_parent("/c/d"));
    assert!(s.gates_at_parent("/e/f"));
    assert_eq!(
        serde_json::to_value(&s).unwrap()["gateAt"]["/a/b"],
        serde_json::json!("every")
    );
}
