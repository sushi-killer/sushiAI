//! Black-box integration tests (retry review): after a failed review the next
//! review sees only the open findings and the task's own changes since.

mod common;

use common::*;
use serde_json::{json, Value};

const ALPHA: &str = "Alpha is done";
const BETA: &str = "Beta is done";

/// One fake plays implementer, advisor and reviewer. Attempt 1 writes a.txt and
/// only1.txt; attempt 2 rewrites a.txt, and writes other.txt when $ALSO_OTHER
/// is 1. The attempt-1 review fails on `a.txt:1` and rules Alpha met, Beta
/// unmet. A narrow review (its brief has "## Settled criteria") passes with
/// Beta met; the later full review answers $FULL_VERDICT.
const FAKE_RETRY_REVIEW_SCRIPT: &str = r###"#!/bin/sh
brief=$(cat)
case "$brief" in
*"An implement attempt at this task failed"*)
  printf '%s\n' '{"type":"result","total_cost_usd":0.03,"usage":{"input_tokens":1,"output_tokens":1},"result":"Fix a.txt."}' ;;
"## Review"*)
  echo review >> "$LOG_DIR/reviews.log"
  case "$brief" in
  *"## Settled criteria"*)
    json='{"type":"result","result":"```sushi-review\n{\"verdict\":\"PASS\",\"findings\":[],\"criteria\":[{\"criterion\":\"Beta is done\",\"met\":true,\"evidence\":\"now fine\"}]}\n```"}' ;;
  *"## Previous review"*)
    if [ "$FULL_VERDICT" = FAIL ]; then
      json='{"type":"result","result":"```sushi-review\n{\"verdict\":\"FAIL\",\"findings\":[\"P1: other.txt:1 - broken\"]}\n```"}'
    else
      json='{"type":"result","result":"```sushi-review\n{\"verdict\":\"PASS\",\"findings\":[]}\n```"}'
    fi ;;
  *)
    json='{"type":"result","result":"```sushi-review\n{\"verdict\":\"FAIL\",\"findings\":[\"P1: a.txt:1 - fix it\"],\"criteria\":[{\"criterion\":\"Alpha is done\",\"met\":true,\"evidence\":\"seen\"},{\"criterion\":\"Beta is done\",\"met\":false,\"evidence\":\"missing\"}]}\n```"}' ;;
  esac
  printf '%s\n' "$json" ;;
*)
  echo impl >> "$LOG_DIR/impl.log"
  if [ "$(wc -l < "$LOG_DIR/impl.log")" -le 1 ]; then
    echo 1 > a.txt
    echo 1 > only1.txt
  else
    echo 2 > a.txt
    if [ "$ALSO_OTHER" = 1 ]; then echo 2 > other.txt; fi
  fi
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}' ;;
esac
"###;

struct Run {
    daemon: Daemon,
    task: Value,
    task_id: String,
    log_dir: tempfile::TempDir,
}

impl Run {
    fn brief(&self, attempt: u32, review_dir: &str) -> Option<String> {
        std::fs::read_to_string(
            self.daemon
                .data_dir()
                .join("tasks")
                .join(&self.task_id)
                .join(format!("runs/{attempt}/{review_dir}/brief.md")),
        )
        .ok()
    }

    fn reviews_run(&self) -> usize {
        std::fs::read_to_string(self.log_dir.path().join("reviews.log"))
            .map(|t| t.lines().count())
            .unwrap_or(0)
    }

    fn finish(self) {
        let worktree = self.task["worktree"].as_str().unwrap().to_string();
        self.daemon.shutdown_and_wait();
        remove_dir_retrying(worktree);
    }
}

fn run(also_other: bool, full_verdict: &str) -> (Run, Value) {
    let log_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        log_dir.path(),
        "fake-retry-review.sh",
        FAKE_RETRY_REVIEW_SCRIPT,
    );
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("LOG_DIR", log_dir.path().to_str().unwrap()),
        ("ALSO_OTHER", if also_other { "1" } else { "0" }),
        ("FULL_VERDICT", full_verdict),
    ]);
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!("claude-opus");
    settings["maxAttempts"] = json!(2);
    daemon.request("settings.set", json!({"settings": settings}));
    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Retry review",
            "goal": "Write the files",
            "criteria": [ALPHA, BETA],
            "verify": ["true"],
            "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    let settled = poll_task_status(&daemon, &task_id);
    (
        Run {
            daemon,
            task,
            task_id,
            log_dir,
        },
        settled,
    )
}

