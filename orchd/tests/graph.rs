//! Black-box integration tests (graph): spawn the real `orchd` binary and drive
//! it over its NDJSON unix socket.

mod common;

use common::*;
use std::path::Path;
use std::time::{Duration, Instant};

fn review_off(daemon: &Daemon) {
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    settings["answerPolicy"] = serde_json::json!(false);
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
}

fn implement_attempts(task: &serde_json::Value) -> Vec<serde_json::Value> {
    task["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["stage"] == "implement")
        .cloned()
        .collect()
}

/// Plans a top-level request as two parts (B after A), plans each part as
/// one task, and implements each part by writing its file.
const GRAPH_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
out() {
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  printf '%s\n' "{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":$1}"
}
report='"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"'
case "$input" in
  *sushi-plan*)
    case "$input" in
      *PART_A*) out '"```sushi-plan\n{\"title\":\"Part A\",\"goal\":\"PART_A: create a.txt\",\"verify\":[\"test -f a.txt\"]}\n```"' ;;
      *PART_B*) out '"```sushi-plan\n{\"title\":\"Part B\",\"goal\":\"PART_B: create b.txt next to a.txt\",\"verify\":[\"test -f a.txt && test -f b.txt\"]}\n```"' ;;
      *) out '"```sushi-plan\n{\"title\":\"Both parts\",\"goal\":\"Create a.txt, then b.txt\",\"verify\":[\"test -f a.txt && test -f b.txt\"],\"subtasks\":[{\"key\":\"a\",\"title\":\"Part A\",\"request\":\"PART_A create a.txt\"},{\"key\":\"b\",\"title\":\"Part B\",\"request\":\"PART_B create b.txt\",\"dependsOn\":[\"a\"]}]}\n```"' ;;
    esac
    ;;
  *PART_A*) echo a > a.txt; out "$report" ;;
  *PART_B*) test -f a.txt && echo b > b.txt; out "$report" ;;
esac
"#;

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
    std::thread::sleep(Duration::from_millis(1500));
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
        let _ = std::fs::remove_dir_all(wt.as_str().unwrap());
    }
}

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

    // The parent's verify needs both files, so it fails on the base: the
    // parent asks before any subtask starts, and the subtasks are meant to
    // make it pass.
    let asked = settle(&daemon, &parent_id);
    assert_eq!(asked["status"], "waiting", "task JSON: {asked}");
    assert_eq!(asked["question"]["kind"], "preexisting_failure", "{asked}");
    assert!(
        asked["question"]["options"]
            .to_string()
            .contains("keep this check"),
        "{asked}"
    );
    daemon.request(
        "task.answer",
        serde_json::json!({"id": parent_id, "answer": "keep this check"}),
    );
    let parent = poll_until(&daemon, &parent_id, Duration::from_secs(30), |s| {
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
    assert!(b["decisions"].to_string().contains("started from"), "{b}");
    let total = a["costUsd"].as_f64().unwrap() + b["costUsd"].as_f64().unwrap();
    assert!(
        parent["costUsd"].as_f64().unwrap() >= total - 1e-9,
        "parent cost includes its children: {parent}"
    );

    daemon.shutdown_and_wait();
    for t in [&parent, *a, *b] {
        let _ = std::fs::remove_dir_all(t["worktree"].as_str().unwrap());
    }
}

#[test]
fn overlapping_subtask_paths_add_a_dependency_edge_to_the_created_children() {
    let script_text = GRAPH_SCRIPT
        .replace(
            r#"\"request\":\"PART_A create a.txt\""#,
            r#"\"request\":\"PART_A create a.txt\",\"paths\":[\"src/a\"]"#,
        )
        .replace(r#"\"dependsOn\":[\"a\"]"#, r#"\"paths\":[\"src/a/b.rs\"]"#);
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
    let tasks = daemon.request("task.list", serde_json::json!({"repo": parent["repo"]}));
    let children: Vec<&serde_json::Value> = tasks
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["parent"] == parent_id.as_str())
        .collect();
    let a = children.iter().find(|t| t["title"] == "Part A").unwrap();
    let b = children.iter().find(|t| t["title"] == "Part B").unwrap();
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
        let _ = std::fs::remove_dir_all(t["worktree"].as_str().unwrap());
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
    // after Part A was created.
    let parent_dir = repo.path().join(".sushiai/worktrees");
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
    let _ = std::fs::remove_dir_all(&blocked);
    let _ = std::fs::remove_dir_all(parent["worktree"].as_str().unwrap());
}

/// Two parts of a hand-built graph. Each writes its own file, or with
/// `SHARED` both write the same one; a retry told about a conflict keeps
/// both lines. Every run notes how many runs were live when it started.
const PARTS_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
mkdir -p "$PARTS_LOG/live"
touch "$PARTS_LOG/live/$$"
ls "$PARTS_LOG/live" | wc -l | tr -d ' ' >> "$PARTS_LOG/counts"
case "$input" in
  *"These files conflict"*) printf 'one\ntwo\n' > shared.txt ;;
  *PART_ONE*) sleep 1; if [ -n "$SHARED" ]; then echo one > shared.txt; else echo one > one.txt; fi ;;
  *PART_TWO*) sleep 1; if [ -n "$SHARED" ]; then echo two > shared.txt; else echo two > two.txt; fi ;;
