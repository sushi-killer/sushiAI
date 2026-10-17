use super::*;

#[test]
fn a_task_with_depends_on_is_not_started_until_its_dependency_is_done() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        "#!/bin/sh\ncat > /dev/null\necho changed > CHANGED_MARKER.txt\necho '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}'\necho '{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":\"```sushi-report\\n{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}\\n```\"}'\n",
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    review_off(&daemon);
    let repo = init_git_repo();
    let create = |title: &str, depends_on: serde_json::Value| {
        daemon.request(
            "task.create",
            serde_json::json!({
                "repo": repo.path().to_str().unwrap(),
                "title": title,
                "goal": "Make a trivial change",
                "verify": ["test -f CHANGED_MARKER.txt"],
                "dependsOn": depends_on,
                "start": false,
            }),
        )
    };
    let first = create("First", serde_json::json!([]));
    let first_id = first["id"].as_str().unwrap().to_string();
    let second = create("Second", serde_json::json!([first_id]));
    let second_id = second["id"].as_str().unwrap().to_string();
    assert_eq!(second["dependsOn"], serde_json::json!([first_id]));

    // Created with a dependency, the second task starts on its own, but its
    // loop must not run an attempt while the first is not done.
    // Absence check: this window gives a forbidden start time to happen.
    holds_for("a dependent task staying queued", ABSENCE_WINDOW, || {
        let t = daemon.request("task.get", serde_json::json!({"id": second_id}));
        t["status"] == "queued" && t["attempts"].as_array().unwrap().is_empty()
    });
    let waiting = daemon.request("task.get", serde_json::json!({"id": second_id}));
    assert_eq!(waiting["status"], "queued", "task JSON: {waiting}");
    assert!(waiting["attempts"].as_array().unwrap().is_empty());

    daemon.request("task.start", serde_json::json!({"id": first_id}));
    let first = settle(&daemon, &first_id);
    assert_eq!(first["status"], "done", "task JSON: {first}");
    let second = settle(&daemon, &second_id);
    assert_eq!(second["status"], "done", "task JSON: {second}");
    assert!(
        second["attempts"][0]["startedAt"].as_i64() >= first["attempts"][0]["endedAt"].as_i64(),
        "second started before first ended: {second}"
    );
    let listed = daemon.request("task.list", serde_json::json!({"repo": first["repo"]}));
    assert!(listed
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["dependsOn"] == serde_json::json!([first_id])));

    let worktrees = [first["worktree"].clone(), second["worktree"].clone()];
    daemon.shutdown_and_wait();
    for wt in worktrees {
        remove_dir_retrying(wt.as_str().unwrap());
    }
}

#[test]
fn task_create_rejects_a_dependency_cycle_and_an_unknown_id_and_creates_nothing() {
    let daemon = Daemon::spawn(&[]);
    let repo = init_git_repo();
    let repo_path = repo.path().to_str().unwrap();
    let create = |extra: serde_json::Value| {
        let mut params = serde_json::json!({
            "repo": repo_path,
            "title": "T",
            "goal": "G",
            "start": false,
        });
        params
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        raw_request_with_params(&daemon.socket, "task.create", params)
    };
    let parent = create(serde_json::json!({}))["result"].clone();
    let parent_id = parent["id"].as_str().unwrap().to_string();
    // Waits for the parent, which in turn would wait for its new child.
    let waiter = create(serde_json::json!({"dependsOn": [parent_id]}))["result"].clone();
    let waiter_id = waiter["id"].as_str().unwrap().to_string();
    let worktrees_before = git_out(repo.path(), &["worktree", "list"]);

    let rejected = [
        (
            serde_json::json!({"parent": parent_id, "dependsOn": [waiter_id]}),
            "cycle",
        ),
        (
            serde_json::json!({"parent": parent_id, "dependsOn": [parent_id]}),
            "cycle",
        ),
        (
            serde_json::json!({"dependsOn": [uuid_like()]}),
            "unknown dependency",
        ),
        (
            serde_json::json!({"dependsOn": ["nope"]}),
            "unknown dependency",
        ),
        (serde_json::json!({"parent": uuid_like()}), "unknown parent"),
    ];
    for (extra, why) in rejected {
        let v = create(extra.clone());
        let message = v["error"]["message"].as_str().unwrap_or_default();
        assert!(message.contains(why), "{extra}: {v}");
    }
    let listed = daemon.request("task.list", serde_json::json!({"repo": parent["repo"]}));
    assert_eq!(listed.as_array().unwrap().len(), 2, "{listed}");
    assert_eq!(
        git_out(repo.path(), &["worktree", "list"]),
        worktrees_before
    );

    daemon.shutdown_and_wait();
    for t in [&parent, &waiter] {
        remove_dir_retrying(t["worktree"].as_str().unwrap());
    }
}

