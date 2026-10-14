//! Environment failures get one fresh base checkout, never a cached base
//! answer. Ordinary failures still get their separate flaky rerun.

mod common;

use common::*;
use serde_json::{json, Value};
use std::path::Path;

const FAKE: &str = r###"#!/bin/sh
brief=$(cat)
case "$brief" in
'## Route the review'*) exit 0 ;;
"## Review"*)
  printf '%s\n' '{"type":"result","result":"```sushi-review\n{\"verdict\":\"PASS\",\"findings\":[]}\n```"}' ;;
*)
  if [ -n "$BASE_LOG" ]; then
    until [ -f "$BASE_LOG" ] && [ "$(wc -l < "$BASE_LOG")" -ge "${BASE_MIN_RUNS:-2}" ]; do sleep 0.05; done
    # Let the background job finish before the final lookup: an unknown
    # result must not start another pair of base runs.
    while [ -n "$(ls -A "$SUSHIAI_HOME/orchestrator/base-runs")" ]; do sleep 0.05; done
  fi
  echo changed > CHANGED_MARKER.txt
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}' ;;
esac
"###;

fn prepare(daemon: &Daemon) {
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!("claude-opus");
    settings["maxAttempts"] = json!(1);
    fit_sandbox(&mut settings);
    daemon.request("settings.set", json!({"settings": settings}));
}

fn create(daemon: &Daemon, repo: &Path, check: &str) -> String {
    let task = daemon.request(
        "task.create",
        json!({
            "repo": repo.to_str().unwrap(),
            "title": "Base environment failure",
            "goal": "Write the marker",
            "verify": ["true"],
            "finalVerify": [check],
            "start": true,
        }),
    );
    task["id"].as_str().unwrap().to_string()
}

fn waiting(daemon: &Daemon, id: &str) -> Value {
    wait_until("the task to ask after its first attempt", || {
        let task = daemon.request("task.get", json!({"id": id}));
        // A base question waits after the first attempt; a task whose own
        // check keeps failing ends `failed` once the ladder is spent.
        (task["status"] == "failed"
            || (task["status"] == "waiting" && task["attempts"].as_array().unwrap().len() == 1))
            .then_some(task)
    })
}

fn cached(daemon: &Daemon) -> Vec<Value> {
    match std::fs::read_to_string(daemon.data_dir().join("base-results.json")) {
        Ok(text) => serde_json::from_str(&text).unwrap(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => panic!("reading base results: {e}"),
    }
}

fn base_paths(log: &Path) -> Vec<String> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(String::from)
        .collect()
}

fn assert_fresh_and_removed(log: &Path, count: usize) {
    let paths = base_paths(log);
    assert_eq!(paths.len(), count, "{paths:?}");
    for (i, path) in paths.iter().enumerate() {
        assert!(!paths[..i].contains(path), "fresh base worktree: {paths:?}");
        assert!(
            !Path::new(path).exists(),
            "base worktree was removed: {path}"
        );
    }
}

fn assert_unknown(daemon: &Daemon, task: &Value, check: &str) {
    assert_ne!(task["question"]["kind"], "preexisting_failure", "{task}");
    assert!(
        task["questionHistory"]
            .as_array()
            .into_iter()
            .flatten()
            .all(|q| q["kind"] != "preexisting_failure"),
        "{task}"
    );
    assert!(
        cached(daemon)
            .iter()
            .all(|r| r["outcome"]["command"] != check),
        "an environment result must not be cached"
    );
}

#[test]
fn an_install_failure_is_retried_fresh_before_a_plain_base_failure_is_quoted() {
    let scripts = tempfile::tempdir().unwrap();
    let harness = fake_harness_script(scripts.path(), "fake.sh", FAKE);
    let log = scripts.path().join("base.log");
    let check_script = fake_harness_script(
        scripts.path(),
        "check.sh",
        &format!(
            "#!/bin/sh\ncase \"$PWD\" in */base-runs/*)\n echo \"$PWD\" >> {log}\n if [ \"$(wc -l < {log})\" -eq 1 ]; then echo 'npm ERR! eExIsT'; exit 1; fi;;\nesac\ncase \"$PWD\" in */base-runs/*);; *) echo worktree;; esac\necho 'plain base failure'; exit 1\n",
            log = log.display(),
        ),
    );
    let check = format!("sh {}", check_script.display());
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", harness.to_str().unwrap())]);
    prepare(&daemon);
    let repo = init_git_repo();
    let id = create(&daemon, repo.path(), &check);
    let task = waiting(&daemon, &id);
    assert_eq!(task["question"]["kind"], "preexisting_failure", "{task}");
    let text = task["question"]["text"].as_str().unwrap();
    assert!(text.contains("plain base failure"), "{task}");
    assert!(!text.to_ascii_lowercase().contains("eexist"), "{task}");
    let rows = cached(&daemon);
    assert!(rows.iter().any(|r| {
        r["outcome"]["command"] == check && r["outcome"]["tail"] == "plain base failure\n"
    }));
    assert!(rows.iter().all(|r| !r["outcome"]["tail"]
        .as_str()
        .unwrap()
        .to_ascii_lowercase()
        .contains("eexist")));
    assert_fresh_and_removed(&log, 3);
    let wt = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(wt);
}

