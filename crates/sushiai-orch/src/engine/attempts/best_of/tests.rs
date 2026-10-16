use crate::engine::attempts::best_of::candidate::{
    CandidateEvidence, CandidateRun, EvaluatedCandidates,
};
use crate::engine::attempts::best_of::evaluate::disqualify_restored;
use crate::engine::attempts::best_of::evaluate::Eval;
use crate::engine::attempts::best_of::pick::run_pick;
use crate::engine::attempts::best_of::run::BestOfRun;
use crate::engine::attempts::best_of::sibling::create_sibling;
use crate::engine::attempts::prepare::ImplementRun;
use crate::engine::decision::implement_attempt_count;
use crate::engine::test_support::attempt_with_failure;
use crate::engine::test_support::init_git_repo;
use crate::engine::test_support::slow_helper_task;
use crate::engine::test_support::task_with_status;
use crate::harness::cancel::CancelToken;
use crate::policy::best_of::second_route;
use crate::policy::routing::Harness;
use crate::policy::routing::Tier;
use crate::policy::settings::Settings;
use crate::policy::variant::Variant;
use crate::task::attempt::Attempt;
use crate::task::attempt::CheckTally;
use crate::task::attempt::Stage;
use crate::task::checks::types::PlannedTest;
use crate::task::state::TaskStatus;
use std::path::PathBuf;

#[cfg(test)]
mod route_tests {
    use super::{
        attempt_with_failure, implement_attempt_count, run_pick, second_route, slow_helper_task,
        task_with_status, Attempt, BestOfRun, CancelToken, CandidateEvidence, CandidateRun,
        CheckTally, Eval, EvaluatedCandidates, Harness, ImplementRun, PathBuf, Settings, Stage,
        TaskStatus, Tier, Variant,
    };

    #[tokio::test]
    async fn best_of_pick_timed_out_keeps_the_normal_no_answer_path() {
        let (app, _dir, task, route) = slow_helper_task();
        let mut settings = app.settings.read().unwrap().clone();
        settings.review = route.id.clone();
        let worktree = PathBuf::from(&task.worktree);
        let candidate = || CandidateEvidence {
            run: CandidateRun {
                route: route.clone(),
                worktree: worktree.clone(),
                run_dir: app.store.run_dir(&task.id, 1),
                outcome: Ok(crate::harness::RunOutcome::default()),
                cost: 0.0,
            },
            checks: Eval {
                passed: true,
                verify_ok: true,
                failing: 0,
                verify: vec![],
                tally: CheckTally::default(),
            },
            restored: vec![],
        };
        let candidates = EvaluatedCandidates {
            a: candidate(),
            b: candidate(),
            decisions: vec![],
        };
        let run_dir = app.store.run_dir(&task.id, 1);
        let run = BestOfRun {
            primary: ImplementRun {
                task: &task,
                route: &route,
                settings: &settings,
                worktree: &worktree,
                attempt_n: 1,
                run_dir: &run_dir,
            },
            secondary: &route,
            brief: "",
            deny_read: &[],
        };
        let mut decisions = Vec::new();
        let (pick, _) = run_pick(&app, &run, &candidates, &mut decisions, &CancelToken::new())
            .await
            .ok()
            .expect("the picker must start");
        assert!(pick.is_none());
        assert_eq!(decisions, ["best-of pick timed out after 0.25s"]);
        assert_eq!(
            app.store.helper_run_dirs(&task.id, "pick", Some(1)).len(),
            1
        );
        assert!(!app.store.run_dir(&task.id, 1).join("pick").exists());
    }

    #[test]
    fn best_of_first_implement_attempt_ignores_plan_numbers() {
        let settings = Settings::default();
        let route = settings
            .routes
            .iter()
            .find(|r| r.harness == Harness::Claude)
            .unwrap();
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().tier = Tier::Hard;
        task.apply_variant(Variant {
            best_of: 2,
            ..Default::default()
        });
        task.fixture_data().attempts = vec![Attempt {
            stage: Stage::Plan,
            ..attempt_with_failure(1, "plan")
        }];
        task.fixture_data()
            .attempts
            .push(attempt_with_failure(2, "first implement"));
        assert_eq!(
            second_route(&task, &settings, route, implement_attempt_count(&task) == 1)
                .unwrap()
                .unwrap()
                .harness,
            Harness::Codex
        );
        task.fixture_data()
            .attempts
            .push(attempt_with_failure(3, "second implement"));
        assert!(
            second_route(&task, &settings, route, implement_attempt_count(&task) == 1)
                .unwrap()
                .is_none()
        );
    }
}

#[cfg(test)]
mod disqualify_tests {
    use super::{
        create_sibling, disqualify_restored, init_git_repo, task_with_status, CheckTally, Eval,
        PlannedTest, TaskStatus,
    };

    fn passed_eval() -> Eval {
        Eval {
            passed: true,
            verify_ok: true,
            failing: 0,
            verify: vec![],
            tally: CheckTally::default(),
        }
    }

    #[test]
    fn a_candidate_whose_protected_files_were_restored_does_not_count_as_passed() {
        let mut clean = passed_eval();
        let mut touched = passed_eval();
        let mut decisions = Vec::new();
        disqualify_restored(&mut clean, &[], "a", &mut decisions);
        disqualify_restored(
            &mut touched,
            &["tests/t.test".to_string()],
            "b",
            &mut decisions,
        );
        assert!(clean.passed && clean.failing == 0);
        assert!(!touched.passed && touched.failing >= 1);
        assert_eq!(decisions.len(), 1);
        assert!(decisions[0].contains("candidate b") && decisions[0].contains("tests/t.test"));
    }

    #[test]
    fn both_candidate_worktrees_get_the_planner_tests() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_git_repo(&repo);
        let base = crate::git::history::head_sha(&repo).unwrap();
        let files = vec![PlannedTest {
            path: "tests/planner.txt".into(),
            content: "expect\n".into(),
        }];
        let b = dir.path().join("task-b");
        create_sibling(&repo, "task/b", &b, &base, &files).unwrap();
        assert_eq!(
            std::fs::read_to_string(b.join("tests/planner.txt")).unwrap(),
            "expect\n"
        );
        assert_eq!(
            crate::git::history::head_sha(&b).unwrap(),
            base,
            "the tests are not a commit"
        );
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().base_sha = base;
        task.fixture_data().planner_tests.files = files;
        std::fs::write(b.join("impl.txt"), "work\n").unwrap();
        assert_eq!(task.own_diff().changed_files(&b).unwrap(), vec!["impl.txt"]);
    }
}
