//! Black-box integration tests (file queue): tasks on one base never hold the
//! same path at once, subtasks with overlapping paths relay one branch, and
//! a parent runs at most `childParallel` subtasks. Real daemon, real git.

mod common;

use common::*;
use std::path::Path;
use std::time::{Duration, Instant};

/// Writes for the task whose goal names `FILE_<name>`: `a` makes `shared.txt`,
/// `b` appends to it, anything else writes `<name>.txt`. A task holds until
/// `$GATES/hold_<name>` is gone; `$GATES/started_<name>` says it began.
const QUEUE_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
name=$(printf '%s' "$input" | grep -o 'FILE_[a-z0-9]*' | head -1 | sed 's/FILE_//')
touch "$GATES/started_$name"
while [ -e "$GATES/hold_$name" ]; do sleep 0.1; done
case "$name" in
  a) echo a > shared.txt ;;
  b) echo b >> shared.txt ;;
  *) echo "$name" > "$name.txt" ;;
esac
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}'
"#;

fn daemon(gates: &Path) -> (Daemon, tempfile::TempDir) {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake-claude.sh", QUEUE_SCRIPT);
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("GATES", gates.to_str().unwrap()),
    ]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
    (daemon, scripts)
}

fn repo_on_work_branch() -> tempfile::TempDir {
    let repo = init_git_repo();
    git_out(repo.path(), &["checkout", "-q", "-b", "work"]);
    repo
}

fn create(
    daemon: &Daemon,
    repo: &Path,
    name: &str,
    verify: &str,
    paths: &[&str],
    land: bool,
) -> String {
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.to_str().unwrap(),
            "title": format!("Add {name}"),
            "goal": format!("FILE_{name}: write the file"),
            "verify": [verify],
            "paths": paths,
            "land": land,
            "start": false,
        }),
    );
    task["id"].as_str().unwrap().to_string()
}

fn start(daemon: &Daemon, id: &str) {
    daemon.request("task.start", serde_json::json!({"id": id}));
}

fn hold(gates: &Path, name: &str) {
    std::fs::write(gates.join(format!("hold_{name}")), "").unwrap();
}

fn release(gates: &Path, name: &str) {
    let _ = std::fs::remove_file(gates.join(format!("hold_{name}")));
}

