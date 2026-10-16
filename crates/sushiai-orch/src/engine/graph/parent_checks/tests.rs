use super::checked_for_delivery;
use crate::delivery::pr::validation;
use crate::engine::run_control::TaskSignals;
use crate::engine::state::App;
use crate::engine::test_support::{
    git_in, init_git_repo, task_with_status, test_app_fitting_sandbox,
};
use crate::task::checks::types::{Baseline, Check};
use crate::task::{Task, TaskStatus};
use std::collections::BTreeMap;
use std::path::Path;

fn parent(app: &App, repo: &Path) -> Task {
    std::fs::create_dir_all(repo).unwrap();
    init_git_repo(repo);
    let mut task = task_with_status(TaskStatus::Running);
    let data = task.fixture_data();
    data.repo = repo.display().to_string();
    data.worktree = data.repo.clone();
    data.base_sha = git_in(repo, &["rev-parse", "HEAD"]);
    data.branch = git_in(repo, &["rev-parse", "--abbrev-ref", "HEAD"]);
    app.store.save_task(&task).unwrap();
    task
}

async fn admitted(app: &std::sync::Arc<App>, task: &mut Task) -> Task {
    match checked_for_delivery(app, task, &TaskSignals::new()).await {
        Ok(task) => task,
        Err(_) => panic!("parent checks did not admit delivery"),
    }
}

#[tokio::test]
async fn a_parent_without_an_attempt_records_every_actual_check_for_pr_validation() {
    let (app, dir) = test_app_fitting_sandbox();
    let repo = dir.path().join("repo");
    let mut task = parent(&app, &repo);
    let command = "printf 'criterion receipt\\n'";
    let held = "printf 'held receipt\\n'";
    let data = task.fixture_data();
    data.criteria = vec!["Keep the existing contract".into()];
    data.verify = vec!["printf 'verify receipt\\n'".into()];
    data.final_verify = vec!["printf 'final receipt\\n'".into()];
    data.checks = vec![Check {
        criterion: 0,
        run: command.into(),
        baseline: Some(Baseline::Fail),
    }];
    data.held_out = Some(Check {
        criterion: 0,
        run: held.into(),
        baseline: Some(Baseline::Fail),
    });
    app.store.save_task(&task).unwrap();
    let commit = task.base_sha.clone();
    let tree = git_in(&repo, &["rev-parse", "HEAD^{tree}"]);
    let ready = admitted(&app, &mut task).await;
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    for checked in [&ready, &saved] {
        assert!(checked.attempts.is_empty());
        assert_eq!(checked.delivery_checks.len(), 4);
        let validation = validation::section(checked, &BTreeMap::new());
        assert!(!validation.contains("No check results"), "{validation}");
        for result in &checked.delivery_checks {
            assert_eq!(result.code, Some(0), "{result:?}");
            assert_eq!(result.commit.as_deref(), Some(commit.as_str()));
            assert_eq!(result.tree.as_deref(), Some(tree.as_str()));
            assert!(validation.contains(&result.command), "{validation}");
        }
        assert!(
            validation.contains(&format!("checkout commit `{commit}`")),
            "{validation}"
        );
        assert!(
            validation.contains(&format!("checked tree `{tree}`")),
            "{validation}"
        );
        assert!(checked
            .delivery_checks
            .iter()
            .any(|r| r.command == "held-out check (criterion 0)"));
        assert!(!validation.contains(held), "{validation}");
        assert!(checked
            .delivery_checks
            .iter()
            .all(|result| !result.tail.contains(held)));
    }
}

