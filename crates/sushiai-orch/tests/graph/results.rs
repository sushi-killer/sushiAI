use super::*;

#[test]
fn a_dependents_first_brief_carries_the_landed_dependencys_results() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        &dependency_results_script("echo dep > dep.txt"),
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    review_off(&daemon);
    let repo = init_git_repo();
    let dep = create_dependency(&daemon, repo.path(), "test -f dep.txt");
    let dep_id = dep["id"].as_str().unwrap().to_string();
    let child = create_dependent(
        &daemon,
        repo.path(),
        &dep_id,
        serde_json::json!({
            "title": "The dependent",
            "goal": "MAKE_CHILD: create child.txt",
            "verify": ["test -f child.txt"],
        }),
    );
    assert_eq!(child["status"], "done", "task JSON: {child}");
    let child_id = child["id"].as_str().unwrap();

    let brief = run_file(&daemon, child_id, "brief.md");
    assert!(brief.contains("## Landed dependencies"), "{brief}");
    assert!(brief.contains("DEP_SUMMARY"), "{brief}");
    assert!(brief.contains("Agent: DEP_DECISION"), "{brief}");
    assert!(brief.contains("DEP_HANDOFF"), "{brief}");
    assert!(brief.contains("dep.txt"), "{brief}");
    assert!(
        brief.find("## Landed dependencies").unwrap() < brief.find("## Report format").unwrap()
    );
    assert!(!run_file(&daemon, &dep_id, "brief.md").contains("## Landed dependencies"));

    let worktrees = [dep["worktree"].clone(), child["worktree"].clone()];
    daemon.shutdown_and_wait();
    for wt in worktrees {
        remove_dir_retrying(wt.as_str().unwrap());
    }
}

#[test]
fn a_plan_brief_carries_the_results_of_a_dependency_that_was_already_done() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        &dependency_results_script("echo dep > dep.txt"),
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    review_off(&daemon);
    let repo = init_git_repo();
    let dep = create_dependency(&daemon, repo.path(), "test -f dep.txt");
    let dep_id = dep["id"].as_str().unwrap().to_string();
    let child = create_dependent(
        &daemon,
        repo.path(),
        &dep_id,
        serde_json::json!({"request": "build on the dependency"}),
    );
    // The plan attempt passed; the drafted task waits for the owner.
    assert_eq!(child["attempts"][0]["stage"], "plan", "task JSON: {child}");
    let brief = plan_file(&daemon, child["id"].as_str().unwrap(), "plan/brief.md");
    assert!(brief.contains("sushi-plan"), "not the plan brief: {brief}");
    assert!(brief.contains("## Landed dependencies"), "{brief}");
    assert!(brief.contains("DEP_SUMMARY"), "{brief}");
    assert!(
        brief.find("## Landed dependencies").unwrap() < brief.find("## Report format").unwrap()
    );

    let worktrees = [dep["worktree"].clone(), child["worktree"].clone()];
    daemon.shutdown_and_wait();
    for wt in worktrees {
        remove_dir_retrying(wt.as_str().unwrap());
    }
}

#[test]
fn a_plan_brief_written_before_its_dependency_lands_has_no_landed_section() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        &dependency_results_script("echo dep > dep.txt"),
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    review_off(&daemon);
    let repo = init_git_repo();
    let dep = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "The dependency",
            "goal": "MAKE_DEP: write the dependency's files",
            "verify": ["test -f dep.txt"],
            "start": false,
        }),
    );
    let dep_id = dep["id"].as_str().unwrap().to_string();
    let child = create_dependent(
        &daemon,
        repo.path(),
        &dep_id,
        serde_json::json!({"request": "build on the dependency"}),
    );
    assert_eq!(child["attempts"][0]["stage"], "plan", "task JSON: {child}");
    let brief = plan_file(&daemon, child["id"].as_str().unwrap(), "plan/brief.md");
    assert!(brief.contains("sushi-plan"), "not the plan brief: {brief}");
    assert!(!brief.contains("## Landed dependencies"), "{brief}");

    let worktrees = [dep["worktree"].clone(), child["worktree"].clone()];
    daemon.shutdown_and_wait();
    for wt in worktrees {
        if let Some(wt) = wt.as_str() {
            remove_dir_retrying(wt);
        }
    }
}

