//! Black-box integration tests (review rounds): finding ids, the two-run
//! bound on an attempt's review loop, and what happens to a finding that
//! comes back or repeats.

mod common;

use common::*;
use serde_json::{json, Value};
use std::path::Path;

/// One fake for every Claude role. A review brief ("## Review...") takes the
/// n-th line of `$LOG_DIR/review.plan` (the last line again when n runs
/// past it) and is kept as `review.brief.<n>`. An advisor brief gets advice.
/// An implement run keeps its argv as `implement.args.<pid>`, appends to
/// CHANGED_MARKER.txt, creates `fixed` when its brief names a criterion made
/// from a review finding, and answers `implement.reply`, or `implement.disputed`
/// when its brief shows a failed review and that file exists.
const FAKE_ROUNDS_SCRIPT: &str = r###"#!/bin/sh
brief=$(cat)
case "$brief" in
"An implement attempt at this task failed"*)
  printf '%s\n' '{"type":"result","total_cost_usd":0.03,"usage":{"input_tokens":1,"output_tokens":1},"result":"Fix it."}' ;;
"## Review"*)
  n=$(( $(ls "$LOG_DIR"/review.brief.* 2>/dev/null | wc -l) + 1 ))
  printf '%s' "$brief" > "$LOG_DIR/review.brief.$n"
  line=$(sed -n "${n}p" "$LOG_DIR/review.plan")
  [ -z "$line" ] && line=$(tail -n 1 "$LOG_DIR/review.plan")
  printf '%s\n' "$line" ;;
*)
  printf '%s\n' "$@" > "$LOG_DIR/implement.args.$$"
  echo changed >> CHANGED_MARKER.txt
  case "$brief" in *"Review finding F-"*) touch "$LOG_DIR/fixed" ;; esac
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  reply="$LOG_DIR/implement.reply"
  case "$brief" in
  *"The last attempt failed review"*) [ -f "$LOG_DIR/implement.disputed" ] && reply="$LOG_DIR/implement.disputed" ;;
  esac
  cat "$reply" ;;
esac
"###;

const JUDGE_SCRIPT: &str = "#!/bin/sh\n[ \"$1\" = --version ] && { echo codex-cli 0.1; exit 0; }\nbrief=$(cat)\necho judge >> \"$LOG_DIR/judge.log\"\nprintf '%s' \"$brief\" > \"$LOG_DIR/judge.brief.$$\"\nprintf '%s\\n' '{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"```sushi-judge\\n{\\\"finding_valid\\\": false, \\\"why\\\": \\\"the ruling\\\"}\\n```\"}}'\n";

fn review_line(body: Value) -> String {
    json!({"type": "result", "result": format!("```sushi-review\n{body}\n```")}).to_string()
}

fn fail(findings: Value) -> String {
    review_line(json!({"verdict": "FAIL", "findings": findings}))
}

fn pass(findings: Value) -> String {
    review_line(json!({"verdict": "PASS", "findings": findings}))
}

fn no_verdict() -> String {
    json!({"type": "result", "result": "Looks fine to me."}).to_string()
}

fn report(summary: &str, disputes: Value) -> String {
    let body = json!({
        "outcome": "complete", "summary": summary, "decisions": [], "question": "",
        "disputes": disputes,
    });
    json!({
        "type": "result", "total_cost_usd": 0.01,
        "usage": {"input_tokens": 1, "output_tokens": 1},
        "result": format!("```sushi-report\n{body}\n```"),
    })
    .to_string()
}

struct Rig {
    daemon: Daemon,
    dir: tempfile::TempDir,
    repo: tempfile::TempDir,
}

