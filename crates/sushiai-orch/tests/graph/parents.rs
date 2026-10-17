use super::*;

#[test]
fn a_finished_parent_lands_on_its_base_as_one_commit_titled_with_its_title() {
    let (daemon, _scripts) = split_daemon("test -f README.md");
    let repo = repo_on_work_branch();
    let parent_id = create_split(&daemon, repo.path(), true);

    let parent = settle(&daemon, &parent_id);
    assert_eq!(parent["status"], "done", "task JSON: {parent}");
    assert!(
        implement_attempts(&parent).is_empty(),
        "a parent implements nothing itself: {parent}"
    );
    let head = git_out(repo.path(), &["rev-parse", "work"]);
    assert_eq!(parent["landedSha"], head.as_str(), "task JSON: {parent}");
    let subjects = git_out(repo.path(), &["log", "--format=%s", "work"]);
    assert_eq!(subjects, "Parent\ninit", "log of work");
    for file in ["one.txt", "two.txt"] {
        git_out(repo.path(), &["cat-file", "-e", &format!("work:{file}")]);
    }
    assert!(!Path::new(parent["worktree"].as_str().unwrap()).exists());
    let children = children_of(&daemon, &parent);
    assert_eq!(children.len(), 2);

    daemon.shutdown_and_wait();
    remove_worktrees(&children.iter().chain([&parent]).collect::<Vec<_>>());
}

#[test]
fn a_parent_check_that_fails_after_its_subtasks_landed_gets_one_agent_attempt_and_then_lands() {
    // Passes on the base, fails once the subtasks' files are there.
    let (daemon, _scripts) = split_daemon(
        "if [ -f one.txt ] && [ ! -f fix.txt ]; then echo NEEDS-$(echo FIX); exit 3; fi",
    );
    let repo = repo_on_work_branch();
    let parent_id = create_split(&daemon, repo.path(), true);

    let parent = poll_until(&daemon, &parent_id, |s| {
        matches!(s, "done" | "failed" | "stopped" | "waiting")
    });
    assert_eq!(parent["status"], "done", "task JSON: {parent}");
    assert_eq!(implement_attempts(&parent).len(), 1, "task JSON: {parent}");
    assert!(
        run_file(&daemon, &parent_id, "brief.md").contains("NEEDS-FIX"),
        "the brief carries the failure output"
    );
    let head = git_out(repo.path(), &["rev-parse", "work"]);
    assert_eq!(parent["landedSha"], head.as_str(), "task JSON: {parent}");
    for file in ["one.txt", "two.txt", "fix.txt"] {
        git_out(repo.path(), &["cat-file", "-e", &format!("work:{file}")]);
    }
    let subjects = git_out(repo.path(), &["log", "--format=%s", "work"]);
    assert_eq!(subjects, "Parent\ninit", "log of work");

    let children = children_of(&daemon, &parent);
    daemon.shutdown_and_wait();
    remove_worktrees(&children.iter().chain([&parent]).collect::<Vec<_>>());
}

#[test]
fn a_parent_verify_that_fails_on_the_base_asks_before_any_subtask_starts() {
    let (daemon, _scripts) = split_daemon("echo no library targets; exit 101");
    let repo = repo_on_work_branch();
    let parent_id = create_split(&daemon, repo.path(), false);

    let parent = settle(&daemon, &parent_id);
    assert_eq!(parent["status"], "waiting", "task JSON: {parent}");
    let question = &parent["question"];
    assert_eq!(question["kind"], "preexisting_failure", "{parent}");
    let text = question["text"].as_str().unwrap();
    assert!(text.contains("echo no library targets; exit 101"), "{text}");
    assert!(text.contains("no library targets"), "{text}");
    assert!(
        text.contains(&parent["baseSha"].as_str().unwrap()[..7]),
        "{text}"
    );
    assert_eq!(
        question["options"],
        serde_json::json!(["keep all", "drop all", "stop"])
    );
    let children = children_of(&daemon, &parent);
    assert_eq!(children.len(), 2);
    for c in &children {
        assert!(
            c["attempts"]
                .as_array()
                .unwrap()
                .iter()
                .all(|a| a["stage"] != "implement"),
            "a subtask started before the answer: {c}"
        );
        assert!(c["status"] != "running" && c["status"] != "done", "{c}");
    }

    // Dropping the check lets the subtasks run and the parent finish.
    daemon.request(
        "task.answer",
        serde_json::json!({"id": parent_id, "answer": "drop all"}),
    );
    let parent = poll_until(&daemon, &parent_id, |s| {
        matches!(s, "done" | "failed" | "stopped")
    });
    assert_eq!(parent["status"], "done", "task JSON: {parent}");
    assert_eq!(parent["verify"], serde_json::json!([]), "{parent}");

    let children = children_of(&daemon, &parent);
    daemon.shutdown_and_wait();
    remove_worktrees(&children.iter().chain([&parent]).collect::<Vec<_>>());
}

