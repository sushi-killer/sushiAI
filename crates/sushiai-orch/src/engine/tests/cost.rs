#[test]
fn resumed_claude_cost_is_own_spend_and_excludes_review_costs() {
    let (app, _dir) = test_app();
    let mut task = task_with_status(TaskStatus::Running);
    for (n, session, cost) in [(1, "other", 8.0), (2, "resumed", 0.1), (3, "resumed", 0.2)] {
        let mut a = attempt_with_failure(n, "verify");
        a.session_id = Some(session.into());
        a.cost_usd = Some(cost);
        a.review_cost_usd = Some(9.0);
        task.fixture_data().attempts.push(a);
    }
    app.store.save_task(&task).unwrap();
    let mut outcome = harness::RunOutcome {
        cost_usd: Some(0.7),
        ..Default::default()
    };
    price_resumed_attempt(
        &app,
        &task.id,
        4,
        Harness::Claude,
        Some("resumed"),
        &mut outcome,
    );
    assert!((outcome.cost_usd.unwrap() - 0.4).abs() < 1e-9);
    assert!(!outcome.cost_estimated);
    let mut streamed = harness::RunOutcome {
        cost_usd: Some(20.0),
        ..Default::default()
    };
    harness::feed_stream_line(
        Harness::Claude,
        r#"{"type":"assistant","message":{"id":"own","model":"claude-sonnet-5-5","usage":{"input_tokens":1000,"output_tokens":100}}}"#,
        &mut streamed,
    );
    let expected = streamed
        .streamed_cost(&app.settings.read().unwrap().prices)
        .unwrap();
    price_resumed_attempt(
        &app,
        &task.id,
        4,
        Harness::Claude,
        Some("resumed"),
        &mut streamed,
    );
    assert_eq!(streamed.cost_usd, Some(expected));
    assert!(streamed.cost_estimated);
}

#[test]
fn an_interrupted_resumed_attempt_settles_only_its_own_session_spend() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::new(tmp.path()).unwrap();
    let mut task = task_with_status(TaskStatus::Running);
    let dir = store.run_dir(&task.id, 2);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("run.json"),
        r#"{"argv":["-p","--resume","sess-1"]}"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("events.jsonl"),
        r#"{"type":"result","total_cost_usd":0.7}"#,
    )
    .unwrap();
    let mut previous = attempt_with_failure(1, "verify");
    previous.session_id = Some("sess-1".into());
    previous.cost_usd = Some(0.3);
    task.fixture_data().cost_usd = 0.3;
    task.fixture_data().attempts = vec![previous, attempt_with_failure(2, "verify")];
    settle_unfinished_cost(&mut task, 1, &store, &Settings::default().prices);
    assert!((task.cost_usd - 0.7).abs() < 1e-9);
    assert!((task.attempts[1].cost_usd.unwrap() - 0.4).abs() < 1e-9);
}

#[test]
fn a_plan_attempt_settles_only_the_runs_whose_cost_is_not_saved_yet() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::new(tmp.path()).unwrap();
    let task = task_with_status(TaskStatus::Drafting);
    let plan = store.run_dir(&task.id, 1).join("plan");
    std::fs::create_dir_all(&plan).unwrap();
    let result = |cost: f64| {
        format!(
            r#"{{"type":"result","total_cost_usd":{cost},"usage":{{"input_tokens":1,"output_tokens":1}},"result":"x"}}"#
        )
    };
    std::fs::write(plan.join("events.jsonl"), result(0.25)).unwrap();
    std::fs::write(plan.join("events-retry.jsonl"), result(0.5)).unwrap();
    let prices = Settings::default().prices;
    let settle = |saved: Option<f64>| {
        let mut task = task.clone();
        task.fixture_data().cost_usd = saved.unwrap_or(0.0);
        task.fixture_data().attempts = vec![Attempt {
            stage: Stage::Plan,
            cost_usd: saved,
            ..attempt_with_failure(1, "x")
        }];
        settle_unfinished_cost(&mut task, 0, &store, &prices);
        (task.attempts[0].cost_usd, task.cost_usd)
    };
    // No cost saved yet: every plan run's file counts.
    assert_eq!(settle(None), (Some(0.75), 0.75));
    // Died in the retry: the first run's cost was saved before it.
    assert_eq!(settle(Some(0.25)), (Some(0.75), 0.75));
}

