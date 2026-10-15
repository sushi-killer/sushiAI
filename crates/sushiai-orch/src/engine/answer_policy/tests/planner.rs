use super::publication::PolicyResult;
use super::*;
use crate::engine::answer_policy::planner::{
    parse_planner_verdict, planner_check_criterion, PlannerVerdict,
};
use crate::task::{Baseline, Check};
fn sourced(task: &mut Task, command: &str, source: CheckSource) {
    task.fixture_data()
        .planner_tests
        .sources
        .push(SourcedCheck {
            command: command.to_string(),
            source,
        });
}

#[test]
fn a_planner_check_verdict_reads_drop_rewrite_and_retry() {
    assert_eq!(
        parse_planner_verdict("{\"verdict\": \"drop\", \"reason\": \"gone\"}"),
        Some(PlannerVerdict::Drop("gone".into()))
    );
    assert_eq!(
        parse_planner_verdict("```json\n{\"verdict\": \"Retry\"}\n```"),
        Some(PlannerVerdict::Retry("no reason given".into()))
    );
    assert_eq!(
        parse_planner_verdict(
            "{\"verdict\": \"rewrite\", \"command\": \" grep -q  x a.txt \", \"criterionText\": \"\", \"reason\": \"renamed\"}"
        ),
        Some(PlannerVerdict::Rewrite {
            command: "grep -q  x a.txt".into(),
            criterion_text: String::new(),
            reason: "renamed".into(),
        })
    );
    // A rewrite without a command, an unknown verdict or no JSON: unusable.
    assert!(parse_planner_verdict("{\"verdict\": \"rewrite\", \"command\": \" \"}").is_none());
    assert!(parse_planner_verdict("{\"verified\": true}").is_none());
    assert!(parse_planner_verdict("no idea").is_none());
}

#[test]
fn only_a_criterion_whose_every_check_is_the_planners_qualifies() {
    let settings = Settings::default();
    let mut task = task_with_status(TaskStatus::Waiting);
    task.fixture_data().criteria = vec![
        "zero".into(),
        "one -- check: grep -q one a.txt".into(),
        "two".into(),
    ];
    sourced(&mut task, "grep -q one a.txt", CheckSource::Planner);
    // No check at all: not settled here.
    assert!(!planner_check_criterion(&settings, &task, 0));
    assert!(planner_check_criterion(&settings, &task, 1));
    task.fixture_data().checks.push(Check {
        criterion: 2,
        run: "test -f b.txt".into(),
        baseline: Some(Baseline::Fail),
    });
    sourced(&mut task, "test -f b.txt", CheckSource::Planner);
    assert!(planner_check_criterion(&settings, &task, 2));
    // One repo or owner command among them keeps the owner's path.
    task.fixture_data().held_out = Some(Check {
        criterion: 2,
        run: "npm test".into(),
        baseline: Some(Baseline::Fail),
    });
    sourced(&mut task, "npm test", CheckSource::Repo);
    assert!(!planner_check_criterion(&settings, &task, 2));
    task.fixture_data().planner_tests.sources[0].source = CheckSource::Owner;
    assert!(!planner_check_criterion(&settings, &task, 1));
}

/// A waiting task whose criterion 0 has only a check of `source`, the
/// policy off, and an impossible claim on that criterion.
fn claimed(source: CheckSource) -> (Arc<App>, tempfile::TempDir, Task) {
    let (app, dir, mut task, _) = slow_helper_task();
    app.settings.write().unwrap().answer_policy = false;
    task.fixture_data().criteria = vec!["Keep the API -- check: grep -q api api.txt".into()];
    task.fixture_data().checks = vec![Check {
        criterion: 0,
        run: "grep -q api api.txt".into(),
        baseline: Some(Baseline::Fail),
    }];
    task.fixture_data().planner_tests.sources = vec![SourcedCheck {
        command: "grep -q api api.txt".into(),
        source,
    }];
    let claim = crate::engine::attempts::report::Impossible {
        criterion: 0,
        evidence: "the design has no api.txt".into(),
    };
    task.fixture_data().question = Some(impossible_question(&task, &claim));
    task.fixture_data().status = TaskStatus::Waiting;
    app.store.save_task(&task).unwrap();
    (app, dir, task)
}

fn only_answer(task: &Task) -> &Assumption {
    assert_eq!(task.assumptions.len(), 1, "{:?}", task.assumptions);
    &task.assumptions[0]
}

#[tokio::test]
async fn a_planner_check_without_a_judge_route_is_dropped_by_policy() {
    let (app, _dir, mut task) = claimed(CheckSource::Planner);
    app.settings.write().unwrap().brief_check_route = String::new();
    let answer = try_policy_answer(&app, &mut task, &CancelToken::new()).await;
    assert!(matches!(answer, PolicyResult::Answered(ref answer) if answer == "drop criterion"));
    assert!(task.question.is_none());
    assert_eq!(task.status, TaskStatus::Queued);
    let a = only_answer(&task);
    assert_eq!(
        (a.by.as_str(), a.kind),
        ("policy", Some(QuestionKind::Impossible))
    );
    assert!(!a.overturned);
    assert!(a.evidence.contains("no judge route"), "{}", a.evidence);
    assert_eq!(task.loop_state.impossible_answers, vec![0]);
    assert_eq!(
        crate::engine::chat::test_support::fake_overseer_calls(Path::new(&task.worktree)),
        0
    );
}

#[tokio::test]
async fn a_planner_check_whose_judge_gives_no_reply_is_dropped_by_policy() {
    // The fake judge is slow: it times out with no reply.
    let (app, _dir, mut task) = claimed(CheckSource::Planner);
    let answer = try_policy_answer(&app, &mut task, &CancelToken::new()).await;
    assert!(matches!(answer, PolicyResult::Answered(ref answer) if answer == "drop criterion"));
    let a = only_answer(&task);
    assert_eq!(a.by, "policy");
    assert!(
        a.evidence.contains("written by the planner"),
        "{}",
        a.evidence
    );
    assert_helper_timeout(&task, "answer judge");
}

#[tokio::test]
async fn a_planner_check_whose_answer_budget_is_spent_is_dropped_without_a_judge() {
    let (app, _dir, mut task) = claimed(CheckSource::Planner);
    let question = task.question.clone().unwrap();
    for _ in 0..MAX_POLICY_ANSWERS {
        record_policy_answer(
            &mut task,
            &question,
            &AutomaticAnswer::Judge {
                text: ("retry").to_string(),
                evidence: ("e").to_string(),
            },
            1,
        );
    }
    let answer = try_policy_answer(&app, &mut task, &CancelToken::new()).await;
    assert!(matches!(answer, PolicyResult::Answered(ref answer) if answer == "drop criterion"));
    let last = task.assumptions.last().unwrap();
    assert_eq!(last.by, "policy");
    assert!(last.evidence.contains("spent"), "{}", last.evidence);
    assert_eq!(
        crate::engine::chat::test_support::fake_overseer_calls(Path::new(&task.worktree)),
        0
    );
}

#[tokio::test]
async fn a_repo_or_owner_check_is_left_to_the_owner() {
    for source in [CheckSource::Repo, CheckSource::Owner] {
        let (app, _dir, mut task) = claimed(source);
        app.settings.write().unwrap().brief_check_route = String::new();
        assert!(matches!(
            try_policy_answer(&app, &mut task, &CancelToken::new()).await,
            PolicyResult::Open
        ));
        assert!(task.question.is_some());
        assert!(task.assumptions.is_empty());
    }
}
