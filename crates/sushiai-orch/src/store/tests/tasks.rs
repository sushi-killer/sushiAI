use super::super::task::parse_task;
use super::super::Store;
use crate::task::test_support::sample_task;
use crate::task::{
    AttemptStatus, BaseAnswer, CheckSource, QuestionKind, TaskStatus, STATE_VERSION,
};
use std::fs;

#[test]
fn an_older_task_gets_the_source_of_each_planner_command() {
    let mut task = sample_task("old", TaskStatus::Waiting, AttemptStatus::Passed);
    task.fixture_data().criteria = vec!["a -- check: grep -q x f".into(), "b".into()];
    task.fixture_data().request = Some("Please run `make owner` too".into());
    task.fixture_data().verify = vec!["true".into(), "go test ./...".into(), "make owner".into()];
    let mut value = serde_json::to_value(&task).unwrap();
    value["plannerTests"] = serde_json::json!({
        "commands": ["true", "grep -q x f", "make owner", "make other", "go test ./..."]
    });
    let task = parse_task(&value.to_string()).unwrap();
    let source = |c: &str| {
        task.planner_tests
            .sources
            .iter()
            .find(|s| s.command == c)
            .map(|s| s.source)
    };
    assert_eq!(source("true"), Some(CheckSource::Repo));
    // A legacy command the plan no longer holds is not recorded: the repo's.
    assert_eq!(source("make other"), None);
    assert_eq!(source("go test ./..."), Some(CheckSource::Repo));
    assert_eq!(source("grep -q x f"), Some(CheckSource::Planner));
    assert_eq!(source("make owner"), Some(CheckSource::Owner));
    assert!(task.planner_tests.legacy_commands.is_empty());
}

#[test]
fn a_plan_records_who_wrote_each_command_and_the_stricter_source_wins() {
    let mut task = sample_task("new", TaskStatus::Waiting, AttemptStatus::Passed);
    task.fixture_data().criteria = vec![
        "a -- check: pytest".into(),
        "b -- check: grep -q x f".into(),
    ];
    task.fixture_data().verify = vec!["pytest".into()];
    task.fixture_data().final_verify = vec!["make owner".into()];
    let sources = task
        .planner_check_commands()
        .into_iter()
        .map(|(command, in_verify)| {
            let source = match (command.as_str(), in_verify) {
                ("make owner", _) => CheckSource::Owner,
                ("pytest", true) => CheckSource::Repo,
                _ => CheckSource::Planner,
            };
            crate::task::SourcedCheck { command, source }
        })
        .collect();
    task.record_check_sources(sources);
    let got: Vec<_> = task
        .planner_tests
        .sources
        .iter()
        .map(|s| (s.command.as_str(), s.source))
        .collect();
    assert_eq!(
        got,
        vec![
            ("pytest", CheckSource::Repo),
            ("make owner", CheckSource::Owner),
            ("grep -q x f", CheckSource::Planner),
        ]
    );
}

#[test]
fn task_round_trips_through_store() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let task = sample_task("t1", TaskStatus::Running, AttemptStatus::Running);
    store.save_task(&task).unwrap();
    let loaded = store.load_task("t1").unwrap().unwrap();
    assert_eq!(loaded.id, "t1");
    assert_eq!(loaded.branch, "task/do-thing");
    assert_eq!(loaded.attempts.len(), 1);
    assert!(store.load_task("missing").unwrap().is_none());
}

#[test]
fn archived_flag_round_trips_and_defaults_false_for_a_pre_existing_task_json() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();

    let mut task = sample_task("t1", TaskStatus::Done, AttemptStatus::Passed);
    task.fixture_data().archived = true;
    store.save_task(&task).unwrap();
    let loaded = store.load_task("t1").unwrap().unwrap();
    assert!(loaded.archived);

    // A task.json written before `archived` existed has no such key at
    // all -- simulate that directly rather than via `Task`, which would
    // always serialize the field.
    let old_dir = store.task_dir("old");
    fs::create_dir_all(&old_dir).unwrap();
    let mut value =
        serde_json::to_value(sample_task("old", TaskStatus::Done, AttemptStatus::Passed)).unwrap();
    value.as_object_mut().unwrap().remove("archived");
    fs::write(
        old_dir.join("task.json"),
        serde_json::to_vec_pretty(&value).unwrap(),
    )
    .unwrap();
    let old_loaded = store.load_task("old").unwrap().unwrap();
    assert!(!old_loaded.archived);
}

