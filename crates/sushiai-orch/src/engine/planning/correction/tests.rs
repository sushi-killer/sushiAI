use super::*;
use crate::engine::test_support::{attempt_with_failure, task_with_status, test_app};
use crate::task::amendment::Amendment;
use crate::task::checks::authority::{CheckAuthority, Origin};
use crate::task::TaskStatus;

#[test]
fn a_stopped_or_pending_task_cannot_admit_a_resumed_planner_attempt() {
    for stopped in [true, false] {
        let (app, _dir) = test_app();
        let mut task = task_with_status(TaskStatus::Drafting);
        let mut previous = attempt_with_failure(1, "earlier plan");
        previous.stage = Stage::Plan;
        previous.session_id = Some("saved-planner".into());
        task.fixture_data().attempts = vec![previous];
        app.store.save_task(&task).unwrap();
        if stopped {
            task.resume_phase(TaskStatus::Stopped, 2);
            app.store.save_task(&task).unwrap();
        } else {
            task.queue_pending_amendment(
                Amendment {
                    criteria: Some(vec!["Owner's queued acceptance".into()]),
                    ..Default::default()
                },
                Origin::Owner,
                &CheckAuthority::default(),
                2,
            )
            .unwrap();
            app.store.save_task_with_pending(&task, None).unwrap();
        }
        let route = app.settings.read().unwrap().routes[0].clone();
        let result = PlannerCorrection::resume(&app, &task.id, 0, &route);
        assert!(matches!(
            (stopped, result),
            (true, Err(Uncorrected::Ended)) | (false, Err(Uncorrected::Superseded))
        ));
        let saved = app.store.load_task(&task.id).unwrap().unwrap();
        assert_eq!(
            serde_json::to_value(saved).unwrap(),
            serde_json::to_value(task).unwrap()
        );
    }
}