#[test]
fn persistent_environment_failures_run_exactly_twice_and_remain_unknown() {
    for marker in [
        "npm ERR! EEXIST",
        "FaIlEd To InStAlL dependencies",
        "SETUP FAILED",
    ] {
        let scripts = tempfile::tempdir().unwrap();
        let harness = fake_harness_script(scripts.path(), "fake.sh", FAKE);
        let log = scripts.path().join("base.log");
        let check_script = fake_harness_script(
            scripts.path(),
            "check.sh",
            &format!(
                "#!/bin/sh\ncase \"$PWD\" in */base-runs/*) echo \"$PWD\" >> {}; echo '{marker}'; exit 1;; esac\necho 'task failure'; exit 1\n",
                log.display(),
            ),
        );
        let check = format!("sh {}", check_script.display());
        let daemon = Daemon::spawn(&[
            ("ORCHD_CLAUDE_BIN", harness.to_str().unwrap()),
            ("BASE_LOG", log.to_str().unwrap()),
        ]);
        prepare(&daemon);
        let repo = init_git_repo();
        let id = create(&daemon, repo.path(), &check);
        let task = waiting(&daemon, &id);
        assert_unknown(&daemon, &task, &check);
        assert_fresh_and_removed(&log, 2);
        // Another task may try the same pair again: the first task's
        // completed job receipt is not a cached answer for this base.
        let id2 = create(&daemon, repo.path(), &check);
        let task2 = waiting(&daemon, &id2);
        assert_unknown(&daemon, &task2, &check);
        assert_fresh_and_removed(&log, 4);
        let wt = task["worktree"].as_str().unwrap().to_string();
        let wt2 = task2["worktree"].as_str().unwrap().to_string();
        daemon.shutdown_and_wait();
        remove_dir_retrying(wt);
        remove_dir_retrying(wt2);
    }
}

