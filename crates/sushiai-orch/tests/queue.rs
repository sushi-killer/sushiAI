//! Black-box integration tests (file queue): a lease holds only the files a
//! task really changed, so tasks whose plans overlap but whose edits do not
//! run and land together; a real conflict gets one more attempt on the
//! newest base; subtasks with overlapping paths merge into one task with a
//! checklist; a parent runs at most `childParallel` subtasks; a waiting task
//! shows what it waits for. Real daemon, real git.

mod common;

use common::*;
use std::path::Path;
use std::time::Duration;

/// Writes for the task whose goal names `FILE_<name>`: `a` makes `shared.txt`,
/// `b` appends to it, `early` writes `early.txt` before it holds (a file a
/// shell command wrote, no Edit tool), anything else writes `<name>.txt`. A
/// conflicted `shared.txt` is resolved by keeping both sides. A task holds
/// until `$GATES/hold_<name>` is gone; `$GATES/started_<name>` says it began.
const QUEUE_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
name=$(printf '%s' "$input" | grep -o 'FILE_[a-z0-9]*' | head -1 | sed 's/FILE_//')
touch "$GATES/started_$name"
if [ "$name" = early ]; then echo early > early.txt; fi
while [ -e "$GATES/hold_$name" ]; do sleep 0.1; done
if grep -q '<<<<<<<' shared.txt 2>/dev/null; then printf 'a\nb\n' > shared.txt; else
case "$name" in
  a) echo a > shared.txt ;;
  b) echo b >> shared.txt ;;
  *) echo "$name" > "$name.txt" ;;
esac
fi
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
    // A route for the cheap tier that runs on the fake harness.
    cheap_route_on_claude(&daemon);
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
    wait_until(what, || cond().then_some(()));
}

fn started(gates: &Path, name: &str) -> bool {
    gates.join(format!("started_{name}")).exists()
}

