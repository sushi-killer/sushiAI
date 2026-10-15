use super::clustering::cluster_key;
use crate::engine::test_support::task_with_status;
use crate::policy::evolution::EvolutionSettings;
use crate::task::evolution::signals::{ExcerptRef, Signal, SignalKind, StoredSignal};
use crate::task::evolution::{Proposal, ProposalForm, ProposalStatus, Track};
use crate::task::{Task, TaskStatus};

/// An answered question for tests; the owner gave `answer`.
pub(crate) fn owner_answer(question: &str, answer: &str) -> crate::task::AnsweredQuestion {
    crate::task::AnsweredQuestion {
        question: question.into(),
        options: Vec::new(),
        kind: crate::task::QuestionKind::AgentQuestion,
        asked_by: None,
        asked_at: None,
        answer: answer.into(),
        answered_at: 1,
        answered_by: crate::task::AnsweredBy::Owner,
        failure_signature: None,
        commands: Vec::new(),
    }
}

pub(super) fn signal(kind: SignalKind, task: &str, repo: &str, detail: &str) -> StoredSignal {
    StoredSignal {
        signal: Signal {
            kind,
            task_id: task.into(),
            repo: repo.into(),
            attempt: 1,
            detail: detail.into(),
            wasted_calls: 0,
            wasted_usd: 0.0,
            excerpt_ref: ExcerptRef {
                file: "tasks/x/runs/1/events.jsonl".into(),
                from_line: 1,
                to_line: 1,
            },
        },
        ts: 0,
    }
}

pub(super) fn waste(mut recorded: StoredSignal, calls: u32, usd: f64) -> StoredSignal {
    recorded.signal.wasted_calls = calls;
    recorded.signal.wasted_usd = usd;
    recorded
}

pub(super) fn thresholds(tasks: u32, calls: u32, usd: f64) -> EvolutionSettings {
    EvolutionSettings {
        min_tasks: tasks,
        min_wasted_calls: calls,
        min_wasted_usd: usd,
        ..Default::default()
    }
}

pub(super) fn small_cluster() -> Vec<StoredSignal> {
    vec![
        waste(
            signal(SignalKind::Loop, "a", "/r", "reads foo 3 times"),
            4,
            0.2,
        ),
        waste(
            signal(SignalKind::Loop, "b", "/r", "reads foo 7 times"),
            6,
            0.3,
        ),
    ]
}

pub(super) fn proposal(status: ProposalStatus, cluster_key: &str) -> Proposal {
    Proposal {
        id: uuid::Uuid::new_v4().to_string(),
        cluster_key: cluster_key.into(),
        kind: "loop".into(),
        repo: "/r".into(),
        track: Track::Repo,
        form: ProposalForm::Script,
        change: "c".into(),
        evidence: "e".into(),
        metric: "m".into(),
        test: "t".into(),
        status,
        created_at: 1,
        cost_usd: 0.0,
        fingerprint: None,
        eval_command: None,
        task_id: None,
        adopted_at: None,
        before: None,
        after: None,
        reason: None,
    }
}

pub(super) fn finished(id: &str, updated_at: i64) -> Task {
    let mut t = task_with_status(TaskStatus::Done);
    t.fixture_data().id = id.into();
    t.fixture_data().repo = "/r".into();
    t.fixture_data().updated_at = updated_at;
    t
}

pub(super) fn measured(after: u32, hits: u32) -> (Proposal, Vec<Task>, Vec<StoredSignal>) {
    let key = cluster_key(&waste(signal(SignalKind::Loop, "", "/r", "d"), 0, 0.0).signal);
    let mut p = proposal(ProposalStatus::Adopted, &key);
    p.adopted_at = Some(1000);
    p.task_id = Some("own".into());
    let mut tasks = vec![finished("own", 1000)];
    let mut signals = Vec::new();
    for i in 0..4 {
        tasks.push(finished(&format!("b{i}"), 100 + i));
        signals.push(waste(
            signal(SignalKind::Loop, &format!("b{i}"), "/r", "d"),
            1,
            0.0,
        ));
    }
    for i in 0..after {
        tasks.push(finished(&format!("a{i}"), 2000 + i64::from(i)));
        if i < hits {
            signals.push(waste(
                signal(SignalKind::Loop, &format!("a{i}"), "/r", "d"),
                1,
                0.0,
            ));
        }
    }
    (p, tasks, signals)
}
