use super::*;

#[test]
fn a_land_task_carries_its_work_onto_an_amended_base_and_lands() {
    let (daemon, _scripts) = daemon();
    let repo = repo_on_work_branch();
    let root = repo.path();
    let task = create(&daemon, root, "one", "test -f one.txt", true);
    let id = task["id"].as_str().unwrap().to_string();
    let old_base = task["baseSha"].as_str().unwrap().to_string();
    // The base tip is amended after the task recorded it.
    std::fs::write(root.join("amended.txt"), "amended\n").unwrap();
    git_out(root, &["add", "amended.txt"]);
    git_out(root, &["commit", "-q", "--amend", "-m", "amended tip"]);
    let amended = git_out(root, &["rev-parse", "HEAD"]);
    daemon.request("task.start", serde_json::json!({"id": id}));
    let task = until_done(&daemon, &id);
    assert_eq!(task["status"], "done", "{task}");
    for attempt in task["attempts"].as_array().unwrap() {
        assert!(
            !attempt.to_string().contains("was rewritten, not moved"),
            "{task}"
        );
    }
    assert_eq!(task["landedSha"], git_out(root, &["rev-parse", "work"]));
    assert_eq!(commits(root, "work"), vec!["Add one", "amended tip"]);
    let ancestor = std::process::Command::new("git")
        .args(["merge-base", "--is-ancestor", &old_base, "work"])
        .current_dir(root)
        .status()
        .unwrap();
    assert!(!ancestor.success());
    let d = decisions(&task);
    assert!(d.contains("rewritten") && d.contains(&amended[..8]), "{d}");
    daemon.shutdown_and_wait();
}

#[test]
fn an_amended_base_that_conflicts_fails_an_attempt_and_lands_after_the_next() {
    let (daemon, _scripts) = daemon();
    let repo = repo_on_work_branch();
    let root = repo.path();
    let verify = "grep -q b shared.txt && ! grep -q '<<<<<<<' shared.txt";
    let task = create(&daemon, root, "shared", verify, true);
    let id = task["id"].as_str().unwrap().to_string();
    std::fs::write(root.join("shared.txt"), "a\n").unwrap();
    git_out(root, &["add", "shared.txt"]);
    git_out(root, &["commit", "-q", "--amend", "-m", "amended tip"]);
    let amended = git_out(root, &["rev-parse", "work"]);
    daemon.request("task.start", serde_json::json!({"id": id}));
    let task = until_done(&daemon, &id);
    assert_eq!(task["status"], "done", "{task}");
    let d = decisions(&task);
    assert!(d.contains("rewritten") && d.contains(&amended[..8]), "{d}");
    let attempts = task["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2, "{task}");
    assert_eq!(attempts[0]["status"], "failed");
    assert!(
        attempts[0]["failure"]["detail"]
            .as_str()
            .unwrap()
            .contains("These files conflict: shared.txt"),
        "{task}"
    );
    assert_eq!(attempts[0]["failure"]["kind"], "conflict", "{task}");
    assert!(
        attempts[1]["reason"].as_str().unwrap().starts_with("tier "),
        "{task}"
    );
    let brief = std::fs::read_to_string(
        daemon
            .data_dir()
            .join("tasks")
            .join(&id)
            .join("runs")
            .join(attempts[1]["n"].as_u64().unwrap().to_string())
            .join("brief.md"),
    )
    .unwrap();
    assert!(
        brief.contains("## Base moved") && brief.contains("shared.txt"),
        "{brief}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("shared.txt")).unwrap(),
        "a\nb\n"
    );
    assert_eq!(task["landedSha"], git_out(root, &["rev-parse", "work"]));
    daemon.shutdown_and_wait();
}

#[test]
fn a_branch_that_moves_during_the_final_checks_is_carried_again_without_a_new_attempt() {
    let (daemon, _scripts) = daemon();
    let repo = repo_on_work_branch();
    let root = repo.path();
    let scratch = tempfile::tempdir().unwrap();
    let id = land_race_task(
        &daemon,
        root,
        scratch.path(),
        serde_json::json!({"title":"Add one","goal":"FILE_one: write the file","verify":["test -f one.txt"]}),
        ("owner.txt", "owner\\n"),
    );
    let done = until_done(&daemon, &id);
    assert_eq!(done["status"], "done", "{done}");
    let implement: Vec<_> = done["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["stage"] == "implement")
        .collect();
    assert_eq!(implement.len(), 1, "{done}");
    assert!(implement[0]["failure"].is_null(), "{done}");
    let d = decisions(&done);
    assert!(
        d.contains("moved while landing; carrying the work onto it again"),
        "{d}"
    );
    assert_eq!(done["landedSha"], git_out(root, &["rev-parse", "work"]));
    let log = commits(root, "work");
    assert_eq!(
        log[..4],
        [
            "Add one",
            "owner commit two",
            "owner commit one",
            "earlier landing"
        ],
        "{log:?}"
    );
    assert!(root.join("one.txt").exists() && root.join("owner.txt").exists());
    assert_eq!(git_out(root, &["status", "--porcelain", "-uno"]), "");
    daemon.shutdown_and_wait();
}

#[test]
fn a_branch_that_moves_during_the_final_checks_onto_a_conflict_still_fails_an_attempt() {
    let (daemon, _scripts) = daemon();
    let repo = repo_on_work_branch();
    let root = repo.path();
    let scratch = tempfile::tempdir().unwrap();
    let verify = "grep -q b shared.txt && ! grep -q '<<<<<<<' shared.txt";
    let id = land_race_task(
        &daemon,
        root,
        scratch.path(),
        serde_json::json!({"title":"Add shared","goal":"FILE_shared: write the file","verify":[verify]}),
        ("shared.txt", "a\\n"),
    );
    let done = until_done(&daemon, &id);
    assert_eq!(done["status"], "done", "{done}");
    let attempts = done["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2, "{done}");
    assert_eq!(attempts[0]["failure"]["kind"], "conflict", "{done}");
    assert!(
        attempts[0]["failure"]["detail"]
            .as_str()
            .unwrap()
            .contains("These files conflict: shared.txt"),
        "{done}"
    );
    assert!(decisions(&done).contains("moved while landing"), "{done}");
    assert_eq!(done["landedSha"], git_out(root, &["rev-parse", "work"]));
    daemon.shutdown_and_wait();
}
