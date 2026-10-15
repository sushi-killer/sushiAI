use super::*;

#[test]
fn a_two_child_graph_runs_the_shared_gate_once_at_the_parent() {
    let run = gate_daemon(false, None);
    let gate: &[&str] = &["npm run ci"];
    let (parent, children) = gate_graph(&run, [gate, gate, gate]);

    assert_eq!(parent["status"], "done", "task JSON: {parent}");
    for c in &children {
        assert_eq!(c["status"], "done", "task JSON: {c}");
        assert_eq!(
            deferred_lines(c),
            ["Orchestrator: Deferred to parent: npm run ci"],
            "{c}"
        );
        assert!(
            c["finalVerify"].to_string().contains("npm run ci"),
            "the stored finalVerify is kept: {c}"
        );
    }
    let all: Vec<&serde_json::Value> = children.iter().chain([&parent]).collect();
    assert_eq!(npm_runs_in(&run, &all), [0, 0, 1]);
    finish_gate(run, &parent, &children);
}

#[test]
fn a_carried_child_does_not_run_a_parent_deferred_final_at_landing() {
    let run = gate_daemon_env(false, None, &[("BARRIER", "1")]);
    let gate: &[&str] = &["npm run ci"];
    // Hold One until Two is past its post-agent rebase, then hold Two until
    // One has landed so the carry happens under the landing lock.
    let parent = gate_graph_with(
        &run,
        [gate, gate, gate],
        [
            r#"test -f one.txt && i=0 && while [ ! -f "$PARTS_LOG/two-verifying" ]; do [ "$i" -lt 1500 ] || exit 1; sleep 0.2; i=$((i+1)); done"#,
            r#"test -f two.txt && touch "$PARTS_LOG/two-verifying" && i=0 && while [ ! -f "$PARTS_LOG/one-landed" ]; do [ "$i" -lt 1500 ] || exit 1; sleep 0.2; i=$((i+1)); done"#,
        ],
    );
    let children = children_of(&run.daemon, &parent);
    let one = children.iter().find(|c| c["title"] == "One").unwrap();
    poll_until(&run.daemon, one["id"].as_str().unwrap(), |s| s == "done");
    std::fs::write(run.npm_log.parent().unwrap().join("one-landed"), b"").unwrap();
    let parent = settle(&run.daemon, parent["id"].as_str().unwrap());
    let children = children_of(&run.daemon, &parent);
    assert_eq!(children.len(), 2);

    assert_eq!(parent["status"], "done", "task JSON: {parent}");
    for c in &children {
        assert_eq!(c["status"], "done", "task JSON: {c}");
    }
    let carried = children
        .iter()
        .filter(|c| {
            decisions_of(c)
                .iter()
                .any(|d| d.starts_with("Land: carried the work onto"))
        })
        .count();
    assert_eq!(carried, 1, "one child lands on a moved parent branch");
    let all: Vec<&serde_json::Value> = children.iter().chain([&parent]).collect();
    assert_eq!(npm_runs_in(&run, &all), [0, 0, 1]);
    finish_gate(run, &parent, &children);
}

#[test]
fn gate_at_every_runs_the_gate_in_every_child() {
    let run = gate_daemon(false, Some("every"));
    let gate: &[&str] = &["npm run ci"];
    let (parent, children) = gate_graph(&run, [gate, gate, gate]);

    assert_eq!(parent["status"], "done", "task JSON: {parent}");
    for c in &children {
        assert_eq!(c["status"], "done", "task JSON: {c}");
        assert!(deferred_lines(c).is_empty(), "{c}");
    }
    let runs = npm_runs_in(&run, &children.iter().collect::<Vec<_>>());
    assert_eq!(runs, [1, 1]);
    finish_gate(run, &parent, &children);
}

#[test]
fn a_child_only_final_still_runs_in_the_child() {
    let run = gate_daemon(false, None);
    let (parent, children) = gate_graph(&run, [&[], &["npm run ci"], &[]]);

    assert_eq!(parent["status"], "done", "task JSON: {parent}");
    let one = children.iter().find(|c| c["title"] == "One").unwrap();
    assert!(deferred_lines(one).is_empty(), "{one}");
    assert_eq!(npm_runs_in(&run, &[one]), [1]);
    finish_gate(run, &parent, &children);
}

#[test]
fn a_parent_gate_failure_gets_one_attempt_with_the_failing_tail() {
    let run = gate_daemon(true, None);
    let gate: &[&str] = &["npm run ci"];
    let (parent, children) = gate_graph(&run, [gate, gate, gate]);

    let parent_id = parent["id"].as_str().unwrap();
    let parent = poll_until(&run.daemon, parent_id, |s| {
        matches!(s, "done" | "failed" | "stopped" | "waiting")
    });
    assert_eq!(parent["status"], "done", "task JSON: {parent}");
    assert_eq!(implement_attempts(&parent).len(), 1, "task JSON: {parent}");
    let brief = run_file(&run.daemon, parent_id, "brief.md");
    assert!(brief.contains("npm run ci"), "{brief}");
    assert!(brief.contains("GATE-BROKEN-TAIL"), "{brief}");
    for c in &children {
        assert_eq!(c["status"], "done", "task JSON: {c}");
    }
    finish_gate(run, &parent, &children);
}

#[test]
fn a_parent_skips_a_command_its_child_passed_on_the_same_tree() {
    let run = gate_daemon(false, Some("every"));
    let repo = run.repo.path().to_str().unwrap();
    let parent = run.daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo, "title": "Parent", "goal": "One part",
            "verify": ["test -f one.txt"], "finalVerify": ["npm run ci"], "start": false,
        }),
    );
    let parent_id = parent["id"].as_str().unwrap().to_string();
    run.daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo, "title": "One", "goal": "PART_ONE",
            "verify": ["test -f one.txt"], "finalVerify": ["npm run ci"], "parent": parent_id,
        }),
    );
    let parent = settle(&run.daemon, &parent_id);
    let children = children_of(&run.daemon, &parent);

    assert_eq!(parent["status"], "done", "task JSON: {parent}");
    assert_eq!(children.len(), 1);
    assert!(
        decisions_of(&parent)
            .iter()
            .any(|d| d.contains("already passed on tree")),
        "{parent}"
    );
    let all: Vec<&serde_json::Value> = children.iter().chain([&parent]).collect();
    assert_eq!(npm_runs_in(&run, &all), [1, 0]);
    finish_gate(run, &parent, &children);
}
