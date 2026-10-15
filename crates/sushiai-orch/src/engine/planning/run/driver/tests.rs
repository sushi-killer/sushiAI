use super::*;
use crate::engine::planning::draft::parse_plan;
use crate::engine::planning::publication::PlanProposal;
use crate::engine::test_support::{task_with_status, test_app_fitting_sandbox};
use crate::task::checks::snapshot::CheckSnapshot;
use crate::task::{AttemptStatus, TaskStatus};

fn review_round(
    app: &Arc<App>,
    directory: &tempfile::TempDir,
) -> (Box<PlannerRound>, Box<PlanProposal>) {
    let settings = {
        let mut settings = app.settings.write().unwrap();
        settings.brief_check_route.clear();
        settings.plan_review.clear();
        settings.planner.clear();
        settings.review.clear();
        settings.clone()
    };
    let mut task = task_with_status(TaskStatus::Drafting);
    task.fixture_data().worktree = directory.path().to_string_lossy().into_owned();
    task.fixture_data().repo = task.worktree.clone();
    task.fixture_data().request = Some("Create the requested change".into());
    app.store.save_task(&task).unwrap();
    let based_on = CheckSnapshot::capture(&task);
    let draft = parse_plan("```sushi-plan\n{\"title\":\"Accepted plan\",\"goal\":\"Implement it\",\"criteria\":[\"It works\"],\"verify\":[\"true\"]}\n```").unwrap();
    let route = settings.routes[0].clone();
    let round = Box::new(PlannerRound::new(app, task, route, settings));
    let proposal = Box::new(PlanProposal {
        based_on,
        draft,
        attempt: round.index,
    });
    (round, proposal)
}

fn initial(flow: &PlanningFlow) -> PlanningState {
    PlanningInput::Admission
        .enter(flow.entry())
        .unwrap_or_else(|error| panic!("{error}"))
}

#[tokio::test]
async fn real_review_questions_split_command_and_finish_follow_the_yaml_edges() {
    for auto_start in [false, true] {
        let (app, directory) = test_app_fitting_sandbox();
        let (round, proposal) = review_round(&app, &directory);
        let id = round.task.id.clone();
        let flow = PlanningFlow::compiled().unwrap();
        let mut session = PlanningSession::new(
            &id,
            (round.route.id.clone(), "selected".into()),
            auto_start,
            flow,
        );
        let state = PlanningState::Running(PlanningOperation::Review { round, proposal });
        let result = session
            .drive(&app, &TaskSignals::new(), &mut None, state)
            .await;
        assert!(matches!(
            (auto_start, result),
            (true, PlanOutcome::Proceed) | (false, PlanOutcome::StoppedForReview)
        ));
        assert_eq!(
            session.trace,
            vec![
                (
                    PlanningStep::Review,
                    PlanningOutcome::Ready,
                    PlanningStep::Questions
                ),
                (
                    PlanningStep::Questions,
                    PlanningOutcome::Ready,
                    PlanningStep::Split
                ),
                (
                    PlanningStep::Split,
                    PlanningOutcome::Ready,
                    PlanningStep::Command
                ),
                (
                    PlanningStep::Command,
                    PlanningOutcome::Ready,
                    PlanningStep::Finish
                ),
                (
                    PlanningStep::Finish,
                    PlanningOutcome::Complete,
                    PlanningStep::Return
                ),
            ]
        );
        let saved = app.store.load_task(&id).unwrap().unwrap();
        assert_eq!(saved.title, "Accepted plan");
        assert_eq!(saved.criteria, ["It works"]);
        assert_eq!(saved.verify, ["true"]);
        assert_eq!(saved.attempts[0].status, AttemptStatus::Passed);
        assert_eq!(
            saved.status,
            if auto_start {
                TaskStatus::Queued
            } else {
                TaskStatus::Stopped
            }
        );
    }
}

