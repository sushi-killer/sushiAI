//! Black-box integration tests (graph): spawn the real `orchd` binary and drive
//! it over its NDJSON unix socket.

mod common;

use common::attempts::{plan_file, run_file};
use common::harness::{cheap_route_on_claude, fake_harness_script};
use common::native_sandbox::fit_sandbox;
use common::polling::{
    deadline, holds_for, poll_until, remove_dir_retrying, settle, wait_until, ABSENCE_WINDOW,
};
use common::process::{socket_of, spawn_daemon, wait_for_exit, Daemon};
use common::protocol::{raw_request_with_params, request_on, stop_daemon, wait_for_socket};
use common::repository::{git_out, init_git_repo};

use std::path::Path;

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
  *"You review a coding task plan"*) printf '%s\n' '{"type":"result","result":"{\"findings\": []}"}' ;;
  *sushi-plan*)
    case "$input" in
      *PART_A*) out '"```sushi-plan\n{\"title\":\"Part A\",\"goal\":\"PART_A: create a.txt\",\"verify\":[\"test -f a.txt\"]}\n```"' ;;
      *PART_B*) out '"```sushi-plan\n{\"title\":\"Part B\",\"goal\":\"PART_B: create b.txt next to a.txt\",\"verify\":[\"test -f a.txt && test -f b.txt\"]}\n```"' ;;
      *) out '"```sushi-plan\n{\"title\":\"Both parts\",\"goal\":\"Create a.txt, then b.txt\",\"verify\":[\"test -f a.txt && test -f b.txt\"],\"subtasks\":[{\"key\":\"a\",\"title\":\"Part A\",\"request\":\"PART_A create a.txt\",\"paths\":[\"a.txt\"],\"check\":\"test -f a.txt\"},{\"key\":\"b\",\"title\":\"Part B\",\"request\":\"PART_B create b.txt\",\"paths\":[\"b.txt\"],\"check\":\"test -f b.txt\",\"dependsOn\":[\"a\"]}]}\n```"' ;;
    esac
    ;;
  *PART_A*) echo a > a.txt; out "$report" ;;
  *PART_B*) test -f a.txt && echo b > b.txt; out "$report" ;;
esac
"#;

/// Two parts of a hand-built graph. Each writes its own file, or with
/// `SHARED` both write the same one; a retry told about a conflict keeps
/// both lines. Every run notes how many runs were live when it started.
const PARTS_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
# Holds the run open for a second, or until a second run has started, for at
# most $PEER_WAIT tenths of a second. A run that finishes at once leaves its
# `seen` file, so the run that started first still finds it.
peer_wait() {
  i=0
  while [ "$i" -lt "${PEER_WAIT:-10}" ]; do
    [ "$(ls "$PARTS_LOG/seen" | wc -l)" -ge 2 ] && return
    sleep 0.1
    i=$((i+1))
  done
}
mkdir -p "$PARTS_LOG/live" "$PARTS_LOG/seen"
touch "$PARTS_LOG/live/$$" "$PARTS_LOG/seen/$$"
ls "$PARTS_LOG/live" | wc -l | tr -d ' ' >> "$PARTS_LOG/counts"
case "$input" in
  *"These files conflict"*) printf 'one\ntwo\n' > shared.txt ;;
  *PART_ONE*) peer_wait; if [ -n "$SHARED" ]; then echo one > shared.txt; else echo one > one.txt; fi ;;
  *PART_TWO*) peer_wait; if [ -n "$SHARED" ]; then echo two > shared.txt; else echo two > two.txt; fi ;;
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