/// A task.json written with fields since removed from `Variant` and
/// `Attempt` must still load: serde ignores unknown fields by default,
/// so dropping them from the structs is not a breaking change for a
/// task already on disk.
#[test]
fn a_task_json_with_fields_since_removed_from_variant_and_attempt_still_loads() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let task_dir = store.task_dir("old");
    fs::create_dir_all(&task_dir).unwrap();
    let mut value =
        serde_json::to_value(sample_task("old", TaskStatus::Done, AttemptStatus::Passed)).unwrap();
    // A flag removed from `Variant` (2026-09-28) must not break loading.
    value["variant"]["leanContext"] = serde_json::json!(true);
    value["attempts"][0]["skills"] = serde_json::json!(["deslop"]);
    fs::write(
        task_dir.join("task.json"),
        serde_json::to_vec_pretty(&value).unwrap(),
    )
    .unwrap();
    let loaded = store.load_task("old").unwrap().unwrap();
    assert_eq!(loaded.id, "old");
    assert_eq!(loaded.attempts.len(), 1);
}

#[test]
fn a_task_with_legacy_state_lines_loads_with_typed_fields() {
    use crate::task::{Question, QuestionKind};
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut task = sample_task("legacy", TaskStatus::Queued, AttemptStatus::Blocked);
    task.fixture_data().unreviewed = true;
    task.fixture_data().decisions = vec![
            "Orchestrator: restarted a failed task".into(),
            "Orchestrator: restarted a dependency that failed on the environment".into(),
            "Ladder: changed approach after attempt 1 (the same tier; the planner re-plans the rest)".into(),
            "Land: the carry onto b changed f which the review saw".into(),
            "Orchestrator: Attempts keep failing -> continue (r)".into(),
            "Overseer: answered like owner precedent -- Attempts keep failing -> continue".into(),
            "Lead: Attempts keep failing -> continue".into(),
            "Orchestrator: subtask \"Part (a)\" (child-1) landed unreviewed: no review route gave a verdict".into(),
            "Orchestrator: attempt 1 committed unreviewed: its open review question was retired".into(),
        ];
    let mut question = Question::new(
        "Criterion 2 cannot be met as written: x. Evidence: a. Evidence: b",
        vec![],
        QuestionKind::Impossible,
        crate::task::AskedBy::Implement,
        crate::clock::now_ms(),
    );
    question.commands = vec!["npm test".into(), "npm run lint".into()];
    task.fixture_data().question = Some(question);
    store.save_task(&task).unwrap();
    // Written by an older version: no state version, no typed fields.
    let path = store.task_path("legacy");
    let mut value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    value.as_object_mut().unwrap().remove("stateVersion");
    fs::write(&path, value.to_string()).unwrap();

    let loaded = store.load_task("legacy").unwrap().unwrap();
    let state = &loaded.loop_state;
    assert_eq!(state.state_version, STATE_VERSION);
    assert_eq!(state.unreviewed_attempt, Some(1));
    assert!(state.agent_restarted && state.dependency_restarted);
    assert_eq!(state.approach_changed_after, Some(1));
    assert_eq!(state.rereviews, 1);
    assert_eq!(state.non_owner_continues, 3);
    assert_eq!(state.unreviewed_parts.len(), 1);
    assert_eq!(state.unreviewed_parts[0].id, "child-1");
    assert_eq!(state.unreviewed_parts[0].title, "Part (a)");
    let q = loaded.question.clone().unwrap();
    assert_eq!(q.criterion, Some(2));
    assert_eq!(q.evidence.as_deref(), Some("a. Evidence: b"));
    // Loaded and saved again, the state stays as it is: the lines are not read twice.
    let mut again = store.load_task("legacy").unwrap().unwrap();
    again.fixture_data().loop_state.rereviews = 0;
    store.save_task(&again).unwrap();
    assert_eq!(
        store
            .load_task("legacy")
            .unwrap()
            .unwrap()
            .loop_state
            .rereviews,
        0
    );
}