#[tokio::test]
async fn a_base_covered_parent_final_retains_its_raw_failure_and_typed_waiver() {
    let (app, dir) = test_app_fitting_sandbox();
    let repo = dir.path().join("repo");
    let mut task = parent(&app, &repo);
    let command = "printf 'not ok 1 - inherited_parent_case\\n'; exit 1";
    task.fixture_data().final_verify = vec![command.into()];
    app.store.save_task(&task).unwrap();
    let commit = task.base_sha.clone();
    let tree = git_in(&repo, &["rev-parse", "HEAD^{tree}"]);
    let ready = admitted(&app, &mut task).await;
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    for checked in [&ready, &saved] {
        assert!(checked.attempts.is_empty());
        assert_eq!(checked.delivery_checks.len(), 1);
        let raw = &checked.delivery_checks[0];
        assert_eq!(raw.command, command);
        assert_eq!(raw.code, Some(1));
        assert_eq!(raw.commit.as_deref(), Some(commit.as_str()));
        assert_eq!(raw.tree.as_deref(), Some(tree.as_str()));
        assert!(checked
            .assumptions
            .iter()
            .any(|a| a.commands == [command] && a.answer == crate::task::questions::WAIVED));
        let validation = validation::section(checked, &BTreeMap::new());
        assert!(validation.contains("failed (exit 1)"), "{validation}");
        let waived = crate::delivery::pr::body::waived_section(checked);
        assert!(waived.contains("inherited_parent_case"), "{waived}");
    }
}

#[test]
fn a_parent_check_judgment_does_not_replace_an_owner_revocation() {
    let (app, _dir) = crate::engine::test_support::test_app();
    let mut task = crate::engine::test_support::task_with_status(TaskStatus::Running);
    app.store.save_task(&task).unwrap();
    let input = crate::engine::persistence::contract::ContractInput::capture(&task);
    let waiver = crate::task::Assumption {
        question: "The command failed".into(),
        answer: crate::task::questions::WAIVED.into(),
        evidence: "The base has the same named failure".into(),
        by: crate::task::questions::WAIVER_BY.into(),
        kind: Some(crate::task::QuestionKind::PreexistingFailure),
        attempt: None,
        commands: vec!["check".into()],
        overturned: false,
        owner_answer: None,
    };
    task.record_assumption(waiver.clone());
    app.update_task(&task.id, |fresh| {
        fresh.record_assumption(waiver);
        fresh.overturn_assumption(0, "Keep this check".into());
    })
    .unwrap();
    let published = super::publish_waivers(
        &app,
        &task,
        &input,
        &["check".into()],
        &crate::harness::cancel::CancelToken::new(),
    );
    match published {
        Ok(crate::engine::persistence::contract::Publication::Applied(commands)) => {
            assert_eq!(commands, ["check"]);
        }
        Ok(crate::engine::persistence::contract::Publication::Deferred(
            crate::engine::questions::parking::WaitEnd::Changed,
        )) => {}
        _ => panic!("owner revocation must reject the stale waiver"),
    }
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.assumptions.len(), 1);
    assert!(saved.assumptions[0].overturned);
    assert_eq!(
        saved.assumptions[0].owner_answer.as_deref(),
        Some("Keep this check")
    );
    assert!(crate::delivery::pr::body::waived_section(&saved).is_empty());
}

struct BaseBarrier {
    entered: std::path::PathBuf,
    released: std::path::PathBuf,
}

impl BaseBarrier {
    async fn wait(&self) {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while !self.entered.exists() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the actual base command must reach its barrier");
    }

    fn release(&self) {
        std::fs::write(&self.released, "release").unwrap();
    }
}

impl Drop for BaseBarrier {
    fn drop(&mut self) {
        let _ = std::fs::write(&self.released, "release");
    }
}

fn planned_parent(app: &App, dir: &Path) -> (Task, BaseBarrier) {
    let repo = dir.join("repo");
    let mut task = parent(app, &repo);
    std::fs::write(
        repo.join("planned-parent-check.sh"),
        "printf 'not ok 1 - parent_planned_case\\n'; exit 1",
    )
    .unwrap();
    let barrier = BaseBarrier {
        entered: dir.join("base-entered"),
        released: dir.join("base-released"),
    };
    let command = format!(
        "if test -f planned-parent-check.sh; then sh planned-parent-check.sh; else touch '{}'; while ! test -f '{}'; do sleep 0.01; done; printf 'not ok 1 - parent_planned_case\\n'; exit 1; fi",
        barrier.entered.display(), barrier.released.display()
    );
    task.fixture_data().paths = vec!["planned-parent-check.sh".into()];
    task.fixture_data().final_verify = vec![command];
    app.store.save_task(&task).unwrap();
    (task, barrier)
}

