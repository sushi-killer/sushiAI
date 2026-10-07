//! Black-box integration tests (variant.groundedChecks): spawn the real `orchd`
//! binary with a fake harness and drive it over its NDJSON unix socket.

mod common;

use common::*;
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;

/// One fake plays every role. `$LOG_DIR` keeps each run's brief
/// (`brief.<pid>`), the reviewer's brief (`review-brief.md`) and a marker per
/// implement run (`implement.<n>`); the implement body sees `$n`, the number
/// of implement runs before this one. A plan brief gets the event line in
/// `$LOG_DIR/plan.json`.
fn fake(implement: &str) -> String {
    format!(
        r#"#!/bin/sh
cat > "$LOG_DIR/brief.$$"
if head -c 9 "$LOG_DIR/brief.$$" | grep -q '## Review'; then
  cp "$LOG_DIR/brief.$$" "$LOG_DIR/review-brief.md"
  printf '%s\n' '{{"type":"result","result":"```sushi-review\n{{\"verdict\":\"PASS\",\"findings\":[]}}\n```"}}'
  exit 0
fi
if grep -q 'sushi-plan' "$LOG_DIR/brief.$$"; then
  cat "$LOG_DIR/plan.json"
  exit 0
fi
emit() {{
  printf '%s\n' '{{"type":"system","subtype":"init","session_id":"sess-fake"}}'
  printf '%s\n' "{{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{{\"input_tokens\":1,\"output_tokens\":1}},\"result\":$1}}"
}}
REPORT='"```sushi-report\n{{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}}\n```"'
n=$(ls "$LOG_DIR"/implement.* 2>/dev/null | wc -l | tr -d ' ')
touch "$LOG_DIR/implement.$n"
{implement}
"#
    )
}

struct Setup {
    daemon: Daemon,
    log: tempfile::TempDir,
    _scripts: tempfile::TempDir,
}

fn setup(script: &str, review: bool, env: &[(&str, &str)]) -> Setup {
    let scripts = tempfile::tempdir().unwrap();
    let script = if script.starts_with("#!") {
        script.to_string()
    } else {
        fake(script)
    };
    let path = fake_harness_script(scripts.path(), "fake-claude.sh", &script);
    let log = tempfile::tempdir().unwrap();
    let mut vars = vec![
        ("ORCHD_CLAUDE_BIN", path.to_str().unwrap()),
        ("LOG_DIR", log.path().to_str().unwrap()),
    ];
    vars.extend_from_slice(env);
    let daemon = Daemon::spawn(&vars);
    // The same fake reviews (`auto` would pick a route on the other harness).
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!(if review { "claude-opus" } else { "" });
    daemon.request("settings.set", json!({"settings": settings}));
    Setup {
        daemon,
        log,
        _scripts: scripts,
    }
}

fn repo_with(files: &[(&str, &str)]) -> tempfile::TempDir {
    let repo = init_git_repo();
    for (name, body) in files {
        std::fs::write(repo.path().join(name), body).unwrap();
    }
    if !files.is_empty() {
        git_out(repo.path(), &["add", "."]);
        git_out(repo.path(), &["commit", "-q", "-m", "fixtures"]);
    }
    repo
}

fn create(daemon: &Daemon, repo: &Path, extra: Value) -> Value {
    let mut params = json!({
        "repo": repo.to_str().unwrap(),
        "title": "Grounded",
        "goal": "Make the checks pass",
        "variant": {"groundedChecks": true},
    });
    for (k, v) in extra.as_object().unwrap() {
        params[k] = v.clone();
    }
    daemon.request("task.create", params)
}

