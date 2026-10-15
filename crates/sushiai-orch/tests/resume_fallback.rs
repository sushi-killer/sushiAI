//! A rejected native resume and its fresh fallback share one attempt budget.
mod common;

use common::harness::fake_harness_script;
use common::polling::{poll_until, remove_dir_retrying, wait_until};
use common::process::Daemon;
use common::repository::init_git_repo;
use serde_json::{json, Value};

const FAKE: &str = r###"#!/usr/bin/env python3
import json, os, pathlib, sys, time
root = pathlib.Path(os.environ['FALLBACK_LOG'])
brief = sys.stdin.read()
mode = os.environ.get('FALLBACK_MODE', 'normal')
if brief.startswith('## Route the review'):
    sys.exit(0)
if brief.startswith('## Review'):
    if pathlib.Path('SECOND').exists() and not (root / 'review-started').exists():
        (root / 'review-started').touch()
        time.sleep(45)
    print(json.dumps({'type': 'result', 'total_cost_usd': 0, 'result': '```sushi-review\n' + json.dumps({'verdict': 'PASS', 'findings': [], 'criteria': []}) + '\n```'}), flush=True)
    sys.exit(0)
call = root / 'calls'
n = int(call.read_text()) + 1 if call.exists() else 1
call.write_text(str(n))
(root / f'argv-{n}.json').write_text(json.dumps(sys.argv))
(root / f'brief-{n}.txt').write_text(brief)
def emit(value):
    print(json.dumps(value), flush=True)
def usage(tokens, suffix=''):
    emit({'type': 'assistant', 'message': {'id': f'gap-{n}{suffix}', 'model': 'fallback-fixture', 'content': [], 'usage': {'input_tokens': tokens, 'output_tokens': 0}}})
if '--resume' in sys.argv:
    if os.environ.get('THIRD_ATTEMPT') == '1':
        if n == 4:
            pathlib.Path('THIRD').touch()
            emit({'type': 'system', 'subtype': 'init', 'session_id': 'sess-fresh'})
            emit({'type': 'result', 'total_cost_usd': 0.12, 'usage': {'input_tokens': 2000, 'output_tokens': 0}, 'result': 'done'})
            sys.exit(0)
        emit({'type': 'result', 'total_cost_usd': 0.11, 'usage': {'input_tokens': 10000, 'output_tokens': 0}, 'is_error': True, 'result': 'session unavailable'})
        sys.exit(1)
    usage(10000)
    if mode == 'missing':
        pathlib.Path(__file__).unlink()
    print('session unavailable after partial usage', file=sys.stderr, flush=True)
    sys.exit(1)
emit({'type': 'system', 'subtype': 'init', 'session_id': 'sess-fallback' if n == 1 else 'sess-fresh'})
if n == 1:
    pathlib.Path('FIRST').write_text('first\n')
    tokens, cost = 1000, 0.01
else:
    pathlib.Path('SECOND').write_text('second\n')
    tokens, cost = 10000, 0.1
if n > 1 and mode == 'cap':
    usage(1000, '-initial')
    (root / 'fresh-running').touch()
    until = time.monotonic() + 40
    while not (root / 'release-fresh').exists() and time.monotonic() < until:
        time.sleep(0.01)
    usage(9000, '-remaining')
else:
    usage(tokens)
if n > 1 and os.environ.get('HOLD_FRESH') == '1':
    (root / 'fresh-running').touch()
    time.sleep(30)
report = {'outcome': 'complete', 'summary': 'done', 'decisions': [], 'question': ''}
emit({'type': 'result', 'total_cost_usd': cost, 'usage': {'input_tokens': tokens, 'output_tokens': 0}, 'result': '```sushi-report\n' + json.dumps(report) + '\n```'})
"###;

struct Fixture {
    daemon: Daemon,
    log: tempfile::TempDir,
    _repo: tempfile::TempDir,
    scripts: tempfile::TempDir,
    mode: &'static str,
    id: String,
}

