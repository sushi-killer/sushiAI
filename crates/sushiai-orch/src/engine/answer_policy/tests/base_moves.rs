use super::*;
use crate::engine::test_support::{git_in, init_git_repo, task_with_status, test_app};

#[test]
fn a_default_branch_landing_may_not_move_is_never_waited_for() {
    let (app, dir) = test_app();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    init_git_repo(&repo);
    git_in(&repo, &["branch", "-M", "main"]);
    let on_main = |status| {
        let mut task = task_with_status(status);
        task.fixture_data().repo = repo.display().to_string();
        task.fixture_data().worktree = task.repo.clone();
        task.fixture_data().base_ref = Some("main".into());
        task.fixture_data().base_sha = git_in(&repo, &["rev-parse", "HEAD"]);
        task
    };
    let task = on_main(TaskStatus::Waiting);
    let mut other = on_main(TaskStatus::Running);
    other.fixture_data().id = "other".into();
    app.store.save_task(&task).unwrap();
    app.store.save_task(&other).unwrap();
    let question = Question::new(
        "make verify already fails on base",
        vec![PRE_EXISTING_RETRY.into(), "stop".into()],
        QuestionKind::PreexistingFailure,
        AskedBy::Verify,
        crate::clock::now_ms(),
    );

    let (_, base_moves) = policy_base_context(&app, &task, &question);
    assert!(!base_moves, "nothing may land on main, so it never moves");

    app.settings
        .write()
        .unwrap()
        .land_on_default_repos
        .insert(task.repo.clone(), true);
    let (_, base_moves) = policy_base_context(&app, &task, &question);
    assert!(base_moves, "another live task may land on main");
}

#[test]
fn only_a_task_that_can_land_counts_as_moving_the_base() {
    let (app, dir) = test_app();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    init_git_repo(&repo);
    git_in(&repo, &["branch", "-M", "main"]);
    app.settings
        .write()
        .unwrap()
        .land_on_default_repos
        .insert(repo.display().to_string(), true);
    let on_main = |id: &str, status| {
        let mut task = task_with_status(status);
        task.fixture_data().id = id.into();
        task.fixture_data().repo = repo.display().to_string();
        task.fixture_data().worktree = task.repo.clone();
        task.fixture_data().base_ref = Some("main".into());
        task.fixture_data().base_sha = git_in(&repo, &["rev-parse", "HEAD"]);
        task
    };
    let red = on_main("red", TaskStatus::Waiting);
    app.store.save_task(&red).unwrap();
    let question = Question::new(
        "make verify already fails on base",
        vec![PRE_EXISTING_RETRY.into(), "stop".into()],
        QuestionKind::PreexistingFailure,
        AskedBy::Verify,
        crate::clock::now_ms(),
    );
    let moves = |app: &App| policy_base_context(app, &red, &question).1;

    // Another red task, waiting for an answer, queued, or in the bounded
    // wait for this very move, can land nothing.
    let mut asking = on_main("asking", TaskStatus::Waiting);
    let queued = on_main("queued", TaskStatus::Queued);
    let mut in_wait = on_main("in_wait", TaskStatus::Running);
    in_wait.fixture_data().queue.wait = Some(Wait::Dependency {
        titles: vec![format!("{} main (retrying)", BASE_WAIT_TITLE)],
    });
    asking.fixture_data().question = Some(question.clone());
    for other in [&asking, &queued, &in_wait] {
        app.store.save_task(other).unwrap();
    }
    assert!(!moves(&app), "two red tasks must not wait on each other");

    let running = on_main("running", TaskStatus::Running);
    app.store.save_task(&running).unwrap();
    assert!(moves(&app), "a running task can land");
    let mut landing = running.clone();
    landing.fixture_data().status = TaskStatus::Landing;
    app.store.save_task(&landing).unwrap();
    assert!(moves(&app), "a landing task moves the base");
}

#[test]
fn the_policy_budget_is_per_kind_and_cause_not_per_task() {
    let (app, _dir) = crate::engine::test_support::test_app();
    let mut task = task_with_status(TaskStatus::Waiting);
    let spent = Question::new(
        "make verify already fails on base",
        vec![],
        QuestionKind::PreexistingFailure,
        AskedBy::Verify,
        crate::clock::now_ms(),
    );
    for _ in 0..MAX_POLICY_ANSWERS {
        record_policy_answer(
            &mut task,
            &spent,
            &AutomaticAnswer::Policy {
                text: ("keep all").to_string(),
                evidence: ("e").to_string(),
            },
            1,
        );
    }
    assert!(!policy_open(&app, &task, &spent), "that kind is spent");

    // Other kinds still have their own budget, the judge's included.
    let impossible = |n: usize| {
        let mut q = Question::new(
            format!("Criterion {n} cannot be met as written: x. Evidence: y"),
            vec!["retry".into(), "drop criterion".into()],
            QuestionKind::Impossible,
            AskedBy::Implement,
            crate::clock::now_ms(),
        );
        q.criterion = Some(n);
        q
    };
    assert!(policy_open(&app, &task, &impossible(0)));
    let attempts = Question::new(
        "Attempts keep failing",
        vec![],
        QuestionKind::AttemptsFailing,
        AskedBy::Implement,
        crate::clock::now_ms(),
    );
    assert!(policy_open(&app, &task, &attempts));

    // One criterion that the judge refuted three times is spent; another is not.
    for _ in 0..MAX_POLICY_ANSWERS {
        record_policy_answer(
            &mut task,
            &impossible(0),
            &AutomaticAnswer::Judge {
                text: ("retry").to_string(),
                evidence: ("e").to_string(),
            },
            1,
        );
    }
    assert!(!policy_open(&app, &task, &impossible(0)));
    assert!(policy_open(&app, &task, &impossible(1)));
}
