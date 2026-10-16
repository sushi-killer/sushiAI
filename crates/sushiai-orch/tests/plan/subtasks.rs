use super::*;

#[test]
fn a_split_drafts_children_that_skip_their_own_planner() {
    let (daemon, _scripts, _repo, parent) = run_split(COMPLETE_PART_B);
    assert_eq!(parent["status"], "done", "{parent}");
    let children = children_of(&daemon, &parent);
    assert_eq!(children.len(), 2, "{children:?}");
    for child in &children {
        assert_eq!(child["status"], "done", "{child}");
        assert_eq!(child["plannedByParent"], true, "{child}");
        assert_eq!(plan_attempts(child), 0, "{child}");
        assert!(!child["verify"].as_array().unwrap().is_empty(), "{child}");
        assert!(!child["criteria"].as_array().unwrap().is_empty(), "{child}");
        assert!(!child["goal"].as_str().unwrap().is_empty(), "{child}");
    }
    let a = children.iter().find(|c| c["title"] == "Part A").unwrap();
    assert_eq!(a["finalVerify"], serde_json::json!(["true"]));
    assert_eq!(a["paths"], serde_json::json!(["a.txt"]));
    assert_eq!(a["plannedTier"], "standard");
    assert_eq!(
        a["visualCriteria"],
        serde_json::json!(["a screenshot of a.txt"])
    );

    let costs: Vec<serde_json::Value> =
        std::fs::read_to_string(daemon.data_dir().join("costs.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
    let planned: Vec<&serde_json::Value> = costs.iter().filter(|r| r["stage"] == "plan").collect();
    assert!(!planned.is_empty(), "the parent was planned: {costs:?}");
    assert!(
        planned.iter().all(|r| r["taskId"] == parent["id"]),
        "a child was planned: {planned:?}"
    );
    daemon.shutdown_and_wait();
}

#[test]
fn an_incomplete_part_draft_is_still_planned_by_its_own_planner() {
    let (daemon, _scripts, _repo, parent) = run_split(INCOMPLETE_PART_B);
    assert_eq!(parent["status"], "done", "{parent}");
    let children = children_of(&daemon, &parent);
    assert_eq!(children.len(), 2, "{children:?}");
    let a = children.iter().find(|c| c["title"] == "Part A").unwrap();
    assert_eq!(a["plannedByParent"], true, "{a}");
    assert_eq!(plan_attempts(a), 0, "{a}");
    let b = children.iter().find(|c| c["id"] != a["id"]).unwrap();
    assert_ne!(b["plannedByParent"], true, "{b}");
    assert_eq!(plan_attempts(b), 1, "{b}");
    assert_eq!(b["status"], "done", "{b}");
    daemon.shutdown_and_wait();
}

#[test]
fn a_parent_drafted_part_keeps_its_drafted_checks_and_held_out() {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake-claude.sh", GROUNDED_PART_SCRIPT);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    settings["answerPolicy"] = serde_json::json!(false);
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
    let repo = init_git_repo();
    let parent = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "request": "build the part",
            "start": true,
        }),
    );
    let parent_id = parent["id"].as_str().unwrap().to_string();
    let parent = settle(&daemon, &parent_id);
    let children = children_of(&daemon, &parent);
    assert_eq!(children.len(), 2, "{parent}");
    let a = children.iter().find(|c| c["title"] == "Part A").unwrap();
    assert_eq!(a["plannedByParent"], true, "{a}");
    assert_eq!(plan_attempts(a), 0, "{a}");
    let checks = a["checks"].as_array().unwrap();
    assert_eq!(checks.len(), 1, "the out-of-range check is dropped: {a}");
    assert_eq!(checks[0]["criterion"], 0);
    assert_eq!(checks[0]["run"], "test -f a.txt");
    assert_eq!(a["heldOut"]["run"], "test -s a.txt", "{a}");
    assert_eq!(
        a["briefCheck"]["screenshot"], "mkdir -p artifacts && echo png > artifacts/a.png",
        "{a}"
    );
    daemon.shutdown_and_wait();
}