impl Rig {
    fn new(plan: &[String]) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let claude = fake_harness_script(dir.path(), "fake-claude.sh", FAKE_ROUNDS_SCRIPT);
        let codex = fake_harness_script(dir.path(), "fake-judge.sh", JUDGE_SCRIPT);
        std::fs::write(dir.path().join("review.plan"), plan.join("\n") + "\n").unwrap();
        std::fs::write(
            dir.path().join("implement.reply"),
            report("done", json!([])),
        )
        .unwrap();
        let daemon = Daemon::spawn(&[
            ("ORCHD_CLAUDE_BIN", claude.to_str().unwrap()),
            ("ORCHD_CODEX_BIN", codex.to_str().unwrap()),
            ("LOG_DIR", dir.path().to_str().unwrap()),
        ]);
        let mut settings = daemon.request("settings.get", json!({}));
        settings["planReview"] = json!("");
        settings["maxAttempts"] = json!(6);
        settings["answerPolicy"] = json!(false);
        settings["review"] = json!("claude-opus");
        // Sonnet stays the implementer but is too weak to judge: Codex judges, at
        // any tier.
        for route in settings["routes"].as_array_mut().unwrap() {
            if route["id"] == "claude-sonnet" {
                route["strength"] = json!(1);
            }
            if route["id"] == "codex" {
                route["strength"] = json!(3);
            }
        }
        daemon.request("settings.set", json!({"settings": settings}));
        Rig {
            daemon,
            dir,
            repo: init_git_repo(),
        }
    }

    fn plan(&self, plan: &[String]) {
        self.file("review.plan", &(plan.join("\n") + "\n"));
    }

    fn file(&self, name: &str, text: &str) {
        std::fs::write(self.dir.path().join(name), text).unwrap();
    }

    fn run(&self, goal: &str) -> (String, Value) {
        let task = self.daemon.request(
            "task.create",
            json!({
                "repo": self.repo.path().to_str().unwrap(),
                "title": "Rounds",
                "goal": goal,
                "verify": ["true"],
                "start": true,
            }),
        );
        let id = task["id"].as_str().unwrap().to_string();
        let settled = poll_task_status(&self.daemon, &id);
        (id, settled)
    }

    fn count(&self, prefix: &str) -> usize {
        std::fs::read_dir(self.dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(prefix))
            .count()
    }

    fn run_dir(&self, task_id: &str, attempt: u32) -> std::path::PathBuf {
        self.daemon
            .data_dir()
            .join("tasks")
            .join(task_id)
            .join(format!("runs/{attempt}"))
    }

    fn finish(self, settled: &Value) {
        let worktree = settled["worktree"].as_str().map(str::to_string);
        self.daemon.shutdown_and_wait();
        if let Some(w) = worktree {
            let _ = std::fs::remove_dir_all(w);
        }
    }
}

