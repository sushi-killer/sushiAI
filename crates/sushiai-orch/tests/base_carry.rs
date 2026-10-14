//! Real-git daemon regressions for retries that carry task and graph work
//! onto a repaired base, and for retry answers that outlive the daemon.

mod common;

use common::*;
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;

const RETRY: &str = "retry after the base is fixed";
const CHECK: &str = "grep -q fixed status.txt";
const FAKE: &str = r###"#!/bin/sh
input=$(cat)
out() {
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  printf '%s\n' "{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":$1}"
}
report='"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"'
case "$input" in
  *'## Conflict-only attempt'*) printf 'base\nchild\n' > README.md; out "$report" ;;
  *sushi-plan*)
    case "$input" in
      *OWN_TARGET*) out '"```sushi-plan\n{\"title\":\"Fix target check\",\"goal\":\"OWN_TARGET: fix grep -q fixed status.txt\",\"verify\":[\"true\"],\"finalVerify\":[\"grep -q fixed status.txt\"]}\n```"' ;;
      *IDLE_BASE*) out '"```sushi-plan\n{\"title\":\"Unrelated task\",\"goal\":\"IDLE_BASE: edit README without changing status.txt\",\"verify\":[\"true\"],\"finalVerify\":[\"grep -q fixed status.txt\"]}\n```"' ;;
      *SECOND_WORK*) out '"```sushi-plan\n{\"title\":\"Second child\",\"goal\":\"SECOND_WORK: create second.txt\",\"verify\":[\"test -f second.txt\"]}\n```"' ;;
      *CHILD_WORK*) out '"```sushi-plan\n{\"title\":\"Child\",\"goal\":\"CHILD_WORK: edit README\",\"verify\":[\"true\"]}\n```"' ;;
      *) out '"```sushi-plan\n{\"title\":\"Parent\",\"goal\":\"Finish the children\",\"verify\":[\"true\"],\"finalVerify\":[\"grep -q fixed status.txt\"],\"subtasks\":[{\"key\":\"child\",\"title\":\"Child\",\"request\":\"CHILD_WORK edit README\",\"paths\":[\"README.md\"],\"check\":\"grep -q child README.md\"},{\"key\":\"second\",\"title\":\"Second child\",\"request\":\"SECOND_WORK create second.txt\",\"paths\":[\"second.txt\"],\"check\":\"test -f second.txt\",\"dependsOn\":[\"child\"]}]}\n```"' ;;
    esac ;;
  *OWN_TARGET*|*'## Failed checks'*)
    if [ -e .own-target-retried ]; then printf 'fixed\n' > status.txt; else touch .own-target-retried; fi
    printf 'child\n' > README.md; out "$report" ;;
  *SECOND_WORK*) echo second > second.txt; out "$report" ;;
  *) echo child > README.md; out "$report" ;;
esac
"###;

fn configure(socket: &Path, policy: bool) {
    let mut settings = request_on(socket, "settings.get", json!({}));
    settings["review"] = json!("");
    settings["briefCheckRoute"] = json!("");
    settings["planReview"] = json!("");
    settings["answerPolicy"] = json!(policy);
    fit_sandbox(&mut settings);
    request_on(socket, "settings.set", json!({"settings": settings}));
}

fn repo() -> tempfile::TempDir {
    let repo = init_git_repo();
    git_out(repo.path(), &["checkout", "-q", "-b", "work"]);
    std::fs::write(repo.path().join("status.txt"), "broken\n").unwrap();
    std::fs::write(repo.path().join(".gitignore"), "LOCAL.txt\n").unwrap();
    git_out(repo.path(), &["add", "."]);
    git_out(repo.path(), &["commit", "-q", "-m", "broken base"]);
    repo
}

