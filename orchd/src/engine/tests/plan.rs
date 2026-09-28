#[test]
fn check_subtasks_accepts_a_serial_pair_and_rejects_bad_graphs() {
    let part = |key: &str, deps: &[&str]| brief::PlanSubtask {
        key: key.into(),
        title: key.to_uppercase(),
        request: format!("do {key}"),
        depends_on: deps.iter().map(|d| d.to_string()).collect(),
        paths: vec![],
    };
    assert!(check_subtasks(&[part("a", &[]), part("b", &["a"])]).is_ok());
    assert!(check_subtasks(&[part("a", &[])]).is_err());
    assert!(check_subtasks(&[part("a", &["b"]), part("b", &["a"])])
        .unwrap_err()
        .contains("cycle"));
    assert!(check_subtasks(&[part("a", &[]), part("b", &["z"])])
        .unwrap_err()
        .contains("unknown key"));
    assert!(check_subtasks(&[part("a", &[]), part("a", &[])])
        .unwrap_err()
        .contains("duplicate"));
    let many: Vec<_> = (0..=MAX_SUBTASKS)
        .map(|i| part(&i.to_string(), &[]))
        .collect();
    assert!(check_subtasks(&many).is_err());
}

#[test]
fn a_subtask_request_carries_the_whole_and_what_was_decided() {
    let mut parent = task_with_status(TaskStatus::Running);
    parent.decisions = vec!["Owner: Which default? -> dark".into(), "Jev: x".into()];
    let text = subtask_request(&parent, "  write the toggle ");
    assert!(text.starts_with("write the toggle\n\nThis is one part of a larger task, \"Do thing\""));
    assert!(text.contains("- Owner: Which default? -> dark"));
    assert!(!text.contains("Jev: x"));
}

fn pathed(key: &str, deps: &[&str], paths: &[&str]) -> brief::PlanSubtask {
    brief::PlanSubtask {
        key: key.into(),
        title: key.to_uppercase(),
        request: format!("do {key}"),
        depends_on: deps.iter().map(|d| d.to_string()).collect(),
        paths: paths.iter().map(|p| p.to_string()).collect(),
    }
}

#[test]
fn siblings_with_nested_paths_are_serialised_but_not_a_shared_string_prefix() {
    let (out, decisions) =
        serialise_overlapping(&[pathed("a", &[], &["src/a"]), pathed("b", &[], &["src/a/b.rs"])]);
    assert_eq!(out[1].depends_on, vec!["a".to_string()]);
    assert_eq!(decisions, vec!["Serialised b after a: both touch src/a".to_string()]);

    let (out, decisions) =
        serialise_overlapping(&[pathed("a", &[], &["src/a"]), pathed("b", &[], &["src/ab"])]);
    assert!(out[1].depends_on.is_empty());
    assert!(decisions.is_empty());
}

#[test]
fn a_subtask_without_paths_waits_for_every_unordered_earlier_sibling() {
    let (out, decisions) = serialise_overlapping(&[
        pathed("a", &[], &["x"]),
        pathed("b", &[], &["y"]),
        pathed("c", &[], &[]),
    ]);
    assert_eq!(out[2].depends_on, vec!["a".to_string(), "b".to_string()]);
    assert_eq!(decisions.len(), 2);
    assert!(out[1].depends_on.is_empty());
}

#[test]
fn an_existing_order_is_kept_and_no_duplicate_edge_is_added() {
    let (out, decisions) = serialise_overlapping(&[
        pathed("a", &[], &["src"]),
        pathed("b", &["a"], &["src"]),
        pathed("c", &["b"], &["src"]),
    ]);
    assert_eq!(out[1].depends_on, vec!["a".to_string()]);
    assert_eq!(out[2].depends_on, vec!["b".to_string()]);
    assert!(decisions.is_empty());
}