esac
rm -f "$PARTS_LOG/live/$$"
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}'
"#;

/// A parent created by hand and its two children; returns the ids.
fn hand_built_graph(
    daemon: &Daemon,
    repo: &Path,
    parent_verify: &str,
    child_verify: [&str; 2],
) -> (String, [String; 2]) {
    let parent = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.to_str().unwrap(),
            "title": "Parent",
            "goal": "Both parts",
            "verify": [parent_verify],
            // Left unstarted until its children are attached, or it would
            // implement the whole request itself.
            "start": false,
        }),
    );
    let parent_id = parent["id"].as_str().unwrap().to_string();
    let child = |title: &str, goal: &str, verify: &str| {
        let t = daemon.request(
            "task.create",
            serde_json::json!({
                "repo": repo.to_str().unwrap(),
                "title": title,
                "goal": goal,
                "verify": [verify],
                "parent": parent_id,
            }),
        );
        assert_eq!(t["parent"], parent_id.as_str());
        assert_eq!(
            t["baseRef"], parent["branch"],
            "a child branches from its parent"
        );
        t["id"].as_str().unwrap().to_string()
    };
    let one = child("One", "PART_ONE", child_verify[0]);
    let two = child("Two", "PART_TWO", child_verify[1]);
    (parent_id, [one, two])
}

#[test]
fn two_independent_children_run_concurrently_and_both_land() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-claude.sh", PARTS_SCRIPT);
    let log = tempfile::tempdir().unwrap();
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("PARTS_LOG", log.path().to_str().unwrap()),
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
        let _ = std::fs::remove_dir_all(t["worktree"].as_str().unwrap());
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
        let _ = std::fs::remove_dir_all(t["worktree"].as_str().unwrap());
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
        raw_request_with_params(&daemon.socket, "task.create", params, Some(&daemon.token))
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
        let _ = std::fs::remove_dir_all(t["worktree"].as_str().unwrap());
    }
}