fn uuid_like() -> String {
    "4f1c7a52-2d3b-4c9e-9a1e-0b7f6d5e4c3a".to_string()
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
    wait_until("the first run to start", || {
        log.join("slept").exists().then_some(())
    });
    let id = task["id"].as_str().unwrap();
    daemon.request("task.stop", serde_json::json!({"id": id}));
    poll_until(daemon, id, |s| s == "stopped");
    task
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
  *"You review a coding task plan"*) printf '%s\n' '{{"type":"result","result":"{{\"findings\": []}}"}}' ;;
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
  *"You review a coding task plan"*) printf '%s\n' '{"type":"result","result":"{\"findings\": []}"}' ;;
  *sushi-plan*)
    case "$input" in
      *PART_ONE*) out '"```sushi-plan\n{\"title\":\"Part One\",\"goal\":\"PART_ONE: create one.txt\",\"verify\":[\"test -f one.txt\"]}\n```"' ;;
      *PART_TWO*) out '"```sushi-plan\n{\"title\":\"Part Two\",\"goal\":\"PART_TWO: create two.txt\",\"verify\":[\"test -f two.txt\"]}\n```"' ;;
      *) out '"```sushi-plan\n{\"title\":\"Parent\",\"goal\":\"Both parts\",\"verify\":[\"__PARENT_VERIFY__\"]__PARENT_FINAL__,\"subtasks\":[{\"key\":\"a\",\"title\":\"Part One\",\"request\":\"PART_ONE create one.txt\",\"paths\":[\"one.txt\"],\"check\":\"test -f one.txt\"},{\"key\":\"b\",\"title\":\"Part Two\",\"request\":\"PART_TWO create two.txt\",\"paths\":[\"two.txt\"],\"check\":\"test -f two.txt\"}]}\n```"' ;;
    esac
    ;;
  *NEEDS-FIX*) echo fix > fix.txt; out "$report" ;;
  *PART_ONE*) echo one > one.txt; out "$report" ;;
  *PART_TWO*) echo two > two.txt; out "$report" ;;
esac
"#;

fn split_daemon(parent_verify: &str) -> (Daemon, tempfile::TempDir) {
    let scripts_dir = tempfile::tempdir().unwrap();
    let body = SPLIT_SCRIPT
        .replace("__PARENT_VERIFY__", parent_verify)
        .replace("__PARENT_FINAL__", "");
    let script = fake_harness_script(scripts_dir.path(), "fake-claude.sh", &body);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    review_off(&daemon);
    (daemon, scripts_dir)
}

/// `split_daemon` whose parent plan also names one `finalVerify` command.
fn split_daemon_with_final(parent_verify: &str, final_check: &str) -> (Daemon, tempfile::TempDir) {
    let scripts_dir = tempfile::tempdir().unwrap();
    let body = SPLIT_SCRIPT
        .replace("__PARENT_VERIFY__", parent_verify)
        .replace(
            "__PARENT_FINAL__",
            &format!(",\\\"finalVerify\\\":[\\\"{final_check}\\\"]"),
        );
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
        remove_dir_retrying(t["worktree"].as_str().unwrap());
    }
}

/// `commands` as the `verify` list of the split script's parent plan.
fn verify_list(commands: &[&str]) -> String {
    commands.join("\\\",\\\"")
}

fn decisions_of(task: &serde_json::Value) -> Vec<String> {
    task["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d.as_str().unwrap().to_string())
        .collect()
}

fn dirs_named(root: &Path, prefix: &str, found: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_name().to_string_lossy().starts_with(prefix) {
            found.push(path.display().to_string());
        }
        if path.is_dir() {
            dirs_named(&path, prefix, found);
        }
    }
}

/// A fake `npm` that appends its working directory to `$NPM_LOG` and exits 0,
/// except when `$NPM_FAIL` is set: while `one.txt` exists and `fix.txt` does
/// not, it prints a failure tail and exits 3.
const FAKE_NPM: &str = r#"#!/bin/sh
echo "$PWD" >> "$NPM_LOG"
if [ -n "$NPM_FAIL" ] && [ -f one.txt ] && [ ! -f fix.txt ]; then
  echo "GATE-BROKEN-$(echo TAIL) NEEDS-$(echo FIX)"
  exit 3
fi
exit 0
"#;

