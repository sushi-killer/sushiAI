//! Running graph controls exercised through the daemon and a gated harness.
mod common;

use common::*;
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;

const FAKE: &str = r###"#!/usr/bin/env python3
import json, os, pathlib, re, subprocess, sys, time
brief = sys.stdin.read()
gates = pathlib.Path(os.environ['GATES'])
if 'sushi-plan' in brief and (gates / 'relay_plan').exists():
    plan = {'title': 'Relay graph', 'goal': 'Complete both parts', 'verify': ['true'], 'subtasks': [
        {'key': 'first', 'title': 'First', 'goal': 'CASE_first: change shared.txt', 'request': 'CASE_first: change shared.txt', 'criteria': ['Change shared.txt'], 'verify': ['true'], 'paths': ['shared.txt'], 'tier': 'standard'},
        {'key': 'second', 'title': 'Second', 'goal': 'CASE_second: continue the first part', 'request': 'CASE_second: continue the first part', 'criteria': ['Continue the first part'], 'verify': ['test "$(cat health.txt)" = ok'], 'paths': ['shared.txt', 'health.txt', 'second.txt'], 'tier': 'standard'},
    ]}
    print(json.dumps({'type': 'result', 'result': '```sushi-plan\n' + json.dumps(plan) + '\n```'}))
    sys.exit(0)
name = re.search(r'CASE_([a-z]+)', brief).group(1)
config = json.loads((gates / ('config_' + name)).read_text())
count = gates / ('count_' + name)
n = int(count.read_text()) + 1 if count.exists() else 1
count.write_text(str(n))
(gates / ('brief_' + name + '_' + str(n))).write_text(brief)
(gates / ('head_' + name + '_' + str(n))).write_text(subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip())
file = pathlib.Path(config.get('file', name + '.txt'))
mode = config.get('mode', 'complete')
if mode == 'conflict':
    if n == 1:
        file.write_text('part change\n')
    elif '## Conflict-only attempt' in brief:
        (gates / 'conflict_markers').write_text(file.read_text())
        # Deliberately leave markers; a failed conflict attempt must not loop.
        pathlib.Path('health.txt').write_text('broken\n')
elif mode != 'fail':
    file.write_text(name + '\n')
    if mode == 'retry':
        pathlib.Path('health.txt').write_text('broken\n' if n == 1 else 'ok\n')
(gates / ('started_' + name + '_' + str(n))).touch()
while (gates / ('hold_' + name + '_' + str(n))).exists():
    time.sleep(0.05)
print(json.dumps({'type': 'system', 'subtype': 'init', 'session_id': 'fake-' + name + '-' + str(n)}))
if mode == 'fail':
    print(json.dumps({'type': 'result', 'is_error': True, 'result': 'same harness failure', 'total_cost_usd': 0.01}))
else:
    report = {'outcome': 'complete', 'summary': 'done', 'decisions': [], 'question': ''}
    print(json.dumps({'type': 'result', 'total_cost_usd': 0.01, 'result': '```sushi-report\n' + json.dumps(report) + '\n```'}))
"###;

fn daemon(gates: &Path) -> (Daemon, tempfile::TempDir) {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake.py", FAKE);
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("GATES", gates.to_str().unwrap()),
    ]);
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!("");
    settings["briefCheckRoute"] = json!("");
    settings["answerPolicy"] = json!(false);
    settings["autoAnswer"] = json!(false);
    settings["maxAttempts"] = json!(1);
    fit_sandbox(&mut settings);
    daemon.request("settings.set", json!({"settings": settings}));
    cheap_route_on_claude(&daemon);
    (daemon, scripts)
}

fn repo() -> tempfile::TempDir {
    let repo = init_git_repo();
    git_out(repo.path(), &["checkout", "-q", "-b", "work"]);
    commit_file(repo.path(), "health.txt", "ok\n");
    commit_file(repo.path(), "shared.txt", "initial\n");
    repo
}