async fn actual_failed_final(app: &App, task: &Task) -> Vec<crate::task::VerifyOutcome> {
    let cancel = crate::harness::cancel::CancelToken::new();
    let results = crate::engine::verification::runner::run_verify_commands(
        crate::engine::verification::runner::CheckRunner::new(
            app,
            Path::new(&task.worktree),
            None,
            &task.base_sha,
            &cancel,
        ),
        &task.final_verify,
    )
    .await;
    assert_eq!(results[0].code, Some(1));
    results
}

#[tokio::test]
async fn a_planned_keep_after_a_base_check_preserves_an_owner_revocation() {
    let (app, dir) = test_app_fitting_sandbox();
    let (mut task, barrier) = planned_parent(&app, dir.path());
    task.record_assumption(crate::task::Assumption {
        question: "May the inherited check be waived?".into(),
        answer: crate::task::questions::WAIVED.into(),
        evidence: "The base has the same failure".into(),
        by: crate::task::questions::WAIVER_BY.into(),
        kind: Some(crate::task::QuestionKind::PreexistingFailure),
        attempt: None,
        commands: task.final_verify.clone(),
        overturned: false,
        owner_answer: None,
    });
    app.store.save_task(&task).unwrap();
    let results = actual_failed_final(&app, &task).await;
    let (worker_app, mut checked) = (app.clone(), task.clone());
    let running = tokio::spawn(async move {
        let gate = crate::engine::graph::base::parent_finals_on_base(
            &worker_app,
            &mut checked,
            results,
            &TaskSignals::new(),
        )
        .await;
        (gate, checked)
    });
    barrier.wait().await;
    app.dispatch(
        "task.overturn",
        serde_json::json!({"id": task.id, "index": 0, "answer": "Keep this check"}),
    )
    .await
    .unwrap();
    barrier.release();
    let (gate, checked) = running.await.unwrap();
    assert!(matches!(
        gate,
        crate::engine::graph::base::ParentGate::Again
    ));
    assert!(checked.loop_state.kept_checks.is_empty());
    let mut checked = app.store.load_task(&task.id).unwrap().unwrap();
    let results = actual_failed_final(&app, &checked).await;
    let gate = crate::engine::graph::base::parent_finals_on_base(
        &app,
        &mut checked,
        results,
        &TaskSignals::new(),
    )
    .await;
    assert!(matches!(
        gate,
        crate::engine::graph::base::ParentGate::Go(_)
    ));
    assert!(checked.assumptions[0].overturned);
    app.save_task_logged(&checked);
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert!(saved.assumptions[0].overturned);
    assert_eq!(
        saved.assumptions[0].owner_answer.as_deref(),
        Some("Keep this check")
    );
    assert!(crate::delivery::pr::body::waived_section(&saved).is_empty());
    assert_eq!(saved.loop_state.kept_checks, task.final_verify);
    assert_eq!(checked.loop_state.kept_checks, task.final_verify);
}

