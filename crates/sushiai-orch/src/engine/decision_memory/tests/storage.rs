use super::*;

#[tokio::test]
async fn decision_memory_records_every_answer_source() {
    let (app, _dir) = test_app();
    let q = free_q("Which way?", &["A", "B"]);
    for (by, name) in [
        (AnsweredBy::Owner, "owner"),
        (AnsweredBy::Lead, "lead"),
        (AnsweredBy::Overseer, "overseer"),
        (AnsweredBy::Orchestrator, "chat"),
    ] {
        remember(&app, "/r1", &q, "A", by);
        let (list, _) = precedents::list(&app.data_dir, "/r1");
        assert!(
            list.iter().any(|p| p.by == name && p.answer == "A"),
            "{name}"
        );
    }
    // A waiting-SLA default and a rule are policy; a judge is its own source.
    let mut policy = task_with_status(TaskStatus::Queued);
    policy.fixture_data().repo = "/r1".into();
    record_answer_for(
        &mut policy,
        Some(&q),
        &crate::engine::questions::reply::Reply {
            text: ("B").to_string(),
            default_evidence: Some(str::to_string("nobody answered")),
            like: (None).map(str::to_string),
        },
        AnsweredBy::Owner,
    );
    app.store.save_task(&policy).unwrap();
    let mut judge = task_with_status(TaskStatus::Queued);
    judge.fixture_data().repo = "/r1".into();
    record_policy_answer(
        &mut judge,
        &q,
        &AutomaticAnswer::Judge {
            text: ("B").to_string(),
            evidence: ("verified the evidence").to_string(),
        },
        1,
    );
    app.store.save_task(&judge).unwrap();
    let (list, _) = precedents::list(&app.data_dir, "/r1");
    let find = |by: &str| list.iter().find(|p| p.by == by && p.answer == "B");
    assert_eq!(
        find("policy").map(|p| p.why.as_str()),
        Some("nobody answered")
    );
    assert!(find("judge").is_some_and(|p| p.why == "verified the evidence"));
    assert!(list.iter().all(|p| p.kind == QuestionKind::AgentQuestion));
    let file = precedents::path(&app.data_dir);
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[tokio::test]
async fn decision_memory_keeps_facts_but_never_a_check_output_tail() {
    let (app, _dir) = test_app();
    let mut q = base_q();
    q.text = "cargo test fails on base deadbee: token sk-live-SECRET99 leaked".into();
    let mut task = task_with_status(TaskStatus::Queued);
    task.fixture_data().repo = "/r1".into();
    let mut attempt = attempt_with_failure(1, "verify:x");
    attempt.changed_files = vec!["src/a.rs".into()];
    attempt.verify = vec![VerifyOutcome {
        command: "cargo test".into(),
        code: Some(1),
        tail: "setup failed: EEXIST sk-live-SECRET99".into(),
        ms: 1,
        load: 0.0,
        cores: 1,
        load_timeout: false,
        decisions: vec![],
        setup_failed: true,
        ..Default::default()
    }];
    task.fixture_data().attempts.push(attempt);
    record_answer_for(
        &mut task,
        Some(&q),
        &crate::engine::questions::reply::Reply {
            text: (RETRY).to_string(),
            default_evidence: (None).map(str::to_string),
            like: (None).map(str::to_string),
        },
        AnsweredBy::Owner,
    );
    app.store.save_task(&task).unwrap();
    let text = std::fs::read_to_string(precedents::path(&app.data_dir)).unwrap();
    assert!(!text.contains("SECRET99"), "{text}");
    let (list, _) = precedents::list(&app.data_dir, "/r1");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].commands, ["cargo test"]);
    assert_eq!(list[0].signals, ["install"]);
    assert_eq!(list[0].files, ["src/a.rs"]);
    assert_eq!(list[0].goal, "Do the thing");
    assert_eq!(list[0].question, None);
}