fn implement_attempts(task: &Value) -> Vec<Value> {
    task["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["stage"] == "implement")
        .cloned()
        .collect()
}

fn cleanup(s: Setup, tasks: &[&Value]) {
    let worktrees: Vec<String> = tasks
        .iter()
        .map(|t| t["worktree"].as_str().unwrap().to_string())
        .collect();
    s.daemon.shutdown_and_wait();
    for wt in worktrees {
        let _ = std::fs::remove_dir_all(wt);
    }
}

#[test]
fn a_check_that_fails_on_base_gates_the_attempt_and_one_that_passes_is_not_grounded() {
    // Attempt 0 changes a file but does not meet the criterion; attempt 1 does.
    let s = setup(
        r#"if [ "$n" = 0 ]; then
  git status --porcelain > "$LOG_DIR/status.first"
  cat README.md > "$LOG_DIR/readme.first"
  echo x > CHANGED
else
  echo ok > impl.txt
fi
emit "$REPORT""#,
        false,
        &[],
    );
    let repo = repo_with(&[]);
    let task = create(
        &s.daemon,
        repo.path(),
        json!({
            "criteria": ["impl.txt says ok", "already true", "needs a tool"],
            "verify": ["true"],
            "checks": [
                // Creates a file and edits a tracked one, then fails on base.
                {"criterion": 0, "run": "echo junk > side.txt; echo more >> README.md; grep -q ok impl.txt"},
                {"criterion": 1, "run": "true"},
                {"criterion": 2, "run": "no-such-tool-anywhere --version"},
                {"criterion": 9, "run": "dropped: out of range"},
                {"criterion": 0, "run": "   "},
            ],
        }),
    );
    assert_eq!(task["checks"].as_array().unwrap().len(), 3, "{task}");
    let id = task["id"].as_str().unwrap().to_string();
    let done = settle(&s.daemon, &id);
    assert_eq!(done["status"], "done", "task JSON: {done}");

    let baselines: Vec<&Value> = done["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| &c["baseline"])
        .collect();
    assert_eq!(baselines, vec!["fail", "pass", "env"], "{done}");
    let decisions: Vec<&str> = done["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d.as_str().unwrap())
        .collect();
    assert!(decisions.contains(
        &"Orchestrator: check for criterion 1 already passes on base, not grounded: true"
    ));
    assert!(decisions.contains(
        &"Orchestrator: check for criterion 2 could not run on base (exit 127), left out: no-such-tool-anywhere --version"
    ));

    // The gate: attempt 1 fails with kind verify until the check passes.
    let attempts = implement_attempts(&done);
    assert_eq!(attempts.len(), 2, "{done}");
    assert_eq!(attempts[0]["failure"]["kind"], "verify");
    assert!(attempts[0]["failure"]["detail"]
        .as_str()
        .unwrap()
        .starts_with(
            "echo junk > side.txt; echo more >> README.md; grep -q ok impl.txt exited 2."
        ));
    assert_eq!(attempts[1]["status"], "passed");

    // The baseline left the worktree as it found it.
    let status = std::fs::read_to_string(s.log.path().join("status.first")).unwrap();
    assert_eq!(status, "", "the worktree was dirty when attempt 1 began");
    let readme = std::fs::read_to_string(s.log.path().join("readme.first")).unwrap();
    assert_eq!(readme, "hello\n");

    // The gated check is in the brief, the passing and unrunnable ones are not.
    let brief = run_file(&s.daemon, &id, "brief.md");
    assert!(brief.contains("- [0] impl.txt says ok"), "{brief}");
    assert!(brief.contains("## Checks"));
    assert!(brief.contains("- criterion [0]: `echo junk > side.txt"));
    let section = brief.split("## Checks").nth(1).unwrap();
    let section = section.split("\n## ").next().unwrap();
    assert!(!section.contains("no-such-tool-anywhere") && !section.contains("`true`"));
    assert!(brief.contains("```sushi-impossible"));
    cleanup(s, &[&done]);
}

#[test]
fn a_failing_held_out_check_fails_the_attempt_and_its_command_never_leaks() {
    // The held-out command prints itself (held.sh holds exactly that text).
    let held = "cat held.sh; test -f hidden.txt";
    let s = setup(
        r#"echo ok > impl.txt
if [ "$n" != 0 ]; then echo ok > hidden.txt; fi
emit "$REPORT""#,
        true,
        &[],
    );
    let repo = repo_with(&[("held.sh", held)]);
    let task = create(
        &s.daemon,
        repo.path(),
        json!({
            "criteria": ["impl.txt says ok", "hidden.txt exists"],
            "verify": ["true"],
            "checks": [
                {"criterion": 0, "run": "grep -q ok impl.txt"},
                {"criterion": 1, "run": "true"},
            ],
            "heldOut": {"criterion": 1, "run": held},
        }),
    );
    let id = task["id"].as_str().unwrap().to_string();
    let done = settle(&s.daemon, &id);
    assert_eq!(done["status"], "done", "task JSON: {done}");
    assert_eq!(done["heldOut"]["baseline"], "fail", "{done}");

    let attempts = implement_attempts(&done);
    assert_eq!(attempts.len(), 2, "{done}");
    assert_eq!(attempts[0]["failure"]["kind"], "heldout");
    let detail = attempts[0]["failure"]["detail"].as_str().unwrap();
    assert!(
        detail.starts_with("Held-out check for criterion 1 failed: hidden.txt exists. exited 1.\n"),
        "{detail}"
    );
    assert!(detail.contains("<held-out check>"), "{detail}");
    let outcomes = attempts[0]["verify"].to_string();
    assert!(
        outcomes.contains("held-out check (criterion 1)"),
        "{outcomes}"
    );

    // The second attempt is told which criterion, never how it is checked.
    let second = std::fs::read_to_string(
        s.daemon
            .data_dir()
            .join("tasks")
            .join(&id)
            .join("runs/2/brief.md"),
    )
    .unwrap();
    assert!(second.contains("Held-out check for criterion 1 failed: hidden.txt exists."));
    assert!(!second.contains(held));

    // Nothing the implementer or reviewer could read holds the command.
    fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n == "plan") {
                    continue;
                }
                walk(&path, out);
            } else {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    walk(
        &s.daemon.data_dir().join("tasks").join(&id).join("runs"),
        &mut files,
    );
    assert!(files.iter().any(|f| f.ends_with("runs/2/brief.md")));
    for f in &files {
        let text = String::from_utf8_lossy(&std::fs::read(f).unwrap()).to_string();
        assert!(
            !text.contains(held),
            "{} leaks the held-out command",
            f.display()
        );
    }
    let decisions = done["decisions"].to_string();
    assert!(!decisions.contains(held), "{decisions}");
    assert!(!attempts
        .iter()
        .any(|a| a["failure"].to_string().contains(held)));
    assert!(!attempts
        .iter()
        .any(|a| a["verify"].to_string().contains(held)));

    // The review names the result by criterion only, and the check that
    // passed on the base as not grounded.
    let review = std::fs::read_to_string(s.log.path().join("review-brief.md")).unwrap();
    assert!(
        review.contains("Held-out check for criterion 1 (hidden.txt exists): passed."),
        "{review}"
    );
    assert!(
        review.contains("`true` already passed on the base, so it is not grounded"),
        "{review}"
    );
    assert!(!review.contains(held));
    for f in std::fs::read_dir(s.log.path()).unwrap().flatten() {
        let name = f.file_name().to_string_lossy().to_string();
        if name.starts_with("brief.") {
            let text = std::fs::read_to_string(f.path()).unwrap();
            assert!(!text.contains(held), "{name} leaks the held-out command");
        }
    }
    cleanup(s, &[&done]);
}

#[test]
fn a_sushi_impossible_block_asks_the_owner_and_dropping_the_criterion_finishes_the_task() {
    let s = setup(
        r#"if [ "$n" = 0 ]; then
  emit '"```sushi-impossible\n{\"criterion\":1,\"evidence\":\"premise is wrong\"}\n```\n```sushi-report\n{\"outcome\":\"blocked\",\"summary\":\"cannot\",\"decisions\":[],\"question\":\"\"}\n```"'
else
  echo ok > impl2.txt
  emit "$REPORT"
fi"#,
        false,
        &[],
    );
    let repo = repo_with(&[]);
    let task = create(
        &s.daemon,
        repo.path(),
        json!({
            "criteria": ["zero", "one is impossible", "two"],
            "verify": ["true"],
            "checks": [
                {"criterion": 1, "run": "grep -q ok impl1.txt"},
                {"criterion": 2, "run": "grep -q ok impl2.txt"},
            ],
        }),
    );
    let id = task["id"].as_str().unwrap().to_string();
    let waiting = settle(&s.daemon, &id);
    assert_eq!(waiting["status"], "waiting", "task JSON: {waiting}");
    assert_eq!(
        waiting["question"]["text"],
        "Criterion 1 cannot be met as written: one is impossible. Evidence: premise is wrong"
    );
    assert_eq!(
        waiting["question"]["options"],
        json!(["drop criterion", "retry", "stop"])
    );
    // No retry started behind the owner's back.
    assert_eq!(implement_attempts(&waiting).len(), 1);
    assert_eq!(implement_attempts(&waiting)[0]["status"], "blocked");

    s.daemon
        .request("task.answer", json!({"id": id, "answer": "drop criterion"}));
    let done = poll_until(&s.daemon, &id, Duration::from_secs(30), |st| {
        st == "done" || st == "failed" || st == "stopped"
    });
    assert_eq!(done["status"], "done", "task JSON: {done}");
    assert_eq!(done["criteria"], json!(["zero", "two"]));
    assert_eq!(done["checks"].as_array().unwrap().len(), 1, "{done}");
    assert_eq!(done["checks"][0]["criterion"], 1);
    assert_eq!(done["checks"][0]["run"], "grep -q ok impl2.txt");
    let decisions = done["decisions"].to_string();
    assert!(decisions.contains("Owner: drop criterion"), "{decisions}");
    assert!(
        decisions.contains(
            "Orchestrator: dropped criterion 1 (one is impossible) as the owner answered"
        ),
        "{decisions}"
    );
    assert_eq!(implement_attempts(&done).len(), 2);
    cleanup(s, &[&done]);
}

#[test]
fn an_invalid_sushi_impossible_block_is_ignored() {
    // Criterion 7 does not exist: the attempt goes on as an ordinary one.
    let s = setup(
        r#"echo x > CHANGED
emit '"```sushi-impossible\n{\"criterion\":7,\"evidence\":\"nope\"}\n```\n```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"'"#,
        false,
        &[],
    );
    let repo = repo_with(&[]);
    let task = create(
        &s.daemon,
        repo.path(),
        json!({"criteria": ["zero"], "verify": ["true"]}),
    );
    let id = task["id"].as_str().unwrap().to_string();
    let done = settle(&s.daemon, &id);
    assert_eq!(done["status"], "done", "task JSON: {done}");
    cleanup(s, &[&done]);
}

#[test]
fn the_planner_stores_validated_checks_when_the_flag_is_on() {
    let s = setup(
        r#"echo ok > x.txt; echo ok > y.txt
emit "$REPORT""#,
        false,
        &[],
    );
    let plan = r#"```sushi-plan
{"title":"Planned","goal":"make x and y","tier":"standard","criteria":["x exists","y exists"],"verify":["true"],
 "checks":[{"criterion":0,"run":"test -f x.txt"},{"criterion":5,"run":"nope"},{"criterion":1,"run":"  "}],
 "heldOut":{"criterion":1,"run":"test -f y.txt"}}
```"#;
    let event = json!({"type": "result", "result": plan}).to_string();
    std::fs::write(s.log.path().join("plan.json"), format!("{event}\n")).unwrap();
    let repo = repo_with(&[]);
    let task = s.daemon.request(
        "task.create",
        json!({
            "repo": repo.path().to_str().unwrap(),
            "request": "make x and y",
            "variant": {"groundedChecks": true},
        }),
    );
    let id = task["id"].as_str().unwrap().to_string();
    let done = settle(&s.daemon, &id);
    assert_eq!(done["status"], "done", "task JSON: {done}");
    assert_eq!(
        done["checks"],
        json!([{"criterion": 0, "run": "test -f x.txt", "baseline": "fail"}])
    );
    assert_eq!(
        done["heldOut"],
        json!({"criterion": 1, "run": "test -f y.txt", "baseline": "fail"})
    );
    let plan_brief = std::fs::read_to_string(
        s.daemon
            .data_dir()
            .join("tasks")
            .join(&id)
            .join("runs/1/plan/brief.md"),
    )
    .unwrap();
    assert!(
        plan_brief.contains("\"heldOut\":{\"criterion\":0"),
        "{plan_brief}"
    );
    // Checks are never written into the repository.
    assert!(!repo.path().join("checks.json").exists());
    cleanup(s, &[&done]);
}

#[test]
fn with_the_flag_off_planner_checks_are_ignored_and_the_brief_is_unchanged() {
    let s = setup(
        r#"echo ok > x.txt
emit "$REPORT""#,
        false,
        &[],
    );
    let plan = r#"```sushi-plan
{"title":"Planned","goal":"make x","tier":"standard","criteria":["x exists"],"verify":["true"],
 "checks":[{"criterion":0,"run":"test -f x.txt"}],"heldOut":{"criterion":0,"run":"true"}}
```"#;
    let event = json!({"type": "result", "result": plan}).to_string();
    std::fs::write(s.log.path().join("plan.json"), format!("{event}\n")).unwrap();
    let repo = repo_with(&[]);
    let task = s.daemon.request(
        "task.create",
        json!({"repo": repo.path().to_str().unwrap(), "request": "make x"}),
    );
    let id = task["id"].as_str().unwrap().to_string();
    let done = settle(&s.daemon, &id);
    assert_eq!(done["status"], "done", "task JSON: {done}");
    assert!(
        done.get("checks").is_none() && done.get("heldOut").is_none(),
        "{done}"
    );
    let brief = run_file(&s.daemon, &id, "brief.md");
    assert!(
        brief.contains("- x exists\n")
            && !brief.contains("## Checks")
            && !brief.contains("sushi-impossible")
    );
    cleanup(s, &[&done]);
}

/// The parent's plan is `$PLAN_KIND`: its checks log a line only once
/// `b.txt` exists, so a line in the log means the check ran after the
/// children landed.
const PARENT_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
out() {
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  printf '%s\n' "{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":$1}"
}
report='"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"'
held=''
if [ -n "$WITH_HELD" ]; then
  held=',\"heldOut\":{\"criterion\":0,\"run\":\"test -f b.txt && echo x >> $LOG_DIR/held.log; test -f c.txt\"}'
fi
case "$input" in
  *sushi-plan*)
    case "$input" in
      *PART_A*) out '"```sushi-plan\n{\"title\":\"Part A\",\"goal\":\"PART_A: create a.txt\",\"verify\":[\"test -f a.txt\"]}\n```"' ;;
      *PART_B*) out '"```sushi-plan\n{\"title\":\"Part B\",\"goal\":\"PART_B: create b.txt next to a.txt\",\"verify\":[\"test -f a.txt && test -f b.txt\"]}\n```"' ;;
      *) out '"```sushi-plan\n{\"title\":\"Both parts\",\"goal\":\"Create a.txt, then b.txt\",\"criteria\":[\"both files exist\"],\"verify\":[\"test -f a.txt && test -f b.txt\"],\"checks\":[{\"criterion\":0,\"run\":\"test -f b.txt && echo x >> $LOG_DIR/checks.log\"}]'"$held"',\"subtasks\":[{\"key\":\"a\",\"title\":\"Part A\",\"request\":\"PART_A create a.txt\"},{\"key\":\"b\",\"title\":\"Part B\",\"request\":\"PART_B create b.txt\",\"dependsOn\":[\"a\"]}]}\n```"' ;;
    esac
    ;;
  *PART_A*) echo a > a.txt; out "$report" ;;
  *PART_B*) test -f a.txt && echo b > b.txt; out "$report" ;;