/// `PARTS_SCRIPT` that also writes fix.txt for a run told about a gate
/// failure and, with `BARRIER` set, makes a part wait until both parts are
/// running, so the second to land always finds the parent branch moved.
fn gate_script() -> String {
    PARTS_SCRIPT
        .replace(
            "case \"$input\" in\n",
            "case \"$input\" in\n  *NEEDS-FIX*) echo fix > fix.txt ;;\n",
        )
        .replace(
            "rm -f \"$PARTS_LOG/live/$$\"\n",
            "if [ -n \"$BARRIER\" ]; then\n  touch \"$PARTS_LOG/arrived.$$\"\n  i=0\n  while [ \"$(ls \"$PARTS_LOG\" | grep -c '^arrived')\" -lt 2 ] && [ $i -lt 150 ]; do sleep 0.2; i=$((i+1)); done\nfi\nrm -f \"$PARTS_LOG/live/$$\"\n",
        )
}

struct GateRun {
    daemon: Daemon,
    repo: tempfile::TempDir,
    npm_log: std::path::PathBuf,
    _dirs: Vec<tempfile::TempDir>,
}

/// A daemon whose harness is `gate_script` and whose `npm` is the fake one.
fn gate_daemon(fail: bool, gate_at: Option<&str>) -> GateRun {
    gate_daemon_env(fail, gate_at, &[])
}

fn gate_daemon_env(fail: bool, gate_at: Option<&str>, extra: &[(&str, &str)]) -> GateRun {
    let scripts = tempfile::tempdir().unwrap();
    let bin = tempfile::tempdir().unwrap();
    let log = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake-claude.sh", &gate_script());
    fake_harness_script(bin.path(), "npm", FAKE_NPM);
    let npm_log = log.path().join("npm.log");
    let path = format!(
        "{}:{}",
        bin.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut vars = vec![
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("PARTS_LOG", log.path().to_str().unwrap()),
        ("NPM_LOG", npm_log.to_str().unwrap()),
        ("NPM_FAIL", if fail { "1" } else { "" }),
        ("PATH", &path),
    ];
    vars.extend_from_slice(extra);
    let daemon = Daemon::spawn(&vars);
    review_off(&daemon);
    let repo = init_git_repo();
    if let Some(at) = gate_at {
        let mut settings = daemon.request("settings.get", serde_json::json!({}));
        settings["gateAt"] = serde_json::json!({ repo.path().to_str().unwrap(): at });
        daemon.request("settings.set", serde_json::json!({"settings": settings}));
    }
    GateRun {
        daemon,
        repo,
        npm_log,
        _dirs: vec![scripts, bin, log],
    }
}

/// A path with symlinks resolved, also when its last parts are gone.
fn canon(path: &str) -> std::path::PathBuf {
    let mut rest = std::path::PathBuf::new();
    let mut head = Path::new(path).to_path_buf();
    loop {
        if let Ok(real) = std::fs::canonicalize(&head) {
            return real.join(rest);
        }
        let Some(name) = head.file_name().map(|n| n.to_owned()) else {
            return Path::new(path).to_path_buf();
        };
        rest = Path::new(&name).join(rest);
        head.pop();
    }
}

/// How many times `npm` ran in each given task worktree (scratch base-check
/// worktrees are not task worktrees and are not counted).
fn npm_runs_in(run: &GateRun, tasks: &[&serde_json::Value]) -> Vec<usize> {
    let log = std::fs::read_to_string(&run.npm_log).unwrap_or_default();
    let lines: Vec<std::path::PathBuf> = log.lines().map(canon).collect();
    tasks
        .iter()
        .map(|t| {
            let wt = canon(t["worktree"].as_str().unwrap());
            lines.iter().filter(|l| **l == wt).count()
        })
        .collect()
}

fn deferred_lines(task: &serde_json::Value) -> Vec<String> {
    decisions_of(task)
        .into_iter()
        .filter(|d| d.contains("Deferred to parent"))
        .collect()
}

/// A parent with `finals[0]` and two children with `finals[1..]`, started.
fn gate_graph(run: &GateRun, finals: [&[&str]; 3]) -> (serde_json::Value, Vec<serde_json::Value>) {
    let parent = gate_graph_with(run, finals, ["test -f one.txt", "test -f two.txt"]);
    let parent = settle(&run.daemon, parent["id"].as_str().unwrap());
    let children = children_of(&run.daemon, &parent);
    assert_eq!(children.len(), 2);
    (parent, children)
}

/// Creates the gate graph with per-child verify commands, without waiting for it to settle.
fn gate_graph_with(
    run: &GateRun,
    finals: [&[&str]; 3],
    child_verify: [&str; 2],
) -> serde_json::Value {
    let repo = run.repo.path().to_str().unwrap();
    let parent = run.daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo, "title": "Parent", "goal": "Both parts",
            "verify": ["test -f one.txt && test -f two.txt"],
            "finalVerify": finals[0], "start": false,
        }),
    );
    let parent_id = parent["id"].as_str().unwrap().to_string();
    for (i, (title, goal)) in [("One", "PART_ONE"), ("Two", "PART_TWO")]
        .iter()
        .enumerate()
    {
        run.daemon.request(
            "task.create",
            serde_json::json!({
                "repo": repo, "title": title, "goal": goal,
                "verify": [child_verify[i]],
                "finalVerify": finals[i + 1], "parent": parent_id,
            }),
        );
    }
    parent
}

