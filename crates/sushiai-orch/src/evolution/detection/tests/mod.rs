use super::*;
use crate::engine::test_support::{attempt_with_failure, task_with_status};
use crate::task::{Attempt, Failure, FailureKind, ReviewResult, TaskStatus, Verdict};
use serde_json::json;

mod attempts;
mod graph;
mod questions;
mod throwaways;

pub(super) fn task(id: &str, created: i64) -> Task {
    let mut t = task_with_status(TaskStatus::Done);
    t.fixture_data().id = id.into();
    t.fixture_data().created_at = created;
    t
}

pub(super) fn call(name: &str, input: Value) -> Value {
    json!({"type":"assistant","message":{"content":[{"type":"tool_use","name":name,"input":input}]}})
}

pub(super) fn read(path: &str) -> Value {
    call("Read", json!({"file_path": path}))
}

pub(super) fn run(task: &Task, others: &[Task], evs: &[(&str, u32, Vec<Value>)]) -> Vec<Signal> {
    let mut all = vec![task.clone()];
    all.extend(others.iter().cloned());
    let events: AttemptEvents = evs
        .iter()
        .map(|(id, n, v)| ((id.to_string(), *n), v.clone()))
        .collect();
    Detection::for_task(task, &events, &Settings::default()).finish(&all, &[], &[])
}

pub(super) fn of_kind(signals: &[Signal], kind: SignalKind) -> Vec<&Signal> {
    signals.iter().filter(|s| s.kind == kind).collect()
}

pub(super) fn failure(kind: FailureKind, detail: &str, signature: &str) -> Failure {
    Failure {
        kind,
        detail: detail.into(),
        signature: signature.into(),
    }
}

pub(super) fn reviewed(mut a: Attempt, findings: &[&str]) -> Attempt {
    a.review = Some(ReviewResult {
        verdict: Verdict::Fail,
        findings: findings.iter().map(|f| f.to_string()).collect(),
        repeated: Vec::new(),
        severities: Vec::new(),
        details: Vec::new(),
        criteria: Vec::new(),
    });
    a
}

pub(super) fn process_settings(repo: &str, min_calls: u32) -> Settings {
    let mut settings = Settings::default();
    settings.evolution.process_min_calls = min_calls;
    settings
        .evolution
        .process_files
        .insert(repo.into(), vec!["docs/LESSONS.md".into()]);
    settings
}