#[test]
fn a_persisted_install_failure_is_discarded_after_a_restart() {
    let scripts = tempfile::tempdir().unwrap();
    let harness = fake_harness_script(scripts.path(), "fake.sh", FAKE);
    let env = [("ORCHD_CLAUDE_BIN", harness.to_str().unwrap())];
    let log = scripts.path().join("base.log");
    let check = format!(
        "case \"$PWD\" in */base-runs/*) echo \"$PWD\" >> {};; esac; case \"$PWD\" in */base-runs/*);; *) echo worktree;; esac; echo 'plain failure'; exit 1",
        log.display(),
    );
    let mut daemon = Daemon::spawn(&env);
    prepare(&daemon);
    let repo = init_git_repo();
    let id = create(&daemon, repo.path(), &check);
    let task = waiting(&daemon, &id);
    assert_eq!(task["question"]["kind"], "preexisting_failure", "{task}");
    assert_fresh_and_removed(&log, 2);
    let mut rows = cached(&daemon);
    let row = rows
        .iter_mut()
        .find(|r| r["outcome"]["command"] == check)
        .unwrap();
    row["outcome"]["tail"] = json!("npm ERR! EEXIST: failed to install");
    std::fs::write(
        daemon.data_dir().join("base-results.json"),
        serde_json::to_vec(&rows).unwrap(),
    )
    .unwrap();

    daemon.restart(&env);
    assert!(cached(&daemon)
        .iter()
        .all(|r| r["outcome"]["command"] != check));
    let id2 = create(&daemon, repo.path(), &check);
    let task2 = waiting(&daemon, &id2);
    assert_eq!(task2["question"]["kind"], "preexisting_failure", "{task2}");
    assert!(task2["question"]["text"]
        .as_str()
        .unwrap()
        .contains("plain failure"));
    assert_fresh_and_removed(&log, 4);
    let wt = task["worktree"].as_str().unwrap().to_string();
    let wt2 = task2["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(wt);
    remove_dir_retrying(wt2);
}

#[test]
fn base_worktree_setup_errors_get_one_fresh_retry() {
    for always_fails in [false, true] {
        let scripts = tempfile::tempdir().unwrap();
        let harness = fake_harness_script(scripts.path(), "fake.sh", FAKE);
        let log = scripts.path().join("setup.log");
        let setup = json!({"setup": format!(
            "case \"$PWD\" in */base-runs/*) echo \"$PWD\" >> {log}; if {condition}; then echo broken >&2; exit 4; fi;; esac; true",
            log = log.display(),
            condition = if always_fails { "true".to_string() } else { format!("[ \"$(wc -l < {})\" -eq 1 ]", log.display()) },
        )});
        let daemon = Daemon::spawn(&[
            ("ORCHD_CLAUDE_BIN", harness.to_str().unwrap()),
            ("BASE_LOG", log.to_str().unwrap()),
        ]);
        prepare(&daemon);
        let repo = init_git_repo();
        std::fs::create_dir_all(repo.path().join(".sushiai")).unwrap();
        std::fs::write(repo.path().join(".sushiai/setup.json"), setup.to_string()).unwrap();
        git_out(repo.path(), &["add", ".sushiai/setup.json"]);
        git_out(repo.path(), &["commit", "-q", "-m", "add setup"]);
        let check = "case \"$PWD\" in */base-runs/*);; *) echo worktree;; esac; echo 'plain failure'; exit 1";
        let id = create(&daemon, repo.path(), check);
        let task = waiting(&daemon, &id);
        if always_fails {
            assert_unknown(&daemon, &task, check);
            assert_fresh_and_removed(&log, 2);
        } else {
            assert_eq!(task["question"]["kind"], "preexisting_failure", "{task}");
            assert!(task["question"]["text"]
                .as_str()
                .unwrap()
                .contains("plain failure"));
            assert_fresh_and_removed(&log, 3);
        }
        let wt = task["worktree"].as_str().unwrap().to_string();
        daemon.shutdown_and_wait();
        remove_dir_retrying(wt);
    }
}

#[test]
fn environment_and_flaky_reruns_have_separate_budgets() {
    for ordinary_failure in [false, true] {
        let scripts = tempfile::tempdir().unwrap();
        let harness = fake_harness_script(scripts.path(), "fake.sh", FAKE);
        let log = scripts.path().join("base.log");
        let check_script = fake_harness_script(
            scripts.path(),
            "check.sh",
            &format!(
                "#!/bin/sh\ncase \"$PWD\" in */base-runs/*) echo \"$PWD\" >> {log}; n=$(wc -l < {log}); if [ \"$n\" -eq 1 ]; then echo EEXIST; exit 1; fi; if {ordinary_failure} && [ \"$n\" -eq 2 ]; then echo 'ordinary failure'; exit 1; fi;; esac; true\n",
                log = log.display(),
            ),
        );
        let check = format!("sh {}", check_script.display());
        let daemon = Daemon::spawn(&[
            ("ORCHD_CLAUDE_BIN", harness.to_str().unwrap()),
            ("BASE_LOG", log.to_str().unwrap()),
            ("BASE_MIN_RUNS", if ordinary_failure { "3" } else { "2" }),
        ]);
        prepare(&daemon);
        let repo = init_git_repo();
        let id = create(&daemon, repo.path(), &check);
        let task = poll_until(&daemon, &id, |s| matches!(s, "done" | "waiting" | "failed"));
        assert_eq!(task["status"], "done", "{task}");
        assert_fresh_and_removed(&log, if ordinary_failure { 3 } else { 2 });
        assert_eq!(
            task["decisions"].as_array().unwrap().iter().any(|d| {
                let d = d.as_str().unwrap();
                d.contains(&check) && d.contains("flaky on")
            }),
            ordinary_failure,
            "{task}"
        );
        assert!(cached(&daemon)
            .iter()
            .any(|r| { r["outcome"]["command"] == check && r["outcome"]["code"] == 0 }));
        let wt = task["worktree"].as_str().unwrap().to_string();
        daemon.shutdown_and_wait();
        remove_dir_retrying(wt);
    }
}
