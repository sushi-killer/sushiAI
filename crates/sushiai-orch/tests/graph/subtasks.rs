use super::*;

#[test]
fn a_plan_with_two_dependent_subtasks_lands_both_on_the_parent_in_order() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-claude.sh", GRAPH_SCRIPT);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    review_off(&daemon);
    let repo = init_git_repo();
    let parent = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "request": "build both parts",
            "start": true,
        }),
    );
    let parent_id = parent["id"].as_str().unwrap().to_string();

    // The parent's verify needs both files, which are absent on the base and
    // created by the subtasks: kept by rule, no question.
    let parent = poll_until(&daemon, &parent_id, |s| {
        matches!(s, "done" | "failed" | "stopped")
    });
    assert_eq!(parent["status"], "done", "task JSON: {parent}");
    assert!(
        implement_attempts(&parent).is_empty(),
        "a parent implements nothing itself: {parent}"
    );
    let tasks = daemon.request("task.list", serde_json::json!({"repo": parent["repo"]}));
    let children: Vec<&serde_json::Value> = tasks
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["parent"] == parent_id.as_str())
        .collect();
    assert_eq!(children.len(), 2, "tasks: {tasks}");
    let a = children.iter().find(|t| t["title"] == "Part A").unwrap();
    let b = children.iter().find(|t| t["title"] == "Part B").unwrap();
    assert_eq!(a["status"], "done", "task JSON: {a}");
    assert_eq!(b["status"], "done", "task JSON: {b}");
    assert_eq!(b["dependsOn"], serde_json::json!([a["id"]]));
    assert!(
        b["attempts"].as_array().unwrap().last().unwrap()["startedAt"].as_i64()
            >= a["attempts"].as_array().unwrap().last().unwrap()["endedAt"].as_i64(),
        "B started before A landed: {b}"
    );

    // Both commits are on the parent's branch, B's on top of A's.
    let branch = parent["branch"].as_str().unwrap();
    let subjects = git_out(repo.path(), &["log", "--format=%s", "-3", branch]);
    assert_eq!(subjects, "Part B\nPart A\ninit", "log of {branch}");
    assert!(!Path::new(parent["worktree"].as_str().unwrap()).exists());
    for file in ["a.txt", "b.txt"] {
        git_out(
            repo.path(),
            &["cat-file", "-e", &format!("{branch}:{file}")],
        );
    }
    assert!(
        b["decisions"]
            .to_string()
            .contains("Rebase: carried the work onto"),
        "{b}"
    );
    assert_eq!(
        b["baseSha"],
        git_out(repo.path(), &["rev-parse", &format!("{branch}^")]),
        "{b}"
    );
    let total = a["costUsd"].as_f64().unwrap() + b["costUsd"].as_f64().unwrap();
    assert!(
        parent["costUsd"].as_f64().unwrap() >= total - 1e-9,
        "parent cost includes its children: {parent}"
    );

    daemon.shutdown_and_wait();
    for t in [&parent, *a, *b] {
        remove_dir_retrying(t["worktree"].as_str().unwrap());
    }
}

#[test]
fn overlapping_subtask_paths_merge_into_one_task_with_a_checklist() {
    let script_text = GRAPH_SCRIPT
        .replace(r#"\"paths\":[\"a.txt\"]"#, r#"\"paths\":[\"src/a\"]"#)
        .replace(r#"\"paths\":[\"b.txt\"]"#, r#"\"paths\":[\"src/a/b.rs\"]"#)
        .replace(
            "*PART_A*) echo a > a.txt; out",
            "*PART_A*) echo a > a.txt; echo b > b.txt; out",
        );
    assert_ne!(script_text, GRAPH_SCRIPT);
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-claude.sh", &script_text);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    review_off(&daemon);
    let repo = init_git_repo();
    let parent = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "request": "build both parts",
            "start": true,
        }),
    );
    let parent_id = parent["id"].as_str().unwrap().to_string();
    let parent = settle(&daemon, &parent_id);
    assert_eq!(parent["status"], "done", "{parent}");
    let tasks = daemon.request("task.list", serde_json::json!({"repo": parent["repo"]}));
    let children = tasks
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["parent"] == parent_id.as_str())
        .count();
    assert_eq!(children, 0, "{tasks}");
    let checklist = parent["checklist"].as_array().unwrap();
    assert_eq!(checklist.len(), 2, "{parent}");
    assert!(checklist[0]["text"].as_str().unwrap().contains("PART_A"));
    assert!(checklist[1]["text"].as_str().unwrap().contains("PART_B"));
    assert!(checklist.iter().all(|i| i["passes"] == true), "{parent}");
    assert!(
        parent["decisions"]
            .to_string()
            .contains("Plan gate: merged 2 subtasks into one task with a checklist"),
        "{parent}"
    );

    let worktree = parent["worktree"].clone();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree.as_str().unwrap());
}