fn commit_file(repo: &Path, name: &str, text: &str) {
    std::fs::write(repo.join(name), text).unwrap();
    git_out(repo, &["add", name]);
    git_out(repo, &["commit", "-q", "-m", "base fix"]);
}

fn configure(gates: &Path, name: &str, mode: &str, file: &str, hold: bool) {
    std::fs::write(
        gates.join(format!("config_{name}")),
        json!({"mode": mode, "file": file}).to_string(),
    )
    .unwrap();
    if hold {
        std::fs::write(gates.join(format!("hold_{name}_1")), "").unwrap();
    }
}

fn started(gates: &Path, name: &str, n: u32) {
    wait_until(&format!("{name} attempt {n}"), || {
        gates
            .join(format!("started_{name}_{n}"))
            .exists()
            .then_some(())
    });
}

fn create(
    daemon: &Daemon,
    repo: &Path,
    name: &str,
    paths: &[&str],
    parent: Option<&str>,
    deps: &[&str],
    verify: &str,
) -> String {
    let task = daemon.request(
        "task.create",
        json!({
            "repo": repo, "title": name, "goal": format!("CASE_{name}: implement the change"),
            "verify": [verify], "paths": paths, "parent": parent, "dependsOn": deps, "start": false,
        }),
    );
    task["id"].as_str().unwrap().to_string()
}

fn get(daemon: &Daemon, id: &str) -> Value {
    daemon.request("task.get", json!({"id": id}))
}

fn waiting(daemon: &Daemon, id: &str) -> Value {
    poll_until(daemon, id, |s| s == "waiting")
}

