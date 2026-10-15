use super::*;
use crate::engine::evolution::test_support::owner_answer;
use crate::engine::test_support::{task_with_status, test_app};
use crate::task::evolution::signals::{Signal, SignalKind};
use crate::task::VerifyOutcome;

fn lines(app: &App, file: &str) -> Vec<String> {
    std::fs::read_to_string(app.data_dir.join("evolution").join(file))
        .unwrap_or_default()
        .lines()
        .map(String::from)
        .collect()
}

#[tokio::test]
async fn evolution_second_call_appends_nothing() {
    let (app, _dir) = test_app();
    let mut t = task_with_status(TaskStatus::Done);
    t.fixture_data().question_history = vec![owner_answer("which one?", "first")];
    app.store.save_task(&t).unwrap();
    app.detect_evolution_signals(&t.id).await;
    let first = lines(&app, "signals.jsonl");
    assert_eq!(first.len(), 1);
    let signal: Signal = serde_json::from_str(&first[0]).unwrap();
    assert_eq!(signal.kind, SignalKind::OwnerQuestion);
    assert_eq!(lines(&app, "detected.jsonl").len(), 1);
    app.detect_evolution_signals(&t.id).await;
    assert_eq!(lines(&app, "signals.jsonl"), first);
    assert_eq!(lines(&app, "detected.jsonl").len(), 1);
}

#[tokio::test]
async fn evolution_every_recorded_signal_carries_a_timestamp() {
    let (app, _dir) = test_app();
    let mut t = task_with_status(TaskStatus::Done);
    t.fixture_data().question_history = vec![owner_answer("which one?", "first")];
    app.store.save_task(&t).unwrap();
    let before = now_ms();
    app.detect_evolution_signals(&t.id).await;
    let line = &lines(&app, "signals.jsonl")[0];
    let v: serde_json::Value = serde_json::from_str(line).unwrap();
    assert!(v["ts"].as_i64().unwrap() >= before);
}

#[tokio::test]
async fn evolution_task_without_signals_is_still_recorded_once() {
    let (app, _dir) = test_app();
    let t = task_with_status(TaskStatus::Done);
    app.store.save_task(&t).unwrap();
    app.detect_evolution_signals(&t.id).await;
    assert!(lines(&app, "signals.jsonl").is_empty());
    assert_eq!(lines(&app, "detected.jsonl").len(), 1);
}

#[tokio::test]
async fn evolution_eval_and_unfinished_tasks_are_skipped() {
    let (app, _dir) = test_app();
    let mut eval = task_with_status(TaskStatus::Done);
    eval.fixture_data().eval_set = Some("set".into());
    eval.fixture_data().question_history = vec![owner_answer("q", "a")];
    let mut running = task_with_status(TaskStatus::Running);
    running.fixture_data().question_history = vec![owner_answer("q", "a")];
    app.store.save_task(&eval).unwrap();
    app.store.save_task(&running).unwrap();
    app.detect_evolution_signals(&eval.id).await;
    app.detect_evolution_signals(&running.id).await;
    assert!(lines(&app, "signals.jsonl").is_empty());
    assert!(lines(&app, "detected.jsonl").is_empty());
}

#[test]
fn evolution_failing_commands_are_distinct_and_include_parent_check() {
    let mut t = task_with_status(TaskStatus::Failed);
    let out = |command: &str, code| VerifyOutcome {
        command: command.into(),
        code,
        tail: String::new(),
        ms: 0,
        ..Default::default()
    };
    let mut a = crate::engine::test_support::attempt_with_failure(1, "s");
    a.verify = vec![
        out("npm test", Some(1)),
        out("cargo fmt", Some(0)),
        out("slow", None),
    ];
    let mut b = a.clone();
    b.n = 2;
    t.fixture_data().attempts = vec![a, b];
    t.fixture_data().decisions = vec!["reworded".into()];
    t.fixture_data().loop_state.final_check_failures = vec!["npm run ci".into()];
    assert_eq!(failing_commands(&t), vec!["npm test", "npm run ci"]);
}