impl Fixture {
    fn start(cap: f64, mode: &'static str) -> Self {
        let hold = mode == "hold";
        let third = mode == "third";
        let scripts = tempfile::tempdir().unwrap();
        let log = tempfile::tempdir().unwrap();
        let script = fake_harness_script(scripts.path(), "fallback.py", FAKE);
        let missing_codex = scripts.path().join("missing-codex");
        let mut extra = vec![
            ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
            ("FALLBACK_LOG", log.path().to_str().unwrap()),
            ("HOLD_FRESH", if hold { "1" } else { "0" }),
            ("THIRD_ATTEMPT", if third { "1" } else { "0" }),
            ("FALLBACK_MODE", mode),
        ];
        if mode == "missing" {
            extra.push(("ORCHD_CODEX_BIN", missing_codex.to_str().unwrap()));
        }
        let daemon = Daemon::spawn(&extra);
        let mut settings = daemon.request("settings.get", json!({}));
        settings["review"] = json!(if mode == "review" {
            "claude-sonnet"
        } else {
            ""
        });
        settings["sandbox"] = json!("host");
        settings["maxAttempts"] = json!(if third { 3 } else { 2 });
        settings["tiers"] = json!({
            "mechanical": "claude-sonnet", "standard": "claude-sonnet", "hard": "claude-sonnet",
        });
        settings["prices"]["fallback-fixture"] =
            json!({"input": 10.0, "cachedInput": 0.0, "output": 0.0});
        daemon.request("settings.set", json!({"settings": settings}));
        let repo = init_git_repo();
        let task = daemon.request(
            "task.create",
            json!({
                "repo": repo.path(), "title": "Resume fallback accounting", "goal": "Create SECOND",
                "verify": [if third { "test -f THIRD" } else { "test -f SECOND" }], "variant": {"maxAttemptCostUsd": cap},
                "land": false, "start": true,
            }),
        );
        Self {
            daemon,
            log,
            _repo: repo,
            scripts,
            mode,
            id: task["id"].as_str().unwrap().into(),
        }
    }

    fn restart(&mut self) {
        let script = self.scripts.path().join("fallback.py");
        self.daemon.restart(&[
            ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
            ("FALLBACK_LOG", self.log.path().to_str().unwrap()),
            ("HOLD_FRESH", if self.mode == "hold" { "1" } else { "0" }),
            (
                "THIRD_ATTEMPT",
                if self.mode == "third" { "1" } else { "0" },
            ),
            ("FALLBACK_MODE", self.mode),
        ]);
    }

    fn settled(&self) -> Value {
        poll_until(&self.daemon, &self.id, |s| {
            matches!(s, "done" | "waiting" | "failed" | "stopped")
        })
    }

    fn calls(&self) -> u64 {
        std::fs::read_to_string(self.log.path().join("calls"))
            .unwrap()
            .parse()
            .unwrap()
    }

    fn fresh_cap(&self) -> f64 {
        let argv: Vec<String> = serde_json::from_str(
            &std::fs::read_to_string(self.log.path().join("argv-3.json")).unwrap(),
        )
        .unwrap();
        argv.windows(2)
            .find(|args| args[0] == "--max-budget-usd")
            .unwrap()[1]
            .parse()
            .unwrap()
    }