fn decision_contains(task: &Value, text: &str) -> bool {
    task["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d.as_str().unwrap().contains(text))
}

fn start(daemon: &Daemon, id: &str) {
    daemon.request("task.start", json!({"id": id}));
}

fn graph(
    daemon: &Daemon,
    repo: &Path,
    name: &str,
    paths: &[&str],
    verify: &str,
) -> (String, String) {
    let parent = create(daemon, repo, "parent", &[], None, &[], "true");
    let part = create(daemon, repo, name, paths, Some(&parent), &[], verify);
    (parent, part)
}

#[test]
fn base_commit_outside_leases_reaches_a_live_part_before_its_next_attempt() {
    let gates = tempfile::tempdir().unwrap();
    configure(gates.path(), "part", "retry", "part.txt", true);
    std::fs::write(gates.path().join("hold_part_2"), "").unwrap();
    let (daemon, _scripts) = daemon(gates.path());
    let repo = repo();
    let (parent, part) = graph(
        &daemon,
        repo.path(),
        "part",
        &["part.txt", "health.txt"],
        "test \"$(cat health.txt)\" = ok",
    );
    started(gates.path(), "part", 1);
    commit_file(repo.path(), "outside.txt", "base fix\n");
    let fix = git_out(repo.path(), &["rev-parse", "HEAD"]);
    std::fs::remove_file(gates.path().join("hold_part_1")).unwrap();
    // maxAttempts=1 asks before the next boundary; the live loop keeps its lease.
    waiting(&daemon, &part);
    daemon.request("task.answer", json!({"id": part, "answer": "continue"}));
    started(gates.path(), "part", 2);
    let task = get(&daemon, &part);
    assert_eq!(
        std::fs::read_to_string(Path::new(task["worktree"].as_str().unwrap()).join("outside.txt"))
            .unwrap(),
        "base fix\n"
    );
    assert_eq!(
        std::fs::read_to_string(gates.path().join("head_part_2")).unwrap(),
        fix
    );
    let parent_task = get(&daemon, &parent);
    assert_eq!(
        git_out(
            Path::new(parent_task["worktree"].as_str().unwrap()),
            &["merge-base", &fix, "HEAD"]
        ),
        fix
    );
    assert!(
        decision_contains(&parent_task, "Sync: merged"),
        "{parent_task}"
    );
    std::fs::remove_file(gates.path().join("hold_part_2")).unwrap();
    assert_eq!(settle(&daemon, &part)["status"], "done");
}

#[test]
fn base_commit_on_held_paths_waits_for_explicit_sync() {
    let gates = tempfile::tempdir().unwrap();
    configure(gates.path(), "part", "retry", "part.txt", true);
    std::fs::write(gates.path().join("hold_part_2"), "").unwrap();
    let (daemon, _scripts) = daemon(gates.path());
    let repo = repo();
    let (parent, part) = graph(
        &daemon,
        repo.path(),
        "part",
        &["shared.txt", "part.txt", "health.txt"],
        "test \"$(cat health.txt)\" = ok",
    );
    started(gates.path(), "part", 1);
    commit_file(repo.path(), "shared.txt", "base fix\n");
    let fix = git_out(repo.path(), &["rev-parse", "HEAD"]);
    std::fs::remove_file(gates.path().join("hold_part_1")).unwrap();
    waiting(&daemon, &part);
    daemon.request("task.answer", json!({"id": part, "answer": "continue"}));
    started(gates.path(), "part", 2);
    let task = get(&daemon, &part);
    let wt = Path::new(task["worktree"].as_str().unwrap());
    assert_eq!(
        std::fs::read_to_string(wt.join("shared.txt")).unwrap(),
        "initial\n"
    );
    let parent_task = get(&daemon, &parent);
    let parent_wt = Path::new(parent_task["worktree"].as_str().unwrap());
    assert_ne!(git_out(parent_wt, &["merge-base", &fix, "HEAD"]), fix);
    let synced = daemon.request("task.sync", json!({"id": part}));
    assert_eq!(synced["synced"], true, "{synced}");
    assert_eq!(git_out(parent_wt, &["merge-base", &fix, "HEAD"]), fix);
    std::fs::remove_file(gates.path().join("hold_part_2")).unwrap();
    assert_eq!(settle(&daemon, &part)["status"], "done");
}

#[test]
fn conflicting_base_commit_gives_one_attempt_with_the_conflict_in_the_brief() {
    let gates = tempfile::tempdir().unwrap();
    configure(gates.path(), "part", "conflict", "shared.txt", true);
    let (daemon, _scripts) = daemon(gates.path());
    let mut settings = daemon.request("settings.get", json!({}));
    settings["maxAttempts"] = json!(3);
    daemon.request("settings.set", json!({"settings": settings}));
    let repo = repo();
    let (parent, part) = graph(
        &daemon,
        repo.path(),
        "part",
        &["shared.txt", "health.txt"],
        "test \"$(cat health.txt)\" = ok",
    );
    started(gates.path(), "part", 1);
    commit_file(repo.path(), "shared.txt", "base change\n");
    // On-demand sync is allowed to overlap the graph's held paths.
    let sync = daemon.request("task.sync", json!({"id": part}));
    assert_eq!(sync["id"], parent);
    assert_eq!(sync["synced"], true, "{sync}");
    std::fs::remove_file(gates.path().join("hold_part_1")).unwrap();
    started(gates.path(), "part", 2);
    let brief = std::fs::read_to_string(gates.path().join("brief_part_2")).unwrap();
    assert!(
        brief.contains("## Conflict-only attempt") && brief.contains("shared.txt"),
        "{brief}"
    );
    let markers = std::fs::read_to_string(gates.path().join("conflict_markers")).unwrap();
    assert!(
        markers.contains("<<<<<<<")
            && markers.contains("base change")
            && markers.contains("part change"),
        "{markers}"
    );
    let parked = wait_until("the failed conflict attempt to park", || {
        let task = get(&daemon, &part);
        task["park"].is_object().then_some(task)
    });
    assert_eq!(parked["status"], "queued", "{parked}");
    assert!(parked["question"].is_null(), "{parked}");
    assert_eq!(parked["attempts"].as_array().unwrap().len(), 2, "{parked}");
    assert_eq!(parked["attempts"][1]["routeId"], "claude-cheap");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        std::fs::read_to_string(gates.path().join("count_part")).unwrap(),
        "2"
    );
}

