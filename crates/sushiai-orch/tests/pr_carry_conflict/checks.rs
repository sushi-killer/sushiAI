use super::{git_out, setup, Setup, FAKE_CLAUDE, URL};
use serde_json::{json, Value};

const REPAIR: &str = r###"*"## PR check repair"*)
printf '%s' "$brief" > "$dir/check-repair-brief.md"
echo repair >> "$dir/check-repair-runs"
if [ -f "$dir/repair-green" ]; then echo green > repair-status.txt; fi
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"checked the carried failure\",\"decisions\":[],\"question\":\"\"}\n```"}'
;;
"###;

fn fixture(flaky: bool) -> Setup {
    let s = setup(true, &[]);
    let script = FAKE_CLAUDE.replacen(
        "case \"$brief\" in\n",
        &format!("case \"$brief\" in\n{REPAIR}"),
        1,
    );
    std::fs::write(s.tools.path().join("fake-claude.sh"), script).unwrap();
    let failure = if flaky {
        "if [ ! -e .flaky-once ]; then touch .flaky-once; printf 'not ok 1 - post_carry_flaky_case_7\\n'; exit 1; fi"
    } else {
        "if [ \"$(cat repair-status.txt 2>/dev/null)\" != green ]; then printf 'not ok 1 - post_carry_case_17\\n'; exit 1; fi"
    };
    std::fs::write(s.repo.path().join("ci.sh"), format!("if [ -e after-carry ] && [ -e pr.txt ]; then\n{failure}\nfi\nprintf 'ok 1 - post_carry_check\\n'\n")).unwrap();
    std::fs::write(s.repo.path().join(".gitignore"), ".flaky-once\n").unwrap();
    git_out(s.repo.path(), &["add", "."]);
    git_out(s.repo.path(), &["commit", "-qm", "check fixture"]);
    git_out(s.repo.path(), &["push", "-q", "origin", "main"]);
    s.advance_origin(&[("after-carry", "new base\n")]);
    s
}

fn creates(s: &Setup) -> String {
    s.create_with(
        "test -f pr.txt",
        json!({"finalVerify":["sh ci.sh"],"variant":{"maxAttempts":1,"land":false}}),
    )
}

