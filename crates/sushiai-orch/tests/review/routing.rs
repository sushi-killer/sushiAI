use super::*;

#[test]
fn a_declined_finding_the_review_upholds_is_resolved_and_the_task_commits() {
    let settled = run_decline_task(true);
    assert_eq!(settled["status"], "done", "{settled}");
    let assumption = settled["assumptions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["by"] == "review")
        .unwrap_or_else(|| panic!("no review assumption: {settled}"));
    assert_eq!(assumption["answer"], "finding dropped", "{settled}");
    assert_eq!(assumption["kind"], "review_dispute", "{settled}");
    assert_eq!(assumption["attempt"], 2, "{settled}");
    assert_eq!(assumption["evidence"], "a.rs:1", "{settled}");
    assert!(
        decision_lines(&settled).iter().any(|d| d.starts_with(
            "Orchestrator: review finding F-1 declined by the implementer and upheld by the review; resolved:"
        ) && d.contains("null check missing")),
        "{settled}"
    );
}

#[test]
fn a_declined_finding_the_review_does_not_uphold_stays_blocking() {
    let settled = run_decline_task(false);
    assert_eq!(settled["status"], "failed", "{settled}");
    let lines = decision_lines(&settled);
    assert!(
        lines.iter().any(|d| d.starts_with(
            "Orchestrator: review finding F-1 declined by the implementer but not upheld; it stays open:"
        )),
        "{settled}"
    );
    assert!(
        !lines.iter().any(|d| d.contains("upheld by the review")),
        "{settled}"
    );
    assert!(settled["assumptions"]
        .as_array()
        .map(|a| a.iter().all(|x| x["by"] != "review"))
        .unwrap_or(true));
}

#[test]
fn the_route_turn_chooses_the_areas_the_full_review_is_briefed_with() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-route.sh", FAKE_ROUTE_SCRIPT);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    review_by_claude(&daemon);
    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Route",
            "goal": "Change one function",
            "criteria": [],
            "verify": ["true"],
            "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    let settled = poll_task_status(&daemon, &task_id);
    assert_eq!(settled["status"], "done", "{settled}");
    let brief = std::fs::read_to_string(
        daemon
            .data_dir()
            .join("tasks")
            .join(&task_id)
            .join("helpers/review-1/brief.md"),
    )
    .unwrap();
    assert!(brief.contains("## Review areas"), "{brief}");
    assert!(brief.contains("Code: read the changed logic"), "{brief}");
    assert!(!brief.contains("Docs: check the documentation"), "{brief}");
    assert!(
        decision_lines(&settled)
            .iter()
            .any(|d| d == "Orchestrator: attempt 1 review areas: code (one function)"),
        "{settled}"
    );
    daemon.shutdown_and_wait();
}

#[test]
fn an_unreviewed_subtask_lands_on_its_parent_and_the_parent_completes_unreviewed() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        "#!/bin/sh\nbrief=$(cat)\ncase \"$brief\" in\n'## Route the review'*) exit 0 ;;\n\"## Review\"*) echo '{\"type\":\"result\",\"result\":\"Looks fine to me.\"}' ;;\n*) echo changed > CHANGED_MARKER.txt\necho '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}'\necho '{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":\"```sushi-report\\n{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}\\n```\"}' ;;\nesac\n",
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    review_by_claude(&daemon);
    let repo = init_git_repo();
    let make = |title: &str, parent: Option<&str>, start: bool| {
        let task = daemon.request(
            "task.create",
            json!({
                "repo": repo.path().to_str().unwrap(), "title": title,
                "goal": "Make a trivial change", "criteria": [], "verify": ["true"],
                "parent": parent, "start": start,
            }),
        );
        task["id"].as_str().unwrap().to_string()
    };
    let parent_id = make("Whole", None, false);
    let child_id = make("Part", Some(&parent_id), false);
    daemon.request("task.start", json!({"id": parent_id}));

    let parent = poll_until(&daemon, &parent_id, |s| {
        matches!(s, "done" | "failed" | "stopped" | "waiting")
    });
    let child = daemon.request("task.get", json!({"id": child_id}));
    assert_eq!(child["status"], "done", "{child}");
    assert_eq!(child["unreviewed"], true, "{child}");
    assert!(child["decisions"].to_string().contains("Land: landed on"));
    assert_eq!(parent["status"], "done", "{parent}");
    assert_eq!(parent["unreviewed"], true, "{parent}");
    let decisions = parent["decisions"].to_string();
    assert!(decisions.contains("landed unreviewed"), "{decisions}");
    assert!(!decisions.contains("onto main"), "{decisions}");

    let worktree = parent["worktree"].as_str().unwrap_or_default().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}
