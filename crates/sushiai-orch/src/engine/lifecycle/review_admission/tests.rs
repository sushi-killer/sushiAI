use super::{prepare, AdmissionEnd};
use crate::engine::lifecycle::execution::RunReceipt;
use crate::engine::run_control::TaskSignals;
use crate::engine::state::App;
use crate::engine::test_support::test_app;
use crate::harness::run_files::RunFiles;
use crate::task::attempts::{record_report, record_run_completion, AttemptReport, RunCompletion};
use crate::task::test_support::sample_task;
use crate::task::{Assumption, AttemptStatus, QuestionKind, Task, TaskStatus, Usage};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// What is left of this test's shared wait budget.
fn left() -> Duration {
    crate::procs::testing::expiry().saturating_duration_since(Instant::now())
}

fn inspected_native(app: &App) -> (Task, RunReceipt) {
    let mut task = sample_task(
        &uuid::Uuid::new_v4().to_string(),
        TaskStatus::Running,
        AttemptStatus::Running,
    );
    task.fixture_data().criteria = vec!["Original acceptance".into()];
    task.record_assumption(Assumption {
        question: "Accept the inherited check?".into(),
        answer: crate::task::questions::WAIVED.into(),
        evidence: "The base has the same named failure".into(),
        by: crate::task::questions::WAIVER_BY.into(),
        kind: Some(QuestionKind::PreexistingFailure),
        attempt: None,
        commands: vec!["check".into()],
        overturned: false,
        owner_answer: None,
    });
    record_run_completion(
        &mut task,
        0,
        RunCompletion {
            session: None,
            fingerprint: None,
            usage: Usage {
                input: 100,
                output: 20,
                cached: 50,
            },
            first_turn_tokens: None,
            cost: Some(0.8),
            estimated: false,
        },
    );
    app.store.save_task(&task).unwrap();
    let mut settings = app.settings.read().unwrap().clone();
    settings.protected_paths.clear();
    let run = RunReceipt {
        route: settings.routes[0].clone(),
        settings,
        attempt_n: task.attempts[0].n,
        idx: 0,
        worktree: task.worktree.clone().into(),
        base_sha: task.base_sha.clone(),
        run_dir: app.store.run_dir(&task.id, task.attempts[0].n),
    };
    std::fs::create_dir_all(&run.run_dir).unwrap();
    let files = RunFiles::of(&run.run_dir.join("events.jsonl"));
    std::fs::write(
        files.raw,
        serde_json::json!({
            "type":"result", "total_cost_usd":0.8, "result":"paid native report",
            "usage":{"input_tokens":100,"output_tokens":20,"cache_read_input_tokens":50}
        })
        .to_string()
            + "\n",
    )
    .unwrap();
    std::fs::write(files.exit, "0\n").unwrap();
    record_report(
        &mut task,
        0,
        AttemptReport {
            summary: Some("unsaved inspection summary".into()),
            handoff: Some("unsaved inspection handoff".into()),
            declines: Vec::new(),
        },
    );
    (task, run)
}

async fn admission_before_cancellation(
    app: &Arc<App>,
    task: &mut Task,
    run: &RunReceipt,
) -> Result<(), AdmissionEnd> {
    let signals = TaskSignals::new();
    let cancel = signals.cancel.clone();
    let (finish, watchdog) = std::sync::mpsc::channel();
    let guard = std::thread::spawn(move || {
        if watchdog.recv_timeout(left()).is_err() {
            cancel.cancel();
        }
    });
    let mut permit = None;
    let result = tokio::time::timeout(left(), prepare(app, task, run, &signals, &mut permit)).await;
    let _ = finish.send(());
    guard.join().unwrap();
    result.unwrap_or_else(|_| panic!("review admission kept retrying a stale authority"))
}

#[tokio::test]
async fn stale_review_healing_yields_to_the_outer_reload_and_keeps_native_observations() {
    for overturn in [true, false] {
        let (app, _dir) = test_app();
        let (mut task, run) = inspected_native(&app);
        if overturn {
            app.dispatch(
                "task.overturn",
                serde_json::json!({
                    "id":task.id, "index":0, "answer":"This check must pass"
                }),
            )
            .await
            .unwrap();
        } else {
            let amended = app
                .dispatch(
                    "task.amend",
                    serde_json::json!({
                        "id":task.id, "criteria":["Owner's revised acceptance"]
                    }),
                )
                .await
                .unwrap();
            assert_eq!(amended["pending"], false);
        }
        let result = admission_before_cancellation(&app, &mut task, &run).await;
        assert!(
            matches!(result, Err(AdmissionEnd::Changed)),
            "review admission did not yield after an Owner change"
        );
        assert_eq!(
            task.attempts[0].summary.as_deref(),
            Some("unsaved inspection summary")
        );
        assert_eq!(
            task.attempts[0].handoff.as_deref(),
            Some("unsaved inspection handoff")
        );
        let mut fresh = app.store.load_task(&task.id).unwrap().unwrap();
        assert_eq!(fresh.cost_usd, 0.8);
        assert_eq!(fresh.attempts.len(), 1);
        let usage = fresh.attempts[0].usage.as_ref().unwrap();
        assert_eq!((usage.input, usage.output, usage.cached), (100, 20, 50));
        if overturn {
            assert!(fresh.assumptions[0].overturned);
        } else {
            assert_eq!(fresh.criteria, ["Owner's revised acceptance"]);
        }
        assert_eq!(
            crate::engine::recovery::resumable_attempt(&app, &fresh),
            Some(0)
        );
        let adopted = crate::engine::lifecycle::adopted::follow(
            &app,
            &mut fresh,
            0,
            &crate::harness::cancel::CancelToken::new(),
        )
        .await;
        assert_eq!(
            adopted
                .run_result
                .unwrap_or_else(|_| panic!("the finished native run was not re-adopted"))
                .final_text
                .as_deref(),
            Some("paid native report")
        );
        assert_eq!(fresh.attempts.len(), 1);
        assert_eq!(fresh.cost_usd, 0.8);
    }
}
