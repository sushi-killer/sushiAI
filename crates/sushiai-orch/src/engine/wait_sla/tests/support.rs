use crate::api::events::Event;
use crate::clock::now_ms;
use crate::engine::owner_notice;
use crate::engine::questions::parking::{QuestionSlot, WaitEnd};
use crate::engine::questions::wake::wake_orchestrator_for_waiting;
use crate::engine::run_control::TaskControl;
use crate::engine::state::App;
use crate::engine::test_support::task_with_status;
use crate::harness::cancel::CancelToken;
use crate::policy::waits::HOUR_MS;
use crate::task::AskedBy;
use crate::task::Question;
use crate::task::QuestionKind;
use crate::task::Task;
use crate::task::TaskStatus;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Duration;
use tokio::sync::broadcast;

pub(super) const T0: i64 = 1_800_000_000_000;

pub(super) const HALF_HOUR: i64 = HOUR_MS / 2;

pub(super) fn waiting_in(repo: &Path, kind: QuestionKind, options: &[&str]) -> Task {
    let mut task = task_with_status(TaskStatus::Waiting);
    task.fixture_data().repo = repo.to_string_lossy().to_string();
    let mut question = Question::new(
        "Which way?",
        options.iter().map(|o| o.to_string()).collect(),
        kind,
        AskedBy::Implement,
        crate::clock::now_ms(),
    );
    question.asked_at = Some(T0);
    task.fixture_data().question = Some(question);
    task
}

pub(super) async fn calls_reach(repo: &Path, n: usize) {
    for _ in 0..400 {
        if crate::engine::chat::test_support::fake_overseer_calls(repo) >= n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("overseer calls never reached {n}");
}

pub(super) async fn park_live(
    app: &Arc<App>,
    mut task: Task,
) -> tokio::task::JoinHandle<(Result<String, WaitEnd>, Task)> {
    let id = task.id.clone();
    let pending = Arc::new(StdMutex::new(None));
    let cancel = CancelToken::new();
    let waiter = {
        let (app, pending, cancel) = (app.clone(), pending.clone(), cancel.clone());
        tokio::spawn(async move {
            let _id = task.id.clone();
            let answer = QuestionSlot::new(
                &crate::engine::run_control::TaskSignals {
                    resume: Default::default(),
                    cancel: cancel.clone(),
                    pending_answer: pending.clone(),
                    pending_amend: std::sync::Arc::new(std::sync::Mutex::new(None)),
                },
                &mut None,
            )
            .answer(&app, &mut task)
            .await;
            (answer, task)
        })
    };
    for _ in 0..200 {
        if pending.lock().unwrap().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    app.controls.lock().unwrap().insert(
        id,
        TaskControl {
            signals: crate::engine::run_control::TaskSignals {
                resume: Default::default(),
                cancel,
                pending_answer: pending,
                pending_amend: Arc::new(StdMutex::new(None)),
            },
            handle: tokio::spawn(async {}),
        },
    );
    waiter
}

pub(super) fn kinds(app: &App, repo: &str, kind: &str) -> usize {
    crate::store::overseer::read(&app.data_dir, Some(repo), None)
        .iter()
        .filter(|e| e.kind == kind)
        .count()
}

pub(super) fn told(app: &App, task: &Task, ts: i64) {
    let key = owner_notice::decision_key(&task.id, task.question.as_ref().unwrap());
    let mut entry = crate::task::notes::overseer::Entry::new(
        "asked_owner",
        &task.repo,
        Some(&task.id),
        "Q",
        crate::clock::now_ms(),
    )
    .with_decision(&[key]);
    entry.ts = ts;
    app.record_overseer(entry);
}

/// The `heldForOverseer` flag of each broadcast of task `id`, in order.
pub(super) fn held_flags(rx: &mut broadcast::Receiver<Event>, id: &str) -> Vec<bool> {
    let mut flags = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let Event::Task { task } = event {
            if task.id == id {
                flags.push(task.question.as_ref().is_some_and(|q| q.held_for_overseer));
            }
        }
    }
    flags
}

pub(super) fn held(app: &App, id: &str) -> bool {
    app.store
        .load_task(id)
        .unwrap()
        .unwrap()
        .question
        .clone()
        .unwrap()
        .held_for_overseer
}

/// A quiet window that holds the local minute of `at`, in any zone.
pub(super) fn window_around(at: i64) -> String {
    let m = owner_notice::minute_of_day(at);
    let to = (m + 120) % 1440;
    format!("{:02}:{:02}-{:02}:{:02}", m / 60, m % 60, to / 60, to % 60)
}

pub(super) fn boundary_note(tasks: &[&Task]) -> String {
    let lines: Vec<String> = tasks
        .iter()
        .map(|t| format!("- Permission \u{2014} {} ({})", t.title, t.id))
        .collect();
    let noun = if tasks.len() == 1 {
        "question needs"
    } else {
        "questions need"
    };
    format!(
        "While quiet hours were on, {} {noun} your answer:\n{}",
        tasks.len(),
        lines.join("\n")
    )
}

/// Parks an owner-only question while quiet hours are on.
pub(super) fn park_in_quiet(app: &App, repo: &Path, options: &[&str]) -> Task {
    app.settings.write().unwrap().quiet_hours = window_around(now_ms());
    let task = waiting_in(repo, QuestionKind::Permission, options);
    app.store.save_task(&task).unwrap();
    wake_orchestrator_for_waiting(app, &task);
    task
}

pub(super) fn fresh_app(dir: &Path) -> Arc<App> {
    let app = App::new(dir.join("data"), dir.join("home"), "orchd".to_string()).unwrap();
    app.assume_harnesses_installed();
    app.settings.write().unwrap().quiet_hours = String::new();
    app
}
