use super::*;

#[test]
fn a_task_started_from_a_landed_done_tasks_branch_lands_on_its_base() {
    let (daemon, _scripts) = daemon();
    let repo = repo_on_work_branch();
    let root = repo.path();
    let first = create(&daemon, root, "first", "test -f first.txt", true);
    let first_id = first["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": first_id}));
    let first = until_done(&daemon, &first_id);
    let first_branch = first["branch"].as_str().unwrap();
    let first_branch_head = git_out(root, &["rev-parse", first_branch]);

    let next = create_from(&daemon, root, "next", first_branch, true);
    let next_id = next["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": next_id}));
    let next = until_done(&daemon, &next_id);
    assert_eq!(
        next["landedSha"],
        git_out(root, &["rev-parse", "work"]),
        "{next}"
    );
    assert_eq!(
        git_out(root, &["rev-parse", first_branch]),
        first_branch_head
    );
    assert!(root.join("first.txt").exists() && root.join("next.txt").exists());
    let lines = decisions(&next);
    assert!(
        lines.contains("Add first") && lines.contains(first_branch) && lines.contains("work"),
        "{lines}"
    );
    daemon.shutdown_and_wait();
}

#[test]
fn a_task_started_from_an_unlanded_done_tasks_branch_lands_both_on_its_base() {
    let (daemon, _scripts) = daemon();
    let repo = repo_on_work_branch();
    let root = repo.path();
    let first = create(&daemon, root, "first", "test -f first.txt", false);
    let first_id = first["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": first_id}));
    let first = until_done(&daemon, &first_id);
    let first_branch = first["branch"].as_str().unwrap();
    let first_branch_head = git_out(root, &["rev-parse", first_branch]);

    let next = create_from(&daemon, root, "next", first_branch, true);
    let next_id = next["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": next_id}));
    let next = until_done(&daemon, &next_id);
    let landed = git_out(root, &["rev-parse", "work"]);
    assert_eq!(next["landedSha"], landed, "{next}");
    assert_eq!(
        git_out(root, &["rev-parse", first_branch]),
        first_branch_head
    );
    assert!(root.join("first.txt").exists() && root.join("next.txt").exists());
    let first = daemon.request("task.get", serde_json::json!({"id": first_id}));
    assert_eq!(first["landedSha"], landed, "{first}");
    assert!(decisions(&first).contains("Add next"), "{first}");
    daemon.shutdown_and_wait();
}

#[test]
fn a_done_tasks_branch_whose_base_is_refused_is_never_landed_on() {
    let (daemon, _scripts) = daemon();
    let repo = init_git_repo();
    let root = repo.path();
    let default = git_out(root, &["rev-parse", "--abbrev-ref", "HEAD"]);
    let first = create_from(&daemon, root, "first", &default, false);
    let first_id = first["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": first_id}));
    let first = until_done(&daemon, &first_id);
    let first_branch = first["branch"].as_str().unwrap();
    let first_branch_head = git_out(root, &["rev-parse", first_branch]);

    let next = create_from(&daemon, root, "next", first_branch, true);
    let next_id = next["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": next_id}));
    let next = until_done(&daemon, &next_id);
    assert!(next.get("landedSha").is_none(), "{next}");
    assert_eq!(next["status"], "done", "{next}");
    assert!(
        decisions(&next).contains("refused") && decisions(&next).contains(&default),
        "{next}"
    );
    assert_eq!(
        git_out(root, &["rev-parse", first_branch]),
        first_branch_head
    );
    let next_branch = next["branch"].as_str().unwrap();
    assert_ne!(git_out(root, &["rev-parse", next_branch]), next["baseSha"]);
    assert_eq!(
        git_out(root, &["show", &format!("{next_branch}:next.txt")]),
        "next"
    );
    daemon.shutdown_and_wait();
}

#[test]
fn task_land_on_a_done_tasks_branch_lands_on_its_base() {
    let (daemon, _scripts) = daemon();
    let repo = repo_on_work_branch();
    let root = repo.path();
    let first = create(&daemon, root, "first", "test -f first.txt", false);
    let first_id = first["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": first_id}));
    let first = until_done(&daemon, &first_id);
    let first_branch = first["branch"].as_str().unwrap();
    let first_branch_head = git_out(root, &["rev-parse", first_branch]);

    let next = create_from(&daemon, root, "next", first_branch, false);
    let next_id = next["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": next_id}));
    until_done(&daemon, &next_id);
    daemon.request("task.land", serde_json::json!({"id": next_id}));
    let next = poll_until(&daemon, &next_id, |s| matches!(s, "done" | "failed"));
    let landed = git_out(root, &["rev-parse", "work"]);
    assert_eq!(next["landedSha"], landed, "{next}");
    assert_eq!(
        git_out(root, &["rev-parse", first_branch]),
        first_branch_head
    );
    assert!(root.join("first.txt").exists() && root.join("next.txt").exists());
    let first = daemon.request("task.get", serde_json::json!({"id": first_id}));
    assert_eq!(first["landedSha"], landed, "{first}");
    daemon.shutdown_and_wait();
}

