#[test]
fn attempt_cost_subtracts_what_earlier_attempts_on_the_session_already_paid() {
    // Real figures from one task: fresh $2.13, resumed $2.94, resumed $3.04.
    let with = |n, resumed, cost: Option<f64>| Attempt {
        session_id: Some("s1".into()),
        resumed,
        cost_usd: cost,
        ..attempt_with_failure(n, "x")
    };
    let first = with(1, false, Some(2.13));
    let second = with(2, true, None);
    let second_cost = attempt_cost(std::slice::from_ref(&first), &second, 2.94, false);
    assert!((second_cost - 0.81).abs() < 1e-9);
    let third = with(3, true, None);
    let paid = [first, with(2, true, Some(second_cost))];
    assert!((attempt_cost(&paid, &third, 3.04, false) - 0.10).abs() < 1e-9);
    // A fresh session is never discounted.
    assert_eq!(attempt_cost(&paid, &with(4, false, None), 1.5, false), 1.5);
    // An estimate covers its own run only.
    assert_eq!(attempt_cost(&paid, &third, 0.4, true), 0.4);
    // An attempt killed mid-run and estimated on recovery is still part
    // of the session total the resume reports, so it is subtracted too:
    // the task pays 2.13 + 0.5 + 0.37 = 3.0, the session's own total.
    let killed = Attempt {
        cost_estimated: true,
        ..with(2, true, Some(0.5))
    };
    let after_kill = [with(1, false, Some(2.13)), killed];
    assert!((attempt_cost(&after_kill, &third, 3.0, false) - 0.37).abs() < 1e-9);
}

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
