#[test]
fn check_subtasks_accepts_a_serial_pair_and_rejects_bad_graphs() {
    let part = |key: &str, deps: &[&str]| brief::PlanSubtask {
        key: key.into(),
        title: key.to_uppercase(),
        request: format!("do {key}"),
        depends_on: deps.iter().map(|d| d.to_string()).collect(),
        paths: vec![],
        ..Default::default()
    };
    assert!(check_subtasks(&[part("a", &[]), part("b", &["a"])]).is_ok());
    assert!(check_subtasks(&[part("a", &[])]).is_err());
    assert!(
        check_subtasks(&[part("a", &["b"]), part("b", &["a"])])
            .unwrap_err()
            .contains("cycle")
    );
    assert!(
        check_subtasks(&[part("a", &[]), part("b", &["z"])])
            .unwrap_err()
            .contains("unknown key")
    );
    assert!(
        check_subtasks(&[part("a", &[]), part("a", &[])])
            .unwrap_err()
            .contains("duplicate")
    );
    let many: Vec<_> = (0..=MAX_SUBTASKS)
        .map(|i| part(&i.to_string(), &[]))
        .collect();
    assert!(check_subtasks(&many).is_err());
}

#[test]
fn a_subtask_request_carries_the_whole_and_what_was_decided() {
    let mut parent = task_with_status(TaskStatus::Running);
    parent.decisions = vec!["Owner: Which default? -> dark".into(), "Planner: x".into()];
    let text = subtask_request(&parent, "  write the toggle ");
    assert!(
        text.starts_with("write the toggle\n\nThis is one part of a larger task, \"Do thing\"")
    );
    assert!(text.contains("- Owner: Which default? -> dark"));
    assert!(!text.contains("Planner: x"));
}

fn pathed(key: &str, deps: &[&str], paths: &[&str]) -> brief::PlanSubtask {
    brief::PlanSubtask {
        key: key.into(),
        title: key.to_uppercase(),
        request: format!("do {key}"),
        depends_on: deps.iter().map(|d| d.to_string()).collect(),
        paths: paths.iter().map(|p| p.to_string()).collect(),
        ..Default::default()
    }
}

#[test]
fn siblings_with_nested_paths_are_serialised_but_not_a_shared_string_prefix() {
    let (out, decisions) = serialise_overlapping(&[
        pathed("a", &[], &["src/a"]),
        pathed("b", &[], &["src/a/b.rs"]),
    ]);
    assert_eq!(out[1].depends_on, vec!["a".to_string()]);
    assert_eq!(
        decisions,
        vec!["Serialised b after a: both touch src/a".to_string()]
    );

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

#[test]
fn overlapping_parts_with_paths_form_one_relay_and_a_disjoint_part_stays_apart() {
    let (mut parts, _) = serialise_overlapping(&[
        pathed("a", &[], &["src"]),
        pathed("b", &[], &["src/b.rs"]),
        pathed("c", &[], &["src"]),
        pathed("d", &[], &["docs"]),
    ]);
    let (links, decisions) = relay_links(&mut parts);
    assert_eq!(links, vec![None, Some(0), Some(1), None]);
    assert_eq!(decisions.len(), 2);
    assert!(parts[2].depends_on.contains(&"b".to_string()));
}

#[test]
fn a_part_without_paths_or_with_a_foreign_dependency_is_not_relayed() {
    let (mut parts, _) = serialise_overlapping(&[pathed("a", &[], &[]), pathed("b", &[], &[])]);
    assert_eq!(relay_links(&mut parts).0, vec![None, None]);

    // c overlaps b but also builds on d, which is outside b's chain.
    let (mut parts, _) = serialise_overlapping(&[
        pathed("b", &[], &["src"]),
        pathed("d", &[], &["docs"]),
        pathed("c", &["d"], &["src"]),
    ]);
    assert_eq!(relay_links(&mut parts).0, vec![None, None, None]);
}

#[test]
fn a_part_that_builds_on_a_middle_link_waits_for_the_chains_last_link() {
    let (mut parts, _) = serialise_overlapping(&[
        pathed("a", &[], &["src"]),
        pathed("b", &[], &["src"]),
        pathed("x", &["a"], &["docs"]),
    ]);
    let (links, _) = relay_links(&mut parts);
    assert_eq!(links, vec![None, Some(0), None]);
    assert!(parts[2].depends_on.contains(&"b".to_string()));
}

#[test]
fn normalized_dependencies_are_checked_before_contracting_the_merged_group() {
    let a = pathed("a", &[], &[]);
    let mut b = pathed("b", &[], &["b.txt"]);
    b.goal = "Create b.txt".to_string();
    b.criteria = vec!["b.txt exists".to_string()];
    b.verify = vec!["test -f b.txt".to_string()];
    let c = pathed("c", &[], &[]);
    let parts = vec![a, b, c];
    assert_eq!(
        plan_gate::merge_parts(&parts),
        plan_gate::MergePlan {
            separate: vec![1],
            merged: vec![0, 2],
        }
    );

    let plan = prepare_subtasks(&parts);
    assert_eq!(plan.parts[1].depends_on, vec!["a".to_string()]);
    assert_eq!(
        plan.parts[2].depends_on,
        vec!["a".to_string(), "b".to_string()]
    );
    assert!(check_subtasks(&plan.parts).is_ok());
    // Contracting a and c around b would produce group -> b -> group.
    // The policy instead keeps their acyclic order in one checklist.
    assert_eq!(
        plan.merge,
        plan_gate::MergePlan {
            separate: vec![],
            merged: vec![0, 1, 2],
        }
    );
    assert_eq!(plan.links, vec![None, None, None]);
}