fn uuid_like() -> String {
    "4f1c7a52-2d3b-4c9e-9a1e-0b7f6d5e4c3a".to_string()
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
    poll_until(&daemon, &slow_id, Duration::from_secs(10), |s| {
        s == "running"
    });
    daemon.request("task.stop", serde_json::json!({"id": slow_id}));

    let asked = poll_until(&daemon, &dependent_id, Duration::from_secs(15), |s| {
        s == "waiting"
    });
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
        let _ = std::fs::remove_dir_all(t["worktree"].as_str().unwrap());
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
    let start = Instant::now();
    while !log.path().join("slept").exists() {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "the child never ran"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

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
    let parent_done = poll_until(&daemon, &parent_id, Duration::from_secs(30), |s| {
        matches!(s, "done" | "failed" | "waiting")
    });
    assert_eq!(parent_done["status"], "done", "{parent_done}");
    let child_done = daemon.request("task.get", serde_json::json!({"id": child_id}));
    assert_eq!(child_done["status"], "done", "{child_done}");

    daemon.shutdown_and_wait();
    for t in [&parent_done, &child_done] {
        let _ = std::fs::remove_dir_all(t["worktree"].as_str().unwrap());
    }
}

/// The first run hangs until it is stopped; every later run finishes.
const SLEEP_ONCE_SCRIPT: &str = "#!/bin/sh\ncat > /dev/null\nif [ ! -f \"$PARTS_LOG/slept\" ]; then touch \"$PARTS_LOG/slept\"; sleep 30; fi\necho changed > CHANGED_MARKER.txt\necho '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}'\necho '{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":\"```sushi-report\\n{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}\\n```\"}'\n";

fn sleep_once_daemon() -> (Daemon, tempfile::TempDir, tempfile::TempDir) {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-claude.sh", SLEEP_ONCE_SCRIPT);
    let log = tempfile::tempdir().unwrap();
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("PARTS_LOG", log.path().to_str().unwrap()),
    ]);
    review_off(&daemon);
    (daemon, scripts_dir, log)
}