#[test]
fn a_dependency_with_many_changed_files_lists_twenty_and_counts_the_rest() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        &dependency_results_script(
            "for i in 01 02 03 04 05 06 07 08 09 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25; do echo x > f$i.txt; done",
        ),
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    review_off(&daemon);
    let repo = init_git_repo();
    let dep = create_dependency(&daemon, repo.path(), "test -f f25.txt");
    let dep_id = dep["id"].as_str().unwrap().to_string();
    let child = create_dependent(
        &daemon,
        repo.path(),
        &dep_id,
        serde_json::json!({
            "title": "The dependent",
            "goal": "MAKE_CHILD: create child.txt",
            "verify": ["test -f child.txt"],
        }),
    );
    assert_eq!(child["status"], "done", "task JSON: {child}");
    let brief = run_file(&daemon, child["id"].as_str().unwrap(), "brief.md");
    let listed = (1..=25)
        .filter(|i| brief.contains(&format!("f{i:02}.txt")))
        .count();
    assert_eq!(listed, 20, "{brief}");
    assert!(brief.contains("+5 more"), "{brief}");

    let worktrees = [dep["worktree"].clone(), child["worktree"].clone()];
    daemon.shutdown_and_wait();
    for wt in worktrees {
        remove_dir_retrying(wt.as_str().unwrap());
    }
}

#[test]
fn a_child_whose_paths_a_landed_dependency_touched_gets_one_refresh() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-claude.sh", REFRESH_SCRIPT);
    let log = scripts_dir.path().join("calls.log");
    std::fs::write(&log, "").unwrap();
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("CALLS_LOG", log.to_str().unwrap()),
    ]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    settings["answerPolicy"] = serde_json::json!(false);
    settings["briefCheckRoute"] = serde_json::json!("claude-sonnet");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
    let repo = init_git_repo();
    let parent = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "request": "build all parts",
            "start": true,
        }),
    );
    let parent_id = parent["id"].as_str().unwrap().to_string();
    let parent = settle(&daemon, &parent_id);
    assert_eq!(parent["status"], "done", "task JSON: {parent}");
    let tasks = daemon.request("task.list", serde_json::json!({"repo": parent["repo"]}));
    let children: Vec<&serde_json::Value> = tasks
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["parent"] == parent_id.as_str())
        .collect();
    let find = |title: &str| *children.iter().find(|t| t["title"] == title).unwrap();
    let (a, b, c) = (find("Part A"), find("Part B"), find("Part C"));
    for t in [a, b, c] {
        assert_eq!(t["status"], "done", "task JSON: {t}");
        assert_eq!(t["plannedByParent"], true, "task JSON: {t}");
    }
    assert_eq!(
        std::fs::read_to_string(&log).unwrap().lines().count(),
        1,
        "exactly one refresh run"
    );

    let costs: Vec<serde_json::Value> =
        std::fs::read_to_string(daemon.data_dir().join("costs.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
    let refreshed = |t: &serde_json::Value| {
        costs
            .iter()
            .filter(|r| r["stage"] == "refresh" && r["taskId"] == t["id"])
            .count()
    };
    assert_eq!(refreshed(b), 1, "{costs:?}");
    assert_eq!(refreshed(c), 0, "{costs:?}");
    assert_eq!(refreshed(a), 0, "{costs:?}");
    let has_refresh_dir = |t: &serde_json::Value| {
        !sushiai_orch::store::Store::new(daemon.data_dir())
            .unwrap()
            .helper_run_dirs(t["id"].as_str().unwrap(), "refresh", None)
            .is_empty()
    };
    assert!(has_refresh_dir(b));
    assert!(!has_refresh_dir(c));
    // The refresh changed only the paths and said so, once.
    assert_eq!(
        b["paths"],
        serde_json::json!(["src/shared.txt", "src/extra.txt"]),
        "{b}"
    );
    assert_eq!(b["verify"], serde_json::json!(["test -f b.txt"]));
    let lines = b["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d.as_str().unwrap_or("").starts_with("Refresh:"))
        .count();
    assert_eq!(lines, 1, "{b}");

    daemon.shutdown_and_wait();
    for t in [&parent, a, b, c] {
        remove_dir_retrying(t["worktree"].as_str().unwrap());
    }
}