#[test]
fn a_repo_allowed_per_repo_lands_on_its_default_branch_others_are_refused_with_the_way_to_allow_it()
{
    let (daemon, _scripts) = daemon();
    let repo = init_git_repo();
    let root = repo.path();
    let other = init_git_repo();
    let default = git_out(root, &["rev-parse", "--abbrev-ref", "HEAD"]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["landOnDefaultRepos"] = serde_json::json!({ root.to_str().unwrap(): true });
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let task = create(&daemon, root, "one", "test -f one.txt", true);
    let id = task["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": id}));
    let done = until_done(&daemon, &id);
    assert_eq!(done["status"], "done", "{done}");
    assert_eq!(done["landedSha"], git_out(root, &["rev-parse", &default]));

    let task = create(&daemon, other.path(), "two", "test -f two.txt", true);
    let id = task["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": id}));
    let done = until_done(&daemon, &id);
    assert!(done.get("landedSha").is_none(), "{done}");
    let lines = decisions(&done);
    assert!(lines.contains("Land: refused"), "{lines}");
    assert!(lines.contains("Settings > Orchestration"), "{lines}");
    assert!(!lines.contains("settings.landOnDefault"), "{lines}");
    // The explicit Land action gives the same readable reason.
    let err = daemon.request_error("task.land", serde_json::json!({"id": id}));
    assert!(err.contains("Settings > Orchestration"), "{err}");
    daemon.shutdown_and_wait();
}

#[test]
fn landing_on_a_default_branch_that_is_not_checked_out_leaves_the_working_copy_alone() {
    let (daemon, _scripts) = daemon();
    let repo = init_git_repo();
    let root = repo.path();
    let default = git_out(root, &["rev-parse", "--abbrev-ref", "HEAD"]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["landOnDefaultRepos"] = serde_json::json!({ root.to_str().unwrap(): true });
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
    // The owner's working copy sits on another branch with a dirty file.
    git_out(root, &["checkout", "-q", "-b", "other"]);
    std::fs::write(root.join("README.md"), "hello\nmy edit\n").unwrap();
    let other_head = git_out(root, &["rev-parse", "other"]);

    let one = create_on(&daemon, root, "one", &default);
    let one_id = one["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": one_id}));
    let one = until_done(&daemon, &one_id);
    assert_eq!(one["status"], "done", "{one}");
    assert_eq!(one["landedSha"], git_out(root, &["rev-parse", &default]));
    // The default branch moved (fast-forward of the task's commit) ...
    assert_eq!(commits(root, &default)[0], "Add one");
    // ... and the working copy did not: same branch, same head, same edit.
    assert_eq!(
        git_out(root, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "other"
    );
    assert_eq!(git_out(root, &["rev-parse", "other"]), other_head);
    assert!(!root.join("one.txt").exists());
    assert_eq!(
        std::fs::read_to_string(root.join("README.md")).unwrap(),
        "hello\nmy edit\n"
    );
    daemon.shutdown_and_wait();
}

#[test]
fn after_land_commands_run_in_the_main_checkout_and_a_failure_never_unlands() {
    let (daemon, _scripts) = daemon();
    let repo = repo_on_work_branch();
    let root = repo.path();
    let repo_path = root.to_str().unwrap();
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["afterLand"] = serde_json::json!([
        {"repo": repo_path, "run": "cp one.txt built.txt"},
        {"repo": repo_path, "run": "echo boom >&2; exit 3"},
        {"repo": "/somewhere/else", "run": "exit 9"},
    ]);
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
    let task = create(&daemon, root, "one", "test -f one.txt", true);
    let id = task["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": id}));
    let done = until_done(&daemon, &id);
    assert_eq!(done["status"], "done", "{done}");
    let lines = decisions(&done);
    assert!(lines.contains("exited 0"), "{lines}");
    assert!(lines.contains("exited 3: boom"), "{lines}");
    assert!(!lines.contains("exit 9"), "{lines}");
    assert!(root.join("built.txt").exists());
    assert_eq!(done["landedSha"], git_out(root, &["rev-parse", "work"]));
    daemon.shutdown_and_wait();
}