/// Creates a task that hangs on its first run, waits until it does, stops it.
fn create_and_stop_first_run(
    daemon: &Daemon,
    repo: &Path,
    log: &Path,
    extra: serde_json::Value,
) -> serde_json::Value {
    let mut params = serde_json::json!({
        "repo": repo.to_str().unwrap(),
        "title": "Hangs once",
        "goal": "Takes a while",
        "verify": ["true"],
        "start": true,
    });
    params
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    let task = daemon.request("task.create", params);
    let start = Instant::now();
    while !log.join("slept").exists() {
        assert!(start.elapsed() < Duration::from_secs(10), "never ran");
        std::thread::sleep(Duration::from_millis(50));
    }
    let id = task["id"].as_str().unwrap();
    daemon.request("task.stop", serde_json::json!({"id": id}));
    poll_until(daemon, id, Duration::from_secs(10), |s| s == "stopped");
    task
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

    let asked = poll_until(&daemon, &parent_id, Duration::from_secs(15), |s| {
        s == "waiting"
    });
    assert!(
        asked["question"]["text"]
            .as_str()
            .unwrap()
            .contains("(stopped)"),
        "{asked}"
    );
    std::thread::sleep(Duration::from_millis(1500));
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
        let _ = std::fs::remove_dir_all(t["worktree"].as_str().unwrap());
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
    let data = data_holder.path().to_path_buf();
    // `parallel` is read at startup: save it with a first daemon, then
    // restart onto the saved settings in the same data dir.
    let socket1 = data.join("orchd1.sock");
    let first = spawn_orchd_raw(&data, &socket1, &fake_bins);
    wait_for_socket(&socket1);
    let token1 = read_control_token(&data);
    let mut settings = request_on(
        &socket1,
        "settings.get",
        serde_json::json!({}),
        Some(&token1),
    );
    settings["parallel"] = serde_json::json!(1);
    settings["briefCheckRoute"] = serde_json::json!("");
    settings["review"] = serde_json::json!("");
    settings["answerPolicy"] = serde_json::json!(false);
    fit_sandbox(&mut settings);
    request_on(
        &socket1,
        "settings.set",
        serde_json::json!({"settings": settings}),
        Some(&token1),
    );
    let _ = request_on(&socket1, "shutdown", serde_json::json!({}), Some(&token1));
    let _ = wait_for_exit(first, Duration::from_secs(5));

    let socket = data.join("orchd2.sock");
    let child = spawn_orchd_raw(&data, &socket, &fake_bins);
    wait_for_socket(&socket);
    let token = read_control_token(&data);
    let call = |m: &str, p: serde_json::Value| request_on(&socket, m, p, Some(&token));
    let repo = init_git_repo();
    let repo_path = repo.path().to_str().unwrap();

    // The blocker holds the only slot; the parent queues behind it.
    let blocker = call(
        "task.create",
        serde_json::json!({"repo": repo_path, "title": "Blocker", "goal": "g",
        "verify": ["true"], "start": true}),
    );
    let start = Instant::now();
    while !log.path().join("slept").exists() {
        assert!(start.elapsed() < Duration::from_secs(10), "never ran");
        std::thread::sleep(Duration::from_millis(50));
    }
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
        Some(&token),
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
    let start = Instant::now();
    let done = loop {
        let t = call("task.get", serde_json::json!({"id": parent_id}));
        if t["status"] == "done" {
            break t;
        }
        assert!(start.elapsed() < Duration::from_secs(20), "{t}");
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(done["attempts"].as_array().unwrap().len(), 1, "{done}");

    let _ = call("shutdown", serde_json::json!({}));
    let _ = wait_for_exit(child, Duration::from_secs(5));
    for t in [&blocker, &parent] {
        let _ = std::fs::remove_dir_all(t["worktree"].as_str().unwrap());
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
    poll_until(&daemon, &dependent_id, Duration::from_secs(15), |s| {
        s == "waiting"
    });
    daemon.request(
        "task.answer",
        serde_json::json!({"id": dependent_id, "answer": "retry the dependency"}),
    );
    let dep_done = poll_until(&daemon, &dep_id, Duration::from_secs(20), |s| {
        matches!(s, "done" | "failed" | "waiting")
    });
    assert_eq!(dep_done["status"], "done", "{dep_done}");
    let dependent_done = poll_until(&daemon, &dependent_id, Duration::from_secs(20), |s| {
        matches!(s, "done" | "failed")
    });
    assert_eq!(dependent_done["status"], "done", "{dependent_done}");
    assert!(dependent_done["question"].is_null(), "{dependent_done}");

    daemon.shutdown_and_wait();
    for t in [&dep, &dependent] {
        let _ = std::fs::remove_dir_all(t["worktree"].as_str().unwrap());
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
    let asked = poll_until(&daemon, &parent_id, Duration::from_secs(15), |s| {
        s == "waiting"
    });
    assert!(asked["question"]["text"].is_string(), "{asked}");
    daemon.request(
        "task.answer",
        serde_json::json!({"id": parent_id, "answer": "drop the dependency"}),
    );
    std::thread::sleep(Duration::from_millis(1500));
    let after = daemon.request("task.get", serde_json::json!({"id": parent_id}));
    assert_eq!(after["status"], "stopped", "{after}");
    assert_eq!(after["attempts"], serde_json::json!([]), "{after}");
    assert!(
        after["decisions"].to_string().contains("no subtasks"),
        "{after}"
    );

    daemon.shutdown_and_wait();
    for t in [&parent, &child] {
        let _ = std::fs::remove_dir_all(t["worktree"].as_str().unwrap());
    }
}

/// The dependency (goal MAKE_DEP) runs `dep_writes` and reports DEP_*;
/// the dependent (goal MAKE_CHILD) writes child.txt; a planner brief gets a
/// plan whose goal is MAKE_CHILD.
fn dependency_results_script(dep_writes: &str) -> String {
    format!(
        r#"#!/bin/sh
input="$(cat)"
out() {{
  printf '%s\n' '{{"type":"system","subtype":"init","session_id":"sess-fake"}}'
  printf '%s\n' "{{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{{\"input_tokens\":1,\"output_tokens\":1}},\"result\":$1}}"
}}
case "$input" in
  *sushi-plan*)
    out '"```sushi-plan\n{{\"title\":\"Child\",\"goal\":\"MAKE_CHILD: create child.txt\",\"verify\":[\"test -f child.txt\"]}}\n```"' ;;
  *MAKE_CHILD*)
    echo child > child.txt
    out '"```sushi-report\n{{\"outcome\":\"complete\",\"summary\":\"child done\",\"decisions\":[],\"question\":\"\"}}\n```"' ;;
  *MAKE_DEP*)
    {dep_writes}
    out '"```sushi-report\n{{\"outcome\":\"complete\",\"summary\":\"DEP_SUMMARY\",\"handoff\":\"DEP_HANDOFF\",\"decisions\":[\"DEP_DECISION\"],\"question\":\"\"}}\n```"' ;;
esac
"#
    )
}

