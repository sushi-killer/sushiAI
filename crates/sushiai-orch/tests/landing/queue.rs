use super::*;

#[test]
fn two_land_tasks_land_one_after_another_as_single_commits() {
    let (daemon, _scripts) = daemon();
    let repo = repo_on_work_branch();
    let root = repo.path();
    let one = create(&daemon, root, "one", "test -f one.txt", true);
    let two = create(&daemon, root, "two", "test -f two.txt", true);
    let (one_id, two_id) = (
        one["id"].as_str().unwrap().to_string(),
        two["id"].as_str().unwrap().to_string(),
    );
    daemon.request("task.start", serde_json::json!({"id": one_id}));
    daemon.request("task.start", serde_json::json!({"id": two_id}));
    let one = until_done(&daemon, &one_id);
    let two = until_done(&daemon, &two_id);
    assert_eq!(one["status"], "done", "{one}");
    assert_eq!(two["status"], "done", "{two}");

    let head = git_out(root, &["rev-parse", "work"]);
    let landed = [
        one["landedSha"].as_str().unwrap(),
        two["landedSha"].as_str().unwrap(),
    ];
    assert_ne!(landed[0], landed[1]);
    assert!(
        landed.contains(&head.as_str()),
        "head {head}, landed {landed:?}"
    );
    let log = commits(root, "work");
    assert_eq!(log.len(), 3, "{log:?}");
    assert!(log.contains(&"Add one".to_string()) && log.contains(&"Add two".to_string()));
    // The checkout of the base branch holds both files.
    assert!(root.join("one.txt").exists() && root.join("two.txt").exists());
    assert_eq!(git_out(root, &["status", "--porcelain", "-uno"]), "");
    assert!(decisions(&one).contains("Land: landed on work"), "{one}");
    daemon.shutdown_and_wait();
}

#[test]
fn a_conflicting_land_task_fails_an_attempt_and_lands_after_the_next_passing_one() {
    let (daemon, _scripts) = daemon();
    let repo = repo_on_work_branch();
    let root = repo.path();
    let verify = "grep -q b shared.txt && ! grep -q '<<<<<<<' shared.txt";
    let second = create(&daemon, root, "shared", verify, true);
    let second_id = second["id"].as_str().unwrap().to_string();
    // The earlier landing puts a different shared.txt on the branch.
    std::fs::write(root.join("shared.txt"), "a\n").unwrap();
    git_out(root, &["add", "shared.txt"]);
    git_out(root, &["commit", "-q", "-m", "earlier landing"]);
    daemon.request("task.start", serde_json::json!({"id": second_id}));
    let second = until_done(&daemon, &second_id);
    assert_eq!(second["status"], "done", "{second}");
    let attempts = second["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2, "{second}");
    assert_eq!(attempts[0]["status"], "failed");
    assert!(
        attempts[0]["failure"]["detail"]
            .as_str()
            .unwrap()
            .contains("These files conflict: shared.txt"),
        "{second}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("shared.txt")).unwrap(),
        "a\nb\n"
    );
    let log = commits(root, "work");
    assert_eq!(log[0], "Add shared", "{log:?}");
    assert_eq!(log[1], "earlier landing");
    assert_eq!(second["landedSha"], git_out(root, &["rev-parse", "work"]));
    daemon.shutdown_and_wait();
}

#[test]
fn a_dirty_base_checkout_makes_the_task_wait_landing_and_is_left_alone() {
    let (daemon, _scripts) = daemon();
    let repo = repo_on_work_branch();
    let root = repo.path();
    std::fs::write(root.join("README.md"), "hello\nmy edit\n").unwrap();
    let before = git_out(root, &["rev-parse", "work"]);
    let task = create(&daemon, root, "one", "test -f one.txt", true);
    let id = task["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": id}));
    let waiting = until_dirty_landing(&daemon, &id);
    assert_eq!(waiting["status"], "landing", "{waiting}");
    assert!(
        decisions(&waiting).contains("uncommitted changes"),
        "{waiting}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("README.md")).unwrap(),
        "hello\nmy edit\n"
    );
    assert_eq!(git_out(root, &["rev-parse", "work"]), before);

    let dirty_since = waiting["dirtySince"].clone();
    assert!(dirty_since.is_i64(), "{waiting}");

    // A retry with the checkout still dirty changes nothing.
    daemon.request("task.start", serde_json::json!({"id": id}));
    // Absence check: this window gives a forbidden landing time to happen.
    holds_for("a dirty landing staying put", ABSENCE_WINDOW, || {
        let t = daemon.request("task.get", serde_json::json!({"id": id}));
        t["status"] == "landing" && t["dirtySince"] == dirty_since
    });
    let still = daemon.request("task.get", serde_json::json!({"id": id}));
    assert_eq!(still["status"], "landing", "{still}");
    assert_eq!(still["dirtySince"], dirty_since, "{still}");
    assert_eq!(
        std::fs::read_to_string(root.join("README.md")).unwrap(),
        "hello\nmy edit\n"
    );

    git_out(root, &["checkout", "--", "README.md"]);
    daemon.request("task.start", serde_json::json!({"id": id}));
    let done = until_done(&daemon, &id);
    assert_eq!(done["status"], "done", "{done}");
    assert_eq!(done["landedSha"], git_out(root, &["rev-parse", "work"]));
    assert!(root.join("one.txt").exists());
    daemon.shutdown_and_wait();
}