#[tokio::test]
async fn cancellation_routes_through_existing_cleanup_before_any_planner_attempt() {
    let (app, _directory) = test_app_fitting_sandbox();
    let task = task_with_status(TaskStatus::Drafting);
    app.store.save_task(&task).unwrap();
    let signals = TaskSignals::new();
    signals.cancel.cancel();
    let flow = PlanningFlow::compiled().unwrap();
    let mut session = PlanningSession::new(&task.id, (String::new(), String::new()), true, flow);
    let result = session
        .drive(&app, &signals, &mut None, initial(flow))
        .await;
    assert!(matches!(result, PlanOutcome::Ended));
    assert_eq!(
        session.trace,
        vec![
            (
                PlanningStep::Prepare,
                PlanningOutcome::Stopped,
                PlanningStep::Cleanup
            ),
            (
                PlanningStep::Cleanup,
                PlanningOutcome::Complete,
                PlanningStep::Return
            ),
        ]
    );
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.status, TaskStatus::Stopped);
    assert!(saved.attempts.is_empty());
}

#[tokio::test]
async fn owner_pending_during_review_routes_to_admission_without_installing_the_old_plan() {
    use crate::engine::persistence::contract::tests::queue_owner;
    let (app, directory) = test_app_fitting_sandbox();
    let (round, proposal) = review_round(&app, &directory);
    let id = round.task.id.clone();
    queue_owner(&app, &round.task);
    let flow = PlanningFlow::compiled().unwrap();
    let mut session =
        PlanningSession::new(&id, (round.route.id.clone(), "selected".into()), true, flow);
    let transfer = PlanningOperation::Review { round, proposal }
        .execute(&mut session, &app, &TaskSignals::new(), &mut None)
        .await;
    assert_eq!(transfer.outcome, PlanningOutcome::Retry);
    let (next, state) = transfer
        .route(flow, PlanningStep::Review)
        .unwrap_or_else(|(_, error)| panic!("{error}"));
    assert_eq!(next, PlanningStep::Prepare);
    assert!(matches!(
        state,
        PlanningState::Running(PlanningOperation::Prepare)
    ));
    let saved = app.store.load_task(&id).unwrap().unwrap();
    assert_eq!(saved.criteria, ["Owner's accepted criterion"]);
    assert!(saved.pending_amendment.is_none());
    assert_ne!(saved.title, "Accepted plan");
    assert_eq!(saved.attempts[0].status, AttemptStatus::Interrupted);
}

#[tokio::test]
async fn the_existing_clarification_limit_returns_through_the_yaml_terminal() {
    let (app, directory) = test_app_fitting_sandbox();
    let (round, _proposal) = review_round(&app, &directory);
    let id = round.task.id.clone();
    let flow = PlanningFlow::compiled().unwrap();
    let mut session = PlanningSession::new(
        &id,
        (round.route.id.clone(), "selected".into()),
        false,
        flow,
    );
    session.clarify_rounds = 2;
    let result = session
        .drive(
            &app,
            &TaskSignals::new(),
            &mut None,
            PlanningState::Running(PlanningOperation::Clarify(round)),
        )
        .await;
    assert!(matches!(result, PlanOutcome::Ended));
    assert_eq!(
        session.trace,
        vec![(
            PlanningStep::Clarify,
            PlanningOutcome::Complete,
            PlanningStep::Return
        )]
    );
    let saved = app.store.load_task(&id).unwrap().unwrap();
    assert_eq!(saved.status, TaskStatus::Failed);
    assert_eq!(
        saved.attempts[0].failure.as_ref().unwrap().kind,
        crate::task::FailureKind::NoDeliverable
    );
}

#[test]
fn incompatible_runtime_resources_fail_before_an_operation_can_execute() {
    assert!(PlanningInput::Admission
        .enter(PlanningStep::Review)
        .is_err());
    assert!(PlanningInput::Outcome(PlanOutcome::Proceed)
        .enter(PlanningStep::Finish)
        .is_err());
}
