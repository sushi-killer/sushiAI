use crate::harness::cancel::CancelToken;

use crate::engine::decision;

use crate::engine::run_control::TaskControl;

#[cfg(test)]
use crate::engine::test_support::planner_made;

#[cfg(test)]
use crate::engine::test_support::task_with_status;

#[cfg(test)]
use crate::engine::test_support::test_app;

use crate::policy::settings::AfterLand;

use crate::policy::settings::ScopedCheck;

use crate::policy::settings::Settings;

use crate::task::Check;

use crate::task::TaskStatus;

use serde_json::json;

use std::sync::Arc;

use std::sync::Mutex as StdMutex;

use crate::engine::rpc::test_support::{answer, waiting_base_check, waiting_impossible};
#[tokio::test]
async fn a_non_owner_cannot_drop_a_shared_check_but_may_retry_or_stop() {
    let (app, _dir) = test_app();
    {
        let mut settings = app.settings.write().unwrap();
        settings.scoped_checks.push(ScopedCheck {
            repo: None,
            command: "make lint*".to_string(),
            paths: vec!["src/**".to_string()],
        });
    }
    let drop = crate::engine::verification::base::answers::PRE_EXISTING_DROP;
    for (command, request) in [
        ("npm run ci", None),
        ("  cargo   test ", None),
        ("make lint-all", None),
        (
            "./scripts/gate.sh --strict",
            Some("Fix the bug; run ./scripts/gate.sh --strict at the end"),
        ),
    ] {
        for source in ["lead", "overseer", "mcp", "chat"] {
            let id = waiting_base_check(&app, command, request);
            let err = answer(&app, &id, &format!("{drop}."), Some(source))
                .await
                .unwrap_err();
            assert!(err.contains("only the owner may drop"), "{command}: {err}");
            let task = app.store.load_task(&id).unwrap().unwrap();
            assert_eq!(task.status, TaskStatus::Waiting, "{command} still waits");
        }
    }
    // A waiting-SLA default (source policy) is refused the same way.
    let id = waiting_base_check(&app, "npm run ci", None);
    let err = app
        .handle_task_answer(
            json!({"id": id, "answer": drop, "source": "policy", "evidence": "waited"}),
        )
        .await
        .unwrap_err();
    assert!(err.contains("only the owner may drop"), "{err}");
    let id = waiting_base_check(&app, "npm run ci", None);
    answer(&app, &id, "stop", Some("overseer")).await.unwrap();
    assert_eq!(
        app.store.load_task(&id).unwrap().unwrap().status,
        TaskStatus::Stopped
    );
    // The owner (no source) may drop it, and an agent may drop a task-specific check.
    let id = waiting_base_check(&app, "npm run ci", None);
    answer(&app, &id, drop, None).await.unwrap();
    let id = waiting_base_check(&app, "node --test tests/planner-made.test.cjs", None);
    let mut task = app.store.load_task(&id).unwrap().unwrap();
    planner_made(
        &mut task,
        "node --test tests/planner-made.test.cjs",
        "tests/planner-made.test.cjs",
    );
    app.store.save_task(&task).unwrap();
    answer(&app, &id, drop, Some("overseer")).await.unwrap();
    // The same command is the owner's when the planner did not write it.
    let id = waiting_base_check(&app, "node --test tests/planner-made.test.cjs", None);
    assert!(answer(&app, &id, drop, Some("overseer")).await.is_err());
    for command in ["cargo nextest run", "go test ./...", "npm run test:daemon"] {
        let id = waiting_base_check(&app, command, None);
        assert!(
            answer(&app, &id, drop, Some("overseer")).await.is_err(),
            "{command}"
        );
    }
}

#[test]
fn every_check_is_the_owners_except_one_the_planner_wrote_for_its_own_file() {
    let (settings, mut task) = (Settings::default(), task_with_status(TaskStatus::Waiting));
    planner_made(
        &mut task,
        "node --test tests/one.test.cjs",
        "tests/one.test.cjs",
    );
    let shared =
        |c: &str| crate::engine::answer_policy::authority::is_shared_check(&settings, &task, c);
    for command in [
        "npm run ci --silent",
        "cd web && cargo test",
        "CI=1 npm test",
        "npm run ci 2>&1 | tail -20",
        "echo start; yarn test",
        "cargo test -p sushiai-orch",
        "(cd web && npm test)",
        "env CI=1 npm test",
        "timeout 900 cargo test",
        "cargo nextest run",
        "go test ./...",
        "pytest",
        "just ci",
        "./gradlew test",
        "npm run test:daemon",
        "node --test tests/other.test.cjs",
    ] {
        assert!(shared(command), "{command}");
    }
    assert!(!shared("node --test tests/one.test.cjs"));
}

