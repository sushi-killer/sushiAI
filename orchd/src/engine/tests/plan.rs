#[test]
fn check_subtasks_accepts_a_serial_pair_and_rejects_bad_graphs() {
    let part = |key: &str, deps: &[&str]| brief::PlanSubtask {
        key: key.into(),
        title: key.to_uppercase(),
        request: format!("do {key}"),
        depends_on: deps.iter().map(|d| d.to_string()).collect(),
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
