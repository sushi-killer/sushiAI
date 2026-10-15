//! Black-box checks of pass reuse and immediate retries through real task gates.

mod common;

use common::harness::fake_harness_script;
use common::polling::{remove_dir_retrying, settle, wait_until};
use common::process::Daemon;
use common::repository::init_git_repo;

use serde_json::{json, Value};

const FAKE: &str = r#"#!/bin/sh
cat > "$LOG_DIR/brief.$$"
echo implement >> "$LOG_DIR/implementations"
n=$(wc -l < "$LOG_DIR/implementations" | tr -d ' ')
if [ "$PAUSE_RETRY" = yes ] && [ "$n" -eq 2 ]; then
  touch "$LOG_DIR/retry-ready"
  while [ ! -f "$LOG_DIR/allow-retry" ]; do sleep 0.05; done
fi
if [ -f "$LOG_DIR/content" ]; then
  cat "$LOG_DIR/content" > CHANGED_MARKER.txt
else
  echo changed > CHANGED_MARKER.txt
fi
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}'
"#;

// Base preflights pass without changing the counter; the task gates count runs.
const CHECK: &str = r#"if [ -f CHANGED_MARKER.txt ]; then echo run >> "$LOG_DIR/counter"; fi"#;

struct Fixture {
    daemon: Daemon,
    repo: tempfile::TempDir,
    log: tempfile::TempDir,
    _scripts: tempfile::TempDir,
}

impl Fixture {
    fn new(pause_retry: bool) -> Self {
        let scripts = tempfile::tempdir().unwrap();
        let script = fake_harness_script(scripts.path(), "fake.sh", FAKE);
        let log = tempfile::tempdir().unwrap();
        let daemon = Daemon::spawn(&[
            ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
            ("LOG_DIR", log.path().to_str().unwrap()),
            ("PAUSE_RETRY", if pause_retry { "yes" } else { "no" }),
        ]);
        let mut settings = daemon.request("settings.get", json!({}));
        settings["review"] = json!("");
        settings["sandbox"] = json!("host");
        settings["verifyTimeoutSecs"] = json!(1);
        settings["maxAttempts"] = json!(3);
        settings["experiments"]["land"] = json!(false);
        daemon.request("settings.set", json!({"settings": settings}));
        Self {
            daemon,
            repo: init_git_repo(),
            log,
            _scripts: scripts,
        }
    }

    fn create(&self, command: &str, final_check: bool) -> Value {
        self.daemon.request(
            "task.create",
            json!({
                "repo": self.repo.path().to_str().unwrap(),
                "title": "Check reuse",
                "goal": "Write the marker",
                "verify": [command],
                "finalVerify": if final_check { vec![command] } else { vec![] },
            }),
        )
    }

    fn runs(&self) -> usize {
        std::fs::read_to_string(self.log.path().join("counter"))
            .unwrap_or_default()
            .lines()
            .count()
    }

    fn finish(self, tasks: &[Value]) {
        self.daemon.shutdown_and_wait();
        for task in tasks {
            remove_dir_retrying(task["worktree"].as_str().unwrap());
        }
    }
}