fn wait_for(what: &str, cond: impl Fn() -> bool) {
    let begin = Instant::now();
    while !cond() {
        assert!(begin.elapsed() < Duration::from_secs(30), "never: {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn started(gates: &Path, name: &str) -> bool {
    gates.join(format!("started_{name}")).exists()
}

fn until_done(daemon: &Daemon, id: &str) -> serde_json::Value {
    poll_until(daemon, id, Duration::from_secs(60), |s| {
        matches!(s, "done" | "failed" | "stopped")
    })
}

fn get(daemon: &Daemon, id: &str) -> serde_json::Value {
    daemon.request("task.get", serde_json::json!({"id": id}))
}

fn decisions(task: &serde_json::Value) -> String {
    task["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d.as_str())
        .collect::<Vec<_>>()
        .join("\n")
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

#[test]
fn a_task_with_overlapping_paths_waits_for_the_first_and_starts_on_top_of_its_landing() {
    let gates = tempfile::tempdir().unwrap();
    let (daemon, _scripts) = daemon(gates.path());
    let repo = repo_on_work_branch();
    let root = repo.path();
    let a = create(
        &daemon,
        root,
        "a",
        "grep -q a shared.txt",
        &["shared.txt"],
        true,
    );
    // The second one needs what the first landed: it fails verify (and
    // conflicts at landing) unless it starts on top of that landing.
    let b = create(
        &daemon,
        root,
        "b",
        "grep -q a shared.txt && grep -q b shared.txt",
        &["shared.txt"],
        true,
    );
    hold(gates.path(), "a");
    start(&daemon, &a);
    wait_for("a started", || started(gates.path(), "a"));
    start(&daemon, &b);

    wait_for("b queued behind a", || {
        get(&daemon, &b)["queueReason"].is_string()
    });
    let waiting = get(&daemon, &b);
    assert_eq!(waiting["status"], "queued", "{waiting}");
    let reason = waiting["queueReason"].as_str().unwrap_or_default();
    assert!(
        reason.contains("Add a") && reason.contains("shared.txt"),
        "{waiting}"
    );
    assert!(decisions(&waiting).contains("Queue: waits for \"Add a\" on shared.txt"));
    std::thread::sleep(Duration::from_millis(600));
    assert!(!started(gates.path(), "b"), "b ran while a held shared.txt");
    assert!(get(&daemon, &b)["attempts"].as_array().unwrap().is_empty());

    release(gates.path(), "a");
    let a_done = until_done(&daemon, &a);
    let b_done = until_done(&daemon, &b);
    assert_eq!(a_done["status"], "done", "{a_done}");
    assert_eq!(b_done["status"], "done", "{b_done}");
    assert!(b_done["queueReason"].is_null(), "{b_done}");
    assert!(
        b_done["attempts"][0]["startedAt"].as_i64() >= a_done["attempts"][0]["endedAt"].as_i64(),
        "b started before a ended"
    );
    // Carried onto the new base head before its attempt: one attempt, no
    // conflict attempt.
    assert_eq!(implement_attempts(&b_done).len(), 1, "{b_done}");
    assert!(
        decisions(&b_done).contains("Rebase: started from work"),
        "{b_done}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("shared.txt")).unwrap(),
        "a\nb\n"
    );
    daemon.shutdown_and_wait();
}

#[test]
fn tasks_with_disjoint_paths_run_at_the_same_time() {
    let gates = tempfile::tempdir().unwrap();
    let (daemon, _scripts) = daemon(gates.path());
    let repo = init_git_repo();
    let one = create(
        &daemon,
        repo.path(),
        "one",
        "test -f one.txt",
        &["src/one"],
        false,
    );
    let two = create(
        &daemon,
        repo.path(),
        "two",
        "test -f two.txt",
        &["src/two"],
        false,
    );
    hold(gates.path(), "one");
    hold(gates.path(), "two");
    start(&daemon, &one);
    start(&daemon, &two);
    wait_for("both running at once", || {
        started(gates.path(), "one") && started(gates.path(), "two")
    });
    assert!(get(&daemon, &one)["queueReason"].is_null());
    assert!(get(&daemon, &two)["queueReason"].is_null());
    release(gates.path(), "one");
    release(gates.path(), "two");
    assert_eq!(until_done(&daemon, &one)["status"], "done");
    assert_eq!(until_done(&daemon, &two)["status"], "done");
    daemon.shutdown_and_wait();
}

/// The token of the run's hook wiring, read from its `settings.json`.
fn hook_token(daemon: &Daemon, task_id: &str) -> String {
    let settings: serde_json::Value =
        serde_json::from_str(&run_file(daemon, task_id, "settings.json")).unwrap();
    let command = settings["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    assert_eq!(
        settings["hooks"]["PreToolUse"][0]["matcher"],
        "Edit|Write|MultiEdit|NotebookEdit|mcp__.*|WebFetch|WebSearch"
    );
    command
        .split("SUSHIAI_ORCH_TOKEN='")
        .nth(1)
        .and_then(|rest| rest.split('\'').next())
        .unwrap()
        .to_string()
}

fn hook_edit(daemon: &Daemon, token: &str, file: &str) -> serde_json::Value {
    daemon.request(
        "hook.edit",
        serde_json::json!({
            "token": token,
            "payload": {"tool_name": "Edit", "tool_input": {"file_path": file}},
        }),
    )
}

fn denied(result: &serde_json::Value) -> Option<String> {
    let out = &result["hookSpecificOutput"];
    (out["permissionDecision"] == "deny").then(|| {
        out["permissionDecisionReason"]
            .as_str()
            .unwrap()
            .to_string()
    })
}

#[test]
fn hook_edit_denies_a_file_another_live_task_holds_and_allows_it_after_that_task_lands() {
    let gates = tempfile::tempdir().unwrap();
    let (daemon, _scripts) = daemon(gates.path());
    let repo = init_git_repo();
    let a = create(
        &daemon,
        repo.path(),
        "a",
        "test -f shared.txt",
        &["shared"],
        false,
    );
    let b = create(&daemon, repo.path(), "b", "test -f shared.txt", &[], false);
    hold(gates.path(), "a");
    hold(gates.path(), "b");
    start(&daemon, &a);
    wait_for("a started", || started(gates.path(), "a"));
    // No declared paths: not queued behind a.
    start(&daemon, &b);
    wait_for("b started", || started(gates.path(), "b"));
    let (token_a, token_b) = (hook_token(&daemon, &a), hook_token(&daemon, &b));
    let b_worktree = get(&daemon, &b)["worktree"].as_str().unwrap().to_string();

    // A declared `shared`: b may not edit inside it.
    let held = format!("{b_worktree}/shared/x.rs");
    let reason = denied(&hook_edit(&daemon, &token_b, &held)).expect("denied");
    assert!(reason.contains("Add a"), "{reason}");
    assert!(reason.contains("Continue with other files"), "{reason}");
    assert!(reason.contains("after that task lands"), "{reason}");
    assert!(decisions(&get(&daemon, &b)).contains("Queue: edit of shared/x.rs refused"));

    // An unleased file is leased to the caller, so the other task is refused.
    let other = format!("{b_worktree}/other.txt");
    assert!(denied(&hook_edit(&daemon, &token_b, &other)).is_none());
    let a_worktree = get(&daemon, &a)["worktree"].as_str().unwrap().to_string();
    let reason = denied(&hook_edit(
        &daemon,
        &token_a,
        &format!("{a_worktree}/other.txt"),
    ))
    .expect("a is refused b's file");
    assert!(reason.contains("Add b"), "{reason}");
    // A file outside the worktree is nobody's.
    assert!(denied(&hook_edit(&daemon, &token_b, "/etc/hosts")).is_none());

    release(gates.path(), "a");
    assert_eq!(until_done(&daemon, &a)["status"], "done");
    assert!(denied(&hook_edit(&daemon, &token_b, &held)).is_none());
    release(gates.path(), "b");
    until_done(&daemon, &b);
    daemon.shutdown_and_wait();
}

/// Plans a request as three parts on the same file (paths overlap), one
/// appending a line each; a fourth part on its own file runs beside them.
const RELAY_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
out() {
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  printf '%s\n' "{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":$1}"
}
report1='"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"handoff\":\"HANDOFF_ONE_MARK\",\"decisions\":[],\"question\":\"\"}\n```"'
report='"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"handoff\":\"finished\",\"decisions\":[],\"question\":\"\"}\n```"'
case "$input" in
  *sushi-plan*)
    case "$input" in
      *PART_ONE*) out '"```sushi-plan\n{\"title\":\"Part 1\",\"goal\":\"PART_ONE: add line 1\",\"verify\":[\"grep -q 1 shared.txt\"]}\n```"' ;;
      *PART_TWO*) out '"```sushi-plan\n{\"title\":\"Part 2\",\"goal\":\"PART_TWO: add line 2\",\"verify\":[\"grep -q 1 shared.txt && grep -q 2 shared.txt\"]}\n```"' ;;
      *PART_THREE*) out '"```sushi-plan\n{\"title\":\"Part 3\",\"goal\":\"PART_THREE: add line 3\",\"verify\":[\"grep -q 2 shared.txt && grep -q 3 shared.txt\"]}\n```"' ;;
      *PART_SIDE*) out '"```sushi-plan\n{\"title\":\"Side\",\"goal\":\"PART_SIDE: side.txt\",\"verify\":[\"test -f side.txt\"]}\n```"' ;;
      *) out '"```sushi-plan\n{\"title\":\"All parts\",\"goal\":\"Build all\",\"verify\":[\"true\"],\"subtasks\":[{\"key\":\"one\",\"title\":\"Part 1\",\"request\":\"PART_ONE\",\"paths\":[\"shared.txt\"]},{\"key\":\"two\",\"title\":\"Part 2\",\"request\":\"PART_TWO\",\"paths\":[\"shared.txt\"]},{\"key\":\"three\",\"title\":\"Part 3\",\"request\":\"PART_THREE\",\"paths\":[\"shared.txt\"]},{\"key\":\"side\",\"title\":\"Side\",\"request\":\"PART_SIDE\",\"paths\":[\"side.txt\"]}]}\n```"' ;;
    esac
    ;;
  *PART_ONE*) echo 1 >> shared.txt; out "$report1" ;;
  *PART_TWO*) case "$input" in *HANDOFF_ONE_MARK*) echo 2 >> shared.txt ;; esac; out "$report" ;;
  *PART_THREE*) echo 3 >> shared.txt; out "$report" ;;
  *PART_SIDE*) echo side > side.txt; out "$report" ;;