#[test]
fn sync_aborts_a_conflict_on_the_parent_and_returns_the_files() {
    let gates = tempfile::tempdir().unwrap();
    configure(gates.path(), "part", "complete", "part.txt", true);
    let (daemon, _scripts) = daemon(gates.path());
    let repo = repo();
    let (parent, part) = graph(&daemon, repo.path(), "part", &["shared.txt"], "true");
    started(gates.path(), "part", 1);
    let task = get(&daemon, &parent);
    let wt = Path::new(task["worktree"].as_str().unwrap());
    commit_file(wt, "shared.txt", "parent change\n");
    let before = git_out(wt, &["rev-parse", "HEAD"]);
    commit_file(repo.path(), "shared.txt", "base change\n");
    let sync = daemon.request("task.sync", json!({"id": part}));
    assert_eq!(sync["conflicts"], json!(["shared.txt"]), "{sync}");
    assert_eq!(git_out(wt, &["rev-parse", "HEAD"]), before);
    assert!(git_out(wt, &["status", "--porcelain"]).is_empty());
    assert_eq!(
        std::fs::read_to_string(wt.join("shared.txt")).unwrap(),
        "parent change\n"
    );
    assert!(!std::process::Command::new("git")
        .args(["rev-parse", "--verify", "MERGE_HEAD"])
        .current_dir(wt)
        .output()
        .unwrap()
        .status
        .success());
    assert!(decision_contains(&get(&daemon, &parent), "merge aborted"));
}

#[test]
fn park_asks_no_dependent_and_releases_leases() {
    let gates = tempfile::tempdir().unwrap();
    for name in ["part", "waiter", "dependent"] {
        configure(gates.path(), name, "complete", &format!("{name}.txt"), true);
    }
    let (mut daemon, scripts) = daemon(gates.path());
    let repo = repo();
    let (parent, part) = graph(&daemon, repo.path(), "part", &["shared.txt"], "true");
    started(gates.path(), "part", 1);
    let part_task = get(&daemon, &part);
    let waiter = daemon.request("task.create", json!({
        "repo": repo.path(), "title": "waiter", "goal": "CASE_waiter: change a file", "verify": ["true"],
        "paths": ["shared.txt"], "base": part_task["baseRef"], "start": true,
    }))["id"].as_str().unwrap().to_string();
    wait_until("a waiter behind the lease", || {
        get(&daemon, &waiter)["queueReason"]
            .is_string()
            .then_some(())
    });
    let dependent = create(
        &daemon,
        repo.path(),
        "dependent",
        &[],
        None,
        &[&part],
        "true",
    );
    let parked = daemon.request(
        "task.park",
        json!({"id": part, "reason": "waits for a flaky check fix", "source": "lead"}),
    );
    assert_eq!(parked["status"], "queued");
    assert_eq!(parked["queueReason"], "waits for a flaky check fix");
    assert_eq!(parked["park"]["by"], "lead");
    assert!(parked["question"].is_null());
    started(gates.path(), "waiter", 1);
    wait_until("the parked loop to be interrupted", || {
        let task = get(&daemon, &part);
        (task["attempts"][0]["status"] == "interrupted").then_some(())
    });
    for id in [&parent, &dependent] {
        let task = get(&daemon, id);
        assert!(task["question"].is_null(), "{task}");
        assert!(
            task["questionHistory"]
                .as_array()
                .is_none_or(|h| h.iter().all(|q| q["kind"] != "dependency_ended")),
            "{task}"
        );
    }
    // Recovery must not silently resume a parked task.
    let script = scripts.path().join("fake.py");
    daemon.restart(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("GATES", gates.path().to_str().unwrap()),
    ]);
    std::thread::sleep(Duration::from_millis(300));
    assert!(get(&daemon, &part)["park"].is_object());
    assert_eq!(
        std::fs::read_to_string(gates.path().join("count_part")).unwrap(),
        "1"
    );
    let stopped = daemon.request("task.stop", json!({"id": part}));
    assert_eq!(stopped["status"], "stopped");
    assert!(stopped["park"].is_null());
}

