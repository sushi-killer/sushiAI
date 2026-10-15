use super::{publish, PlanProposal, Publication};
use crate::engine::planning::draft::parse_plan;
use crate::engine::run_control::TaskSignals;
use crate::engine::test_support::{attempt_with_failure, task_with_status, test_app};
use crate::task::amendment::{Amendment, PendingAmendment};
use crate::task::checks::authority::Origin;
use crate::task::checks::snapshot::CheckSnapshot;
use crate::task::{AttemptStatus, CheckSource, SourcedCheck, Stage, Task, TaskStatus};

fn task() -> Task {
    let mut task = task_with_status(TaskStatus::Drafting);
    let mut attempt = attempt_with_failure(1, "drafting");
    attempt.stage = Stage::Plan;
    attempt.status = AttemptStatus::Running;
    attempt.failure = None;
    task.fixture_data().attempts.push(attempt);
    task
}

fn proposal(task: &Task, verify: &str, finals: &str) -> PlanProposal {
    let draft = parse_plan(&format!(
        "```sushi-plan\n{{\"title\":\"Proposed title\",\"goal\":\"Proposed goal\",\"verify\":{verify},\"finalVerify\":{finals}}}\n```"
    )).unwrap();
    PlanProposal {
        based_on: CheckSnapshot::capture(task),
        draft,
        attempt: 0,
    }
}

#[tokio::test]
async fn an_accepted_dependency_edit_lands_before_the_old_planner_contract_is_discarded() {
    let (app, _dir) = test_app();
    let mut task = task();
    let signals = TaskSignals::new();
    let proposal = proposal(&task, "[]", "[]");
    *signals.pending_amend.lock().unwrap() = Some(PendingAmendment::capture(
        &task,
        Amendment {
            depends_on: Some(vec!["owner-dependency".into()]),
            ..Default::default()
        },
        Origin::Owner,
    ));
    app.store.save_task(&task).unwrap();
    let mut queued = app.store.load_task(&task.id).unwrap().unwrap();
    queued.fixture_data().pending_amendment = signals.pending_amend.lock().unwrap().clone();
    app.store.save_task_with_pending(&queued, None).unwrap();
    let settings = app.settings.read().unwrap().clone();
    assert!(matches!(
        publish(&app, &mut task, proposal, &settings, &signals)
            .await
            .unwrap(),
        Publication::Retry
    ));
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.depends_on, vec!["owner-dependency"]);
    assert_eq!(saved.goal, "Do the thing");
    assert_eq!(saved.attempts[0].status, AttemptStatus::Interrupted);
    assert!(signals.pending_amend.lock().unwrap().is_none());
    assert!(!saved
        .decisions
        .iter()
        .any(|line| line.contains("Amendment not applied")));
}

#[tokio::test]
async fn a_fresh_planner_proposal_installs_without_losing_the_already_admitted_dependency() {
    let (app, _dir) = test_app();
    let mut task = task();
    task.fixture_data().depends_on = vec!["owner-dependency".into()];
    let proposal = proposal(&task, "[]", "[]");
    app.store.save_task(&task).unwrap();
    let settings = app.settings.read().unwrap().clone();
    assert!(matches!(
        publish(&app, &mut task, proposal, &settings, &TaskSignals::new())
            .await
            .unwrap(),
        Publication::Installed(_)
    ));
    assert_eq!(task.depends_on, vec!["owner-dependency"]);
    assert_eq!(task.goal, "Proposed goal");
    assert_eq!(task.attempts[0].status, AttemptStatus::Passed);
}

#[tokio::test]
async fn a_correction_result_cannot_replace_a_newer_owner_contract() {
    let (app, _dir) = test_app();
    let mut task = task();
    let proposal = proposal(&task, "[]", "[]");
    let mut newer = task.clone();
    newer.fixture_data().criteria = vec!["Owner criterion -- check: ./owner-check".into()];
    app.store.save_task(&newer).unwrap();
    let settings = app.settings.read().unwrap().clone();
    assert!(matches!(
        publish(&app, &mut task, proposal, &settings, &TaskSignals::new())
            .await
            .unwrap(),
        Publication::Retry
    ));
    assert_eq!(task.criteria, newer.criteria);
    assert_eq!(task.goal, newer.goal);
}

#[tokio::test]
async fn an_owner_stopped_task_is_unchanged_by_a_returning_plan() {
    let (app, _dir) = test_app();
    let mut task = task();
    let proposal = proposal(&task, "[]", "[]");
    let mut stopped = task.clone();
    stopped.resume_phase(TaskStatus::Stopped, 42);
    app.store.save_task(&stopped).unwrap();
    let settings = app.settings.read().unwrap().clone();
    assert!(matches!(
        publish(&app, &mut task, proposal, &settings, &TaskSignals::new())
            .await
            .unwrap(),
        Publication::Ended
    ));
    assert_eq!(
        serde_json::to_value(task).unwrap(),
        serde_json::to_value(stopped).unwrap()
    );
}

#[tokio::test]
async fn a_planner_cannot_remove_an_owner_repo_or_unknown_check_but_may_move_it_to_finals() {
    for source in [Some(CheckSource::Owner), Some(CheckSource::Repo), None] {
        let (app, _dir) = test_app();
        let mut task = task();
        task.fixture_data().verify = vec!["./shared-check".into()];
        if let Some(source) = source {
            task.fixture_data()
                .planner_tests
                .sources
                .push(SourcedCheck {
                    command: "./shared-check".into(),
                    source,
                });
        }
        app.store.save_task(&task).unwrap();
        let settings = app.settings.read().unwrap().clone();
        let rejected = proposal(&task, "[]", "[]");
        assert!(matches!(
            publish(&app, &mut task, rejected, &settings, &TaskSignals::new())
                .await
                .unwrap(),
            Publication::Ended
        ));
        assert_eq!(task.verify, vec!["./shared-check"]);
        assert_eq!(task.status, TaskStatus::Stopped);
        task.resume_phase(TaskStatus::Drafting, 43);
        app.store.save_task(&task).unwrap();
        let moved = proposal(&task, "[]", "[\"./shared-check\"]");
        assert!(matches!(
            publish(&app, &mut task, moved, &settings, &TaskSignals::new())
                .await
                .unwrap(),
            Publication::Installed(_)
        ));
        assert!(task.verify.is_empty());
        assert_eq!(task.final_verify, vec!["./shared-check"]);
    }
}