#[test]
fn overlapping_subtask_paths_add_a_dependency_edge_to_the_created_children() {
    let script_text = GRAPH_SCRIPT
        .replace(
            r#"\"paths\":[\"a.txt\"]"#,
            r#"\"paths\":[\"src/a\"],\"goal\":\"PART_A: create a.txt\",\"criteria\":[\"a.txt exists -- check: test -f a.txt\"],\"verify\":[\"test -f a.txt\"]"#,
        )
        .replace(
            r#"\"paths\":[\"b.txt\"]"#,
            r#"\"paths\":[\"src/a/b.rs\"],\"goal\":\"PART_B: create b.txt next to a.txt\",\"criteria\":[\"both files exist -- check: test -f a.txt && test -f b.txt\"],\"verify\":[\"test -f a.txt && test -f b.txt\"]"#,
        )
        .replace(r#",\"dependsOn\":[\"a\"]"#, "");
    assert_ne!(script_text, GRAPH_SCRIPT);
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-claude.sh", &script_text);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    review_off(&daemon);
    let repo = init_git_repo();
    let parent = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "request": "build both parts",
            "start": true,
        }),
    );
    let parent_id = parent["id"].as_str().unwrap().to_string();
    let parent = settle(&daemon, &parent_id);
    assert_eq!(parent["status"], "done", "{parent}");
    let tasks = daemon.request("task.list", serde_json::json!({"repo": parent["repo"]}));
    let children: Vec<&serde_json::Value> = tasks
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["parent"] == parent_id.as_str())
        .collect();
    assert_eq!(children.len(), 2, "{tasks}");
    let a = children.iter().find(|t| t["title"] == "Part A").unwrap();
    let b = children.iter().find(|t| t["title"] == "Part B").unwrap();
    for child in [*a, *b] {
        assert_eq!(child["status"], "done", "{child}");
        assert_eq!(child["plannedByParent"], true, "{child}");
    }
    assert_eq!(b["relayOf"], a["id"], "{b}");
    assert_eq!(b["dependsOn"], serde_json::json!([a["id"]]), "{b}");
    assert_eq!(b["paths"], serde_json::json!(["src/a/b.rs"]));
    assert!(
        parent["decisions"]
            .to_string()
            .contains("Serialised b after a: both touch src/a"),
        "{parent}"
    );

    daemon.shutdown_and_wait();
    for t in [&parent, *a, *b] {
        remove_dir_retrying(t["worktree"].as_str().unwrap());
    }
}