esac
"#;

fn children_by_title(daemon: &Daemon, parent: &serde_json::Value) -> Vec<serde_json::Value> {
    let listed = daemon.request("task.list", serde_json::json!({"repo": parent["repo"]}));
    let mut children: Vec<serde_json::Value> = listed
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["parent"] == parent["id"])
        .cloned()
        .collect();
    children.sort_by_key(|t| t["title"].as_str().unwrap().to_string());
    children
}

#[test]
fn overlapping_subtasks_relay_one_branch_and_the_parent_lands_the_chain_once() {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake-claude.sh", RELAY_SCRIPT);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
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
    let parent = poll_until(&daemon, &parent_id, Duration::from_secs(90), |s| {
        matches!(s, "done" | "failed" | "stopped" | "waiting")
    });
    assert_eq!(parent["status"], "done", "{parent}");
    let children = children_by_title(&daemon, &parent);
    assert_eq!(children.len(), 4, "{children:?}");
    let [one, two, three, side] = [&children[0], &children[1], &children[2], &children[3]];
    for c in &children {
        assert_eq!(c["status"], "done", "{c}");
    }
    assert!(one["relayOf"].is_null(), "{one}");
    assert_eq!(two["relayOf"], one["id"], "{two}");
    assert_eq!(three["relayOf"], two["id"], "{three}");
    assert!(side["relayOf"].is_null(), "{side}");

    // Each link starts from the previous one's commit.
    let root = repo.path();
    let tip = |t: &serde_json::Value| git_out(root, &["rev-parse", t["branch"].as_str().unwrap()]);
    let before = |t: &serde_json::Value| {
        git_out(
            root,
            &["rev-parse", &format!("{}^", t["branch"].as_str().unwrap())],
        )
    };
    assert_eq!(before(two), tip(one), "{two}");
    // The last link's own branch is what landed on the parent, so it shows
    // where it started only in its decision line.
    assert!(
        decisions(three).contains(&format!(
            "Relay: continues \"Part 2\" on {}",
            two["branch"].as_str().unwrap()
        )),
        "{three}"
    );
    assert!(decisions(one).contains("Land: committed"), "{one}");
    assert!(!decisions(one).contains("landed on"), "{one}");
    assert!(
        decisions(three).contains("Land: lands the relay's chain"),
        "{three}"
    );

    // The parent's branch got the chain as one commit, and the side part.
    let branch = parent["branch"].as_str().unwrap();
    let log: Vec<String> = git_out(root, &["log", "--format=%s", branch])
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(log.len(), 3, "{log:?}");
    assert!(log.contains(&"Part 3".to_string()) && log.contains(&"Side".to_string()));
    assert_eq!(
        git_out(root, &["show", &format!("{branch}:shared.txt")]),
        "1\n2\n3"
    );
    daemon.shutdown_and_wait();
}