#[tokio::test]
async fn an_owner_amendment_accepted_during_a_base_check_defers_the_planned_keep() {
    let (app, dir) = test_app_fitting_sandbox();
    let (task, barrier) = planned_parent(&app, dir.path());
    let results = actual_failed_final(&app, &task).await;
    let signals = TaskSignals::new();
    app.controls.lock().unwrap().insert(
        task.id.clone(),
        crate::engine::run_control::TaskControl {
            signals: signals.clone(),
            handle: tokio::spawn(async {}),
        },
    );
    let (worker_app, mut checked, worker_signals) = (app.clone(), task.clone(), signals.clone());
    let running = tokio::spawn(async move {
        let gate = crate::engine::graph::base::parent_finals_on_base(
            &worker_app,
            &mut checked,
            results,
            &worker_signals,
        )
        .await;
        (gate, checked)
    });
    barrier.wait().await;
    let accepted = app
        .dispatch(
            "task.amend",
            serde_json::json!({"id": task.id, "criteria": ["Owner's accepted criterion"]}),
        )
        .await
        .unwrap();
    assert_eq!(accepted["pending"], true);
    let receipt = app
        .store
        .load_task(&task.id)
        .unwrap()
        .unwrap()
        .pending_amendment
        .clone()
        .unwrap();
    barrier.release();
    let (gate, checked) = running.await.unwrap();
    assert!(matches!(
        gate,
        crate::engine::graph::base::ParentGate::Again
    ));
    assert!(checked.loop_state.kept_checks.is_empty());
    let mut saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert!(saved.loop_state.kept_checks.is_empty());
    assert_eq!(saved.pending_amendment.as_ref(), Some(&receipt));
    assert_eq!(
        signals.pending_amend.lock().unwrap().as_ref(),
        Some(&receipt)
    );
    assert!(matches!(
        saved.settle_pending_amendment(
            &crate::task::checks::authority::CheckAuthority::default(),
            crate::clock::now_ms()
        ),
        crate::task::amendment::storage::Settlement::Applied
    ));
    app.store
        .save_task_with_pending(&saved, Some(&receipt))
        .unwrap();
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.criteria, ["Owner's accepted criterion"]);
    assert!(saved.pending_amendment.is_none());
    assert!(!saved
        .decisions
        .iter()
        .any(|line| line.starts_with("Amendment not applied:")));
}

#[tokio::test]
async fn a_preflight_planned_keep_preserves_an_owner_revocation() {
    let (app, dir) = test_app_fitting_sandbox();
    let (mut task, barrier) = planned_parent(&app, dir.path());
    task.fixture_data().verify = task.final_verify.clone();
    task.fixture_data().final_verify.clear();
    task.record_assumption(crate::task::Assumption {
        question: "May the inherited check be waived?".into(),
        answer: crate::task::questions::WAIVED.into(),
        evidence: "The base has the same failure".into(),
        by: crate::task::questions::WAIVER_BY.into(),
        kind: Some(crate::task::QuestionKind::PreexistingFailure),
        attempt: None,
        commands: task.verify.clone(),
        overturned: false,
        owner_answer: None,
    });
    app.store.save_task(&task).unwrap();
    let (worker_app, mut checked) = (app.clone(), task.clone());
    let running = tokio::spawn(async move {
        let ready = crate::engine::graph::base::parent_base_preflight(
            &worker_app,
            &mut checked,
            &TaskSignals::new(),
        )
        .await;
        (ready, checked)
    });
    barrier.wait().await;
    app.dispatch(
        "task.overturn",
        serde_json::json!({"id": task.id, "index": 0, "answer": "Keep this check"}),
    )
    .await
    .unwrap();
    barrier.release();
    let (ready, checked) = running.await.unwrap();
    assert!(ready);
    assert!(checked.assumptions[0].overturned);
    app.save_task_logged(&checked);
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert!(saved.assumptions[0].overturned);
    assert_eq!(
        saved.assumptions[0].owner_answer.as_deref(),
        Some("Keep this check")
    );
    assert!(crate::delivery::pr::body::waived_section(&saved).is_empty());
    assert_eq!(saved.loop_state.kept_checks, task.verify);
    assert_eq!(checked.loop_state.kept_checks, task.verify);
}