#[test]
fn a_split_that_cannot_create_every_part_creates_none_and_runs_as_one_task() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-claude.sh", GRAPH_SCRIPT);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    review_off(&daemon);
    let repo = init_git_repo();
    // Part B's worktree path is already taken, so creating Part B fails
    // after Part A was created. A relative override keeps the paths known.
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["worktreeRoot"] = serde_json::json!(".wt");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
    let parent_dir = repo.path().join(".wt");
    let blocked = parent_dir.join("task-part-b");
    std::fs::create_dir_all(&blocked).unwrap();
    std::fs::write(blocked.join("occupied.txt"), "x\n").unwrap();
    let part_a_wt = parent_dir.join("task-part-a");

    let parent = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "request": "build both parts",
            "start": false,
        }),
    );
    let parent_id = parent["id"].as_str().unwrap().to_string();
    let parent = settle(&daemon, &parent_id);
    assert_eq!(parent["status"], "stopped", "task JSON: {parent}");
    assert!(
        parent["decisions"]
            .to_string()
            .contains("could not create the subtasks"),
        "task JSON: {parent}"
    );
    let tasks = daemon.request("task.list", serde_json::json!({"repo": parent["repo"]}));
    let tasks = tasks.as_array().unwrap();
    assert_eq!(tasks.len(), 1, "only the parent is left: {tasks:?}");
    assert!(tasks[0]["parent"].is_null());
    let branches = git_out(repo.path(), &["branch", "--list", "task/part-*"]);
    assert!(branches.is_empty(), "left-over branches: {branches}");
    assert!(!part_a_wt.exists(), "Part A's worktree was left behind");

    daemon.shutdown_and_wait();
    remove_dir_retrying(&blocked);
    remove_dir_retrying(parent["worktree"].as_str().unwrap());
}

#[test]
fn two_independent_children_run_concurrently_and_both_land() {
    let scripts_dir = tempfile::tempdir().unwrap();
    // A longer part, so a loaded machine still overlaps the two runs.
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        &PARTS_SCRIPT.replace("sleep 1;", "sleep 8;"),
    );
    let log = tempfile::tempdir().unwrap();
    // Each child waits for its peer to be live, up to one deadline, so a slow
    // start of the second child cannot make the runs miss each other.
    let peer_wait = (deadline().as_secs() * 10).to_string();
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("PARTS_LOG", log.path().to_str().unwrap()),
        ("PEER_WAIT", &peer_wait),
    ]);
    review_off(&daemon);
    let repo = init_git_repo();
    let (parent_id, [one, two]) = hand_built_graph(
        &daemon,
        repo.path(),
        "test -f one.txt && test -f two.txt",
        ["test -f one.txt", "test -f two.txt"],
    );

    let parent = settle(&daemon, &parent_id);
    assert_eq!(parent["status"], "done", "task JSON: {parent}");
    let children: Vec<serde_json::Value> = [&one, &two]
        .iter()
        .map(|id| daemon.request("task.get", serde_json::json!({"id": id})))
        .collect();
    for t in &children {
        assert_eq!(t["status"], "done", "task JSON: {t}");
        assert!(
            t["decisions"].to_string().contains("Land: landed on"),
            "{t}"
        );
    }
    let counts = std::fs::read_to_string(log.path().join("counts")).unwrap();
    assert!(
        counts.lines().any(|c| c == "2"),
        "the two children never ran at the same time: {counts}"
    );
    let branch = parent["branch"].as_str().unwrap();
    let subjects = git_out(repo.path(), &["log", "--format=%s", branch]);
    assert!(
        subjects.contains("One") && subjects.contains("Two"),
        "{subjects}"
    );

    daemon.shutdown_and_wait();
    for t in children.iter().chain([&parent]) {
        remove_dir_retrying(t["worktree"].as_str().unwrap());
    }
}

#[test]
fn a_child_whose_rebase_conflicts_is_retried_by_the_agent_not_dropped() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-claude.sh", PARTS_SCRIPT);
    let log = tempfile::tempdir().unwrap();
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("PARTS_LOG", log.path().to_str().unwrap()),
        ("SHARED", "1"),
    ]);
    review_off(&daemon);
    cheap_route_on_claude(&daemon);
    let repo = init_git_repo();
    let clean = "! grep -q '<<<<' shared.txt";
    let (parent_id, ids) = hand_built_graph(
        &daemon,
        repo.path(),
        "grep -q one shared.txt && grep -q two shared.txt",
        [
            &format!("grep -q one shared.txt && {clean}"),
            &format!("grep -q two shared.txt && {clean}"),
        ],
    );

    let parent = settle(&daemon, &parent_id);
    assert_eq!(parent["status"], "done", "task JSON: {parent}");
    let children: Vec<serde_json::Value> = ids
        .iter()
        .map(|id| daemon.request("task.get", serde_json::json!({"id": id})))
        .collect();
    for t in &children {
        assert_eq!(t["status"], "done", "task JSON: {t}");
    }
    let retried: Vec<&serde_json::Value> = children
        .iter()
        .filter(|t| implement_attempts(t).len() >= 2)
        .collect();
    assert_eq!(
        retried.len(),
        1,
        "exactly the second to land retries: {children:?}"
    );
    let first = &implement_attempts(retried[0])[0];
    assert_eq!(first["status"], "failed", "{first}");
    assert!(
        first["failure"]["detail"]
            .as_str()
            .unwrap()
            .contains("These files conflict: shared.txt"),
        "{first}"
    );
    let branch = parent["branch"].as_str().unwrap();
    assert_eq!(
        git_out(repo.path(), &["show", &format!("{branch}:shared.txt")]),
        "one\ntwo"
    );

    daemon.shutdown_and_wait();
    for t in children.iter().chain([&parent]) {
        remove_dir_retrying(t["worktree"].as_str().unwrap());
    }
}