/// Plans a split into four independent parts; each holds until its gate goes.
const SPLIT_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
out() {
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  printf '%s\n' "{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":$1}"
}
report='"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"'
plan() { out "\"\`\`\`sushi-plan\\n{\\\"title\\\":\\\"$1\\\",\\\"goal\\\":\\\"$2: write $3.txt\\\",\\\"verify\\\":[\\\"test -f $3.txt\\\"]}\\n\`\`\`\""; }
case "$input" in
  *sushi-plan*)
    case "$input" in
      *PART_A*) plan "Part A" PART_A a ;;
      *PART_B*) plan "Part B" PART_B b ;;
      *PART_C*) plan "Part C" PART_C c ;;
      *PART_D*) plan "Part D" PART_D d ;;
      *) out '"```sushi-plan\n{\"title\":\"Parts\",\"goal\":\"Build all\",\"verify\":[\"true\"],\"subtasks\":[{\"key\":\"a\",\"title\":\"Part A\",\"request\":\"PART_A\",\"paths\":[\"a\"]},{\"key\":\"b\",\"title\":\"Part B\",\"request\":\"PART_B\",\"paths\":[\"b\"],\"dependsOn\":[\"a\"]},{\"key\":\"c\",\"title\":\"Part C\",\"request\":\"PART_C\",\"paths\":[\"c\"]},{\"key\":\"d\",\"title\":\"Part D\",\"request\":\"PART_D\",\"paths\":[\"d\"]}]}\n```"' ;;
    esac
    ;;
  *PART_*)
    name=$(printf '%s' "$input" | grep -o 'PART_[A-D]' | head -1 | sed 's/PART_//' | tr 'A-D' 'a-d')
    touch "$GATES/started_$name"
    while [ -e "$GATES/hold_$name" ]; do sleep 0.1; done
    echo "$name" > "$name.txt"
    out "$report"
    ;;