fn create(socket: &Path, repo: &Path, parent: bool) -> String {
    let mut params = json!({"repo": repo, "start": true, "land": false});
    if parent {
        params["request"] = json!("Finish the children");
    } else {
        params["title"] = json!("Base carry");
        params["goal"] = json!("Edit README");
        params["verify"] = json!(["true"]);
        params["finalVerify"] = json!([CHECK]);
    }
    request_on(socket, "task.create", params)["id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn get(socket: &Path, id: &str) -> Value {
    request_on(socket, "task.get", json!({"id": id}))
}

fn decisions(task: &Value) -> Vec<&str> {
    task["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d.as_str().unwrap())
        .collect()
}

fn attempts(task: &Value) -> Vec<&Value> {
    task["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["stage"] == "implement")
        .collect()
}

fn preexisting_count(task: &Value) -> usize {
    task["questionHistory"].as_array().map_or(0, |history| {
        history
            .iter()
            .filter(|h| h["kind"] == "preexisting_failure")
            .count()
    })
}

fn preexisting_question(socket: &Path, id: &str) -> Value {
    wait_until("the pre-existing final failure", || {
        let task = get(socket, id);
        (task["status"] == "waiting" && task["question"]["kind"] == "preexisting_failure")
            .then_some(task)
    })
}

fn assert_landed_children(socket: &Path, parent: &Value) {
    let tasks = request_on(socket, "task.list", json!({"repo": parent["repo"]}));
    let children: Vec<_> = tasks
        .as_array()
        .unwrap()
        .iter()
        .filter(|task| task["parent"] == parent["id"])
        .collect();
    assert_eq!(children.len(), 2, "{tasks}");
    assert!(
        children.iter().all(|task| task["status"] == "done"),
        "{tasks}"
    );
    assert!(attempts(parent).is_empty(), "{parent}");
    let wt = Path::new(parent["worktree"].as_str().unwrap());
    assert_ne!(
        git_out(wt, &["rev-parse", "HEAD"]),
        parent["baseSha"].as_str().unwrap(),
        "{parent}"
    );
    assert_eq!(git_out(wt, &["show", "HEAD:README.md"]), "child");
    assert_eq!(git_out(wt, &["show", "HEAD:second.txt"]), "second");
}

fn fix_base(repo: &Path, conflict: bool) -> String {
    std::fs::write(repo.join("status.txt"), "fixed\n").unwrap();
    if conflict {
        std::fs::write(repo.join("README.md"), "base\n").unwrap();
    }
    git_out(repo, &["add", "."]);
    git_out(repo, &["commit", "-q", "-m", "fix base"]);
    git_out(repo, &["rev-parse", "HEAD"])
}

fn done(socket: &Path, id: &str) -> Value {
    wait_until("the carried task to finish", || {
        let task = get(socket, id);
        assert!(
            !matches!(task["status"].as_str(), Some("failed" | "stopped")),
            "{task}"
        );
        (task["status"] == "done").then_some(task)
    })
}

fn clean_worktrees(socket: &Path, repo: &Path) -> Vec<String> {
    request_on(socket, "task.list", json!({"repo": repo}))
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["worktree"].as_str().map(str::to_string))
        .collect()
}

fn conflicting_retry(parent: bool) {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake.sh", FAKE);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    configure(&daemon.socket, false);
    cheap_route_on_claude(&daemon);
    let repo = repo();
    let id = create(&daemon.socket, repo.path(), parent);
    let waiting = preexisting_question(&daemon.socket, &id);
    if parent {
        assert_landed_children(&daemon.socket, &waiting);
    }
    daemon.request("task.answer", json!({"id": id, "answer": RETRY}));
    let base = fix_base(repo.path(), true);
    let finished = done(&daemon.socket, &id);
    assert_eq!(finished["baseSha"], base, "{finished}");
    assert_eq!(preexisting_count(&finished), 1, "{finished}");
    assert!(
        decisions(&finished)
            .iter()
            .any(|d| d.contains("conflicts in README.md") && d.contains(&base[..7])),
        "{finished}"
    );
    assert!(
        decisions(&finished)
            .iter()
            .any(|d| d.contains("conflict-only attempt")),
        "{finished}"
    );
    let implementing = attempts(&finished);
    assert_eq!(implementing.len(), if parent { 1 } else { 2 }, "{finished}");
    let conflict = implementing.last().unwrap();
    assert_eq!(conflict["routeId"], "claude-cheap", "{finished}");
    let brief = std::fs::read_to_string(
        daemon
            .data_dir()
            .join("tasks")
            .join(&id)
            .join("runs")
            .join(conflict["n"].as_u64().unwrap().to_string())
            .join("brief.md"),
    )
    .unwrap();
    assert!(brief.contains("## Conflict-only attempt"), "{brief}");
    assert!(
        brief.contains("README.md") && brief.contains(&base[..7]),
        "{brief}"
    );
    assert_eq!(
        git_out(
            repo.path(),
            &[
                "show",
                &format!("{}:README.md", finished["branch"].as_str().unwrap())
            ]
        ),
        "base\nchild"
    );
    let worktrees = clean_worktrees(&daemon.socket, repo.path());
    daemon.shutdown_and_wait();
    for wt in worktrees {
        remove_dir_retrying(wt);
    }
}

#[test]
fn a_parent_carries_landed_child_commits_into_a_conflict_only_attempt() {
    conflicting_retry(true);
}

#[test]
fn a_final_retry_carries_conflicts_into_a_conflict_only_attempt() {
    conflicting_retry(false);
}

fn failed_carry(ignored: bool) {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake.sh", FAKE);
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("ORCHD_CARRY_RETRY_MS", "2000"),
    ]);
    configure(&daemon.socket, false);
    let repo = repo();
    let id = create(&daemon.socket, repo.path(), false);
    let waiting = preexisting_question(&daemon.socket, &id);
    let wt = Path::new(waiting["worktree"].as_str().unwrap());
    let obstruction = if ignored {
        std::fs::write(wt.join("LOCAL.txt"), "local work\n").unwrap();
        std::fs::write(repo.path().join("LOCAL.txt"), "base file\n").unwrap();
        git_out(repo.path(), &["add", "-f", "LOCAL.txt"]);
        wt.join("LOCAL.txt")
    } else {
        let lock = git_out(wt, &["rev-parse", "--git-path", "index.lock"]);
        let lock = wt.join(lock);
        std::fs::write(&lock, "test-owned lock\n").unwrap();
        lock
    };
    daemon.request("task.answer", json!({"id": id, "answer": RETRY}));
    let base = fix_base(repo.path(), false);
    let reason = if ignored {
        "ignored local files: LOCAL.txt"
    } else {
        "index.lock"
    };
    let failed = if ignored {
        let failed = wait_until("a carry question with retry and stop", || {
            let task = get(&daemon.socket, &id);
            (task["status"] == "waiting" && task["question"]["kind"] == "agent_question")
                .then_some(task)
        });
        assert_eq!(failed["question"]["options"], json!(["retry", "stop"]));
        assert!(
            failed["question"]["text"]
                .as_str()
                .unwrap()
                .contains(reason),
            "{failed}"
        );
        failed
    } else {
        let failed = wait_until("a failed carry that is tried again", || {
            let task = get(&daemon.socket, &id);
            decisions(&task)
                .iter()
                .any(|d| d.contains("trying again after the next fetch"))
                .then_some(task)
        });
        assert!(failed["question"].is_null(), "{failed}");
        failed
    };
    assert!(
        decisions(&failed).iter().any(|d| d.contains(reason)),
        "{failed}"
    );
    assert_eq!(attempts(&failed).len(), 1, "{failed}");
    if ignored {
        assert_eq!(
            std::fs::read_to_string(&obstruction).unwrap(),
            "local work\n"
        );
    }
    std::fs::remove_file(obstruction).unwrap();
    if ignored {
        daemon.request("task.answer", json!({"id": id, "answer": "retry"}));
    }
    let finished = done(&daemon.socket, &id);
    assert_eq!(finished["baseSha"], base, "{finished}");
    assert_eq!(preexisting_count(&finished), 1, "{finished}");
    let worktrees = clean_worktrees(&daemon.socket, repo.path());
    daemon.shutdown_and_wait();
    for wt in worktrees {
        remove_dir_retrying(wt);
    }
}