    fn assert_accounted(&self, task: &Value, runs: u64, cost: f64, input: u64) {
        let rows: Vec<Value> = std::fs::read_to_string(self.daemon.data_dir().join("costs.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .filter(|row| row["taskId"] == self.id && row["stage"] == "implement")
            .collect();
        assert_eq!(rows.len() as u64, runs, "{rows:?}");
        assert_eq!(
            rows.iter()
                .map(|row| row["inputTokens"].as_u64().unwrap())
                .sum::<u64>(),
            input,
            "{rows:?}"
        );
        assert!(
            (rows
                .iter()
                .map(|row| row["costUsd"].as_f64().unwrap())
                .sum::<f64>()
                - cost)
                .abs()
                < 1e-9,
            "{rows:?}"
        );
        assert!(
            (task["costUsd"].as_f64().unwrap() - cost).abs() < 1e-9,
            "{task}"
        );
        assert_eq!(
            task["attempts"][1]["usage"]["input"],
            input - 1000,
            "{task}"
        );
        assert!(
            (task["attempts"][1]["costUsd"].as_f64().unwrap() - (cost - 0.01)).abs() < 1e-9,
            "{task}"
        );
    }

    fn assert_attempt_accounted(&self, task: &Value, runs: usize, cost: f64, input: u64) {
        let rows: Vec<Value> = std::fs::read_to_string(self.daemon.data_dir().join("costs.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let own: Vec<_> = rows
            .iter()
            .filter(|row| row["attempt"] == 2 && row["stage"] == "implement")
            .collect();
        assert_eq!(own.len(), runs, "{rows:?}");
        assert!(
            (own.iter()
                .map(|row| row["costUsd"].as_f64().unwrap())
                .sum::<f64>()
                - cost)
                .abs()
                < 1e-9,
            "{rows:?}"
        );
        assert_eq!(
            own.iter()
                .map(|row| row["inputTokens"].as_u64().unwrap())
                .sum::<u64>(),
            input,
            "{rows:?}"
        );
        assert!(
            (task["attempts"][1]["costUsd"].as_f64().unwrap() - cost).abs() < 1e-9,
            "{task}"
        );
        assert_eq!(task["attempts"][1]["usage"]["input"], input, "{task}");
        let ledger: f64 = rows
            .iter()
            .map(|row| row["costUsd"].as_f64().unwrap())
            .sum();
        assert!(
            (task["costUsd"].as_f64().unwrap() - ledger).abs() < 1e-9,
            "{task}"
        );
    }

    fn finish(self, task: &Value) {
        let worktree = task["worktree"].as_str().unwrap().to_string();
        self.daemon.shutdown_and_wait();
        remove_dir_retrying(worktree);
    }
}

#[test]
fn a_billed_rejected_resume_keeps_both_invocations_in_one_attempt_without_duplicating_the_ledger() {
    let fixture = Fixture::start(0.4, "normal");
    let task = fixture.settled();
    assert_eq!(task["status"], "done", "{task}");
    assert_eq!(task["attempts"].as_array().unwrap().len(), 2);
    assert_eq!(fixture.calls(), 3);
    fixture.assert_accounted(&task, 3, 0.21, 21000);
    assert!((fixture.fresh_cap() - 0.3).abs() < 1e-9);
    let brief = std::fs::read_to_string(fixture.log.path().join("brief-3.txt")).unwrap();
    assert!(
        brief.contains("## Task") && brief.contains("test -f SECOND exited 1."),
        "{brief}"
    );
    fixture.finish(&task);
}

#[test]
fn a_fresh_fallback_gets_only_the_remaining_cli_and_streaming_attempt_budget() {
    let fixture = Fixture::start(0.15, "normal");
    let task = fixture.settled();
    assert!((fixture.fresh_cap() - 0.05).abs() < 1e-9);
    assert_eq!(task["attempts"][1]["failure"]["kind"], "budget", "{task}");
    fixture.assert_attempt_accounted(&task, 2, 0.2, 20000);
    fixture.finish(&task);
}

#[test]
fn an_exhausted_rejected_resume_does_not_start_a_fresh_invocation() {
    let fixture = Fixture::start(0.1, "normal");
    let task = fixture.settled();
    assert_eq!(task["attempts"][1]["failure"]["kind"], "budget", "{task}");
    fixture.assert_attempt_accounted(&task, 1, 0.1, 10000);
    fixture.finish(&task);
}

#[test]
fn stopping_a_fresh_fallback_keeps_the_billed_rejection_and_partial_fresh_usage() {
    let mut fixture = Fixture::start(0.4, "hold");
    wait_until("fresh fallback usage", || {
        let events = fixture
            .daemon
            .data_dir()
            .join("tasks")
            .join(&fixture.id)
            .join("runs/2/events.jsonl");
        (fixture.log.path().join("fresh-running").exists()
            && std::fs::read_to_string(events)
                .unwrap_or_default()
                .contains("gap-3"))
        .then_some(())
    });
    fixture
        .daemon
        .request("task.stop", json!({"id": fixture.id}));
    wait_until("cancelled native run accounting", || {
        let summary = fixture.daemon.request("costs.summary", json!({}));
        (summary["totals"]["runs"] == 3).then_some(())
    });
    common::protocol::stop_daemon(&fixture.daemon.socket);
    wait_until("owned daemon shutdown", || {
        fixture.daemon.child.try_wait().ok().flatten().map(|_| ())
    });
    let task: Value = serde_json::from_str(
        &std::fs::read_to_string(
            fixture
                .daemon
                .data_dir()
                .join("tasks")
                .join(&fixture.id)
                .join("task.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(task["status"], "stopped", "{task}");
    fixture.assert_accounted(&task, 3, 0.21, 21000);
    fixture.finish(&task);
}

#[test]
fn a_third_resume_subtracts_only_the_fresh_sessions_spend_from_its_cumulative_total() {
    let fixture = Fixture::start(0.4, "third");
    let task = fixture.settled();
    assert_eq!(task["status"], "done", "{task}");
    assert_eq!(fixture.calls(), 4);
    assert_eq!(task["attempts"].as_array().unwrap().len(), 3);
    assert!(
        (task["costUsd"].as_f64().unwrap() - 0.23).abs() < 1e-9,
        "{task}"
    );
    assert!(
        (task["attempts"][1]["costUsd"].as_f64().unwrap() - 0.2).abs() < 1e-9,
        "{task}"
    );
    assert!(
        (task["attempts"][2]["costUsd"].as_f64().unwrap() - 0.02).abs() < 1e-9,
        "{task}"
    );
    let summary = fixture.daemon.request("costs.summary", json!({}));
    assert_eq!(summary["totals"]["runs"], 4, "{summary}");
    assert_eq!(summary["totals"]["tokens"]["input"], 23000, "{summary}");
    assert!(
        (summary["totals"]["costUsd"].as_f64().unwrap() - 0.23).abs() < 1e-9,
        "{summary}"
    );
    fixture.finish(&task);
}

#[test]
fn restarting_during_review_keeps_the_already_aggregated_fallback_receipt_once() {
    let mut fixture = Fixture::start(0.4, "review");
    wait_until("review after fallback", || {
        fixture
            .log
            .path()
            .join("review-started")
            .exists()
            .then_some(())
    });
    let before = fixture
        .daemon
        .request("task.get", json!({"id": fixture.id}));
    fixture.assert_accounted(&before, 3, 0.21, 21000);
    fixture.restart();
    let task = fixture.settled();
    assert_eq!(task["status"], "done", "{task}");
    assert_eq!(fixture.calls(), 3);
    fixture.assert_accounted(&task, 3, 0.21, 21000);
    fixture.finish(&task);
}

#[test]
fn an_adopted_fresh_fallback_keeps_its_remaining_attempt_cap_after_restart() {
    let mut fixture = Fixture::start(0.15, "cap");
    wait_until("fresh fallback before remaining spend", || {
        let events = fixture
            .daemon
            .data_dir()
            .join("tasks")
            .join(&fixture.id)
            .join("runs/2/events.jsonl");
        std::fs::read_to_string(events)
            .unwrap_or_default()
            .contains("gap-3-initial")
            .then_some(())
    });
    fixture.restart();
    std::fs::write(fixture.log.path().join("release-fresh"), "").unwrap();
    let task = fixture.settled();
    assert_eq!(task["attempts"][1]["failure"]["kind"], "budget", "{task}");
    fixture.assert_attempt_accounted(&task, 2, 0.2, 20000);
    fixture.finish(&task);
}

#[test]
fn a_missing_fresh_executable_keeps_the_billed_rejected_attempt_and_its_number() {
    let fixture = Fixture::start(0.4, "missing");
    let task = fixture.settled();
    assert_eq!(task["status"], "waiting", "{task}");
    assert_eq!(fixture.calls(), 2);
    assert_eq!(task["attempts"].as_array().unwrap().len(), 2, "{task}");
    assert_eq!(task["attempts"][1]["n"], 2);
    assert_eq!(task["attempts"][1]["failure"]["kind"], "error", "{task}");
    fixture.assert_attempt_accounted(&task, 1, 0.1, 10000);
    assert!(
        (task["costUsd"].as_f64().unwrap() - 0.11).abs() < 1e-9,
        "{task}"
    );
    fixture.finish(&task);
}