fn until_done(daemon: &Daemon, id: &str) -> serde_json::Value {
    poll_until(daemon, id, |s| matches!(s, "done" | "failed" | "stopped"))
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
fn tasks_whose_plans_overlap_but_whose_edits_do_not_start_together_and_both_land() {
    let gates = tempfile::tempdir().unwrap();
    let (daemon, _scripts) = daemon(gates.path());
    let repo = repo_on_work_branch();
    let root = repo.path();
    // Both plans name `shared.txt`; the real edits are `one.txt` and `two.txt`.
    let one = create(
        &daemon,
        root,
        "one",
        "test -f one.txt",
        &["shared.txt"],
        true,
    );
    let two = create(
        &daemon,
        root,
        "two",
        "test -f two.txt",
        &["shared.txt"],
        true,
    );
    hold(gates.path(), "one");
    hold(gates.path(), "two");
    start(&daemon, &one);
    start(&daemon, &two);
    // Both run before either one ends: a planned path parks nobody.
    wait_for("both started while both are held", || {
        started(gates.path(), "one") && started(gates.path(), "two")
    });
    assert!(get(&daemon, &one)["wait"].is_null());
    assert!(get(&daemon, &two)["wait"].is_null());
    release(gates.path(), "one");
    release(gates.path(), "two");
    let one_done = until_done(&daemon, &one);
    let two_done = until_done(&daemon, &two);
    for done in [&one_done, &two_done] {
        assert_eq!(done["status"], "done", "{done}");
        assert!(done["landedSha"].is_string(), "{done}");
        assert!(!decisions(done).contains("Queue:"), "{done}");
    }
    assert_ne!(one_done["landedSha"], two_done["landedSha"]);
    let log = git_out(root, &["log", "--format=%s", "work"]);
    assert!(log.contains("Add one") && log.contains("Add two"), "{log}");
    assert!(root.join("one.txt").exists() && root.join("two.txt").exists());
    daemon.shutdown_and_wait();
}

/// The brief file of the implement attempt `n` of a task.
fn attempt_brief(daemon: &Daemon, id: &str, n: u64) -> String {
    std::fs::read_to_string(
        daemon
            .data_dir()
            .join("tasks")
            .join(id)
            .join("runs")
            .join(n.to_string())
            .join("brief.md"),
    )
    .unwrap()
}

#[test]
fn a_real_conflict_gives_one_more_attempt_on_the_newest_base_and_lands_with_both_edits() {
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
    // Both create `shared.txt`: b conflicts once a has landed.
    let b = create(
        &daemon,
        root,
        "b",
        "grep -q a shared.txt && grep -q b shared.txt",
        &["shared.txt"],
        true,
    );
    hold(gates.path(), "a");
    hold(gates.path(), "b");
    start(&daemon, &a);
    start(&daemon, &b);
    wait_for("both started", || {
        started(gates.path(), "a") && started(gates.path(), "b")
    });
    release(gates.path(), "a");
    let a_done = until_done(&daemon, &a);
    assert_eq!(a_done["status"], "done", "{a_done}");
    release(gates.path(), "b");
    let b_done = until_done(&daemon, &b);
    assert_eq!(b_done["status"], "done", "{b_done}");
    assert!(b_done["landedSha"].is_string(), "{b_done}");

    let attempts = implement_attempts(&b_done);
    assert_eq!(attempts.len(), 2, "{b_done}");
    assert_eq!(attempts[0]["failure"]["kind"], "conflict", "{b_done}");
    let first_failure = attempts[0]["failure"]["detail"].as_str().unwrap();
    assert!(first_failure.contains("shared.txt"), "{first_failure}");
    // Attempt 2 starts on a's landing and its brief names the file.
    assert_eq!(b_done["baseSha"], a_done["landedSha"], "{b_done}");
    let brief = attempt_brief(&daemon, &b, attempts[1]["n"].as_u64().unwrap());
    assert!(brief.contains("shared.txt"), "{brief}");
    assert!(
        brief.contains(&a_done["landedSha"].as_str().unwrap()[..7]),
        "{brief}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("shared.txt")).unwrap(),
        "a\nb\n",
        "{b_done}"
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
    assert!(get(&daemon, &one)["wait"].is_null());
    assert!(get(&daemon, &two)["wait"].is_null());
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
fn hook_edit_refuses_a_file_another_live_task_really_changed_and_not_one_it_only_planned() {
    let gates = tempfile::tempdir().unwrap();
    let (daemon, _scripts) = daemon(gates.path());
    let repo = init_git_repo();
    // `early` plans `planned/` but writes `early.txt` from a shell command.
    let a = create(
        &daemon,
        repo.path(),
        "early",
        "test -f early.txt",
        &["planned"],
        false,
    );
    let b = create(&daemon, repo.path(), "two", "test -f two.txt", &[], false);
    hold(gates.path(), "early");
    hold(gates.path(), "two");
    start(&daemon, &a);
    start(&daemon, &b);
    wait_for("both started", || {
        started(gates.path(), "early") && started(gates.path(), "two")
    });
    let (token_a, token_b) = (hook_token(&daemon, &a), hook_token(&daemon, &b));
    let a_worktree = get(&daemon, &a)["worktree"].as_str().unwrap().to_string();
    let b_worktree = get(&daemon, &b)["worktree"].as_str().unwrap().to_string();
    wait_for("the shell command wrote early.txt", || {
        Path::new(&a_worktree).join("early.txt").exists()
    });

    // A file the first task really changed, though no Edit tool touched it.
    let held = format!("{b_worktree}/early.txt");
    let reason = denied(&hook_edit(&daemon, &token_b, &held)).expect("denied");
    assert!(reason.contains("Add early"), "{reason}");
    assert!(reason.contains("Continue with other files"), "{reason}");
    assert!(reason.contains("after that task lands"), "{reason}");
    let refused = get(&daemon, &b);
    assert!(decisions(&refused).contains("Queue: edit of early.txt refused"));
    assert_eq!(
        refused["wait"],
        serde_json::json!({"kind": "file", "taskId": a, "title": "Add early", "path": "early.txt"}),
        "{refused}"
    );

    // A file the first task only planned is free.
    let planned = format!("{b_worktree}/planned/x.rs");
    assert!(denied(&hook_edit(&daemon, &token_b, &planned)).is_none());
    // An unleased file is leased to the caller, so the other task is refused.
    let other = format!("{b_worktree}/other.txt");
    assert!(denied(&hook_edit(&daemon, &token_b, &other)).is_none());
    let reason = denied(&hook_edit(
        &daemon,
        &token_a,
        &format!("{a_worktree}/other.txt"),
    ))
    .expect("a is refused b's file");
    assert!(reason.contains("Add two"), "{reason}");
    // A file outside the worktree is nobody's.
    assert!(denied(&hook_edit(&daemon, &token_b, "/etc/hosts")).is_none());

    release(gates.path(), "early");
    assert_eq!(until_done(&daemon, &a)["status"], "done");
    // The holder ended: the wait is cleared and the edit is allowed.
    wait_for("the file wait to clear", || {
        get(&daemon, &b)["wait"].is_null()
    });
    assert!(denied(&hook_edit(&daemon, &token_b, &held)).is_none());
    release(gates.path(), "two");
    until_done(&daemon, &b);
    daemon.shutdown_and_wait();
}

#[test]
fn a_task_waiting_for_a_global_slot_shows_the_slot_wait() {
    let gates = tempfile::tempdir().unwrap();
    let (daemon, _scripts) = daemon(gates.path());
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["parallel"] = serde_json::json!(1);
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
    let repo = init_git_repo();
    let one = create(&daemon, repo.path(), "one", "test -f one.txt", &[], false);
    let two = create(&daemon, repo.path(), "two", "test -f two.txt", &[], false);
    hold(gates.path(), "one");
    start(&daemon, &one);
    wait_for("one running", || started(gates.path(), "one"));
    start(&daemon, &two);
    let waiting = wait_until("two waits for the slot", || {
        let t = get(&daemon, &two);
        (t["wait"]["kind"] == "slot").then_some(t)
    });
    assert_eq!(waiting["wait"]["running"], 1, "{waiting}");
    assert_eq!(waiting["wait"]["limit"], 1, "{waiting}");
    assert!(!started(gates.path(), "two"));
    release(gates.path(), "one");
    assert_eq!(until_done(&daemon, &one)["status"], "done");
    let two_done = until_done(&daemon, &two);
    assert_eq!(two_done["status"], "done", "{two_done}");
    assert!(two_done["wait"].is_null(), "{two_done}");
    daemon.shutdown_and_wait();
}

/// Plans a request as three parts on the same file (paths overlap), one
/// appending a line each; a fourth part on its own file, with its own check,
/// runs beside them. The three merge into one task with a checklist.
const MERGE_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
out() {
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  printf '%s\n' "{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":$1}"
}
report='"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"'
case "$input" in
  *"You review a coding task plan"*) printf '%s\n' '{"type":"result","result":"{\"findings\": []}"}' ;;
  *sushi-plan*)
    case "$input" in
      *PART_SIDE*) out '"```sushi-plan\n{\"title\":\"Side\",\"goal\":\"PART_SIDE: side.txt\",\"verify\":[\"test -f side.txt\"]}\n```"' ;;
      *PART_ONE*) out '"```sushi-plan\n{\"title\":\"Lines\",\"goal\":\"PART_ONE: add lines 1, 2 and 3\",\"verify\":[\"grep -q 3 shared.txt\"]}\n```"' ;;
      *) out '"```sushi-plan\n{\"title\":\"All parts\",\"goal\":\"Build all\",\"verify\":[\"true\"],\"subtasks\":[{\"key\":\"one\",\"title\":\"Part 1\",\"request\":\"PART_ONE\",\"paths\":[\"shared.txt\"],\"check\":\"grep -qx 1 shared.txt\"},{\"key\":\"two\",\"title\":\"Part 2\",\"request\":\"PART_TWO\",\"paths\":[\"shared.txt\"],\"check\":\"grep -qx 2 shared.txt\"},{\"key\":\"three\",\"title\":\"Part 3\",\"request\":\"PART_THREE\",\"paths\":[\"shared.txt\"],\"check\":\"grep -qx 3 shared.txt\"},{\"key\":\"side\",\"title\":\"Side\",\"request\":\"PART_SIDE\",\"paths\":[\"side.txt\"],\"check\":\"test -f side.txt\"}]}\n```"' ;;
    esac
    ;;
  *PART_SIDE*) echo side > side.txt; out "$report" ;;
  *PART_ONE*) printf '1\n2\n3\n' > shared.txt; out "$report" ;;
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
fn overlapping_subtasks_merge_into_one_child_with_a_checklist_and_the_side_part_stays_apart() {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake-claude.sh", MERGE_SCRIPT);
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
    let parent = poll_until(&daemon, &parent_id, |s| {
        matches!(s, "done" | "failed" | "stopped" | "waiting")
    });
    assert_eq!(parent["status"], "done", "{parent}");
    let children = children_by_title(&daemon, &parent);
    assert_eq!(children.len(), 2, "{children:?}");
    let [merged, side] = [&children[0], &children[1]];
    for c in &children {
        assert_eq!(c["status"], "done", "{c}");
    }
    // The child is planned on its own, so its draft names it.
    assert_eq!(merged["title"], "Lines", "{merged}");
    let checklist = merged["checklist"].as_array().unwrap();
    assert_eq!(checklist.len(), 3, "{merged}");
    assert!(checklist.iter().all(|i| i["passes"] == true), "{merged}");
    assert!(side["checklist"].is_null(), "{side}");
    assert!(
        decisions(&parent).contains("Plan gate: merged 3 subtasks into one task with a checklist"),
        "{parent}"
    );

    // The parent's branch got the merged work and the side part.
    let root = repo.path();
    let branch = parent["branch"].as_str().unwrap();
    let log: Vec<String> = git_out(root, &["log", "--format=%s", branch])
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(log.len(), 3, "{log:?}");
    assert!(log.contains(&"Lines".to_string()) && log.contains(&"Side".to_string()));
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
  *"You review a coding task plan"*) printf '%s\n' '{"type":"result","result":"{\"findings\": []}"}' ;;
  *sushi-plan*)
    case "$input" in
      *PART_A*) plan "Part A" PART_A a ;;
      *PART_B*) plan "Part B" PART_B b ;;
      *PART_C*) plan "Part C" PART_C c ;;
      *PART_D*) plan "Part D" PART_D d ;;
      *) out '"```sushi-plan\n{\"title\":\"Parts\",\"goal\":\"Build all\",\"verify\":[\"true\"],\"subtasks\":[{\"key\":\"a\",\"title\":\"Part A\",\"request\":\"PART_A\",\"paths\":[\"a\"],\"check\":\"test -f a.txt\"},{\"key\":\"b\",\"title\":\"Part B\",\"request\":\"PART_B\",\"paths\":[\"b\"],\"check\":\"test -f b.txt\",\"dependsOn\":[\"a\"]},{\"key\":\"c\",\"title\":\"Part C\",\"request\":\"PART_C\",\"paths\":[\"c\"],\"check\":\"test -f c.txt\"},{\"key\":\"d\",\"title\":\"Part D\",\"request\":\"PART_D\",\"paths\":[\"d\"],\"check\":\"test -f d.txt\"}]}\n```"' ;;
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
    settings["parallel"] = serde_json::json!(8);
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
    // Absence check: this window gives a forbidden start time to happen.
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