#[test]
fn an_ignored_local_file_blocks_the_carry_on_an_owner_question() {
    failed_carry(true);
}

#[test]
fn a_git_error_in_the_carry_is_retried_without_asking_the_owner() {
    failed_carry(false);
}

#[test]
fn a_parent_that_already_merged_the_base_updates_base_sha_without_an_attempt() {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake.sh", FAKE);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    configure(&daemon.socket, false);
    let repo = repo();
    let id = create(&daemon.socket, repo.path(), true);
    let waiting = preexisting_question(&daemon.socket, &id);
    assert_landed_children(&daemon.socket, &waiting);
    let wt = Path::new(waiting["worktree"].as_str().unwrap());
    let base = fix_base(repo.path(), false);
    git_out(wt, &["merge", "-q", "--no-edit", "work"]);
    let merged = git_out(wt, &["rev-parse", "HEAD"]);
    daemon.request("task.answer", json!({"id": id, "answer": RETRY}));
    let finished = done(&daemon.socket, &id);
    assert_eq!(finished["baseSha"], base, "{finished}");
    assert_eq!(
        git_out(
            repo.path(),
            &["rev-parse", finished["branch"].as_str().unwrap()]
        ),
        merged,
        "{finished}"
    );
    assert!(attempts(&finished).is_empty(), "{finished}");
    assert!(
        decisions(&finished)
            .iter()
            .any(|d| d.contains("already contains work") && d.contains(&base[..7])),
        "{finished}"
    );
    assert!(
        !decisions(&finished)
            .iter()
            .any(|d| d.contains("conflict-only attempt")),
        "{finished}"
    );
    assert_eq!(preexisting_count(&finished), 1, "{finished}");
    let worktrees = clean_worktrees(&daemon.socket, repo.path());
    daemon.shutdown_and_wait();
    for wt in worktrees {
        remove_dir_retrying(wt);
    }
}