#[test]
fn a_stopped_dependency_asks_its_dependent_and_dropping_it_lets_the_dependent_run() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        "#!/bin/sh\ninput=\"$(cat)\"\ncase \"$input\" in *SLOW*) sleep 30 ;; esac\necho changed > CHANGED_MARKER.txt\necho '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}'\necho '{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":\"```sushi-report\\n{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}\\n```\"}'\n",
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    review_off(&daemon);
    let repo = init_git_repo();
    let slow = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "SLOW dependency",
            "goal": "Takes a while",
            "verify": ["true"],
            "start": true,
        }),
    );
    let slow_id = slow["id"].as_str().unwrap().to_string();
    let dependent = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Dependent",
            "goal": "Runs after",
            "verify": ["true"],
            "dependsOn": [slow_id],
        }),
    );
    let dependent_id = dependent["id"].as_str().unwrap().to_string();
    poll_until(&daemon, &slow_id, |s| s == "running");
    daemon.request("task.stop", serde_json::json!({"id": slow_id}));

    let asked = poll_until(&daemon, &dependent_id, |s| s == "waiting");
    let question = asked["question"]["text"].as_str().unwrap();
    assert!(
        question.contains("\"SLOW dependency\" (stopped)"),
        "{asked}"
    );
    assert_eq!(
        asked["question"]["options"],
        serde_json::json!(["retry the dependency", "drop the dependency", "stop"])
    );
    daemon.request(
        "task.answer",
        serde_json::json!({"id": dependent_id, "answer": "drop the dependency"}),
    );
    let done = settle(&daemon, &dependent_id);
    assert_eq!(done["status"], "done", "task JSON: {done}");
    assert!(done.get("dependsOn").is_none(), "{done}");

    daemon.shutdown_and_wait();
    for t in [&slow, &dependent] {
        remove_dir_retrying(t["worktree"].as_str().unwrap());
    }
}

#[test]
fn stopping_a_parent_stops_its_children_and_running_it_again_restarts_them() {
    let scripts_dir = tempfile::tempdir().unwrap();
    // The first run hangs until it is stopped; every later run finishes.
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        "#!/bin/sh\ncat > /dev/null\nif [ ! -f \"$PARTS_LOG/slept\" ]; then touch \"$PARTS_LOG/slept\"; sleep 30; fi\necho changed > CHANGED_MARKER.txt\necho '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}'\necho '{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":\"```sushi-report\\n{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}\\n```\"}'\n",
    );
    let log = tempfile::tempdir().unwrap();
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("PARTS_LOG", log.path().to_str().unwrap()),
    ]);
    review_off(&daemon);
    let repo = init_git_repo();
    let parent = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Parent",
            "goal": "One part",
            "start": false,
        }),
    );
    let parent_id = parent["id"].as_str().unwrap().to_string();
    let child = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Only part",
            "goal": "Make a change",
            "verify": ["test -f CHANGED_MARKER.txt"],
            "parent": parent_id,
        }),
    );
    let child_id = child["id"].as_str().unwrap().to_string();
    wait_until("the child to run", || {
        log.path().join("slept").exists().then_some(())
    });

    let stopped = daemon.request("task.stop", serde_json::json!({"id": parent_id}));
    assert_eq!(
        stopped["status"], "stopped",
        "an idle parent stops at once: {stopped}"
    );
    let child_stopped = settle(&daemon, &child_id);
    assert_eq!(child_stopped["status"], "stopped", "{child_stopped}");
    // A stop is not a dependency failure: nobody is asked anything.
    let parent_now = daemon.request("task.get", serde_json::json!({"id": parent_id}));
    assert_eq!(parent_now["status"], "stopped", "{parent_now}");

    daemon.request("task.start", serde_json::json!({"id": parent_id}));
    let parent_done = poll_until(&daemon, &parent_id, |s| {
        matches!(s, "done" | "failed" | "waiting")
    });
    assert_eq!(parent_done["status"], "done", "{parent_done}");
    let child_done = daemon.request("task.get", serde_json::json!({"id": child_id}));
    assert_eq!(child_done["status"], "done", "{child_done}");

    daemon.shutdown_and_wait();
    for t in [&parent_done, &child_done] {
        remove_dir_retrying(t["worktree"].as_str().unwrap());
    }
}

