//! Black-box integration tests (review rounds): finding ids, the two-run
//! bound on an attempt's review loop, and what happens to a finding that
//! comes back or repeats.

mod common;

use common::harness::fake_harness_script;
use common::polling::poll_task_status;
use common::process::Daemon;
use common::repository::init_git_repo;

use serde_json::{json, Value};

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
'## Route the review'*) exit 0 ;;
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

fn report(summary: &str, declines: Value) -> String {
    let body = json!({
        "outcome": "complete", "summary": summary, "decisions": [], "question": "",
        "declines": declines,
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
        // Sonnet stays the implementer but is too weak to review: Codex is the
        // fallback reviewer, at any tier.
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

    /// Review runs recorded for one attempt (helper folders, in order).
    fn review_runs(&self, task_id: &str, attempt: u32) -> usize {
        sushiai_orch::store::Store::new(self.daemon.data_dir())
            .unwrap()
            .helper_run_dirs(task_id, "review", Some(attempt))
            .len()
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

#[test]
fn every_finding_gets_a_stable_id_and_a_status() {
    let rig = Rig::new(&[
        fail(json!(["P1: a.rs:1 - null check missing"])),
        fail(json!([{"finding": "P1: a.rs:9 - null check missing", "id": "F-1"}])),
        pass(json!(["P2: z.rs:3 - a minor thing"])),
    ]);
    let (_, settled) = rig.run("Write the marker");
    assert_eq!(settled["status"], "done", "{settled}");
    let attempts = implements(&settled);
    assert_eq!(attempts.len(), 3, "{settled}");
    let first = attempts[0]["review"]["findings"][0].as_str().unwrap();
    let second = attempts[1]["review"]["findings"][0].as_str().unwrap();
    let id = id_mark(first);
    assert_eq!(id, "F-1");
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
fn a_decline_is_ruled_on_by_id_and_only_an_upheld_one_resolves() {
    let rig = Rig::new(&[
        fail(json!([
            "P1: a.rs:1 - first problem",
            "P1: b.rs:2 - second problem"
        ])),
        review_line(json!({
            "verdict": "PASS",
            "findings": [],
            "declineRulings": [
                {"id": "F-1", "upheld": true},
                {"id": "F-2", "upheld": false}
            ]
        })),
        review_line(json!({
            "verdict": "PASS",
            "findings": [],
            "declineRulings": [{"id": "F-2", "upheld": true}]
        })),
    ]);
    rig.file(
        "implement.disputed",
        &report(
            "done",
            json!([
                {"id": "F-1", "reason": "wrong", "evidence": ["a.rs:1"]},
                {"id": "F-2", "reason": "already_satisfied", "evidence": ["b.rs:2"]}
            ]),
        ),
    );
    let (_, settled) = rig.run("Write the marker");
    assert_eq!(settled["status"], "done", "{settled}");
    let attempts = implements(&settled);
    assert_eq!(attempts.len(), 3, "{settled}");
    // Attempt 2: F-2's decline is not upheld, so it stays blocking even
    // though the reviewer passed; F-1's upheld decline is gone from the list.
    assert_eq!(attempts[1]["review"]["verdict"], "FAIL", "{settled}");
    assert_eq!(
        attempts[1]["review"]["findings"].as_array().unwrap().len(),
        1
    );
    assert!(attempts[1]["review"]["findings"][0]
        .as_str()
        .unwrap()
        .contains("[F-2]"));
    assert_eq!(rig.count("judge.brief."), 0, "no judge turn");
    let lines = decisions(&settled);
    assert!(
        lines
            .iter()
            .any(|d| d.contains("F-1") && d.contains("upheld by the review; resolved")),
        "{settled}"
    );
    assert!(
        lines
            .iter()
            .any(|d| d.contains("F-2") && d.contains("not upheld; it stays open")),
        "{settled}"
    );
    rig.finish(&settled);
}

#[test]
fn a_review_without_a_verdict_moves_to_another_route_then_commits_unreviewed() {
    let rig = Rig::new(&[no_verdict()]);
    let (id, done) = rig.run("Write the marker");
    // Nobody is asked: the third run (another harness) gave no verdict either.
    assert_eq!(done["status"], "done", "{done}");
    assert!(done["question"].is_null(), "{done}");
    assert_eq!(rig.count("review.brief."), 2, "the first run and one rerun");
    assert_eq!(rig.count("judge.brief."), 1, "one run on the other harness");
    assert_eq!(rig.review_runs(&id, 1), 3, "no fourth run");
    let lines = decisions(&done);
    assert!(
        lines
            .iter()
            .any(|d| d.contains("review gave no verdict") && d.contains("reviewing on codex")),
        "{done}"
    );
    assert!(
        lines
            .iter()
            .any(|d| d.contains("attempt 1 committed unreviewed")),
        "{done}"
    );
    rig.finish(&done);
}

/// Attempt 1 reports A (F-1), attempt 2 reports B (A is resolved), attempt 3
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
fn a_returning_finding_without_a_scenario_is_kept_and_blocks_not_dropped_by_its_id() {
    let rig = Rig::new(&returning_plan(
        json!({"finding": "P1: a.rs:5 - null check missing", "id": "F-1"}),
    ));
    let (_, settled) = rig.run("Write the marker");
    assert_eq!(settled["status"], "done", "{settled}");
    let attempts = implements(&settled);
    // No judge turn: the finding stays blocking, so attempt 3 fails its
    // review and attempt 4 passes it.
    assert_eq!(attempts.len(), 4, "{settled}");
    assert_eq!(rig.count("judge.brief."), 0, "no judge turn");
    let id = id_mark(attempts[0]["review"]["findings"][0].as_str().unwrap());
    assert!(
        decisions(&settled)
            .iter()
            .any(|d| d.contains(&id) && d.contains("came back") && d.contains("kept")),
        "{settled}"
    );
    assert_eq!(attempts[2]["review"]["verdict"], "FAIL", "{settled}");
    assert_eq!(attempts[3]["review"]["verdict"], "PASS", "{settled}");
    rig.finish(&settled);
}

#[test]
fn a_returning_finding_with_a_failing_scenario_becomes_a_check_at_once() {
    let rig = Rig::new(&[]);
    let scenario = json!({
        "run": format!("test -f {}", rig.dir.path().join("fixed").display()),
        "fails": "the fix was never made"
    });
    let mut plan = returning_plan(json!({
        "finding": "P1: a.rs:5 - null check missing",
        "id": "F-1",
        "scenario": scenario
    }));
    plan[2] = fail(json!([{
        "finding": "P1: a.rs:5 - null check missing",
        "id": "F-1",
        "scenario": scenario
    }]));
    rig.plan(&plan);
    let (_, settled) = rig.run("Write the marker");
    assert_eq!(settled["status"], "done", "{settled}");
    let attempts = implements(&settled);
    assert_eq!(attempts.len(), 4, "{settled}");
    let third = &attempts[2]["review"];
    assert_eq!(third["verdict"], "FAIL", "{settled}");
    let finding = third["findings"][0].as_str().unwrap();
    assert!(finding.contains("null check missing"), "{finding}");
    assert!(
        !finding.contains("fixed"),
        "the scenario is not part of the finding text: {finding}"
    );
    let id = id_mark(finding);
    let lines = decisions(&settled);
    assert!(
        lines
            .iter()
            .any(|d| d.contains(&id) && d.contains("came back") && d.contains("kept")),
        "{settled}"
    );
    assert!(
        lines
            .iter()
            .any(|d| d.contains(&id) && d.contains("added a criterion and a check")),
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
        json!({"finding": "P1: a.rs:1 - null check missing", "id": "F-1", "scenario": scenario});
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
fn a_twice_repeated_finding_without_a_scenario_stays_open_until_a_review_omits_it() {
    let again = json!({"finding": "P1: a.rs:1 - null check missing", "id": "F-1"});
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
    // No judge rules on it: the third review repeats it with no scenario, so
    // it stays open, and the fourth review no longer names it.
    assert_eq!(settled["status"], "done", "{settled}");
    assert_eq!(implements(&settled).len(), 4, "{settled}");
    assert_eq!(rig.count("judge.brief."), 0, "no judge turn");
    assert!(
        decisions(&settled)
            .iter()
            .any(|d| d.contains("repeated twice with no scenario; it stays open")),
        "{settled}"
    );
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