esac
"#;

fn split_daemon(gates: &Path, child_parallel: u32) -> (Daemon, tempfile::TempDir) {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake-claude.sh", SPLIT_SCRIPT);
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("GATES", gates.to_str().unwrap()),
    ]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    settings["childParallel"] = serde_json::json!(child_parallel);
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
    (daemon, scripts)
}

fn create_split(daemon: &Daemon, repo: &Path) -> serde_json::Value {
    daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.to_str().unwrap(),
            "request": "build all parts",
            "start": true,
        }),
    )
}

#[test]
fn a_parent_runs_at_most_child_parallel_subtasks_at_once() {
    let gates = tempfile::tempdir().unwrap();
    let (daemon, _scripts) = split_daemon(gates.path(), 1);
    let repo = init_git_repo();
    let parent = create_split(&daemon, repo.path());
    let parent_id = parent["id"].as_str().unwrap().to_string();
    let parent = until_done(&daemon, &parent_id);
    assert_eq!(parent["status"], "done", "{parent}");
    let mut spans: Vec<(i64, i64)> = children_by_title(&daemon, &parent)
        .iter()
        .map(|c| {
            let attempts = c["attempts"].as_array().unwrap();
            (
                attempts.first().unwrap()["startedAt"].as_i64().unwrap(),
                attempts.last().unwrap()["endedAt"].as_i64().unwrap(),
            )
        })
        .collect();
    assert_eq!(spans.len(), 4);
    spans.sort();
    for pair in spans.windows(2) {
        assert!(pair[1].0 >= pair[0].1, "two subtasks overlapped: {spans:?}");
    }
    daemon.shutdown_and_wait();
}

#[test]
fn a_subtask_is_planned_after_its_dependencies_landed_and_stopping_the_parent_spends_nothing_on_it()
{
    let gates = tempfile::tempdir().unwrap();
    let (daemon, _scripts) = split_daemon(gates.path(), 3);
    let repo = init_git_repo();
    hold(gates.path(), "a");
    let parent = create_split(&daemon, repo.path());
    let parent_id = parent["id"].as_str().unwrap().to_string();
    wait_for("a running", || started(gates.path(), "a"));
    let parent = get(&daemon, &parent_id);
    let children = children_by_title(&daemon, &parent);
    let b = children.iter().find(|c| c["title"] == "Part B").unwrap();
    // B waits for A: it has not been planned, so nothing was spent on it.
    std::thread::sleep(Duration::from_millis(800));
    let b = get(&daemon, b["id"].as_str().unwrap());
    assert!(b["attempts"].as_array().unwrap().is_empty(), "{b}");
    assert_eq!(b["costUsd"].as_f64().unwrap_or(0.0), 0.0, "{b}");

    daemon.request("task.stop", serde_json::json!({"id": parent_id}));
    let b = get(&daemon, b["id"].as_str().unwrap());
    assert_eq!(b["status"], "stopped", "{b}");
    assert!(b["attempts"].as_array().unwrap().is_empty(), "{b}");
    assert_eq!(b["costUsd"].as_f64().unwrap_or(0.0), 0.0, "{b}");
    release(gates.path(), "a");
    daemon.shutdown_and_wait();
}