#[test]
fn landing_on_the_default_branch_is_refused_unless_land_on_default_is_on() {
    let (daemon, _scripts) = daemon();
    let repo = init_git_repo();
    let root = repo.path();
    let default = git_out(root, &["rev-parse", "--abbrev-ref", "HEAD"]);
    let before = git_out(root, &["rev-parse", "HEAD"]);
    let task = create(&daemon, root, "one", "test -f one.txt", true);
    let id = task["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": id}));
    let done = until_done(&daemon, &id);
    assert_eq!(done["status"], "done", "{done}");
    assert!(done.get("landedSha").is_none(), "{done}");
    assert!(decisions(&done).contains("Land: refused"), "{done}");
    assert_eq!(git_out(root, &["rev-parse", "HEAD"]), before);
    assert!(!root.join("one.txt").exists());
    let branch = done["branch"].as_str().unwrap();
    assert_eq!(commits(root, branch)[0], "Add one");

    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["landOnDefault"] = serde_json::json!(true);
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
    let task = create(&daemon, root, "two", "test -f two.txt", true);
    let id = task["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": id}));
    let done = until_done(&daemon, &id);
    assert_eq!(done["status"], "done", "{done}");
    assert_eq!(done["landedSha"], git_out(root, &["rev-parse", &default]));
    assert!(root.join("two.txt").exists());
    daemon.shutdown_and_wait();
}

#[test]
fn a_task_without_an_explicit_land_lands_on_its_non_default_base() {
    let (daemon, _scripts) = daemon();
    let repo = repo_on_work_branch();
    let root = repo.path();
    let task = create_default_land(&daemon, root, "one", None);
    assert_eq!(task["variant"]["land"], true, "{task}");
    let id = task["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": id}));
    let done = until_done(&daemon, &id);
    assert_eq!(done["status"], "done", "{done}");
    assert_eq!(done["landedSha"], git_out(root, &["rev-parse", "work"]));
    assert!(root.join("one.txt").exists());
    daemon.shutdown_and_wait();
}

#[test]
fn task_land_lands_a_done_unlanded_task_and_source_is_counted() {
    let (daemon, _scripts) = daemon();
    let repo = repo_on_work_branch();
    let root = repo.path();
    let task = create_default_land(&daemon, root, "one", Some(false));
    assert_eq!(task["source"], "cli", "{task}");
    let id = task["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": id}));
    let done = until_done(&daemon, &id);
    assert_eq!(done["status"], "done", "{done}");
    assert!(done.get("landedSha").is_none(), "{done}");
    assert!(!root.join("one.txt").exists());

    assert_eq!(land(&daemon, &id), "null");
    // The task shows `done` the whole time, so wait for the land itself.
    let landed = wait_until("the land to record its sha", || {
        let task = daemon.request("task.get", serde_json::json!({"id": id}));
        task.get("landedSha").is_some().then_some(task)
    });
    assert_eq!(
        landed["landedSha"],
        git_out(root, &["rev-parse", "work"]),
        "{landed}"
    );
    assert!(root.join("one.txt").exists());
    let again = daemon.request_error("task.land", serde_json::json!({"id": id}));
    assert!(again.contains("already landed"), "{again}");

    let ui = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": root.to_str().unwrap(),
            "title": "Add two",
            "goal": "FILE_two: write the file",
            "verify": ["test -f two.txt"],
            "source": "ui",
            "start": false,
        }),
    );
    assert_eq!(ui["source"], "ui");
    let summary = daemon.request("costs.summary", serde_json::json!({}));
    assert_eq!(summary["tasksBySource"]["cli"]["tasks"], 1, "{summary}");
    assert_eq!(summary["tasksBySource"]["ui"]["tasks"], 1, "{summary}");
    daemon.shutdown_and_wait();
}
