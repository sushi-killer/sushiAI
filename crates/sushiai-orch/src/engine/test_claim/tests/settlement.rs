use super::complete;
use crate::engine::questions::parking::{QuestionSlot, WaitEnd};
use crate::engine::run_control::TaskSignals;
use crate::engine::test_support::{attempt_with_failure, task_with_status, test_app};
use crate::task::{AskedBy, Question, QuestionKind, TaskStatus};
use std::time::Duration;

#[tokio::test]
async fn a_contract_change_or_stop_keeps_the_completed_claim_helpers_cost_once() {
    for stopped in [false, true] {
        let (app, _dir) = test_app();
        app.settings.write().unwrap().answer_policy = true;
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().criteria = vec!["Original contract".into()];
        task.fixture_data().attempts = vec![attempt_with_failure(1, "verify:failed")];
        app.store.save_task(&task).unwrap();
        let text = complete(
            &app,
            &mut task,
            crate::harness::RunOutcome {
                cost_usd: Some(0.4),
                final_text: Some("claim ruling".into()),
                ..Default::default()
            },
            "test-judge",
            Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(text, "claim ruling");
        assert_eq!(
            app.store.load_task(&task.id).unwrap().unwrap().cost_usd,
            0.4
        );
        task.ask(
            Question::new(
                "The review gave no verdict",
                vec!["retry".into(), "stop".into()],
                QuestionKind::ReviewNoVerdict,
                AskedBy::Implement,
                1,
            ),
            1,
        );
        let mut fresh = app.store.load_task(&task.id).unwrap().unwrap();
        if stopped {
            fresh.resume_phase(TaskStatus::Stopped, 2);
        } else {
            fresh.fixture_data().criteria = vec!["Owner's new contract".into()];
        }
        app.store.save_task(&fresh).unwrap();
        let signals = TaskSignals::new();
        let mut permit = None;
        let mut slot = QuestionSlot::new(&signals, &mut permit);
        let result = slot.answer(&app, &mut task).await;
        assert_eq!(
            result,
            Err(if stopped {
                WaitEnd::Stopped
            } else {
                WaitEnd::Changed
            })
        );
        let saved = app.store.load_task(&task.id).unwrap().unwrap();
        assert_eq!(saved.criteria, fresh.criteria);
        assert_eq!(saved.status, fresh.status);
        assert_eq!(saved.cost_usd, 0.4);
        assert_eq!(task.cost_usd, 0.4);
        assert!(saved.assumptions.is_empty());
        assert!(saved.question_history.is_empty());
    }
}