#[tokio::test]
async fn an_agent_amendment_cannot_remove_a_shared_check_but_the_owner_can() {
    let (app, _dir) = test_app();
    let mut task = task_with_status(TaskStatus::Queued);
    task.fixture_data().verify = vec![
        "npm run ci".into(),
        "node --test tests/mine.test.cjs".into(),
    ];
    planner_made(
        &mut task,
        "node --test tests/mine.test.cjs",
        "tests/mine.test.cjs",
    );
    task.fixture_data().final_verify = vec!["cargo test".into()];
    app.store.save_task(&task).unwrap();
    let id = task.id.clone();
    for (field, value) in [
        ("verify", json!(["node --test tests/mine.test.cjs"])),
        ("finalVerify", json!([])),
    ] {
        let params = json!({"id": id, field: value, "source": "overseer"});
        let err = app.handle_task_amend(params).await.unwrap_err();
        assert!(err.contains("only the owner may drop"), "{field}: {err}");
    }
    // Keeping the shared check while dropping a task-specific one is fine.
    let keep = json!({"id": id, "verify": ["npm run ci"], "source": "mcp"});
    app.handle_task_amend(keep).await.unwrap();
    // The owner (no source) may remove it.
    let owner = json!({"id": id, "finalVerify": []});
    app.handle_task_amend(owner).await.unwrap();
}

#[tokio::test]
async fn dropping_a_criterion_that_carries_a_shared_check_is_the_owners() {
    let (app, _dir) = test_app();
    for source in ["lead", "overseer", "mcp", "chat"] {
        let id = waiting_impossible(&app);
        let err = answer(&app, &id, "drop criterion", Some(source))
            .await
            .unwrap_err();
        assert!(err.contains("only the owner may drop"), "{source}: {err}");
        let task = app.store.load_task(&id).unwrap().unwrap();
        assert_eq!((task.criteria.len(), task.checks.len()), (2, 1));
    }
    // Retrying is open to an agent.
    let id = waiting_impossible(&app);
    answer(&app, &id, "retry", Some("overseer")).await.unwrap();
    // The owner drops it: the criterion and its check go.
    let id = waiting_impossible(&app);
    answer(&app, &id, "drop criterion", None).await.unwrap();
    let task = app.store.load_task(&id).unwrap().unwrap();
    assert_eq!((task.criteria.len(), task.checks.len()), (1, 0));
}

#[test]
fn the_removal_choke_point_refuses_a_non_owner_for_every_kind_of_removal() {
    let settings = Settings::default();
    let mut task = task_with_status(TaskStatus::Waiting);
    task.fixture_data().criteria = vec!["A -- check: cargo test".into()];
    planner_made(
        &mut task,
        "node --test tests/mine.test.cjs",
        "tests/mine.test.cjs",
    );
    task.fixture_data().verify = vec![
        "npm run ci".into(),
        "node --test tests/mine.test.cjs".into(),
    ];
    use crate::task::checks::authority::Origin::{Other, Owner};
    assert!(decision::drop_criterion(&settings, &mut task, 0, Other).is_err());
    assert_eq!(task.criteria.len(), 1, "still there");
    assert!(
        crate::engine::answer_policy::authority::remove_check_command(
            &settings,
            &mut task,
            "npm run ci",
            Other
        )
        .is_err()
    );
    assert!(
        crate::engine::answer_policy::authority::remove_check_command(
            &settings,
            &mut task,
            "node --test tests/mine.test.cjs",
            Other
        )
        .is_ok()
    );
    assert_eq!(task.verify, vec!["npm run ci".to_string()]);
    let ruled = crate::engine::verification::healing::contract::apply_feasibility(
        &settings,
        &mut task,
        "npm run ci",
        crate::verify::healing::reply::Feasibility::Unrelated { reason: "x".into() },
    );
    assert!(ruled && task.verify == vec!["npm run ci".to_string()]);
    assert!(task.brief_check.non_gating.is_empty());
    assert!(decision::drop_criterion(&settings, &mut task, 0, Owner).is_ok());
    assert!(task.criteria.is_empty());
}