fn implements(task: &Value) -> Vec<&Value> {
    task["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["stage"] == "implement")
        .collect()
}

#[test]
fn a_retry_review_sees_only_the_open_findings_and_the_tasks_own_changes() {
    let (run, settled) = run(false, "PASS");
    assert_eq!(settled["status"], "done", "{settled}");
    let attempts = implements(&settled);
    assert_eq!(attempts.len(), 2, "{settled}");

    // Each reviewed attempt records what its review saw.
    assert_eq!(attempts[0]["reviewScope"], "full", "{settled}");
    assert_eq!(attempts[1]["reviewScope"], "narrow", "{settled}");
    for a in &attempts {
        assert!(a["reviewTree"].as_str().is_some_and(|t| t.len() >= 40));
        assert!(a["reviewBase"].as_str().is_some_and(|t| t.len() >= 40));
    }
    assert_ne!(attempts[0]["reviewTree"], attempts[1]["reviewTree"]);

    let first = run.brief(1, "review").unwrap();
    assert!(first.contains("## Acceptance criteria"), "{first}");
    assert!(first.contains("## Diff"), "{first}");

    let narrow = run.brief(2, "review").unwrap();
    assert!(narrow.contains("P1: a.txt:1 - fix it"), "{narrow}");
    assert!(narrow.contains("Unmet criterion: Beta is done"), "{narrow}");
    let settled_at = narrow.find("## Settled criteria").expect(&narrow);
    assert!(narrow[settled_at..].contains(ALPHA), "{narrow}");
    let open_at = narrow.find("## Criteria to rule on now").expect(&narrow);
    assert!(narrow[open_at..settled_at].contains(BETA), "{narrow}");
    assert!(!narrow[open_at..settled_at].contains(ALPHA), "{narrow}");
    assert!(!narrow.contains("## Acceptance criteria"), "{narrow}");
    assert!(!narrow.contains("## Diff\n"), "{narrow}");
    let delta = &narrow[narrow
        .find("## Changes since the last review")
        .expect(&narrow)..];
    assert!(delta.contains("a.txt"), "{delta}");
    assert!(!delta.contains("only1.txt"), "{delta}");
    assert!(narrow.contains("## Report format"), "{narrow}");

    // The settled criterion still counts as met.
    let results = attempts[1]["criteriaResults"].as_array().unwrap();
    let alpha = results.iter().find(|r| r["text"] == ALPHA).unwrap();
    assert_eq!(alpha["status"], "met", "{settled}");
    assert!(
        alpha["evidence"]
            .as_str()
            .unwrap()
            .contains("settled in the attempt 1 review"),
        "{alpha}"
    );
    let beta = results.iter().find(|r| r["text"] == BETA).unwrap();
    assert_eq!(beta["status"], "met", "{settled}");

    // The delta stayed inside the file the finding names: no second review.
    assert!(run.brief(2, "review-2").is_none());
    assert_eq!(run.reviews_run(), 2);
    run.finish();
}

#[test]
fn a_narrow_pass_with_a_file_no_finding_names_gets_one_full_review() {
    let (run, settled) = run(true, "PASS");
    assert_eq!(settled["status"], "done", "{settled}");
    let attempts = implements(&settled);
    assert_eq!(attempts.len(), 2, "{settled}");
    let narrow = run.brief(2, "review").unwrap();
    assert!(narrow.contains("## Settled criteria"), "{narrow}");
    let full = run.brief(2, "review-2").expect("a full review ran");
    assert!(full.contains("## Diff"), "{full}");
    assert!(full.contains("## Acceptance criteria"), "{full}");
    assert!(!full.contains("## Settled criteria"), "{full}");
    assert_eq!(attempts[1]["reviewScope"], "full", "{settled}");
    assert!(run.brief(2, "review-3").is_none(), "at most one escalation");
    assert_eq!(run.reviews_run(), 3);
    let decisions = settled["decisions"].to_string();
    assert!(
        decisions.contains("other.txt changed since the last full review"),
        "{decisions}"
    );
    run.finish();
}

#[test]
fn a_full_review_that_fails_after_a_narrow_pass_fails_the_attempt() {
    let (run, settled) = run(true, "FAIL");
    assert_ne!(settled["status"], "done", "{settled}");
    let attempts = implements(&settled);
    assert_eq!(attempts.len(), 2, "{settled}");
    assert_eq!(attempts[1]["failure"]["kind"], "review", "{settled}");
    assert_eq!(attempts[1]["reviewScope"], "full", "{settled}");
    assert_eq!(attempts[1]["review"]["verdict"], "FAIL", "{settled}");
    assert!(run.brief(2, "review-2").is_some());
    run.finish();
}