#[test]
fn a_task_queued_for_a_slot_cannot_become_a_parent_and_implements_alone() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-claude.sh", SLEEP_ONCE_SCRIPT);
    let log = tempfile::tempdir().unwrap();
    let fake_bins = [
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("PARTS_LOG", log.path().to_str().unwrap()),
    ];
    let data_holder = tempfile::tempdir().unwrap();
    let home = data_holder.path().to_path_buf();
    // `parallel` is read at startup: save it with a first daemon, then
    // restart onto the saved settings in the same data dir.
    let socket1 = socket_of(&home);
    let first = spawn_daemon(&home, &fake_bins);
    wait_for_socket(&socket1);
    let mut settings = request_on(&socket1, "settings.get", serde_json::json!({}));
    settings["parallel"] = serde_json::json!(1);
    settings["briefCheckRoute"] = serde_json::json!("");
    settings["planReview"] = serde_json::json!("");
    settings["review"] = serde_json::json!("");
    settings["answerPolicy"] = serde_json::json!(false);
    fit_sandbox(&mut settings);
    request_on(
        &socket1,
        "settings.set",
        serde_json::json!({"settings": settings}),
    );
    stop_daemon(&socket1);
    let _ = wait_for_exit(first);

    let socket = socket_of(&home);
    let child = spawn_daemon(&home, &fake_bins);
    wait_for_socket(&socket);
    let call = |m: &str, p: serde_json::Value| request_on(&socket, m, p);
    let repo = init_git_repo();
    let repo_path = repo.path().to_str().unwrap();

    // The blocker holds the only slot; the parent queues behind it.
    let blocker = call(
        "task.create",
        serde_json::json!({"repo": repo_path, "title": "Blocker", "goal": "g",
        "verify": ["true"], "start": true}),
    );
    wait_until("the blocker to run", || {
        log.path().join("slept").exists().then_some(())
    });
    let parent = call(
        "task.create",
        serde_json::json!({"repo": repo_path, "title": "Parent", "goal": "g",
        "verify": ["true"], "start": true}),
    );
    let parent_id = parent["id"].as_str().unwrap().to_string();
    let rejected = raw_request_with_params(
        &socket,
        "task.create",
        serde_json::json!({"repo": repo_path, "title": "Late child", "goal": "g",
        "parent": parent_id}),
    );
    let message = rejected["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("already running"), "{rejected}");
    let listed = call("task.list", serde_json::json!({"repo": parent["repo"]}));
    assert_eq!(listed.as_array().unwrap().len(), 2, "{listed}");

    // Freed slot: the parent is an ordinary task and implements itself.
    call(
        "task.stop",
        serde_json::json!({"id": blocker["id"].as_str().unwrap()}),
    );
    let done = wait_until("the parent to be done", || {
        let t = call("task.get", serde_json::json!({"id": parent_id}));
        (t["status"] == "done").then_some(t)
    });
    assert_eq!(done["attempts"].as_array().unwrap().len(), 1, "{done}");

    stop_daemon(&socket);
    let _ = wait_for_exit(child);
    for t in [&blocker, &parent] {
        remove_dir_retrying(t["worktree"].as_str().unwrap());
    }
}