fn create_dependency(daemon: &Daemon, repo: &Path, verify: &str) -> serde_json::Value {
    let dep = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.to_str().unwrap(),
            "title": "The dependency",
            "goal": "MAKE_DEP: write the dependency's files",
            "verify": [verify],
            "start": false,
        }),
    );
    let id = dep["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": id}));
    let dep = settle(daemon, &id);
    assert_eq!(dep["status"], "done", "task JSON: {dep}");
    dep
}

fn create_dependent(
    daemon: &Daemon,
    repo: &Path,
    dep_id: &str,
    fields: serde_json::Value,
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "repo": repo.to_str().unwrap(),
        "dependsOn": [dep_id],
        "start": false,
    });
    for (k, v) in fields.as_object().unwrap() {
        body[k] = v.clone();
    }
    let created = daemon.request("task.create", body);
    let id = created["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": id}));
    settle(daemon, &id)
}

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
        let _ = std::fs::remove_dir_all(wt.as_str().unwrap());
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
    let brief = run_file(&daemon, child["id"].as_str().unwrap(), "plan/brief.md");
    assert!(brief.contains("sushi-plan"), "not the plan brief: {brief}");
    assert!(brief.contains("## Landed dependencies"), "{brief}");
    assert!(brief.contains("DEP_SUMMARY"), "{brief}");
    assert!(
        brief.find("## Landed dependencies").unwrap() < brief.find("## Report format").unwrap()
    );

    let worktrees = [dep["worktree"].clone(), child["worktree"].clone()];
    daemon.shutdown_and_wait();
    for wt in worktrees {
        let _ = std::fs::remove_dir_all(wt.as_str().unwrap());
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
    let brief = run_file(&daemon, child["id"].as_str().unwrap(), "plan/brief.md");
    assert!(brief.contains("sushi-plan"), "not the plan brief: {brief}");
    assert!(!brief.contains("## Landed dependencies"), "{brief}");

    let worktrees = [dep["worktree"].clone(), child["worktree"].clone()];
    daemon.shutdown_and_wait();
    for wt in worktrees {
        if let Some(wt) = wt.as_str() {
            let _ = std::fs::remove_dir_all(wt);
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
        let _ = std::fs::remove_dir_all(wt.as_str().unwrap());
    }
}

/// Plans a request as a parent (title "Parent", verify `__PARENT_VERIFY__`)
/// with two independent parts that write one.txt and two.txt. Only a run
/// whose brief carries the parent's failure output writes fix.txt.
const SPLIT_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
out() {
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  printf '%s\n' "{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":$1}"
}
report='"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"'
case "$input" in
  *sushi-plan*)
    case "$input" in
      *PART_ONE*) out '"```sushi-plan\n{\"title\":\"Part One\",\"goal\":\"PART_ONE: create one.txt\",\"verify\":[\"test -f one.txt\"]}\n```"' ;;
      *PART_TWO*) out '"```sushi-plan\n{\"title\":\"Part Two\",\"goal\":\"PART_TWO: create two.txt\",\"verify\":[\"test -f two.txt\"]}\n```"' ;;
      *) out '"```sushi-plan\n{\"title\":\"Parent\",\"goal\":\"Both parts\",\"verify\":[\"__PARENT_VERIFY__\"],\"subtasks\":[{\"key\":\"a\",\"title\":\"Part One\",\"request\":\"PART_ONE create one.txt\"},{\"key\":\"b\",\"title\":\"Part Two\",\"request\":\"PART_TWO create two.txt\"}]}\n```"' ;;
    esac
    ;;
  *NEEDS-FIX*) echo fix > fix.txt; out "$report" ;;
  *PART_ONE*) echo one > one.txt; out "$report" ;;
  *PART_TWO*) echo two > two.txt; out "$report" ;;