fn decisions(task: &Value) -> Vec<&str> {
    task["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| line.as_str().unwrap())
        .collect()
}

#[test]
fn a_pass_on_a_tree_is_reused() {
    let fixture = Fixture::new(false);
    let task = fixture.create(CHECK, true);
    let done = settle(&fixture.daemon, task["id"].as_str().unwrap());
    assert_eq!(done["status"], "done", "{done}");
    assert_eq!(
        fixture.runs(),
        1,
        "verify and finalVerify duplicated a pass"
    );
    let prefix = format!("Orchestrator: {CHECK} already passed on tree ");
    let lines = decisions(&done);
    let reused = lines
        .iter()
        .filter(|line| line.starts_with(&prefix) && line.ends_with("; not run again"))
        .collect::<Vec<_>>();
    assert_eq!(reused.len(), 1, "{done}");
    let tree = reused[0]
        .strip_prefix(&prefix)
        .unwrap()
        .strip_suffix("; not run again")
        .unwrap();
    assert_eq!(tree.len(), 8);
    assert!(tree.chars().all(|c| c.is_ascii_hexdigit()));
    let verify = done["attempts"][0]["verify"].as_array().unwrap();
    assert_eq!(verify.len(), 2);
    assert_eq!(verify[1]["code"], 0);
    assert_eq!(verify[1]["ms"], 0);
    fixture.finish(&[done]);
}

#[test]
fn a_different_tree_runs_the_check_again() {
    let fixture = Fixture::new(false);
    std::fs::write(fixture.log.path().join("content"), "first tree\n").unwrap();
    let task = fixture.create(CHECK, true);
    let first = settle(&fixture.daemon, task["id"].as_str().unwrap());
    assert_eq!(first["status"], "done", "{first}");
    assert_eq!(fixture.runs(), 1);
    std::fs::write(fixture.log.path().join("content"), "second tree\n").unwrap();
    let task = fixture.create(CHECK, true);
    let second = settle(&fixture.daemon, task["id"].as_str().unwrap());
    assert_eq!(second["status"], "done", "{second}");
    assert_eq!(
        fixture.runs(),
        2,
        "the new tree reused an older tree's pass"
    );
    fixture.finish(&[first, second]);
}

#[test]
fn a_retry_of_failed_finals_runs_only_the_failed_command() {
    let fixture = Fixture::new(false);
    let passed = r#"if [ -f CHANGED_MARKER.txt ]; then echo run >> "$LOG_DIR/first-final"; fi"#;
    let failed = r#"if [ -f CHANGED_MARKER.txt ]; then echo run >> "$LOG_DIR/second-final"; test "$(wc -l < "$LOG_DIR/implementations")" -ge 2; fi"#;
    let task = fixture.daemon.request(
        "task.create",
        json!({
            "repo": fixture.repo.path().to_str().unwrap(),
            "title": "Retry failed finals",
            "goal": "Write the marker",
            "verify": [CHECK],
            "finalVerify": [passed, failed],
        }),
    );
    let done = settle(&fixture.daemon, task["id"].as_str().unwrap());
    assert_eq!(done["status"], "done", "{done}");
    assert_eq!(done["attempts"].as_array().unwrap().len(), 2, "{done}");
    assert_eq!(fixture.runs(), 1, "verify reran on the same tree");
    let runs = |name| {
        std::fs::read_to_string(fixture.log.path().join(name))
            .unwrap()
            .lines()
            .count()
    };
    assert_eq!(runs("first-final"), 1, "a passed final ran twice");
    assert_eq!(runs("second-final"), 2, "the failed final did not rerun");
    assert!(decisions(&done)
        .iter()
        .any(|line| { line.contains(passed) && line.contains("already passed on tree") }));
    fixture.finish(&[done]);
}

fn interrupted_check(first_runs: &str, expected_code: Value) {
    let fixture = Fixture::new(true);
    let command = format!(
        r#"if [ -f CHANGED_MARKER.txt ]; then echo run >> "$LOG_DIR/counter"; n=$(wc -l < "$LOG_DIR/counter"); if [ "$n" -le 2 ]; then {first_runs}; fi; fi"#
    );
    let task = fixture.create(&command, false);
    let id = task["id"].as_str().unwrap();
    let first = wait_until("the retry implementer after the first gate", || {
        if fixture.log.path().join("retry-ready").exists() {
            Some(fixture.daemon.request("task.get", json!({"id": id})))
        } else {
            None
        }
    });
    assert_eq!(
        fixture.runs(),
        2,
        "the first gate must run exactly twice: {first}"
    );
    assert_eq!(first["attempts"][0]["verify"][0]["code"], expected_code);
    let prefix = format!("Orchestrator: {command} was interrupted after ");
    let lines = decisions(&first);
    let line = lines
        .iter()
        .find(|line| line.starts_with(&prefix))
        .unwrap_or_else(|| panic!("{first}"));
    let durations = line.strip_prefix(&prefix).unwrap();
    let (a, remainder) = durations.split_once(" s (").unwrap();
    assert!(a.parse::<f64>().unwrap() >= 0.0);
    assert_eq!(a.split('.').nth(1).unwrap().len(), 1);
    let b = remainder
        .split_once("; ran it again at once: interrupted after ")
        .unwrap()
        .1
        .strip_suffix(" s")
        .unwrap();
    assert!(b.parse::<f64>().unwrap() >= 0.0);
    assert_eq!(b.split('.').nth(1).unwrap().len(), 1);
    std::fs::write(fixture.log.path().join("allow-retry"), "go").unwrap();
    let done = settle(&fixture.daemon, id);
    assert_eq!(done["status"], "done", "{done}");
    assert_eq!(
        fixture.runs(),
        3,
        "a retry replayed the interrupted result: {done}"
    );
    assert_eq!(done["attempts"][1]["verify"][0]["code"], 0);
    assert!(!done["attempts"][1]["verify"][0]["tail"]
        .as_str()
        .unwrap()
        .contains("timed out"));
    fixture.finish(&[done]);
}

#[test]
fn a_timed_out_check_runs_once_more_and_is_never_replayed() {
    interrupted_check("sleep 60", Value::Null);
}

#[test]
fn a_check_killed_with_137_is_treated_as_interrupted() {
    interrupted_check("exit 137", json!(137));
}