#[test]
fn park_until_resumes_when_that_task_lands() {
    let gates = tempfile::tempdir().unwrap();
    for name in ["paused", "fixer", "manual"] {
        configure(gates.path(), name, "complete", &format!("{name}.txt"), true);
    }
    let (daemon, _scripts) = daemon(gates.path());
    let repo = repo();
    let paused = create(&daemon, repo.path(), "paused", &[], None, &[], "true");
    let fixer = create(&daemon, repo.path(), "fixer", &[], None, &[], "true");
    daemon.request(
        "task.park",
        json!({"id": paused, "reason": "waits for the fix", "until": fixer}),
    );
    start(&daemon, &fixer);
    started(gates.path(), "fixer", 1);
    assert!(!gates.path().join("started_paused_1").exists());
    std::fs::remove_file(gates.path().join("hold_fixer_1")).unwrap();
    assert_eq!(settle(&daemon, &fixer)["status"], "done");
    started(gates.path(), "paused", 1);
    assert!(get(&daemon, &paused)["park"].is_null());
    let manual = create(&daemon, repo.path(), "manual", &[], None, &[], "true");
    start(&daemon, &manual);
    started(gates.path(), "manual", 1);
    daemon.request("task.park", json!({"id": manual, "reason": "pause"}));
    start(&daemon, &manual);
    started(gates.path(), "manual", 2);
    assert!(get(&daemon, &manual)["park"].is_null());
}

#[test]
fn stopped_holder_never_blocks_a_waiter() {
    let gates = tempfile::tempdir().unwrap();
    for name in ["holder", "waiter"] {
        configure(gates.path(), name, "complete", &format!("{name}.txt"), true);
    }
    let (daemon, _scripts) = daemon(gates.path());
    let repo = repo();
    let holder = create(
        &daemon,
        repo.path(),
        "holder",
        &["shared.txt"],
        None,
        &[],
        "true",
    );
    start(&daemon, &holder);
    started(gates.path(), "holder", 1);
    let waiter = create(
        &daemon,
        repo.path(),
        "waiter",
        &["shared.txt"],
        None,
        &[],
        "true",
    );
    start(&daemon, &waiter);
    wait_until("the queued waiter", || {
        get(&daemon, &waiter)["queueReason"]
            .is_string()
            .then_some(())
    });
    let stopped = daemon.request("task.stop", json!({"id": holder, "source": "lead"}));
    assert_eq!(stopped["status"], "stopped", "{stopped}");
    started(gates.path(), "waiter", 1);
    assert!(get(&daemon, &waiter)["queueReason"].is_null());
    assert!(decision_contains(
        &get(&daemon, &holder),
        "Lead: stop (RPC)"
    ));
}

#[test]
fn stopped_part_stays_stopped_while_its_parent_runs() {
    let gates = tempfile::tempdir().unwrap();
    configure(gates.path(), "part", "complete", "part.txt", true);
    let (daemon, _scripts) = daemon(gates.path());
    let mut settings = daemon.request("settings.get", json!({}));
    settings["answerPolicy"] = json!(true);
    daemon.request("settings.set", json!({"settings": settings}));
    let repo = repo();
    let (parent, part) = graph(&daemon, repo.path(), "part", &[], "true");
    started(gates.path(), "part", 1);
    daemon.request("task.stop", json!({"id": part, "source": "lead"}));
    let parent_task = waiting(&daemon, &parent);
    assert_eq!(parent_task["question"]["kind"], "dependency_ended");
    assert_eq!(get(&daemon, &part)["status"], "stopped");
    let error = daemon.request_error(
        "task.answer",
        json!({"id": parent, "answer": "retry the dependency", "source": "lead"}),
    );
    assert!(error.contains("explicitly stopped"), "{error}");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        std::fs::read_to_string(gates.path().join("count_part")).unwrap(),
        "1"
    );
    // The owner's explicit answer may restart it.
    daemon.request(
        "task.answer",
        json!({"id": parent, "answer": "retry the dependency"}),
    );
    assert_eq!(settle(&daemon, &part)["status"], "done");
}