#[test]
fn independent_subtasks_start_at_the_same_time_up_to_child_parallel() {
    let gates = tempfile::tempdir().unwrap();
    let (daemon, _scripts) = split_daemon(gates.path(), 3);
    let repo = init_git_repo();
    for name in ["a", "c", "d"] {
        hold(gates.path(), name);
    }
    let parent = create_split(&daemon, repo.path());
    let parent_id = parent["id"].as_str().unwrap().to_string();
    // A, C and D have separate paths and no dependsOn: all three run while
    // all three are held. B waits for A.
    wait_for("a, c and d running at once", || {
        ["a", "c", "d"].iter().all(|n| started(gates.path(), n))
    });
    assert!(!started(gates.path(), "b"));
    for name in ["a", "c", "d"] {
        release(gates.path(), name);
    }
    let parent = until_done(&daemon, &parent_id);
    assert_eq!(parent["status"], "done", "{parent}");
    daemon.shutdown_and_wait();
}

#[test]
fn a_waiting_subtask_shows_a_dependency_or_a_subtask_slot_wait() {
    let gates = tempfile::tempdir().unwrap();
    let (daemon, _scripts) = split_daemon(gates.path(), 1);
    let repo = init_git_repo();
    for name in ["a", "c", "d"] {
        hold(gates.path(), name);
    }
    let parent = create_split(&daemon, repo.path());
    let parent_id = parent["id"].as_str().unwrap().to_string();
    // One of the three free parts takes the only subtask slot.
    wait_for("a part running", || {
        ["a", "c", "d"].iter().any(|n| started(gates.path(), n))
    });
    let parent = get(&daemon, &parent_id);
    let children = wait_until("the waiting parts show why", || {
        let children = children_by_title(&daemon, &parent);
        let waits = |kind: &str| {
            children
                .iter()
                .filter(|c| c["wait"]["kind"] == kind)
                .count()
        };
        (waits("subtaskSlot") == 2 && waits("dependency") == 1).then_some(children)
    });
    let part_b = children.iter().find(|c| c["title"] == "Part B").unwrap();
    assert_eq!(part_b["wait"]["titles"], serde_json::json!(["Part A"]));
    let slot = children
        .iter()
        .find(|c| c["wait"]["kind"] == "subtaskSlot")
        .unwrap();
    assert_eq!(slot["wait"]["running"], 1, "{slot}");
    assert_eq!(slot["wait"]["limit"], 1, "{slot}");
    for name in ["a", "c", "d"] {
        release(gates.path(), name);
    }
    let parent = until_done(&daemon, &parent_id);
    assert_eq!(parent["status"], "done", "{parent}");
    let finished = children_by_title(&daemon, &parent);
    assert!(finished.iter().all(|c| c["wait"].is_null()), "{finished:?}");
    daemon.shutdown_and_wait();
}

