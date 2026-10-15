#[test]
fn check_subtasks_accepts_a_serial_pair_and_rejects_bad_graphs() {
    let part = |key: &str, deps: &[&str]| crate::engine::planning::draft::PlanSubtask {
        key: key.into(),
        title: key.to_uppercase(),
        request: format!("do {key}"),
        depends_on: deps.iter().map(|d| d.to_string()).collect(),
        paths: vec![],
        ..Default::default()
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
    parent.fixture_data().decisions =
        vec!["Owner: Which default? -> dark".into(), "Planner: x".into()];
    let text = subtask_request(&parent, "  write the toggle ");
    assert!(text.starts_with("write the toggle\n\nThis is one part of a larger task, \"Do thing\""));
    assert!(text.contains("- Owner: Which default? -> dark"));
    assert!(!text.contains("Planner: x"));
}

fn pathed(key: &str, deps: &[&str], paths: &[&str]) -> crate::engine::planning::draft::PlanSubtask {
    crate::engine::planning::draft::PlanSubtask {
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
        plan_gate::grouping::merge_parts(&parts),
        plan_gate::grouping::MergePlan {
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
        plan_gate::grouping::MergePlan {
            separate: vec![],
            merged: vec![0, 1, 2],
        }
    );
    assert_eq!(plan.links, vec![None, None, None]);
}

#[test]
fn plan_route_counts_unicode_distinct_paths_and_excludes_urls() {
    let settings = Settings::default();
    let variant = Variant::default();
    let mut task = task_with_status(TaskStatus::Drafting);
    let request = "Fix café `src/a.rs` [src/a.rs] 'README.md' https://example.test/a.txt";
    task.fixture_data().request = Some(request.into());
    let (id, line) = variant.plan_route_for(&settings, &task);
    assert_eq!(id, "claude-sonnet");
    assert_eq!(
        line,
        format!(
            "Plan route: claude-sonnet (small request: {} chars <= 1200, 2 paths <= 3, no list)",
            request.chars().count()
        )
    );

    let mut settings = settings;
    settings.small_plan_max_chars = request.chars().count();
    settings.small_plan_max_paths = 2;
    assert_eq!(variant.plan_route_for(&settings, &task).0, "claude-sonnet");
    settings.small_plan_max_chars -= 1;
    assert_eq!(variant.plan_route_for(&settings, &task).0, "claude-opus");
    settings.small_plan_max_chars += 1;
    settings.small_plan_max_paths = 1;
    assert!(variant
        .plan_route_for(&settings, &task)
        .1
        .contains("2 paths > 1"));

    task.fixture_data().request = Some("a.12345678 a.123456789 a.a-b a. a/".into());
    settings.small_plan_max_paths = 3;
    assert!(variant
        .plan_route_for(&settings, &task)
        .1
        .contains("2 paths <= 3"));
}

#[test]
fn plan_route_uses_subtask_paths_only_when_nonempty_and_parented() {
    let settings = Settings::default();
    let variant = Variant::default();
    let mut task = task_with_status(TaskStatus::Drafting);
    task.fixture_data().request = Some("Edit a.rs b.rs c.rs d.rs".into());
    task.fixture_data().parent = Some("parent".into());
    task.fixture_data().paths = vec!["src/a".into(), "src/a".into(), "src/b".into()];
    let (id, line) = variant.plan_route_for(&settings, &task);
    assert_eq!(id, "claude-sonnet");
    assert!(line.contains("2 paths <= 3"), "{line}");
    task.fixture_data().paths.clear();
    assert_eq!(variant.plan_route_for(&settings, &task).0, "claude-opus");
    task.fixture_data().paths = vec!["src/a".into()];
    task.fixture_data().parent = None;
    assert_eq!(variant.plan_route_for(&settings, &task).0, "claude-opus");
}

#[test]
fn plan_route_rejects_two_list_items_but_accepts_nonlists_and_one_item() {
    let settings = Settings::default();
    let variant = Variant::default();
    let mut task = task_with_status(TaskStatus::Drafting);
    for request in [
        " 1. first\n2) second",
        "- first\n * second",
        "+ first\n\t- second",
    ] {
        task.fixture_data().request = Some(request.into());
        let (id, line) = variant.plan_route_for(&settings, &task);
        assert_eq!(id, "claude-opus", "{request}");
        assert!(line.contains("not small: 2 list items >= 2"), "{line}");
    }
    for request in [
        "1. first",
        "1.first\n2.second",
        "- \n* ",
        "a - first\nb - second",
    ] {
        task.fixture_data().request = Some(request.into());
        assert_eq!(
            variant.plan_route_for(&settings, &task).0,
            "claude-sonnet",
            "{request}"
        );
    }
}

#[test]
fn plan_route_honors_off_override_and_missing_standard() {
    let mut settings = Settings::default();
    let mut variant = Variant::default();
    let mut task = task_with_status(TaskStatus::Drafting);
    task.fixture_data().request = Some("Fix a.rs".into());
    settings.small_plan_max_chars = 0;
    let (id, line) = variant.plan_route_for(&settings, &task);
    assert_eq!(id, "claude-opus");
    assert_eq!(
        line,
        "Plan route: claude-opus (small-request check off: smallPlanMaxChars = 0)"
    );
    variant.planner_route = Some("claude-sonnet".into());
    assert_eq!(variant.plan_route_for(&settings, &task).0, "claude-sonnet");
    variant.planner_route = None;
    settings.small_plan_max_chars = 1200;
    for standard in [None, Some(""), Some("not-configured")] {
        settings.tiers.remove(&Tier::Standard);
        if let Some(id) = standard {
            settings.tiers.insert(Tier::Standard, id.into());
        }
        let (id, line) = variant.plan_route_for(&settings, &task);
        assert_eq!(id, "claude-opus");
        assert!(
            line.contains("standard route not configured, fell back to planner"),
            "{line}"
        );
    }
    settings.planner.clear();
    assert_eq!(variant.plan_route_for(&settings, &task).0, "");
    settings
        .tiers
        .insert(Tier::Standard, "claude-sonnet".into());
    assert_eq!(variant.plan_route_for(&settings, &task).0, "");
    settings.planner = "claude-opus".into();
    variant.planner_route = Some("claude-opus".into());
    assert_eq!(variant.plan_route_for(&settings, &task).0, "claude-opus");
}