#[test]
fn a_parent_final_that_fails_on_the_base_does_not_hold_back_the_children() {
    let log = tempfile::tempdir().unwrap();
    let ran = log.path().join("final.log");
    // Starts, logs a line, and is still running when the children land: it
    // outlasts the deadline twice over, so the children must not wait for it.
    let final_check = format!(
        "echo start >> {}; sleep {}; exit 1",
        ran.display(),
        deadline().as_secs() * 2
    );
    let (daemon, _scripts) = split_daemon_with_final("true", &final_check);
    let repo = repo_on_work_branch();
    let parent_id = create_split(&daemon, repo.path(), false);

    let (parent, children) = wait_until("both children to be done", || {
        let parent = daemon.request("task.get", serde_json::json!({"id": parent_id}));
        let children = children_of(&daemon, &parent);
        if children.len() == 2 && children.iter().all(|c| c["status"] == "done") {
            return Some((parent, children));
        }
        assert_ne!(parent["status"], "waiting", "no question first: {parent}");
        None
    });
    assert!(parent["question"].is_null(), "{parent}");
    // At least the base run; the parent's own gate may already have run it.
    let started = std::fs::read_to_string(&ran).map_or(0, |t| t.lines().count());
    assert!(
        started >= 1,
        "the final's base run started in the background"
    );

    daemon.shutdown_and_wait();
    remove_worktrees(&children.iter().chain([&parent]).collect::<Vec<_>>());
}

#[test]
fn a_parent_whose_checks_name_absent_planned_files_asks_nothing() {
    let checks = [
        "test -f one.txt",
        "test -f two.txt",
        "test -s one.txt",
        "test -s two.txt",
    ];
    let (daemon, _scripts) = split_daemon(&verify_list(&checks));
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["autoAnswer"] = serde_json::json!(true);
    settings["answerPolicy"] = serde_json::json!(true);
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
    let repo = repo_on_work_branch();
    let parent_id = create_split(&daemon, repo.path(), false);

    let parent = poll_until(&daemon, &parent_id, |s| {
        matches!(s, "done" | "failed" | "stopped")
    });
    assert_eq!(parent["status"], "done", "task JSON: {parent}");
    assert_eq!(
        parent["questionHistory"].as_array().map_or(0, Vec::len),
        0,
        "{parent}"
    );
    let log = daemon.request("overseer.log", serde_json::json!({}));
    let wakes = log
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "woke" && e["text"] != "Woke for done")
        .count();
    // The parent finishing may wake the overseer; nothing else does.
    assert_eq!(wakes, 0, "{log}");
    let children = children_of(&daemon, &parent);
    let mut judged = Vec::new();
    for id in children
        .iter()
        .chain([&parent])
        .map(|t| t["id"].as_str().unwrap())
    {
        dirs_named(
            &daemon.data_dir().join("tasks").join(id),
            "feasibility-",
            &mut judged,
        );
    }
    assert!(judged.is_empty(), "no judge run: {judged:?}");
    let decisions = decisions_of(&parent);
    for check in checks {
        let kept = decisions
            .iter()
            .filter(|d| {
                d.contains(&format!("kept check {check},"))
                    && d.contains("targets absent on base; the plan creates them")
            })
            .count();
        assert_eq!(kept, 1, "{check}: {decisions:?}");
    }
    daemon.shutdown_and_wait();
    remove_worktrees(&children.iter().chain([&parent]).collect::<Vec<_>>());
}

#[test]
fn a_parent_with_three_checks_failing_on_base_gets_one_question() {
    let checks = [
        "echo first; exit 1",
        "echo second; exit 2",
        "echo third; exit 3",
    ];
    let (daemon, _scripts) = split_daemon(&verify_list(&checks));
    let repo = repo_on_work_branch();
    let parent_id = create_split(&daemon, repo.path(), false);

    let parent = settle(&daemon, &parent_id);
    assert_eq!(parent["status"], "waiting", "task JSON: {parent}");
    let question = &parent["question"];
    assert_eq!(question["kind"], "preexisting_failure", "{parent}");
    assert_eq!(question["commands"], serde_json::json!(checks), "{parent}");
    assert_eq!(
        question["options"],
        serde_json::json!(["keep all", "drop all", "stop"])
    );
    daemon.request(
        "task.answer",
        serde_json::json!({"id": parent_id, "answer": "keep all"}),
    );
    let kept = wait_until("the three kept-check decisions", || {
        let parent = daemon.request("task.get", serde_json::json!({"id": parent_id}));
        let kept: Vec<String> = decisions_of(&parent)
            .into_iter()
            .filter(|d| d.starts_with("Orchestrator: kept check "))
            .collect();
        (kept.len() >= 3).then_some(kept)
    });
    assert_eq!(kept.len(), 3, "{kept:?}");
    let parent = daemon.request("task.get", serde_json::json!({"id": parent_id}));
    assert_eq!(
        parent["questionHistory"].as_array().unwrap().len(),
        1,
        "one question was asked: {parent}"
    );
    let children = children_of(&daemon, &parent);
    daemon.shutdown_and_wait();
    remove_worktrees(&children.iter().chain([&parent]).collect::<Vec<_>>());
}