#[test]
fn a_parent_waiting_for_a_stopped_dependency_starts_no_child() {
    let (daemon, _scripts, log) = sleep_once_daemon();
    let repo = init_git_repo();
    let dep = create_and_stop_first_run(&daemon, repo.path(), log.path(), serde_json::json!({}));
    let dep_id = dep["id"].as_str().unwrap().to_string();
    let parent = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Parent",
            "goal": "Both parts",
            "verify": ["true"],
            "dependsOn": [dep_id],
            "start": false,
        }),
    );
    let parent_id = parent["id"].as_str().unwrap().to_string();
    let child = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Only part",
            "goal": "Make a change",
            "verify": ["true"],
            "parent": parent_id,
        }),
    );
    let child_id = child["id"].as_str().unwrap().to_string();

    let asked = poll_until(&daemon, &parent_id, |s| s == "waiting");
    assert!(
        asked["question"]["text"]
            .as_str()
            .unwrap()
            .contains("(stopped)"),
        "{asked}"
    );
    // Absence check: this window gives a forbidden child start time to happen.
    holds_for(
        "the child and its parent staying unstarted",
        ABSENCE_WINDOW,
        || {
            let child = daemon.request("task.get", serde_json::json!({"id": child_id}));
            let parent = daemon.request("task.get", serde_json::json!({"id": parent_id}));
            child["status"] == "queued"
                && child["attempts"] == serde_json::json!([])
                && parent["attempts"] == serde_json::json!([])
        },
    );
    let child_now = daemon.request("task.get", serde_json::json!({"id": child_id}));
    assert_eq!(child_now["status"], "queued", "{child_now}");
    assert_eq!(child_now["attempts"], serde_json::json!([]), "{child_now}");
    let parent_now = daemon.request("task.get", serde_json::json!({"id": parent_id}));
    assert_eq!(
        parent_now["attempts"],
        serde_json::json!([]),
        "{parent_now}"
    );

    daemon.shutdown_and_wait();
    for t in [&dep, &parent, &child] {
        remove_dir_retrying(t["worktree"].as_str().unwrap());
    }
}

#[test]
fn retrying_a_stopped_dependency_finishes_the_dependency_and_its_dependent() {
    let (daemon, _scripts, log) = sleep_once_daemon();
    let repo = init_git_repo();
    let dep = create_and_stop_first_run(&daemon, repo.path(), log.path(), serde_json::json!({}));
    let dep_id = dep["id"].as_str().unwrap().to_string();
    let dependent = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Dependent",
            "goal": "Runs after",
            "verify": ["true"],
            "dependsOn": [dep_id],
        }),
    );
    let dependent_id = dependent["id"].as_str().unwrap().to_string();
    poll_until(&daemon, &dependent_id, |s| s == "waiting");
    daemon.request(
        "task.answer",
        serde_json::json!({"id": dependent_id, "answer": "retry the dependency"}),
    );
    let dep_done = poll_until(&daemon, &dep_id, |s| {
        matches!(s, "done" | "failed" | "waiting")
    });
    assert_eq!(dep_done["status"], "done", "{dep_done}");
    let dependent_done = poll_until(&daemon, &dependent_id, |s| matches!(s, "done" | "failed"));
    assert_eq!(dependent_done["status"], "done", "{dependent_done}");
    assert!(dependent_done["question"].is_null(), "{dependent_done}");

    daemon.shutdown_and_wait();
    for t in [&dep, &dependent] {
        remove_dir_retrying(t["worktree"].as_str().unwrap());
    }
}

#[test]
fn dropping_the_only_subtask_stops_the_parent_instead_of_implementing_it() {
    let (daemon, _scripts, log) = sleep_once_daemon();
    let repo = init_git_repo();
    let parent = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Parent",
            "goal": "One part",
            "verify": ["true"],
            "start": false,
        }),
    );
    let parent_id = parent["id"].as_str().unwrap().to_string();
    let child = create_and_stop_first_run(
        &daemon,
        repo.path(),
        log.path(),
        serde_json::json!({"title": "Only part", "parent": parent_id}),
    );
    let asked = poll_until(&daemon, &parent_id, |s| s == "waiting");
    assert!(asked["question"]["text"].is_string(), "{asked}");
    daemon.request(
        "task.answer",
        serde_json::json!({"id": parent_id, "answer": "drop the dependency"}),
    );
    poll_until(&daemon, &parent_id, |s| s == "stopped");
    // Absence check: this window gives a forbidden attempt time to start.
    holds_for(
        "a stopped parent starting no attempt",
        ABSENCE_WINDOW,
        || {
            let t = daemon.request("task.get", serde_json::json!({"id": parent_id}));
            t["status"] == "stopped" && t["attempts"] == serde_json::json!([])
        },
    );
    let after = daemon.request("task.get", serde_json::json!({"id": parent_id}));
    assert_eq!(after["status"], "stopped", "{after}");
    assert_eq!(after["attempts"], serde_json::json!([]), "{after}");
    assert!(
        after["decisions"].to_string().contains("no subtasks"),
        "{after}"
    );

    daemon.shutdown_and_wait();
    for t in [&parent, &child] {
        remove_dir_retrying(t["worktree"].as_str().unwrap());
    }
}