/// Plans, brief checks and implement runs for the wall-time fixture: the
/// plan replies are files in `$GATES`, the brief check takes a fixed second.
const WALL_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
init='{"type":"system","subtype":"init","session_id":"sess-fake"}'
result() {
  printf '%s\n' "$init"
  printf '%s\n' "{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":$1}"
}
case "$input" in
  *"You check a coding task brief"*)
    sleep 1
    result '"{\"contradiction\": false, \"conflict\": \"\"}"' ;;
  *"You review a coding task plan"*) printf '%s\n' '{"type":"result","result":"{\"findings\": []}"}' ;;
  *sushi-plan*)
    case "$input" in
      *PART_A*) which=a ;;
      *PART_B*) which=b ;;
      *PART_C*) which=c ;;
      *) which=parent ;;
    esac
    printf '%s\n' "$init"
    cat "$GATES/plan_$which.json" ;;
  *PART_*)
    name=$(printf '%s' "$input" | grep -o 'PART_[A-C]' | head -1 | sed 's/PART_//' | tr 'A-C' 'a-c')
    echo "$name" > "$name.txt"
    result '"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"' ;;
esac
"#;

/// A planner reply carrying `plan` as a `sushi-plan` block.
fn plan_reply(plan: serde_json::Value) -> String {
    serde_json::json!({
        "type": "result",
        "total_cost_usd": 0.01,
        "usage": {"input_tokens": 1, "output_tokens": 1},
        "result": format!("```sushi-plan\n{plan}\n```"),
    })
    .to_string()
}