esac
"#;

fn parent_run(with_held: bool) -> (Setup, Value, tempfile::TempDir) {
    let s = setup(
        PARENT_SCRIPT,
        false,
        if with_held {
            &[("WITH_HELD", "1")]
        } else {
            &[]
        },
    );
    // The parent's verify fails on the empty base by design; the policy
    // answers "keep this check" so the children start.
    let mut settings = s.daemon.request("settings.get", json!({}));
    settings["answerPolicy"] = json!(true);
    s.daemon
        .request("settings.set", json!({"settings": settings}));
    let repo = repo_with(&[]);
    let parent = s.daemon.request(
        "task.create",
        json!({
            "repo": repo.path().to_str().unwrap(),
            "request": "build both parts",
            "variant": {"groundedChecks": true},
        }),
    );
    let id = parent["id"].as_str().unwrap().to_string();
    let parent = settle(&s.daemon, &id);
    (s, parent, repo)
}

fn log_lines(s: &Setup, name: &str) -> usize {
    std::fs::read_to_string(s.log.path().join(name))
        .map(|t| t.lines().count())
        .unwrap_or(0)
}

#[test]
fn a_parents_checks_are_baselined_before_the_children_and_run_once_after_they_land() {
    let (s, parent, _repo) = parent_run(false);
    assert_eq!(parent["status"], "done", "task JSON: {parent}");
    // Baselined on the empty base, so it failed there and gates the parent.
    assert_eq!(parent["checks"][0]["baseline"], "fail", "{parent}");
    // Once, after both children landed: the base run logs nothing (no b.txt).
    assert_eq!(log_lines(&s, "checks.log"), 1);
    cleanup(s, &[&parent]);
}