#[test]
fn decision_memory_signals_read_the_check_results() {
    let outcome = |code: Option<i32>, tail: &str| VerifyOutcome {
        command: "npm test".into(),
        code,
        tail: tail.into(),
        ms: 1,
        load: 0.0,
        cores: 1,
        load_timeout: false,
        decisions: vec![],
        ..Default::default()
    };
    let attempt = |n, outcome| {
        let mut a = attempt_with_failure(n, "s");
        a.verify = vec![outcome];
        a
    };
    let mut task = task_with_status(TaskStatus::Queued);
    // Different failures on one tree are flaky; on two trees they are two changes.
    let on_tree = |n, text: &str, tree: &str| {
        let mut a = attempt(n, outcome(Some(1), text));
        a.verify[0].tree = Some(tree.into());
        a
    };
    task.fixture_data().attempts = vec![
        on_tree(1, "not ok 1 - alpha", "tree-a"),
        on_tree(2, "not ok 2 - beta", "tree-a"),
    ];
    assert_eq!(crate::task::attempts::signals::signals_of(&task), ["flaky"]);
    task.fixture_data().attempts = vec![
        on_tree(1, "not ok 1 - alpha", "tree-a"),
        on_tree(2, "not ok 2 - beta", "tree-b"),
    ];
    assert!(crate::task::attempts::signals::signals_of(&task).is_empty());
    task.fixture_data().attempts = vec![attempt(1, outcome(None, "killed after 600s"))];
    assert_eq!(
        crate::task::attempts::signals::signals_of(&task),
        ["timeout"]
    );
    // The install signal comes from a failed setup, never from a check's own words.
    let mut setup = outcome(Some(1), "npm ERR! failed to install");
    setup.setup_failed = true;
    task.fixture_data().attempts = vec![attempt(1, setup)];
    assert_eq!(
        crate::task::attempts::signals::signals_of(&task),
        ["install"]
    );
    task.fixture_data().attempts = vec![attempt(1, outcome(Some(1), "npm ERR! failed to install"))];
    assert!(crate::task::attempts::signals::signals_of(&task).is_empty());
    let mut other = attempt(1, outcome(Some(1), "FAIL tests/other.test.js"));
    other.changed_files = vec!["src/mine.js".into()];
    task.fixture_data().attempts = vec![other];
    assert_eq!(
        crate::task::attempts::signals::signals_of(&task),
        ["outside_diff"]
    );
    let mut mine = attempt(1, outcome(Some(1), "FAIL tests/mine.test.js"));
    mine.changed_files = vec!["tests/mine.test.js".into()];
    task.fixture_data().attempts = vec![mine];
    assert!(crate::task::attempts::signals::signals_of(&task).is_empty());
}

#[tokio::test]
async fn decision_memory_merges_the_same_key_and_caps_a_repo() {
    let (app, _dir) = test_app();
    let q = base_q();
    for _ in 0..3 {
        remember(&app, "/r1", &q, RETRY, AnsweredBy::Owner);
    }
    remember(&app, "/r1", &q, RETRY, AnsweredBy::Lead);
    remember(&app, "/r1", &q, "stop", AnsweredBy::Owner);
    let (list, _) = precedents::list(&app.data_dir, "/r1");
    assert_eq!(list.len(), 3);
    let owner = list
        .iter()
        .find(|p| p.by == "owner" && p.answer == RETRY)
        .unwrap();
    assert_eq!(owner.count, 3);
    assert!(owner.last_at >= owner.first_at);
    // A resave of one task adds nothing.
    let task = answered_task("/r2", &q, RETRY, AnsweredBy::Owner);
    app.store.save_task(&task).unwrap();
    app.store.save_task(&task).unwrap();
    assert_eq!(precedents::list(&app.data_dir, "/r2").0[0].count, 1);

    let mut task = task_with_status(TaskStatus::Queued);
    task.fixture_data().repo = "/r3".into();
    let base = now_ms();
    for i in 0..(precedents::MAX_PER_REPO + 5) {
        task.fixture_data().question_history.push(AnsweredQuestion {
            question: "q".into(),
            options: vec![],
            kind: QuestionKind::PreexistingFailure,
            asked_by: None,
            asked_at: None,
            answer: RETRY.into(),
            answered_at: base + i as i64,
            answered_by: AnsweredBy::Owner,
            failure_signature: None,
            commands: vec![format!("cmd-{i}")],
        });
    }
    app.store.save_task(&task).unwrap();
    let (list, _) = precedents::list(&app.data_dir, "/r3");
    assert_eq!(list.len(), precedents::MAX_PER_REPO);
    assert!(list.iter().all(|p| p.commands[0] != "cmd-4"));
    assert!(list.iter().any(|p| p.commands[0] == "cmd-204"));
}