#[tokio::test]
async fn preflight_applies_an_owner_amendment_before_publishing_a_planned_keep() {
    let (app, dir) = test_app_fitting_sandbox();
    let (mut task, barrier) = planned_parent(&app, dir.path());
    task.fixture_data().verify = task.final_verify.clone();
    task.fixture_data().final_verify.clear();
    app.store.save_task(&task).unwrap();
    let signals = TaskSignals::new();
    app.controls.lock().unwrap().insert(
        task.id.clone(),
        crate::engine::run_control::TaskControl {
            signals: signals.clone(),
            handle: tokio::spawn(async {}),
        },
    );
    let (worker_app, mut checked, worker_signals) = (app.clone(), task.clone(), signals.clone());
    let running = tokio::spawn(async move {
        let ready = crate::engine::graph::base::parent_base_preflight(
            &worker_app,
            &mut checked,
            &worker_signals,
        )
        .await;
        (ready, checked)
    });
    barrier.wait().await;
    let accepted = app
        .dispatch(
            "task.amend",
            serde_json::json!({"id": task.id, "criteria": ["Owner's accepted criterion"]}),
        )
        .await
        .unwrap();
    assert_eq!(accepted["pending"], true);
    barrier.release();
    let (ready, checked) = running.await.unwrap();
    assert!(ready);
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    for current in [&saved, &checked] {
        assert_eq!(current.criteria, ["Owner's accepted criterion"]);
        assert!(current.pending_amendment.is_none());
        assert_eq!(current.loop_state.kept_checks, task.verify);
        assert!(!current
            .decisions
            .iter()
            .any(|line| line.starts_with("Amendment not applied:")));
    }
    assert!(signals.pending_amend.lock().unwrap().is_none());
}

#[tokio::test]
async fn a_reused_parent_waiver_revoked_during_base_judgment_cannot_admit_delivery() {
    let (app, dir) = test_app_fitting_sandbox();
    let repo = dir.path().join("repo");
    let mut task = parent(&app, &repo);
    std::fs::write(repo.join("current-checkout"), "task marker").unwrap();
    let barrier = BaseBarrier {
        entered: dir.path().join("waiver-base-entered"),
        released: dir.path().join("waiver-base-released"),
    };
    let hold = dir.path().join("hold-waiver-base");
    let command = format!(
        "if ! test -f current-checkout && test -f '{}'; then touch '{}'; while ! test -f '{}'; do sleep 0.01; done; fi; printf 'not ok 1 - inherited_parent_case\\n'; exit 1",
        hold.display(), barrier.entered.display(), barrier.released.display()
    );
    task.fixture_data().final_verify = vec![command.clone()];
    app.store.save_task(&task).unwrap();
    let mut seeded = admitted(&app, &mut task).await;
    assert_eq!(seeded.assumptions.len(), 1);
    let original = seeded.assumptions[0].clone();
    assert!(!original.overturned);
    assert_eq!(original.commands.as_slice(), std::slice::from_ref(&command));
    assert_eq!(original.answer, crate::task::questions::WAIVED);
    seeded.resume_phase(TaskStatus::Running, crate::clock::now_ms());
    app.save_task_logged(&seeded);
    app.base_runs.lock().unwrap().clear();
    app.final_base_jobs.lock().unwrap().remove(&task.id);
    std::fs::write(&hold, "hold").unwrap();
    let (worker_app, mut checked) = (app.clone(), seeded);
    let running = tokio::spawn(async move {
        checked_for_delivery(&worker_app, &mut checked, &TaskSignals::new()).await
    });
    barrier.wait().await;
    let before = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(before.assumptions, [original]);
    app.dispatch(
        "task.overturn",
        serde_json::json!({"id": task.id, "index": 0, "answer": "This check must pass"}),
    )
    .await
    .unwrap();
    barrier.release();
    match running.await.unwrap() {
        Err(crate::engine::graph::parent::ParentEnd::Attempt { failure, budget }) => {
            assert_eq!(budget, Some(1));
            assert!(failure.contains(&command), "{failure}");
            assert!(failure.contains("inherited_parent_case"), "{failure}");
        }
        _ => panic!("the revoked reused waiver must restore the actual failed command"),
    }
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.assumptions.len(), 1);
    assert!(saved.assumptions[0].overturned);
    assert_eq!(
        saved.assumptions[0].owner_answer.as_deref(),
        Some("This check must pass")
    );
    assert!(saved.attempts.is_empty());
    assert_ne!(saved.status, TaskStatus::Done);
    assert!(saved.pr_url.is_none());
    assert!(crate::delivery::pr::body::waived_section(&saved).is_empty());
    assert!(saved.delivery_checks.iter().all(|run| run.code == Some(1)));
}

