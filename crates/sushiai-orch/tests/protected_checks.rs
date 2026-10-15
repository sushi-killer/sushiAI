//! Black-box integration tests: the planner's test files are committed before
//! the first attempt, hashed and protected, and the files the checks name are
//! protected with them. Spawns the real `orchd` binary with a fake harness.

mod common;

use common::attempts::run_file;
use common::harness::fake_harness_script;
use common::polling::settle;
use common::process::Daemon;
use common::repository::{git_out, init_git_repo};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::Path;

const TEST_FILE: &str = "tests/new_test.sh";
const TEST_BODY: &str = "#!/bin/sh\ntest -f impl.txt\n";
const SECRET: &str = "SECRET-HELD-OUT-MARKER";

/// One fake plays every role. `$LOG_DIR` keeps each brief (`brief.<pid>`), a
/// marker per implement run, and `order.log` where the reviewer and the
/// commands of the tests append a line; the implement body sees `$n`, the
/// number of implement runs before this one.
fn fake(implement: &str) -> String {
    format!(
        r#"#!/bin/sh
cat > "$LOG_DIR/brief.$$"
if head -c 19 "$LOG_DIR/brief.$$" | grep -q 'Route the review'; then exit 0; fi
if head -c 9 "$LOG_DIR/brief.$$" | grep -q '## Review'; then
  echo review >> "$LOG_DIR/order.log"
  printf '%s\n' '{{"type":"result","result":"```sushi-review\n{{\"verdict\":\"PASS\",\"findings\":[]}}\n```"}}'
  exit 0
fi
if grep -q 'An implementer says one of the task' "$LOG_DIR/brief.$$"; then
  cat "$LOG_DIR/ruling.json"
  exit 0
fi
if grep -q 'Rewrite one acceptance test' "$LOG_DIR/brief.$$"; then
  cat "$LOG_DIR/rewrite.json"
  exit 0
fi
if head -c 40 "$LOG_DIR/brief.$$" | grep -q 'You review a coding task plan'; then
  printf '%s\n' '{{"type":"result","result":"{{\"findings\": []}}"}}'
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

fn setup(implement: &str, review: bool) -> Setup {
    let scripts = tempfile::tempdir().unwrap();
    let path = fake_harness_script(scripts.path(), "fake-claude.sh", &fake(implement));
    let log = tempfile::tempdir().unwrap();
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", path.to_str().unwrap()),
        ("LOG_DIR", log.path().to_str().unwrap()),
    ]);
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

/// Stores the plan the fake planner answers with.
fn plan_with(s: &Setup, plan: Value) {
    let text = format!("```sushi-plan\n{plan}\n```");
    let event = json!({"type": "result", "result": text}).to_string();
    std::fs::write(s.log.path().join("plan.json"), format!("{event}\n")).unwrap();
}

fn reply_file(s: &Setup, name: &str, tag: &str, body: Value) {
    let text = format!("```{tag}\n{body}\n```");
    let event = json!({"type": "result", "result": text}).to_string();
    std::fs::write(s.log.path().join(name), format!("{event}\n")).unwrap();
}

const CLAIM_REPORT: &str = r#"CLAIM='"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\",\"testClaims\":[{\"path\":\"tests/new_test.sh\",\"reason\":\"the test looks for a file the goal never mentions\"}]}\n```"'"#;

fn plan_json(verify: &str, checks: Value, held_out: Value) -> Value {
    json!({
        "title": "Planned",
        "goal": "make impl.txt",
        "tier": "standard",
        "criteria": ["impl.txt exists"],
        "verify": [verify],
        "checks": checks,
        "heldOut": held_out,
        "tests": [{"path": TEST_FILE, "content": TEST_BODY}],
    })
}

fn run_task(s: &Setup, repo: &Path) -> Value {
    let task = s.daemon.request(
        "task.create",
        json!({"repo": repo.to_str().unwrap(), "request": "make impl.txt"}),
    );
    let id = task["id"].as_str().unwrap().to_string();
    settle(&s.daemon, &id)
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

fn read_log(s: &Setup, name: &str) -> String {
    std::fs::read_to_string(s.log.path().join(name)).unwrap_or_default()
}

fn sha256_hex(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn cleanup(s: Setup, task: &Value) {
    let worktree = task["worktree"].as_str().unwrap().to_string();
    s.daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

const CHECK: &str = "sh tests/new_test.sh";

#[test]
fn planner_tests_are_written_before_attempt_one_baselined_hashed_and_committed_only_at_the_end() {
    let s = setup(
        r#"git status --porcelain > "$LOG_DIR/status.$n"
git ls-files -- tests/new_test.sh > "$LOG_DIR/tracked.$n"
git rev-parse HEAD > "$LOG_DIR/head.$n"
echo ok > impl.txt
emit "$REPORT""#,
        false,
    );
    plan_with(
        &s,
        plan_json("true", json!([{"criterion": 0, "run": CHECK}]), Value::Null),
    );
    let repo = repo_with(&[]);
    let done = run_task(&s, repo.path());
    assert_eq!(done["status"], "done", "task JSON: {done}");

    // The first run saw the test file in the tree, untracked, on the base.
    assert_eq!(read_log(&s, "status.0").trim(), "?? tests/");
    assert_eq!(read_log(&s, "tracked.0").trim(), "");
    assert_eq!(
        read_log(&s, "head.0").trim(),
        done["baseSha"].as_str().unwrap()
    );
    assert!(done["plannerTests"]["commit"].is_null(), "{done}");
    assert_eq!(
        done["plannerTests"]["files"],
        json!([{"path": TEST_FILE, "content": TEST_BODY}])
    );

    // The final task commit holds the tests and the work.
    let branch = done["branch"].as_str().unwrap();
    let in_branch = git_out(repo.path(), &["show", &format!("{branch}:{TEST_FILE}")]);
    assert_eq!(in_branch, TEST_BODY.trim());
    assert_eq!(
        git_out(
            repo.path(),
            &[
                "rev-list",
                "--count",
                &format!("{}..{branch}", done["baseSha"].as_str().unwrap())
            ]
        )
        .trim(),
        "1",
        "one commit, no separate tests commit"
    );

    // The visible check failed with the tests in place, so it gates; the task
    // keeps one sha256 per protected file.
    assert_eq!(done["checks"][0]["baseline"], "fail", "{done}");
    assert_eq!(
        done["plannerTests"]["protected"],
        json!({TEST_FILE: sha256_hex(TEST_BODY)})
    );
    assert_eq!(implement_attempts(&done).len(), 1, "{done}");
    cleanup(s, &done);
}

#[test]
fn an_attempt_that_edits_a_planner_test_is_not_failed_the_file_is_restored_and_the_next_brief_says_so(
) {
    let s = setup(
        r#"if [ "$n" = 0 ]; then
  echo 'exit 0' > tests/new_test.sh
else
  cp "$LOG_DIR/brief.$$" "$LOG_DIR/second-brief.md"
  cp tests/new_test.sh "$LOG_DIR/restored.sh"
  echo ok > impl.txt
fi
emit "$REPORT""#,
        false,
    );
    plan_with(
        &s,
        plan_json("true", json!([{"criterion": 0, "run": CHECK}]), Value::Null),
    );
    let repo = repo_with(&[]);
    let done = run_task(&s, repo.path());
    assert_eq!(done["status"], "done", "task JSON: {done}");
    let attempts = implement_attempts(&done);
    assert_eq!(attempts.len(), 2, "{done}");
    // The first attempt failed on the original test (no impl.txt), not as a
    // protected failure, and no question was asked.
    assert_ne!(attempts[0]["failure"]["kind"], "protected", "{done}");
    assert_eq!(attempts[1]["status"], "passed");
    assert_eq!(read_log(&s, "restored.sh"), TEST_BODY);
    let second = read_log(&s, "second-brief.md");
    assert!(second.contains("were discarded"), "{second}");
    assert!(second.contains(TEST_FILE), "{second}");
    let decisions = done["decisions"].to_string();
    assert!(
        decisions.contains("discarded the attempt's edits"),
        "{decisions}"
    );
    assert!(
        done.get("question").is_none() || done["question"].is_null(),
        "{done}"
    );
    cleanup(s, &done);
}

#[test]
fn an_attempt_that_deletes_a_file_a_check_names_gets_it_back_without_failing() {
    let s = setup(
        r#"rm data.txt
echo ok > impl.txt
emit "$REPORT""#,
        false,
    );
    let mut plan = plan_json(
        "true",
        json!([{"criterion": 0, "run": "grep -q fixture data.txt && test -f impl.txt"}]),
        Value::Null,
    );
    plan["tests"] = json!([]);
    plan_with(&s, plan);
    let repo = repo_with(&[("data.txt", "fixture\n")]);
    let done = run_task(&s, repo.path());
    assert_eq!(done["status"], "done", "task JSON: {done}");
    assert_eq!(
        done["plannerTests"]["protected"],
        json!({"data.txt": sha256_hex("fixture\n")})
    );
    assert_eq!(implement_attempts(&done).len(), 1, "{done}");
    assert!(done["decisions"]
        .to_string()
        .contains("discarded the attempt's edits to protected files: data.txt"));
    assert_eq!(
        git_out(
            repo.path(),
            &[
                "show",
                &format!("{}:data.txt", done["branch"].as_str().unwrap())
            ]
        ),
        "fixture"
    );
    cleanup(s, &done);
}

#[test]
fn the_run_blocks_no_write_to_protected_files_and_the_brief_lists_them() {
    let s = setup(r#"echo ok > impl.txt; emit "$REPORT""#, false);
    plan_with(
        &s,
        plan_json("true", json!([{"criterion": 0, "run": CHECK}]), Value::Null),
    );
    let repo = repo_with(&[]);
    let done = run_task(&s, repo.path());
    assert_eq!(done["status"], "done", "task JSON: {done}");
    let id = done["id"].as_str().unwrap();
    let settings: Value = serde_json::from_str(&run_file(&s.daemon, id, "settings.json")).unwrap();
    let deny: Vec<&str> = settings["permissions"]["deny"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d.as_str())
        .collect();
    assert!(
        deny.iter().all(|d| !d.contains(TEST_FILE)),
        "a protected file must not be write-blocked: {deny:?}"
    );
    let brief = run_file(&s.daemon, id, "brief.md");
    let block = brief.split("## Protected files").nth(1).expect(&brief);
    assert!(block.contains(&format!("`{TEST_FILE}`")), "{brief}");
    cleanup(s, &done);
}

#[test]
fn the_held_out_command_is_in_no_implement_brief_and_runs_between_verify_and_review() {
    let s = setup(r#"echo ok > impl.txt; emit "$REPORT""#, true);
    let held = format!("test -f impl.txt && echo held-out >> \"$LOG_DIR/order.log\" # {SECRET}");
    plan_with(
        &s,
        plan_json(
            "echo verify >> \"$LOG_DIR/order.log\"",
            json!([{"criterion": 0, "run": CHECK}]),
            json!({"criterion": 0, "run": held}),
        ),
    );
    let repo = repo_with(&[]);
    let done = run_task(&s, repo.path());
    assert_eq!(done["status"], "done", "task JSON: {done}");
    assert_eq!(done["heldOut"]["baseline"], "fail", "{done}");

    // Neither the runs' briefs nor any brief the fake received names it.
    let id = done["id"].as_str().unwrap();
    let runs = s.daemon.data_dir().join("tasks").join(id).join("runs");
    let mut seen = 0;
    for entry in std::fs::read_dir(&runs).unwrap() {
        let brief = entry.unwrap().path().join("brief.md");
        if let Ok(text) = std::fs::read_to_string(&brief) {
            seen += 1;
            assert!(!text.contains(SECRET), "{}", brief.display());
        }
    }
    assert!(seen >= 1);
    for entry in std::fs::read_dir(s.log.path()).unwrap() {
        let path = entry.unwrap().path();
        if path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("brief.")
        {
            let text = std::fs::read_to_string(&path).unwrap();
            assert!(!text.contains(SECRET), "{}", path.display());
        }
    }

    let order = read_log(&s, "order.log");
    let lines: Vec<&str> = order.lines().collect();
    let first = |name: &str| {
        lines
            .iter()
            .position(|l| *l == name)
            .unwrap_or_else(|| panic!("{name} missing in {lines:?}"))
    };
    assert!(first("verify") < first("held-out"), "{lines:?}");
    assert!(first("held-out") < first("review"), "{lines:?}");
    cleanup(s, &done);
}

#[test]
fn unsafe_and_oversized_planner_test_files_are_dropped_with_a_decision() {
    let s = setup(r#"echo ok > impl.txt; emit "$REPORT""#, false);
    let mut plan = plan_json("true", json!([{"criterion": 0, "run": CHECK}]), Value::Null);
    plan["tests"] = json!([
        {"path": TEST_FILE, "content": TEST_BODY},
        {"path": "/etc/evil", "content": "x"},
        {"path": "../escape.sh", "content": "x"},
        {"path": ".git/hooks/pre-commit", "content": "x"},
        {"path": "tests/big.sh", "content": "x".repeat(64 * 1024 + 1)},
    ]);
    plan_with(&s, plan);
    let repo = repo_with(&[]);
    let done = run_task(&s, repo.path());
    assert_eq!(done["status"], "done", "task JSON: {done}");
    let files = done["plannerTests"]["files"].as_array().unwrap();
    assert_eq!(files.len(), 1, "{done}");
    let decisions = done["decisions"].to_string();
    for path in [
        "/etc/evil",
        "../escape.sh",
        ".git/hooks/pre-commit",
        "tests/big.sh",
    ] {
        assert!(
            decisions.contains("planner test file dropped") && decisions.contains(path),
            "{path}: {decisions}"
        );
    }
    assert!(!repo.path().join("..").join("escape.sh").exists());
    cleanup(s, &done);
}

const WRONG_BODY: &str = "#!/bin/sh\ntest -f missing.txt\n";

#[test]
fn a_confirmed_wrong_test_claim_has_the_planner_rewrite_and_commit_the_test() {
    let s = setup(
        &format!(
            "{CLAIM_REPORT}\necho 'exit 1' > tests/new_test.sh\necho ok > impl.txt\nemit \"$CLAIM\""
        ),
        false,
    );
    let mut plan = plan_json("true", json!([{"criterion": 0, "run": CHECK}]), Value::Null);
    plan["tests"] = json!([{"path": TEST_FILE, "content": WRONG_BODY}]);
    plan_with(&s, plan);
    reply_file(
        &s,
        "ruling.json",
        "sushi-test-ruling",
        json!({"verdict": "confirmed", "why": "the goal never mentions missing.txt"}),
    );
    reply_file(
        &s,
        "rewrite.json",
        "sushi-test",
        json!({"content": TEST_BODY}),
    );
    let repo = repo_with(&[]);
    let done = run_task(&s, repo.path());
    assert_eq!(done["status"], "done", "task JSON: {done}");
    assert_eq!(implement_attempts(&done).len(), 1, "{done}");
    let decisions = done["decisions"].to_string();
    assert!(
        decisions.contains("the planner rewrote protected test"),
        "{decisions}"
    );
    assert!(
        decisions.contains("the goal never mentions missing.txt"),
        "{decisions}"
    );
    assert_eq!(
        done["plannerTests"]["protected"][TEST_FILE],
        sha256_hex(TEST_BODY),
        "{done}"
    );
    assert_eq!(
        git_out(
            repo.path(),
            &[
                "show",
                &format!("{}:{TEST_FILE}", done["branch"].as_str().unwrap())
            ]
        ),
        TEST_BODY.trim_end()
    );
    cleanup(s, &done);
}

#[test]
fn a_rejected_wrong_test_claim_leaves_the_test_unchanged_and_the_agent_continues() {
    let s = setup(
        &format!(
            "{CLAIM_REPORT}\nif [ \"$n\" = 0 ]; then\n  echo 'exit 0' > tests/new_test.sh\n  emit \"$CLAIM\"\nelse\n  echo ok > impl.txt\n  emit \"$REPORT\"\nfi"
        ),
        false,
    );
    plan_with(
        &s,
        plan_json("true", json!([{"criterion": 0, "run": CHECK}]), Value::Null),
    );
    reply_file(
        &s,
        "ruling.json",
        "sushi-test-ruling",
        json!({"verdict": "rejected", "why": "the test is right"}),
    );
    reply_file(
        &s,
        "rewrite.json",
        "sushi-test",
        json!({"content": "exit 0\n"}),
    );
    let repo = repo_with(&[]);
    let done = run_task(&s, repo.path());
    assert_eq!(done["status"], "done", "task JSON: {done}");
    assert_eq!(implement_attempts(&done).len(), 2, "{done}");
    let decisions = done["decisions"].to_string();
    assert!(
        decisions.contains("was rejected: the test is right"),
        "{decisions}"
    );
    assert!(!decisions.contains("the planner rewrote"), "{decisions}");
    assert_eq!(
        done["plannerTests"]["protected"][TEST_FILE],
        sha256_hex(TEST_BODY),
        "{done}"
    );
    cleanup(s, &done);
}

#[test]
fn a_rewrite_that_passes_on_the_base_is_rejected_and_the_test_stays() {
    let s = setup(
        &format!(
            "{CLAIM_REPORT}\necho 'exit 1' > tests/new_test.sh\necho ok > impl.txt\nemit \"$CLAIM\""
        ),
        false,
    );
    let mut plan = plan_json("true", json!([{"criterion": 0, "run": CHECK}]), Value::Null);
    plan["tests"] = json!([{"path": TEST_FILE, "content": WRONG_BODY}]);
    plan_with(&s, plan);
    reply_file(
        &s,
        "ruling.json",
        "sushi-test-ruling",
        json!({"verdict": "confirmed", "why": "the test asks for the wrong file"}),
    );
    // A rewrite that would pass on the base proves nothing.
    reply_file(
        &s,
        "rewrite.json",
        "sushi-test",
        json!({"content": "#!/bin/sh\ntrue\n# no assertion\n"}),
    );
    let repo = repo_with(&[]);
    let done = run_task(&s, repo.path());
    let decisions = done["decisions"].to_string();
    assert!(
        decisions.contains("rewriting tests/new_test.sh failed"),
        "{decisions}"
    );
    assert!(
        decisions.contains("no longer fails on the base"),
        "{decisions}"
    );
    assert_eq!(
        done["plannerTests"]["protected"][TEST_FILE],
        sha256_hex(WRONG_BODY),
        "{done}"
    );
    cleanup(s, &done);
}