#[tokio::test]
async fn an_amendment_cannot_drop_a_shared_held_out_or_criterion_check_or_overwrite_a_pending_one()
{
    let (app, _dir) = test_app();
    let mut task = task_with_status(TaskStatus::Queued);
    task.fixture_data().criteria = vec!["A -- check: npm test".into(), "B".into()];
    task.fixture_data().held_out = Some(Check {
        criterion: 1,
        run: "cargo test".into(),
        baseline: None,
    });
    task.fixture_data().verify = vec!["node --test tests/mine.test.cjs".into()];
    app.store.save_task(&task).unwrap();
    let id = task.id.clone();
    for params in [
        json!({"id": id, "heldOut": null, "source": "overseer"}),
        json!({"id": id, "heldOut": {"criterion": 1, "run": "true"}, "source": "overseer"}),
        json!({"id": id, "criteria": ["A", "B"], "source": "overseer"}),
        json!({"id": id, "criteria": ["A -- check: npm test"], "source": "mcp"}),
    ] {
        let err = app.handle_task_amend(params.clone()).await.unwrap_err();
        assert!(err.contains("only the owner may drop"), "{params}: {err}");
    }
    // Keeping them is fine.
    let keep =
        json!({"id": id, "criteria": ["A -- check: npm test", "B", "C"], "source": "overseer"});
    app.handle_task_amend(keep).await.unwrap();

    // The owner's pending amendment adds `npm run ci`; an agent's later
    // amendment of verify without it cannot overwrite it.
    let pending_slot = Arc::new(StdMutex::new(None));
    app.controls.lock().unwrap().insert(
        id.clone(),
        TaskControl {
            signals: crate::engine::run_control::TaskSignals {
                resume: Default::default(),
                cancel: CancelToken::new(),
                pending_answer: Arc::new(StdMutex::new(None)),
                pending_amend: pending_slot.clone(),
            },
            handle: tokio::spawn(async {}),
        },
    );
    app.handle_task_amend(
        json!({"id": id, "verify": ["node --test tests/mine.test.cjs", "npm run ci"]}),
    )
    .await
    .unwrap();
    assert!(pending_slot.lock().unwrap().is_some());
    let err = app
        .handle_task_amend(
            json!({"id": id, "verify": ["node --test tests/mine.test.cjs"], "source": "overseer"}),
        )
        .await
        .unwrap_err();
    assert!(err.contains("only the owner may drop"), "{err}");
}

#[test]
fn a_judge_cannot_rewrite_a_criterion_to_lose_its_shared_check() {
    let settings = Settings::default();
    let mut task = task_with_status(TaskStatus::Waiting);
    task.fixture_data().criteria = vec!["Works -- check: cargo test".into(), "Docs".into()];
    let rewrite = crate::verify::healing::reply::Feasibility::Rewrite {
        command: "true".into(),
        criterion: Some((0, "Works -- check: grep -q ok f".into())),
        reason: "r".into(),
    };
    assert!(
        crate::engine::verification::healing::contract::apply_feasibility(
            &settings, &mut task, "x", rewrite
        )
    );
    assert_eq!(task.criteria[0], "Works -- check: cargo test");
    assert!(task
        .decisions
        .iter()
        .any(|d| d.contains("only the owner may drop")));
    // A task-specific criterion can be reworded.
    task.fixture_data().criteria[1] = "Docs -- check: grep -q d.md".into();
    planner_made(&mut task, "grep -q d.md", "d.md");
    assert!(
        crate::engine::verification::healing::contract::amend_criterion_for_test(
            &settings,
            &mut task,
            1,
            "Docs -- check: test -f d.md"
        )
        .is_ok()
    );
    // Keeping the shared command is fine.
    assert!(
        crate::engine::verification::healing::contract::amend_criterion_for_test(
            &settings,
            &mut task,
            0,
            "Works well -- check: cargo test"
        )
        .is_ok()
    );
}

#[tokio::test]
async fn an_after_land_command_is_a_shared_check_only_for_its_repo() {
    let (app, _dir) = test_app();
    let mut task = task_with_status(TaskStatus::Waiting);
    task.fixture_data().repo = "/repo/a".to_string();
    let settings = {
        let mut s = app.settings.write().unwrap();
        s.after_land.push(AfterLand {
            repo: "/repo/a".to_string(),
            run: "node --test tests/rb.test.cjs".to_string(),
        });
        s.clone()
    };
    planner_made(
        &mut task,
        "node --test tests/rb.test.cjs",
        "tests/rb.test.cjs",
    );
    assert!(crate::engine::answer_policy::authority::is_shared_check(
        &settings,
        &task,
        "node --test tests/rb.test.cjs"
    ));
    task.fixture_data().repo = "/repo/b".to_string();
    assert!(!crate::engine::answer_policy::authority::is_shared_check(
        &settings,
        &task,
        "node --test tests/rb.test.cjs"
    ));
}