fn attempts(task: &Value) -> usize {
    task["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|attempt| attempt["stage"] == "implement")
        .count()
}

fn repair_count(s: &Setup) -> usize {
    std::fs::read_to_string(s.tools.path().join("check-repair-runs"))
        .unwrap_or_default()
        .lines()
        .count()
}

fn wait_for_owner(s: &Setup, id: &str) -> Value {
    let task = s.wait_task(
        id,
        "persistent carried check did not reach the owner",
        |task| task["question"]["kind"] == "pr_check",
    );
    assert_eq!(task["status"], "waiting", "{task}");
    assert_eq!(task["question"]["forOwner"], true, "{task}");
    assert!(
        task["prUrl"].is_null() && task["prPending"] == true,
        "{task}"
    );
    assert!(task["question"]["text"]
        .as_str()
        .unwrap()
        .contains("post_carry_case_17"));
    assert!(task["question"]["text"]
        .as_str()
        .unwrap()
        .contains("sh ci.sh"));
    assert_eq!(
        attempts(&task),
        2,
        "one original attempt and one repair: {task}"
    );
    assert_eq!(repair_count(s), 1);
    assert!(!s.gh_log().contains("pr create"), "{}", s.gh_log());
    task
}

#[test]
fn a_flaky_carried_final_opens_the_pr_with_its_named_waiver_and_no_repair_step() {
    let s = fixture(true);
    let id = creates(&s);
    let task = s.wait_task(&id, "flaky carried final prevented the PR", |task| {
        task["prUrl"].is_string()
    });
    assert_eq!(task["prUrl"], URL, "{task}");
    assert_eq!(attempts(&task), 1, "{task}");
    assert_eq!(repair_count(&s), 0);
    assert!(
        task["decisions"]
            .to_string()
            .contains("post_carry_flaky_case_7"),
        "{task}"
    );
    assert!(
        task["assumptions"]
            .to_string()
            .contains("post_carry_flaky_case_7"),
        "{task}"
    );
    assert!(task["deliveryChecks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|check| check["command"] == "sh ci.sh" && check["code"] == 1));
    s.daemon.shutdown_and_wait();
}

#[test]
fn a_persistent_carried_final_gets_one_own_step_then_owner_retry_opens_the_pr() {
    let s = fixture(false);
    let id = creates(&s);
    wait_for_owner(&s, &id);
    let brief = std::fs::read_to_string(s.tools.path().join("check-repair-brief.md")).unwrap();
    assert!(
        brief.contains("## PR check repair")
            && brief.contains("post_carry_case_17")
            && brief.contains("sh ci.sh"),
        "{brief}"
    );
    let refusal = s.daemon.request_error(
        "task.answer",
        json!({"id":id,"answer":"retry","source":"overseer"}),
    );
    assert!(refusal.contains("owner"), "{refusal}");
    assert_eq!(attempts(&s.task(&id)), 2);
    std::fs::write(s.tools.path().join("repair-green"), "").unwrap();
    s.daemon
        .request("task.answer", json!({"id":id,"answer":"retry"}));
    let task = s.wait_task(&id, "explicit owner retry did not open the PR", |task| {
        task["prUrl"].is_string()
    });
    assert_eq!(attempts(&task), 3, "{task}");
    assert_eq!(repair_count(&s), 2);
    assert_eq!(task["prUrl"], URL);
    assert!(task["prCheckRepair"].is_null(), "{task}");
    assert_eq!(
        super::blob(s.remote.path(), &s.pushed(&task), "repair-status.txt"),
        "green"
    );
    s.daemon.shutdown_and_wait();
}

#[test]
fn stopping_the_persistent_carried_check_keeps_it_stopped_without_a_push_or_pr() {
    let s = fixture(false);
    let id = creates(&s);
    wait_for_owner(&s, &id);
    let stopped = s
        .daemon
        .request("task.answer", json!({"id":id,"answer":"stop"}));
    assert_eq!(stopped["status"], "stopped", "{stopped}");
    assert_eq!(attempts(&stopped), 2);
    assert_eq!(repair_count(&s), 1);
    assert!(!s.gh_log().contains("pr create"), "{}", s.gh_log());
    let branch = format!("refs/heads/{}", stopped["branch"].as_str().unwrap());
    assert!(git_out(
        s.remote.path(),
        &["for-each-ref", "--format=%(refname)", &branch]
    )
    .is_empty());
    s.daemon.shutdown_and_wait();
}

#[test]
fn the_first_own_repair_that_turns_green_opens_the_pr_without_an_owner_question() {
    let s = fixture(false);
    std::fs::write(s.tools.path().join("repair-green"), "").unwrap();
    let id = creates(&s);
    let task = s.wait_task(&id, "the first green repair did not open its PR", |task| {
        task["prUrl"].is_string()
    });
    assert_eq!(task["prUrl"], URL, "{task}");
    assert_eq!(attempts(&task), 2, "{task}");
    assert_eq!(repair_count(&s), 1);
    assert!(
        task["question"].is_null() && task["prCheckRepair"].is_null(),
        "{task}"
    );
    assert!(
        !task["questionHistory"].to_string().contains("pr_check"),
        "{task}"
    );
    let runs: Vec<_> = task["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|attempt| attempt["stage"] == "implement")
        .collect();
    for field in ["routeId", "harness", "model"] {
        assert_eq!(
            runs[0][field], runs[1][field],
            "the task's own route is retained: {task}"
        );
    }
    assert_eq!(
        super::blob(s.remote.path(), &s.pushed(&task), "repair-status.txt"),
        "green"
    );
    s.daemon.shutdown_and_wait();
}