#[test]
fn backfill_turns_task_costs_into_records_once() {
    let (app, _dir) = test_app();
    let mut task = task_with_status(TaskStatus::Done);
    task.fixture_data().attempts = vec![
        Attempt {
            stage: Stage::Plan,
            cost_usd: Some(0.02),
            ..attempt_with_failure(1, "x")
        },
        Attempt {
            cost_usd: Some(1.0),
            review_cost_usd: Some(0.5),
            evidence: vec![],
            evidence_tree: None,
            review_tree: None,
            review_base: None,
            review_scope: None,
            evidence_base: None,
            evidence_from: None,
            advisor_cost_usd: Some(0.25),
            review_fingerprint: Some(Fingerprint {
                models: vec!["claude-opus-5-5".into()],
                harness: Harness::Claude,
                harness_version: None,
                prompt_hash: String::new(),
            }),
            ..attempt_with_failure(2, "x")
        },
    ];
    app.store.save_task(&task).unwrap();
    assert_eq!(crate::store::costs::backfill(&app.data_dir), 4);
    let mut records = crate::store::costs::read_all(&app.data_dir);
    records.sort_by(|a, b| a.stage.cmp(&b.stage));
    let stages: Vec<&str> = records.iter().map(|r| r.stage.as_str()).collect();
    assert_eq!(stages, ["advisor", "implement", "plan", "review"]);
    assert!(records.iter().all(|r| r.backfilled));
    let review = records.iter().find(|r| r.stage == "review").unwrap();
    assert_eq!(review.cost_usd, 0.5);
    assert_eq!(review.model, "claude-opus-5-5");
    assert_eq!(review.task_id.as_deref(), Some(task.id.as_str()));
    let total: f64 = records.iter().map(|r| r.cost_usd).sum();
    assert!((total - 1.77).abs() < 1e-9);
    // A second run adds nothing, and a live record for a task also keeps it
    // from being derived again.
    assert_eq!(crate::store::costs::backfill(&app.data_dir), 0);
    assert_eq!(crate::store::costs::read_all(&app.data_dir).len(), 4);
}

#[test]
fn helpers_folder_cost_replay_reads_only_the_attempts_review_and_advisor() {
    let (app, _dir) = test_app();
    let mut task = task_with_status(TaskStatus::Queued);
    task.fixture_data().attempts = vec![attempt_with_failure(1, "failure")];
    let result = |cost: f64| format!(r#"{{"type":"result","total_cost_usd":{cost},"result":"x"}}"#);
    for (stage, attempt, cost) in [
        ("review", 1, 0.25),
        ("review", 1, 0.5),
        ("review", 2, 10.0),
        ("advisor", 1, 0.125),
        ("advisor", 2, 20.0),
    ] {
        let dir = app
            .store
            .claim_helper_run_dir(&task.id, stage, Some(attempt))
            .unwrap();
        std::fs::write(dir.join("events.jsonl"), result(cost)).unwrap();
    }
    let legacy = app.store.run_dir(&task.id, 1);
    for stage in ["review", "advisor"] {
        std::fs::create_dir_all(legacy.join(stage)).unwrap();
        std::fs::write(legacy.join(stage).join("events.jsonl"), result(99.0)).unwrap();
    }
    let prices = Settings::default().prices;
    settle_interrupted_review(&mut task, 0, &app.store, &prices);
    assert_eq!(task.attempts[0].review_cost_usd, Some(0.75));
    assert!(settle_interrupted_advisor(&mut task, &app.store, &prices));
    assert_eq!(task.attempts[0].advisor_cost_usd, Some(0.125));
    assert_eq!(task.cost_usd, 0.875);
    settle_interrupted_review(&mut task, 0, &app.store, &prices);
    assert!(!settle_interrupted_advisor(&mut task, &app.store, &prices));
    assert_eq!(task.cost_usd, 0.875);
    let mut old = task.clone();
    old.fixture_data().id = uuid::Uuid::new_v4().to_string();
    old.fixture_data().cost_usd = 0.0;
    old.fixture_data().attempts[0].review_cost_usd = None;
    old.fixture_data().attempts[0].advisor_cost_usd = None;
    for stage in ["review", "advisor"] {
        let dir = app.store.run_dir(&old.id, 1).join(stage);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("events.jsonl"), result(99.0)).unwrap();
    }
    settle_interrupted_review(&mut old, 0, &app.store, &prices);
    assert!(!settle_interrupted_advisor(&mut old, &app.store, &prices));
    assert_eq!(old.cost_usd, 0.0);
}