#[test]
fn a_parent_keeps_carried_children_when_landing_on_the_default_branch_is_refused() {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake.sh", FAKE);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    configure(&daemon.socket, false);
    let repo = repo();
    git_out(repo.path(), &["checkout", "-q", "-B", "main"]);
    let parent = daemon.request(
        "task.create",
        json!({"repo": repo.path(), "request": "Finish the children", "start": true, "land": true}),
    );
    let id = parent["id"].as_str().unwrap();
    let waiting = preexisting_question(&daemon.socket, id);
    assert_landed_children(&daemon.socket, &waiting);
    daemon.request("task.answer", json!({"id": id, "answer": RETRY}));
    let base = fix_base(repo.path(), false);
    let finished = done(&daemon.socket, id);
    assert_eq!(finished["baseSha"], base, "{finished}");
    assert_eq!(preexisting_count(&finished), 1, "{finished}");
    assert!(attempts(&finished).is_empty(), "{finished}");
    assert!(
        decisions(&finished)
            .iter()
            .any(|d| d.contains("Land: refused")),
        "{finished}"
    );
    for (file, expected) in [("README.md", "child"), ("second.txt", "second")] {
        assert_eq!(
            git_out(
                repo.path(),
                &[
                    "show",
                    &format!("{}:{file}", finished["branch"].as_str().unwrap())
                ]
            ),
            expected,
            "{finished}"
        );
    }
    let worktrees = clean_worktrees(&daemon.socket, repo.path());
    daemon.shutdown_and_wait();
    for wt in worktrees {
        remove_dir_retrying(wt);
    }
}

