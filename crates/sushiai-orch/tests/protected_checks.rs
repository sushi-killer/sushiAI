//! Black-box integration tests: the planner's test files are committed before
//! the first attempt, hashed and protected, and the files the checks name are
//! protected with them. Spawns the real `orchd` binary with a fake harness.

mod common;

use common::*;
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
if head -c 9 "$LOG_DIR/brief.$$" | grep -q '## Review'; then
  echo review >> "$LOG_DIR/order.log"
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
fn planner_tests_are_committed_before_attempt_one_baselined_and_hashed() {
    let s = setup(
        r#"git status --porcelain > "$LOG_DIR/status.$n"
git ls-files -- tests/new_test.sh > "$LOG_DIR/tracked.$n"
git log -1 --format=%B > "$LOG_DIR/message.$n"
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

    assert_eq!(
        read_log(&s, "status.0"),
        "",
        "the first run saw a dirty tree"
    );
    assert_eq!(read_log(&s, "tracked.0").trim(), TEST_FILE);
    assert!(
        read_log(&s, "message.0").contains(&format!("Task-Id: {}", done["id"].as_str().unwrap()))
    );

    let commit = done["plannerTests"]["commit"].as_str().unwrap().to_string();
    assert_eq!(commit.len(), 40, "{done}");
    assert_ne!(commit, done["baseSha"].as_str().unwrap());
    let branch = done["branch"].as_str().unwrap();
    git_out(
        repo.path(),
        &["merge-base", "--is-ancestor", &commit, branch],
    );
    assert_eq!(
        done["plannerTests"]["files"],
        json!([{"path": TEST_FILE, "content": TEST_BODY}])
    );

    // The visible check failed on the tests commit, so it gates; the task
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
fn an_attempt_that_edits_a_planner_test_fails_protected_and_the_file_is_restored() {
    let s = setup(
        r#"if [ "$n" = 0 ]; then
  echo 'exit 0' > tests/new_test.sh
  echo ok > impl.txt
else
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
    assert_eq!(attempts[0]["failure"]["kind"], "protected");
    assert!(attempts[0]["failure"]["detail"]
        .as_str()
        .unwrap()
        .contains(TEST_FILE));
    assert_eq!(attempts[1]["status"], "passed");
    assert_eq!(read_log(&s, "restored.sh"), TEST_BODY);
    assert!(
        done.get("question").is_none() || done["question"].is_null(),
        "{done}"
    );
    cleanup(s, &done);
}

#[test]
fn an_attempt_that_deletes_a_file_a_check_names_fails_protected() {
    let s = setup(
        r#"if [ "$n" = 0 ]; then
  rm data.txt
  echo ok > impl.txt
else
  echo ok > impl.txt
fi
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
    let attempts = implement_attempts(&done);
    assert_eq!(attempts.len(), 2, "{done}");
    assert_eq!(attempts[0]["failure"]["kind"], "protected");
    assert!(attempts[0]["failure"]["detail"]
        .as_str()
        .unwrap()
        .contains("data.txt"));
    // Put back from the base: nothing is left of the deletion.
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
fn the_run_denies_edits_to_protected_files_and_the_brief_lists_them() {
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
    let abs = format!("{}/{TEST_FILE}", done["worktree"].as_str().unwrap());
    for tool in ["Edit", "Write"] {
        let rule = format!("{tool}(/{abs})");
        assert!(deny.contains(&rule.as_str()), "{rule} not in {deny:?}");
    }
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
