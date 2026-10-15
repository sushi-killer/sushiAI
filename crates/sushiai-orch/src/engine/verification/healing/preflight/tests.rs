use super::run_base_preflight;
use super::spawn_base_preflight;
use crate::engine::state::App;
use crate::engine::test_support::assert_helper_timeout;
use crate::engine::test_support::attempt_with_failure;
use crate::engine::test_support::slow_helper_task;
use crate::engine::test_support::task_with_status;
use crate::engine::test_support::test_app;
use crate::engine::verification::healing::answers::heal_answers;
use crate::engine::verification::healing::repeated::heal_repeated_unmet;
use crate::harness::cancel::CancelToken;
use crate::task::review::ReviewResult;
use crate::task::state::TaskStatus;
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn a_rewritten_verify_starts_the_preflight_again_for_the_new_commands() {
    let (app, _dir) = test_app();
    let mut task = task_with_status(TaskStatus::Running);
    task.fixture_data().verify = vec!["echo one".into()];
    let cancel = CancelToken::new();
    let id = task.id.clone();
    let job = |app: &Arc<App>| app.base_preflights.lock().unwrap()[&id].clone();
    spawn_base_preflight(&app, &task, &cancel);
    let first = job(&app);
    // The same commands again: the job is not started twice.
    spawn_base_preflight(&app, &task, &cancel);
    assert!(Arc::ptr_eq(&first, &job(&app)));
    // A rewrite replaces the job and stops the old one.
    task.fixture_data().verify = vec!["echo two".into()];
    spawn_base_preflight(&app, &task, &cancel);
    let second = job(&app);
    assert!(!Arc::ptr_eq(&first, &second));
    assert!(second.snapshot.matches(&task));
    assert!(first.cancel.is_cancelled());
    assert!(!second.cancel.is_cancelled());
}
#[cfg(test)]
mod timeout_tests {
    use super::*;

    #[tokio::test]
    async fn feasibility_timed_out_keeps_the_command_unchanged() {
        let (app, _dir, mut task, _) = slow_helper_task();
        task.fixture_data().verify = vec!["false".into()];
        let result = run_base_preflight(&app, &task, &CancelToken::new())
            .await
            .unwrap();
        assert!(result.settled);
        assert!(result.rulings.is_empty());
        task.record_decisions(result.decisions);
        assert_eq!(task.verify, ["false"]);
        assert_helper_timeout(&task, "feasibility");
        let costs = crate::store::costs::read_all(&app.data_dir);
        assert_eq!(costs.len(), 1);
        assert!(costs[0].attempt.is_none());
    }

    #[tokio::test]
    async fn criterion_judge_timed_out_keeps_the_criterion() {
        let (app, _dir, mut task, _) = slow_helper_task();
        let review: ReviewResult = serde_json::from_value(
            json!({"verdict":"FAIL", "criteria":[{"criterion":"Keep the API","index":0,"met":false,"evidence":"missing"}]}),
        )
        .unwrap();
        task.fixture_data().attempts[0].review = Some(review.clone());
        task.fixture_data()
            .attempts
            .push(attempt_with_failure(2, "failure"));
        let criteria = task.criteria.clone();
        assert!(matches!(
            heal_repeated_unmet(&app, &mut task, 1, &review, &CancelToken::new()).await,
            Ok(false)
        ));
        assert_eq!(task.criteria, criteria);
        assert!(task.assumptions.is_empty());
        assert_helper_timeout(&task, "criterion-judge");
    }

    #[tokio::test]
    async fn answer_check_timed_out_keeps_the_criteria() {
        let (app, _dir, mut task, _) = slow_helper_task();
        task.record_decision("Owner: Which API? -> Remove it".into());
        app.store.save_task(&task).unwrap();
        let criteria = task.criteria.clone();
        assert!(matches!(
            heal_answers(&app, &mut task, 1, &CancelToken::new()).await,
            Ok(crate::engine::verification::healing::answers::AnswerHealing::Ready)
        ));
        assert_eq!(task.criteria, criteria);
        assert!(task.assumptions.is_empty());
        assert_helper_timeout(&task, "answer-check");
    }
}