#[test]
fn a_failing_parent_held_out_check_fails_the_parent_without_naming_the_command() {
    let (s, parent, _repo) = parent_run(true);
    // A failing parent check goes to agent attempts, then waits.
    assert_eq!(parent["status"], "waiting", "task JSON: {parent}");
    assert_eq!(parent["heldOut"]["baseline"], "fail", "{parent}");
    assert!(
        !parent["question"].to_string().contains("c.txt"),
        "{parent}"
    );
    let attempts = parent["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["stage"] == "implement")
        .count();
    assert!(attempts >= 1, "{parent}");
    // One parent check, then one run per agent attempt.
    assert_eq!(log_lines(&s, "checks.log"), 1 + attempts);
    assert_eq!(log_lines(&s, "held.log"), 1 + attempts);
    let decisions = parent["decisions"].to_string();
    assert!(
        decisions.contains("the held-out check for criterion 0 failed (heldout)"),
        "{decisions}"
    );
    assert!(!decisions.contains("c.txt"), "{decisions}");
    cleanup(s, &[&parent]);
}

#[test]
fn task_create_validates_checks_and_the_mcp_schema_names_the_flag_once() {
    let s = setup("emit \"$REPORT\"", false, &[]);
    let repo = repo_with(&[]);
    let task = create(
        &s.daemon,
        repo.path(),
        json!({
            "criteria": ["a", "b"],
            "verify": ["true"],
            "start": false,
            "checks": [
                {"criterion": 1, "run": "true"},
                {"criterion": 2, "run": "out of range"},
                {"criterion": 0, "run": ""},
            ],
            "heldOut": {"criterion": 7, "run": "out of range"},
        }),
    );
    assert_eq!(task["checks"], json!([{"criterion": 1, "run": "true"}]));
    assert!(task.get("heldOut").is_none(), "{task}");
    assert_eq!(task["variant"]["groundedChecks"], true);
    let partial = s.daemon.request(
        "task.create",
        json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "t",
            "goal": "g",
            "start": false,
            "variant": {"groundedChecks": false},
        }),
    );
    assert!(partial["variant"].get("groundedChecks").is_none());

    let source =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/mcp.rs")).unwrap();
    assert_eq!(
        source.matches("\"variant\": {\"type\": \"object\"").count(),
        1
    );
    assert!(source.contains("groundedChecks (bool;"));
    cleanup(s, &[&task, &partial]);
}

#[test]
fn a_supplied_baseline_is_ignored_and_the_check_still_runs_on_base() {
    let s = setup(r#"echo ok > impl.txt; emit "$REPORT""#, false, &[]);
    let repo = repo_with(&[]);
    let task = create(
        &s.daemon,
        repo.path(),
        json!({
            "criteria": ["already true"],
            "verify": ["true"],
            "checks": [{"criterion": 0, "run": "true", "baseline": "fail"}],
        }),
    );
    assert!(task["checks"][0].get("baseline").is_none(), "{task}");
    let id = task["id"].as_str().unwrap().to_string();
    let done = settle(&s.daemon, &id);
    assert_eq!(done["status"], "done", "{done}");
    assert_eq!(done["checks"][0]["baseline"], "pass", "{done}");
    assert!(done["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d
            == "Orchestrator: check for criterion 0 already passes on base, not grounded: true"));
    cleanup(s, &[&done]);
}