fn finish_gate(run: GateRun, parent: &serde_json::Value, children: &[serde_json::Value]) {
    run.daemon.shutdown_and_wait();
    remove_worktrees(&children.iter().chain([parent]).collect::<Vec<_>>());
}

/// Part A writes `src/shared.txt`, which part B lists in its paths; B and C
/// depend on A. Every part is drafted completely by the parent planner.
const REFRESH_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
out() {
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  printf '%s\n' "{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":$1}"
}
report='"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"'
case "$input" in
  *"You check a coding task brief"*) out '"{\"contradiction\": false, \"conflict\": \"\"}"' ;;
  *"A coding task was drafted before"*) echo refresh >> "$CALLS_LOG"; out '"{\"paths\": [\"src/shared.txt\", \"src/extra.txt\"]}"' ;;
  *"You review a coding task plan"*) printf '%s\n' '{"type":"result","result":"{\"findings\": []}"}' ;;
  *sushi-plan*) out '"```sushi-plan\n{\"title\":\"All parts\",\"goal\":\"Create the files\",\"verify\":[\"true\"],\"subtasks\":[{\"key\":\"a\",\"title\":\"Part A\",\"goal\":\"MAKE_A: create a.txt and src/shared.txt\",\"criteria\":[\"a.txt exists\"],\"verify\":[\"test -f a.txt\"],\"paths\":[\"a.txt\"],\"check\":\"test -f a.txt\",\"request\":\"REQ_A\"},{\"key\":\"b\",\"title\":\"Part B\",\"goal\":\"MAKE_B: create b.txt\",\"criteria\":[\"b.txt exists\"],\"verify\":[\"test -f b.txt\"],\"paths\":[\"src/shared.txt\"],\"check\":\"test -f b.txt\",\"dependsOn\":[\"a\"],\"request\":\"REQ_B\"},{\"key\":\"c\",\"title\":\"Part C\",\"goal\":\"MAKE_C: create c.txt\",\"criteria\":[\"c.txt exists\"],\"verify\":[\"test -f c.txt\"],\"paths\":[\"c.txt\"],\"check\":\"test -f c.txt\",\"dependsOn\":[\"a\"],\"request\":\"REQ_C\"}]}\n```"' ;;
  *MAKE_A*) mkdir -p src; echo a > a.txt; echo s > src/shared.txt; out "$report" ;;
  *MAKE_B*) echo b > b.txt; out "$report" ;;
  *MAKE_C*) echo c > c.txt; out "$report" ;;
  *) out "$report" ;;
esac
"#;

#[path = "graph/dependencies.rs"]
mod dependencies_tests;
#[path = "graph/gates.rs"]
mod gates_tests;
#[path = "graph/parents.rs"]
mod parents_tests;
#[path = "graph/results.rs"]
mod results_tests;
#[path = "graph/subtasks.rs"]
mod subtasks_tests;