#[test]
fn answering_stop_to_a_parents_base_check_question_stops_the_parent() {
    let (daemon, _scripts) = split_daemon("echo no library targets; exit 101");
    let repo = repo_on_work_branch();
    let parent_id = create_split(&daemon, repo.path(), false);

    let parent = settle(&daemon, &parent_id);
    assert_eq!(parent["status"], "waiting", "task JSON: {parent}");
    daemon.request(
        "task.answer",
        serde_json::json!({"id": parent_id, "answer": "stop"}),
    );
    let parent = poll_until(&daemon, &parent_id, |s| {
        matches!(s, "done" | "failed" | "stopped")
    });
    assert_eq!(parent["status"], "stopped", "task JSON: {parent}");
    let children = children_of(&daemon, &parent);
    for c in &children {
        assert!(
            c["attempts"]
                .as_array()
                .unwrap()
                .iter()
                .all(|a| a["stage"] != "implement"),
            "{c}"
        );
    }
    daemon.shutdown_and_wait();
    remove_worktrees(&children.iter().chain([&parent]).collect::<Vec<_>>());
}

#[test]
fn answering_stop_after_a_daemon_restart_stops_the_parent_and_its_subtasks() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let body = SPLIT_SCRIPT
        .replace("__PARENT_VERIFY__", "echo no library targets; exit 101")
        .replace("__PARENT_FINAL__", "");
    let script = fake_harness_script(scripts_dir.path(), "fake-claude.sh", &body);
    let env = [("ORCHD_CLAUDE_BIN", script.to_str().unwrap())];
    let mut daemon = Daemon::spawn(&env);
    review_off(&daemon);
    let repo = repo_on_work_branch();
    let parent_id = create_split(&daemon, repo.path(), false);
    let parent = settle(&daemon, &parent_id);
    assert_eq!(parent["status"], "waiting", "task JSON: {parent}");

    // A subtask may already be implementing when the daemon goes down; only
    // an attempt started after the restart is a failure.
    let before: Vec<usize> = children_of(&daemon, &parent)
        .iter()
        .map(|c| implement_attempts(c).len())
        .collect();

    // Restart onto the same data dir: no live loop holds the question now.
    daemon.restart(&env);

    daemon.request(
        "task.answer",
        serde_json::json!({"id": parent_id, "answer": "stop"}),
    );
    poll_until(&daemon, &parent_id, |s| s == "stopped");
    // Absence check: this window gives a wrongly relaunched loop time to run its
    // preflight again.
    holds_for(
        "no relaunched loop after the stop answer",
        ABSENCE_WINDOW,
        || {
            let parent = daemon.request("task.get", serde_json::json!({"id": parent_id}));
            parent["status"] == "stopped"
                && parent["question"].is_null()
                && children_of(&daemon, &parent)
                    .iter()
                    .zip(&before)
                    .all(|(c, before)| {
                        implement_attempts(c).len() == *before
                            && c["status"] != "running"
                            && c["status"] != "done"
                    })
        },
    );
    let parent = daemon.request("task.get", serde_json::json!({"id": parent_id}));
    assert_eq!(parent["status"], "stopped", "task JSON: {parent}");
    assert!(parent["question"].is_null(), "{parent}");
    let children = children_of(&daemon, &parent);
    assert_eq!(children.len(), before.len(), "{parent}");
    for (c, before) in children.iter().zip(&before) {
        assert_eq!(implement_attempts(c).len(), *before, "{c}");
        assert!(c["status"] != "running" && c["status"] != "done", "{c}");
    }
    daemon.shutdown_and_wait();
    remove_worktrees(&children.iter().chain([&parent]).collect::<Vec<_>>());
}

#[test]
fn a_parent_in_the_backlog_starts_no_child_until_it_leaves() {
    let (daemon, _scripts, log) = sleep_once_daemon();
    let repo = init_git_repo();
    let parent = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Feature",
            "goal": "Both parts",
            "start": false,
            "backlog": {"bucket": "later"},
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

    // Absence check: this window gives a forbidden child start time to happen.
    holds_for(
        "a backlog parent's child staying unstarted",
        ABSENCE_WINDOW,
        || {
            let t = daemon.request("task.get", serde_json::json!({"id": child_id}));
            t["status"] == "queued"
                && t["attempts"] == serde_json::json!([])
                && !log.path().join("slept").exists()
        },
    );
    let child_now = daemon.request("task.get", serde_json::json!({"id": child_id}));
    assert_eq!(child_now["status"], "queued", "{child_now}");
    assert_eq!(child_now["attempts"], serde_json::json!([]), "{child_now}");
    assert!(!log.path().join("slept").exists());

    daemon.request("task.start", serde_json::json!({"id": parent_id}));
    wait_until("the child to run", || {
        log.path().join("slept").exists().then_some(())
    });

    daemon.shutdown_and_wait();
    for t in [&parent, &child] {
        remove_dir_retrying(t["worktree"].as_str().unwrap());
    }
}