#[test]
fn a_removed_failure_without_a_typed_waiver_is_rejected() {
    let (app, _dir) = crate::engine::test_support::test_app();
    let task = task_with_status(TaskStatus::Running);
    app.store.save_task(&task).unwrap();
    let input = crate::engine::persistence::contract::ContractInput::capture(&task);
    let result = super::publish_waivers(
        &app,
        &task,
        &input,
        &["unproved check".into()],
        &crate::harness::cancel::CancelToken::new(),
    );
    match result {
        Ok(crate::engine::persistence::contract::Publication::Applied(rejected)) => {
            assert_eq!(rejected, ["unproved check"]);
        }
        _ => panic!("removing a failed result requires a current typed waiver"),
    }
    assert!(app
        .store
        .load_task(&task.id)
        .unwrap()
        .unwrap()
        .assumptions
        .is_empty());
}

#[tokio::test]
async fn an_owner_revocation_during_a_held_out_check_reopens_the_parent_failure() {
    let (app, dir) = test_app_fitting_sandbox();
    let repo = dir.path().join("repo");
    let mut task = parent(&app, &repo);
    let barrier = BaseBarrier {
        entered: dir.path().join("held-out-entered"),
        released: dir.path().join("held-out-released"),
    };
    let command = "printf 'not ok 1 - inherited_parent_case\\n'; exit 1";
    let held = format!(
        "touch '{}'; while ! test -f '{}'; do sleep 0.01; done; printf 'held receipt\\n'",
        barrier.entered.display(),
        barrier.released.display()
    );
    task.fixture_data().criteria = vec!["Keep the Owner's acceptance".into()];
    task.fixture_data().final_verify = vec![command.into()];
    task.fixture_data().held_out = Some(Check {
        criterion: 0,
        run: held,
        baseline: Some(Baseline::Fail),
    });
    app.store.save_task(&task).unwrap();
    let (worker_app, mut checked) = (app.clone(), task.clone());
    let running = tokio::spawn(async move {
        checked_for_delivery(&worker_app, &mut checked, &TaskSignals::new()).await
    });
    barrier.wait().await;
    let before = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(before.assumptions.len(), 1);
    assert_eq!(before.assumptions[0].answer, crate::task::questions::WAIVED);
    assert!(!before.assumptions[0].overturned);
    app.dispatch(
        "task.overturn",
        serde_json::json!({"id": task.id, "index": 0, "answer": "The final check must pass"}),
    )
    .await
    .unwrap();
    barrier.release();
    match running.await.unwrap() {
        Err(crate::engine::graph::parent::ParentEnd::Attempt { failure, budget }) => {
            assert_eq!(budget, Some(1));
            assert!(failure.contains(command), "{failure}");
            assert!(failure.contains("inherited_parent_case"), "{failure}");
        }
        _ => panic!("an Owner-revoked waiver must not admit the parent to delivery"),
    }
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert!(saved.assumptions[0].overturned);
    assert_eq!(
        saved.assumptions[0].owner_answer.as_deref(),
        Some("The final check must pass")
    );
    assert_eq!(saved.status, TaskStatus::Running);
    assert!(saved.attempts.is_empty());
    assert!(saved.pr_url.is_none());
    assert!(crate::delivery::pr::body::waived_section(&saved).is_empty());
    assert!(saved
        .delivery_checks
        .iter()
        .any(|run| run.command == command && run.code == Some(1)));
}