#[test]
fn a_version_one_task_gets_base_check_and_permission_state_from_its_lines() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut task = sample_task("v1", TaskStatus::Waiting, AttemptStatus::Passed);
    task.fixture_data().decisions = vec![
        "Orchestrator: kept check npm test, the task is meant to make it pass".into(),
        "Orchestrator: kept check cargo test -- a, b, targets absent on base; the plan creates them".into(),
        "Orchestrator: waited out base abcdef0123; not waiting for it to move again".into(),
        "Lead: base check npm test on 0123456 -> retry after the base is fixed".into(),
        "Owner: base check npm run lint on 0123456 -> drop all".into(),
        "Orchestrator: retry base check npm test after 0123456789abcdef moves".into(),
        "Permission: allowed once: Write(src/a.rs)".into(),
        "Permission: denied: mcp__x__y".into(),
    ];
    let mut policy = crate::task::Assumption {
        question: "npm ci already fails on base 89abcde: boom".into(),
        answer: "retry after the base is fixed".into(),
        evidence: String::new(),
        by: "policy".into(),
        kind: Some(QuestionKind::PreexistingFailure),
        attempt: Some(1),
        commands: vec!["npm ci".into()],
        overturned: false,
        owner_answer: None,
    };
    task.fixture_data().assumptions.push(policy.clone());
    policy.by = "Owner".into();
    task.fixture_data().assumptions.push(policy);
    store.save_task(&task).unwrap();
    let path = store.task_path("v1");
    let mut value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    value["stateVersion"] = 1.into();
    fs::write(&path, value.to_string()).unwrap();

    let state = store.load_task("v1").unwrap().unwrap().loop_state.clone();
    assert_eq!(state.state_version, STATE_VERSION);
    assert_eq!(state.kept_checks, ["npm test", "cargo test -- a, b"]);
    assert_eq!(state.waited_out_bases, ["abcdef0123"]);
    assert_eq!(state.answered_permissions, ["Write(src/a.rs)", "mcp__x__y"]);
    let answer = |command: &str, sha: &str, by: &str, retry: bool| BaseAnswer {
        command: command.into(),
        sha: sha.into(),
        by: by.into(),
        retry,
    };
    assert_eq!(
        state.base_answers,
        [
            answer("npm test", "0123456", "Lead", true),
            answer("npm run lint", "0123456", "Owner", false),
            answer("npm test", "0123456789abcdef", "", true),
            answer("npm ci", "89abcde", "policy", true),
        ]
    );
}

#[test]
fn a_version_two_task_gets_its_final_check_failures_from_the_line() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut task = sample_task("v2", TaskStatus::Failed, AttemptStatus::Passed);
    task.fixture_data().decisions = vec![
        "Orchestrator: every subtask landed, but `npm run ci` exited 2 on b: x".into(),
        "Orchestrator: every subtask landed, but `npm run ci` exited 2 on b: y".into(),
    ];
    store.save_task(&task).unwrap();
    let path = store.task_path("v2");
    let mut value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    value["stateVersion"] = 2.into();
    fs::write(&path, value.to_string()).unwrap();
    let state = store.load_task("v2").unwrap().unwrap().loop_state.clone();
    assert_eq!(state.state_version, STATE_VERSION);
    assert_eq!(state.final_check_failures, ["npm run ci"]);
}

#[test]
fn list_tasks_sorts_newest_first_and_skips_corrupt() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut older = sample_task("older", TaskStatus::Done, AttemptStatus::Passed);
    older.fixture_data().created_at = 1;
    let mut newer = sample_task("newer", TaskStatus::Done, AttemptStatus::Passed);
    newer.fixture_data().created_at = 2;
    store.save_task(&older).unwrap();
    store.save_task(&newer).unwrap();

    // A corrupt task directory should be skipped, not fail the listing.
    let corrupt_dir = store.task_dir("corrupt");
    fs::create_dir_all(&corrupt_dir).unwrap();
    fs::write(corrupt_dir.join("task.json"), b"not json").unwrap();

    let listed = store.list_tasks().unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].id, "newer");
    assert_eq!(listed[1].id, "older");
}
