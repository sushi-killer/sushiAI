use super::*;

#[tokio::test]
async fn answer_judge_timed_out_leaves_the_decision_to_the_owner() {
    let (app, _dir, mut task, _) = slow_helper_task();
    let criteria = task.criteria.clone();
    let claim = crate::engine::attempts::report::Impossible {
        criterion: 0,
        evidence: "the file is gone".into(),
    };
    let question = impossible_question(&task, &claim);
    task.ask(question, crate::clock::now_ms());
    assert!(matches!(
        try_policy_answer(&app, &mut task, &CancelToken::new()).await,
        super::publication::PolicyResult::Open
    ));
    assert_eq!(task.criteria, criteria);
    assert!(task.assumptions.is_empty());
    assert_helper_timeout(&task, "answer judge");
}