#[test]
fn rpc_answers_are_labelled_with_their_source() {
    let gates = tempfile::tempdir().unwrap();
    let (daemon, _scripts) = daemon(gates.path());
    let repo = repo();
    for (name, source, line, by) in [
        ("owner", None, "Owner: continue", "owner"),
        ("lead", Some("lead"), "Lead: continue", "lead"),
        (
            "overseer",
            Some("overseer"),
            "answered by the overseer",
            "overseer",
        ),
        (
            "chat",
            Some("chat"),
            "answered by the orchestrator chat",
            "orchestrator",
        ),
    ] {
        configure(gates.path(), name, "fail", &format!("{name}.txt"), false);
        let id = create(&daemon, repo.path(), name, &[], None, &[], "true");
        start(&daemon, &id);
        let task = waiting(&daemon, &id);
        let next = task["attempts"].as_array().unwrap().last().unwrap()["n"]
            .as_u64()
            .unwrap() as u32
            + 1;
        std::fs::write(gates.path().join(format!("hold_{name}_{next}")), "").unwrap();
        let mut params = json!({"id": id, "answer": "continue"});
        if let Some(source) = source {
            params["source"] = json!(source);
        }
        daemon.request("task.answer", params);
        started(gates.path(), name, next);
        let task = get(&daemon, &id);
        assert!(decision_contains(&task, line), "{task}");
        assert_eq!(task["questionHistory"][0]["answeredBy"], by, "{task}");
        let mut params = json!({"id": id});
        if let Some(source) = source {
            params["source"] = json!(source);
        }
        let stopped = daemon.request("task.stop", params);
        if source.is_none() {
            assert!(decision_contains(&stopped, "Owner: stop (UI)"));
        }
        if source == Some("lead") {
            assert!(decision_contains(&stopped, "Lead: stop (RPC)"));
        }
    }

    for (name, source, line) in [
        ("owner_answer_stop", None, "Owner: stop (UI)"),
        ("lead_answer_stop", Some("lead"), "Lead: stop (RPC)"),
    ] {
        configure(gates.path(), name, "fail", &format!("{name}.txt"), false);
        let id = create(&daemon, repo.path(), name, &[], None, &[], "true");
        start(&daemon, &id);
        waiting(&daemon, &id);
        let mut params = json!({"id": id, "answer": "stop"});
        if let Some(source) = source {
            params["source"] = json!(source);
        }
        let stopped = daemon.request("task.answer", params);
        assert!(decision_contains(&stopped, line), "{stopped}");
    }
}

#[test]
fn third_retry_on_the_same_failure_parks_the_task() {
    let gates = tempfile::tempdir().unwrap();
    configure(gates.path(), "failure", "fail", "failure.txt", false);
    let (daemon, _scripts) = daemon(gates.path());
    let repo = repo();
    let id = create(&daemon, repo.path(), "failure", &[], None, &[], "true");
    start(&daemon, &id);
    let mut signature = Value::Null;
    for (i, source) in [Some("lead"), Some("overseer"), None]
        .into_iter()
        .enumerate()
    {
        let task = waiting(&daemon, &id);
        assert!(task["question"]["failureSignature"].is_string(), "{task}");
        if i == 0 {
            signature = task["question"]["failureSignature"].clone();
        }
        assert_eq!(task["question"]["failureSignature"], signature);
        let mut params = json!({"id": id, "answer": "continue"});
        if let Some(source) = source {
            params["source"] = json!(source);
        }
        daemon.request("task.answer", params);
        wait_until("the retry to be recorded", || {
            let task = get(&daemon, &id);
            (task["questionHistory"]
                .as_array()
                .is_some_and(|h| h.len() > i))
            .then_some(())
        });
    }
    let parked = wait_until("the repeated failure to park", || {
        let task = get(&daemon, &id);
        task["park"].is_object().then_some(task)
    });
    assert_eq!(parked["status"], "queued");
    assert!(parked["question"].is_null());
    assert!(
        parked["queueReason"]
            .as_str()
            .unwrap()
            .contains("failed again after 3 retries"),
        "{parked}"
    );
    assert_eq!(parked["questionHistory"].as_array().unwrap().len(), 3);
    for answer in parked["questionHistory"].as_array().unwrap() {
        assert_eq!(answer["failureSignature"], signature);
    }
}

