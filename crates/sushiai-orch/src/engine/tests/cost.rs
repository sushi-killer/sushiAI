#[test]
fn a_plan_attempt_settles_only_the_runs_whose_cost_is_not_saved_yet() {
    let tmp = tempfile::tempdir().unwrap();
    let plan = tmp.path().join("plan");
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
        let mut task = task_with_status(TaskStatus::Drafting);
        task.cost_usd = saved.unwrap_or(0.0);
        task.attempts = vec![Attempt {
            stage: Stage::Plan,
            cost_usd: saved,
            ..attempt_with_failure(1, "x")
        }];
        settle_unfinished_cost(&mut task, 0, tmp.path(), &prices);
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
    task.attempts = vec![
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
    assert_eq!(crate::costs::backfill(&app.data_dir), 4);
    let mut records = crate::costs::read_all(&app.data_dir);
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
    assert_eq!(crate::costs::backfill(&app.data_dir), 0);
    assert_eq!(crate::costs::read_all(&app.data_dir).len(), 4);
}