esac
"#;

fn split_daemon(parent_verify: &str) -> (Daemon, tempfile::TempDir) {
    let scripts_dir = tempfile::tempdir().unwrap();
    let body = SPLIT_SCRIPT.replace("__PARENT_VERIFY__", parent_verify);
    let script = fake_harness_script(scripts_dir.path(), "fake-claude.sh", &body);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    review_off(&daemon);
    (daemon, scripts_dir)
}

/// A repo whose current branch is the non-default `work`.
fn repo_on_work_branch() -> tempfile::TempDir {
    let repo = init_git_repo();
    git_out(repo.path(), &["checkout", "-q", "-b", "work"]);
    repo
}

fn create_split(daemon: &Daemon, repo: &Path, land: bool) -> String {
    let parent = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.to_str().unwrap(),
            "request": "build both parts",
            "land": land,
            "start": true,
        }),
    );
    parent["id"].as_str().unwrap().to_string()
}

fn children_of(daemon: &Daemon, parent: &serde_json::Value) -> Vec<serde_json::Value> {
    let tasks = daemon.request("task.list", serde_json::json!({"repo": parent["repo"]}));
    tasks
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["parent"] == parent["id"])
        .cloned()
        .collect()
}

fn remove_worktrees(tasks: &[&serde_json::Value]) {
    for t in tasks {
        let _ = std::fs::remove_dir_all(t["worktree"].as_str().unwrap());
    }
}

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

    let parent = poll_until(&daemon, &parent_id, Duration::from_secs(60), |s| {
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
        serde_json::json!(["keep this check", "drop this check", "stop"])
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
        serde_json::json!({"id": parent_id, "answer": "drop this check"}),
    );
    let parent = poll_until(&daemon, &parent_id, Duration::from_secs(60), |s| {
        matches!(s, "done" | "failed" | "stopped")
    });
    assert_eq!(parent["status"], "done", "task JSON: {parent}");
    assert_eq!(parent["verify"], serde_json::json!([]), "{parent}");

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
    let parent = poll_until(&daemon, &parent_id, Duration::from_secs(60), |s| {
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
    let body = SPLIT_SCRIPT.replace("__PARENT_VERIFY__", "echo no library targets; exit 101");
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
    let _ = daemon.request("shutdown", serde_json::json!({}));
    let start = Instant::now();
    while !matches!(daemon.child.try_wait(), Ok(Some(_))) {
        assert!(start.elapsed() < Duration::from_secs(10), "no exit");
        std::thread::sleep(Duration::from_millis(50));
    }
    daemon.socket = daemon.data_dir().join("orchd2.sock");
    daemon.child = spawn_orchd_raw(daemon.data_dir(), &daemon.socket, &env);
    wait_for_socket(&daemon.socket);
    daemon.token = read_control_token(daemon.data_dir());

    daemon.request(
        "task.answer",
        serde_json::json!({"id": parent_id, "answer": "stop"}),
    );
    poll_until(&daemon, &parent_id, Duration::from_secs(15), |s| {
        s == "stopped"
    });
    // Give a wrongly relaunched loop time to run its preflight again.
    std::thread::sleep(Duration::from_millis(1500));
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