#[test]
fn stop_clears_the_question() {
    let gates = tempfile::tempdir().unwrap();
    configure(gates.path(), "live", "fail", "live.txt", false);
    configure(gates.path(), "idle", "fail", "idle.txt", false);
    let (mut daemon, scripts) = daemon(gates.path());
    let repo = repo();
    let live = create(&daemon, repo.path(), "live", &[], None, &[], "true");
    start(&daemon, &live);
    waiting(&daemon, &live);
    let stopped = daemon.request("task.stop", json!({"id": live}));
    assert_eq!(stopped["status"], "stopped");
    assert!(stopped["question"].is_null());
    let idle = create(&daemon, repo.path(), "idle", &[], None, &[], "true");
    start(&daemon, &idle);
    waiting(&daemon, &idle);
    let script = scripts.path().join("fake.py");
    daemon.restart(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("GATES", gates.path().to_str().unwrap()),
    ]);
    assert!(
        get(&daemon, &idle)["question"].is_object(),
        "shutdown preserves the waiting question"
    );
    let stopped = daemon.request("task.stop", json!({"id": idle}));
    assert_eq!(stopped["status"], "stopped");
    assert!(stopped["question"].is_null());
    std::thread::sleep(Duration::from_millis(200));
    assert!(get(&daemon, &live)["question"].is_null());
}

#[test]
fn base_sync_preserves_relay_work_between_attempts() {
    let gates = tempfile::tempdir().unwrap();
    std::fs::write(gates.path().join("relay_plan"), "").unwrap();
    configure(gates.path(), "first", "complete", "shared.txt", false);
    configure(gates.path(), "second", "retry", "second.txt", true);
    std::fs::write(gates.path().join("hold_second_2"), "").unwrap();
    let (daemon, _scripts) = daemon(gates.path());
    let repo = repo();
    let parent = daemon.request(
        "task.create",
        json!({"repo": repo.path(), "request": "Build both relay parts"}),
    );
    let parent_id = parent["id"].as_str().unwrap();
    started(gates.path(), "second", 1);
    let tasks = daemon.request("task.list", json!({"repo": parent["repo"]}));
    let second = tasks
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["title"] == "Second")
        .unwrap();
    let id = second["id"].as_str().unwrap();
    assert!(second["relayOf"].is_string(), "{second}");
    let worktree = Path::new(second["worktree"].as_str().unwrap());
    assert_eq!(
        std::fs::read_to_string(worktree.join("shared.txt")).unwrap(),
        "first\n"
    );
    commit_file(repo.path(), "outside.txt", "base fix\n");
    std::fs::remove_file(gates.path().join("hold_second_1")).unwrap();
    waiting(&daemon, id);
    daemon.request("task.answer", json!({"id": id, "answer": "continue"}));
    started(gates.path(), "second", 2);
    assert_eq!(
        std::fs::read_to_string(worktree.join("outside.txt")).unwrap(),
        "base fix\n"
    );
    assert_eq!(
        std::fs::read_to_string(worktree.join("shared.txt")).unwrap(),
        "first\n"
    );
    std::fs::remove_file(gates.path().join("hold_second_2")).unwrap();
    assert_eq!(settle(&daemon, id)["status"], "done");
    assert_eq!(settle(&daemon, parent_id)["status"], "done");
}