fn retry_survives_restart(
    parent: bool,
    policy: bool,
    agent: bool,
    answer_before_restart: bool,
    sibling_live: bool,
) {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake.sh", FAKE);
    let env = [("ORCHD_CLAUDE_BIN", script.to_str().unwrap())];
    let home = tempfile::tempdir().unwrap();
    let socket = socket_of(home.path());
    let first = spawn_daemon(home.path(), &env);
    wait_for_socket(&socket);
    configure(&socket, policy);
    let repo = repo();
    if sibling_live {
        let sibling = create(&socket, repo.path(), false);
        preexisting_question(&socket, &sibling);
    }
    let id = create(&socket, repo.path(), parent);
    if !policy && answer_before_restart {
        preexisting_question(&socket, &id);
        let mut params = json!({"id": id, "answer": RETRY});
        if agent {
            params["source"] = json!("chat");
        }
        request_on(&socket, "task.answer", params);
    }
    let waiting = if answer_before_restart {
        wait_until("the answered retry to wait for the base", || {
            let task = get(&socket, &id);
            decisions(&task)
                .iter()
                .any(|d| d.contains("waiting for work to move"))
                .then_some(task)
        })
    } else {
        preexisting_question(&socket, &id)
    };
    let before = attempts(&waiting).len();
    if parent {
        assert_landed_children(&socket, &waiting);
    }
    let old_base = waiting["baseSha"].clone();
    assert_eq!(
        preexisting_count(&waiting),
        usize::from(answer_before_restart),
        "{waiting}"
    );
    stop_daemon(&socket);
    let _ = wait_for_exit(first);

    let second = spawn_daemon(home.path(), &env);
    wait_for_socket(&socket);
    if !answer_before_restart {
        request_on(&socket, "task.answer", json!({"id": id, "answer": RETRY}));
    }
    let expected_waits = if answer_before_restart { 2 } else { 1 };
    wait_until("the restarted retry to wait on the same sha", || {
        let task = get(&socket, &id);
        (decisions(&task)
            .iter()
            .filter(|d| d.contains("waiting for work to move"))
            .count()
            >= expected_waits)
            .then_some(task)
    });
    std::thread::sleep(Duration::from_secs(2));
    let resumed = get(&socket, &id);
    assert_eq!(resumed["baseSha"], old_base, "{resumed}");
    assert_eq!(
        attempts(&resumed).len(),
        before,
        "no attempt on the old base: {resumed}"
    );
    assert_eq!(preexisting_count(&resumed), 1, "{resumed}");
    assert!(resumed["question"].is_null(), "{resumed}");
    let base = fix_base(repo.path(), false);
    let finished = done(&socket, &id);
    assert_eq!(finished["baseSha"], base, "{finished}");
    assert_eq!(preexisting_count(&finished), 1, "{finished}");
    if parent {
        assert!(attempts(&finished).is_empty(), "{finished}");
        for (file, expected) in [("README.md", "child"), ("second.txt", "second")] {
            assert_eq!(
                git_out(
                    repo.path(),
                    &[
                        "show",
                        &format!("{}:{file}", finished["branch"].as_str().unwrap())
                    ]
                ),
                expected,
                "{finished}"
            );
        }
    }
    let expected = if policy {
        "policy"
    } else if agent {
        "orchestrator"
    } else {
        "owner"
    };
    assert_eq!(
        finished["questionHistory"][0]["answeredBy"], expected,
        "{finished}"
    );
    let worktrees = clean_worktrees(&socket, repo.path());
    stop_daemon(&socket);
    let _ = wait_for_exit(second);
    for wt in worktrees {
        remove_dir_retrying(wt);
    }
}

#[test]
fn a_parent_does_not_repeat_a_base_question_after_an_owner_retry_and_restart() {
    retry_survives_restart(true, false, false, true, false);
}

#[test]
fn an_owner_retry_still_waits_for_the_base_after_a_restart() {
    retry_survives_restart(false, false, false, true, false);
}

#[test]
fn an_orchestrator_retry_still_waits_for_the_base_after_a_restart() {
    retry_survives_restart(false, false, true, true, false);
}