/// Three independent parts, a fixed brief-check time (1 s) and a fixed time
/// for a base run of each part's check (3 s): the wall time of the whole
/// graph, printed as `wall_ms=`. The same test runs on any commit.
#[test]
fn three_independent_parts_wall_time() {
    let gates = tempfile::tempdir().unwrap();
    let verify = "case \"$PWD\" in */base-runs/*) sleep 3;; esac; true";
    let parts: Vec<serde_json::Value> = ["a", "b", "c"]
        .iter()
        .map(|k| {
            serde_json::json!({
                "key": k, "title": format!("Part {}", k.to_uppercase()),
                "request": format!("PART_{}", k.to_uppercase()),
                "paths": [k], "check": format!("test -f {k}.txt"),
            })
        })
        .collect();
    let parent = serde_json::json!({
        "title": "Parts", "goal": "Build all", "verify": ["true"], "subtasks": parts,
    });
    std::fs::write(gates.path().join("plan_parent.json"), plan_reply(parent)).unwrap();
    for k in ["a", "b", "c"] {
        let part = serde_json::json!({
            "title": format!("Part {}", k.to_uppercase()),
            "goal": format!("PART_{}: write {k}.txt", k.to_uppercase()),
            "verify": [verify],
        });
        std::fs::write(
            gates.path().join(format!("plan_{k}.json")),
            plan_reply(part),
        )
        .unwrap();
    }
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake-claude.sh", WALL_SCRIPT);
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("GATES", gates.path().to_str().unwrap()),
    ]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    settings["childParallel"] = serde_json::json!(3);
    settings["parallel"] = serde_json::json!(8);
    settings["briefCheckRoute"] = serde_json::json!("claude-sonnet");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
    let repo = init_git_repo();
    let begun = std::time::Instant::now();
    let parent = create_split(&daemon, repo.path());
    let parent_id = parent["id"].as_str().unwrap().to_string();
    let parent = until_done(&daemon, &parent_id);
    let wall_ms = begun.elapsed().as_millis();
    assert_eq!(parent["status"], "done", "{parent}");
    assert_eq!(children_by_title(&daemon, &parent).len(), 3);
    println!("wall_ms={wall_ms}");
    daemon.shutdown_and_wait();
}