fn implements(task: &Value) -> Vec<&Value> {
    task["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["stage"] == "implement")
        .collect()
}

fn decisions(task: &Value) -> Vec<String> {
    task["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d.as_str().map(String::from))
        .collect()
}

fn id_mark(finding: &str) -> String {
    let start = finding.rfind(" [F-").expect("an id mark");
    finding[start + 2..finding.len() - 1].to_string()
}

fn exists(path: &Path) -> bool {
    path.exists()
}

#[test]
fn every_finding_gets_a_stable_id_and_a_status() {
    let rig = Rig::new(&[
        fail(json!(["P1: a.rs:1 - null check missing"])),
        fail(json!([{"finding": "P1: a.rs:9 - null check missing", "repeat": true}])),
        pass(json!(["P2: z.rs:3 - a minor thing"])),
    ]);
    let (_, settled) = rig.run("Write the marker");
    assert_eq!(settled["status"], "done", "{settled}");
    let attempts = implements(&settled);
    assert_eq!(attempts.len(), 3, "{settled}");
    let first = attempts[0]["review"]["findings"][0].as_str().unwrap();
    let second = attempts[1]["review"]["findings"][0].as_str().unwrap();
    let id = id_mark(first);
    assert_eq!(id.len(), 8, "{id}");
    assert!(id.starts_with("F-") && id[2..].chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(id, id_mark(second), "the same finding keeps its id");
    let registry = settled["findings"].as_array().unwrap();
    assert_eq!(registry.len(), 2, "{settled}");
    let a = registry.iter().find(|f| f["id"] == id).unwrap();
    assert_eq!(a["status"], "resolved", "{settled}");
    assert_eq!(a["repeats"], 1, "{settled}");
    let z = registry.iter().find(|f| f["id"] != id).unwrap();
    assert_eq!(z["status"], "open", "{settled}");
    rig.finish(&settled);
}

#[test]
fn review_runs_stop_after_round_two_with_the_last_result() {
    let rig = Rig::new(&[
        fail(json!([
            "P1: a.rs:1 - first problem",
            "P1: b.rs:2 - second problem"
        ])),
        fail(json!([
            {"finding": "P1: a.rs:1 - first problem", "repeat": true},
            {"finding": "P1: b.rs:2 - second problem", "repeat": true}
        ])),
        fail(json!([{"finding": "P1: c.rs:3 - third problem", "repeat": true}])),
        pass(json!([])),
    ]);
    rig.file(
        "implement.disputed",
        &report(
            "done",
            json!([
                {"finding": "a.rs:1 - first problem", "rebuttal": "wrong", "evidence": []},
                {"finding": "b.rs:2 - second problem", "rebuttal": "wrong", "evidence": []},
                {"finding": "c.rs:3 - third problem", "rebuttal": "wrong", "evidence": []}
            ]),
        ),
    );
    let (id, settled) = rig.run("Write the marker");
    assert_eq!(settled["status"], "done", "{settled}");
    let attempts = implements(&settled);
    assert_eq!(attempts.len(), 3, "{settled}");
    // Attempt 1: one run. Attempt 2: a judge drop re-ran the review once and
    // the second run's drop would have asked for a third. Attempt 3: one.
    assert_eq!(rig.count("review.brief."), 4);
    let two = rig.run_dir(&id, 2);
    assert!(exists(&two.join("review")) && exists(&two.join("review-2")));
    assert!(!exists(&two.join("review-3")), "no third run");
    assert_eq!(attempts[1]["review"]["verdict"], "FAIL", "{settled}");
    assert!(attempts[1]["review"]["findings"][0]
        .as_str()
        .unwrap()
        .contains("third problem"));
    assert!(
        decisions(&settled)
            .iter()
            .any(|d| d == "Orchestrator: attempt 2 review stopped after 2 runs"),
        "{settled}"
    );
    rig.finish(&settled);
}

#[test]
fn the_owner_gets_no_retry_after_round_two() {
    let rig = Rig::new(&[no_verdict()]);
    let (id, waiting) = rig.run("Write the marker");
    assert_eq!(waiting["status"], "waiting", "{waiting}");
    assert!(waiting["question"]["text"]
        .as_str()
        .unwrap()
        .contains("no verdict"));
    assert_eq!(
        waiting["question"]["options"],
        json!(["approve"]),
        "{waiting}"
    );
    assert_eq!(rig.count("review.brief."), 2, "the first run and one rerun");
    let dir = rig.run_dir(&id, 1);
    assert!(exists(&dir.join("review")) && exists(&dir.join("review-2")));
    assert!(!exists(&dir.join("review-3")));
    assert!(
        decisions(&waiting)
            .iter()
            .any(|d| d == "Orchestrator: attempt 1 review stopped after 2 runs"),
        "{waiting}"
    );
    rig.daemon
        .request("task.answer", json!({"id": id, "answer": "approve"}));
    let done = poll_until(&rig.daemon, &id, |s| {
        matches!(s, "done" | "failed" | "stopped")
    });
    assert_eq!(done["status"], "done", "{done}");
    rig.finish(&done);
}

/// Attempt 1 reports A, attempt 2 reports B (A is resolved), attempt 3
/// reports A again as `returning`.
fn returning_plan(returning: Value) -> Vec<String> {
    vec![
        fail(json!(["P1: a.rs:1 - null check missing"])),
        fail(json!(["P1: b.rs:2 - index out of range"])),
        fail(json!([returning])),
        pass(json!([])),
    ]
}

#[test]
fn a_returning_finding_without_a_new_scenario_is_dropped() {
    let rig = Rig::new(&returning_plan(
        json!({"finding": "P1: a.rs:5 - null check missing"}),
    ));
    let (_, settled) = rig.run("Write the marker");
    assert_eq!(settled["status"], "done", "{settled}");
    let attempts = implements(&settled);
    assert_eq!(attempts.len(), 3, "{settled}");
    let id = id_mark(attempts[0]["review"]["findings"][0].as_str().unwrap());
    assert!(
        decisions(&settled)
            .iter()
            .any(|d| d.contains(&id) && d.contains("came back") && d.contains("dropped")),
        "{settled}"
    );
    assert_eq!(attempts[2]["review"]["verdict"], "PASS", "{settled}");
    rig.finish(&settled);
}

#[test]
fn a_returning_finding_with_a_new_scenario_is_kept() {
    let rig = Rig::new(&returning_plan(json!({
        "finding": "P1: a.rs:5 - null check missing",
        "scenario": {"run": "test -f NEVER_THERE", "fails": "the file is missing"}
    })));
    let (_, settled) = rig.run("Write the marker");
    assert_eq!(settled["status"], "done", "{settled}");
    let attempts = implements(&settled);
    assert_eq!(attempts.len(), 4, "{settled}");
    let third = &attempts[2]["review"];
    assert_eq!(third["verdict"], "FAIL", "{settled}");
    let finding = third["findings"][0].as_str().unwrap();
    assert!(finding.contains("null check missing"), "{finding}");
    assert!(
        !finding.contains("NEVER_THERE"),
        "the scenario is not part of the finding text: {finding}"
    );
    let id = id_mark(finding);
    assert!(
        decisions(&settled)
            .iter()
            .any(|d| d.contains(&id) && d.contains("new failing scenario") && d.contains("kept")),
        "{settled}"
    );
    rig.finish(&settled);
}

#[test]
fn a_twice_repeated_finding_with_a_failing_scenario_becomes_a_check() {
    let rig = Rig::new(&[]);
    // Fails until an implement brief carries the criterion made from the
    // finding: the fake then creates `fixed`.
    let scenario = json!({
        "run": format!("test -f {}", rig.dir.path().join("fixed").display()),
        "fails": "the fix was never made"
    });
    let first = json!({"finding": "P1: a.rs:1 - null check missing", "scenario": scenario});
    let again =
        json!({"finding": "P1: a.rs:1 - null check missing", "scenario": scenario, "repeat": true});
    rig.plan(&[
        fail(json!([first])),
        fail(json!([again])),
        fail(json!([again])),
        pass(json!([])),
    ]);
    let (_, settled) = rig.run("Write the marker");
    assert_eq!(settled["status"], "done", "{settled}");
    assert_eq!(implements(&settled).len(), 4, "{settled}");
    let id = id_mark(
        implements(&settled)[0]["review"]["findings"][0]
            .as_str()
            .unwrap(),
    );
    let criteria: Vec<&str> = settled["criteria"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c.as_str())
        .collect();
    let added = criteria
        .iter()
        .position(|c| c.starts_with(&format!("Review finding {id} is fixed -- check: test ")))
        .unwrap_or_else(|| panic!("no criterion for {id}: {settled}"));
    let checks = settled["checks"].as_array().unwrap();
    assert!(
        checks
            .iter()
            .any(|c| c["criterion"] == added && c["run"].as_str().unwrap().contains("fixed")),
        "{settled}"
    );
    assert!(
        decisions(&settled)
            .iter()
            .any(|d| d.contains(&id) && d.contains("added a criterion and a check")),
        "{settled}"
    );
    assert_eq!(rig.count("judge.log"), 0, "no judge run");
    rig.finish(&settled);
}

#[test]
fn a_twice_repeated_finding_without_a_scenario_goes_to_the_judge() {
    let again = json!({"finding": "P1: a.rs:1 - null check missing", "repeat": true});
    let rig = Rig::new(&[
        fail(json!(["P1: a.rs:1 - null check missing"])),
        fail(json!([again])),
        fail(json!([again])),
        pass(json!([])),
    ]);
    rig.file(
        "implement.reply",
        &report("the check sits at a.rs:1", json!([])),
    );
    let (_, settled) = rig.run("Write the marker");
    // The judge drops it at attempt 3, the review re-runs once and passes;
    // the owner is never asked.
    assert_eq!(settled["status"], "done", "{settled}");
    assert_eq!(implements(&settled).len(), 3, "{settled}");
    let judge = std::fs::read_to_string(rig.dir.path().join("judge.log")).unwrap();
    assert_eq!(judge.lines().count(), 1);
    let briefs: Vec<String> = std::fs::read_dir(rig.dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("judge.brief."))
        .map(|e| std::fs::read_to_string(e.path()).unwrap())
        .collect();
    assert!(
        briefs[0].contains("the check sits at a.rs:1"),
        "the implementer's report is the rebuttal: {}",
        briefs[0]
    );
    assert!(
        decisions(&settled)
            .iter()
            .any(|d| d.contains("repeated twice with no scenario")),
        "{settled}"
    );
    assert!(settled["question"].is_null(), "{settled}");
    rig.finish(&settled);
}

#[test]
fn the_next_implement_attempt_starts_a_fresh_session() {
    let rig = Rig::new(&[
        fail(json!(["P1: a.rs:1 - null check missing"])),
        pass(json!([])),
    ]);
    let (_, settled) = rig.run("Write the marker");
    assert_eq!(settled["status"], "done", "{settled}");
    assert_eq!(implements(&settled).len(), 2, "{settled}");
    assert_eq!(rig.count("implement.args."), 2);
    for entry in std::fs::read_dir(rig.dir.path()).unwrap().flatten() {
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with("implement.args.")
        {
            continue;
        }
        let argv = std::fs::read_to_string(entry.path()).unwrap();
        assert!(
            argv.lines().all(|l| l != "--resume" && l != "resume"),
            "{argv}"
        );
    }
    rig.finish(&settled);
}

#[test]
fn the_review_brief_asks_only_for_correctness_and_requirement_gaps() {
    let rig = Rig::new(&[pass(json!([]))]);
    let (_, settled) = rig.run("Write the marker");
    assert_eq!(settled["status"], "done", "{settled}");
    let brief = std::fs::read_to_string(rig.dir.path().join("review.brief.1")).unwrap();
    assert!(
        brief.contains("Report only correctness bugs and requirement gaps"),
        "{brief}"
    );
    let format = &brief[brief.find("## Report format").unwrap()..];
    assert!(format.contains("\"kind\""), "{format}");
    assert!(format.contains("\"scenario\""), "{format}");
    rig.finish(&settled);
}

#[test]
fn style_and_nit_findings_become_follow_ups() {
    let rig = Rig::new(&[fail(json!([
        {"finding": "P1: s.rs:1 - rename the variable", "kind": "style"},
        {"finding": "P2: n.rs:2 - trailing space", "kind": "nit"}
    ]))]);
    let (_, settled) = rig.run("Write the marker");
    assert_eq!(settled["status"], "done", "{settled}");
    let attempts = implements(&settled);
    assert_eq!(
        attempts.len(),
        1,
        "never blocks, even on attempt 1: {settled}"
    );
    assert!(attempts[0]["review"]["findings"]
        .as_array()
        .map(|f| f.is_empty())
        .unwrap_or(true));
    let follow_ups = settled["briefCheck"]["followUps"].to_string();
    assert!(follow_ups.contains("rename the variable"), "{follow_ups}");
    assert!(follow_ups.contains("trailing space"), "{follow_ups}");
    rig.finish(&settled);
}