#[test]
fn two_red_tasks_asking_about_one_base_do_not_make_the_policy_wait_for_each_other() {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake.sh", FAKE);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    configure(&daemon.socket, true);
    let repo = repo();
    // Both tasks sit on the same question: neither can move the base, so the
    // policy must not tell either of them to wait for the other.
    let first = create(&daemon.socket, repo.path(), false);
    preexisting_question(&daemon.socket, &first);
    let second = create(&daemon.socket, repo.path(), false);
    let waiting = preexisting_question(&daemon.socket, &second);
    std::thread::sleep(Duration::from_secs(2));
    for id in [&first, &second] {
        let task = get(&daemon.socket, id);
        assert_eq!(task["status"], "waiting", "{task}");
        assert!(task["question"].is_object(), "{task}");
        assert!(
            decisions(&task).iter().all(|d| !d.contains("to move past")),
            "{task}"
        );
    }
    assert_eq!(preexisting_count(&waiting), 0, "{waiting}");
    let worktrees = clean_worktrees(&daemon.socket, repo.path());
    daemon.shutdown_and_wait();
    for wt in worktrees {
        remove_dir_retrying(wt);
    }
}

#[test]
fn an_answer_to_a_base_question_after_restart_waits_for_the_base() {
    retry_survives_restart(false, false, false, false, false);
}

#[test]
fn a_targeted_final_keeps_the_check_and_retries_on_the_unchanged_base() {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake.sh", FAKE);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    configure(&daemon.socket, true);
    let repo = repo();
    let mut params = json!({"repo": repo.path(), "start": true, "land": false,
        "title": "Fix target check", "goal": "OWN_TARGET: fix grep -q fixed status.txt",
        "verify": ["true"], "finalVerify": [CHECK]});
    let id = request_on(&daemon.socket, "task.create", params.take())["id"]
        .as_str()
        .unwrap()
        .to_string();
    let original_base = get(&daemon.socket, &id)["baseSha"].clone();
    let finished = done(&daemon.socket, &id);
    assert_eq!(finished["baseSha"], original_base, "{finished}");
    let implementing = attempts(&finished);
    assert_eq!(implementing.len(), 2, "{finished}");
    assert!(
        implementing.last().unwrap()["verify"]
            .as_array()
            .unwrap()
            .iter()
            .any(|result| result["command"] == CHECK && result["code"] == 0),
        "{finished}"
    );
    assert!(
        decisions(&finished)
            .iter()
            .any(|d| d.contains("kept check") && d.contains("task is meant to make it pass")),
        "{finished}"
    );
    // The rule keeps the check by itself: no question, no waiting for the base.
    assert!(
        !decisions(&finished)
            .iter()
            .any(|d| d.contains("to move past")),
        "{finished}"
    );
    assert_eq!(preexisting_count(&finished), 0, "{finished}");
    let worktrees = clean_worktrees(&daemon.socket, repo.path());
    daemon.shutdown_and_wait();
    for wt in worktrees {
        remove_dir_retrying(wt);
    }
}

#[test]
fn an_untargeted_final_on_an_idle_base_stays_open_for_the_overseer() {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake.sh", FAKE);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    configure(&daemon.socket, true);
    let repo = repo();
    let id = request_on(
        &daemon.socket,
        "task.create",
        json!({"repo": repo.path(), "start": true, "land": false,
            "title": "Unrelated task", "goal": "IDLE_BASE: edit README without changing status.txt",
            "verify": ["true"], "finalVerify": [CHECK]}),
    )["id"]
        .as_str()
        .unwrap()
        .to_string();
    let waiting = preexisting_question(&daemon.socket, &id);
    assert_eq!(
        waiting["question"]["kind"], "preexisting_failure",
        "{waiting}"
    );
    assert!(
        decisions(&waiting)
            .iter()
            .all(|d| !d.contains("to move past")),
        "{waiting}"
    );
    assert!(
        decisions(&waiting)
            .iter()
            .all(|decision| !decision.starts_with("Policy:")),
        "{waiting}"
    );
    let worktrees = clean_worktrees(&daemon.socket, repo.path());
    daemon.shutdown_and_wait();
    for wt in worktrees {
        remove_dir_retrying(wt);
    }
}
