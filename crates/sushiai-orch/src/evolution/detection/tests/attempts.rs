use super::*;

#[test]
fn evolution_loop_signal_wastes_the_attempts_calls_and_cost() {
    let mut t = task("t", 10);
    let mut a = attempt_with_failure(1, "loop:x");
    a.failure = Some(failure(FailureKind::Loop, "same call", "loop:x"));
    a.cost_usd = Some(4.0);
    t.fixture_data().attempts = vec![a];
    let evs = vec![read("/a"), read("/b"), read("/c"), read("/d")];
    let s = run(&t, &[], &[("t", 1, evs)]);
    let l = of_kind(&s, SignalKind::Loop);
    assert_eq!(l.len(), 1);
    assert_eq!((l[0].attempt, l[0].wasted_calls), (1, 4));
    assert!((l[0].wasted_usd - 4.0).abs() < 1e-9);
    assert_eq!(l[0].excerpt_ref.file, "tasks/t/runs/1/events.jsonl");
    assert_eq!(
        (l[0].excerpt_ref.from_line, l[0].excerpt_ref.to_line),
        (1, 4)
    );
    let v = serde_json::to_value(l[0]).unwrap();
    assert_eq!(v["kind"], "loop");
    assert_eq!(v["taskId"], "t");
    assert_eq!(v["excerptRef"]["fromLine"], 1);
}

#[test]
fn evolution_no_calls_or_no_cost_means_zero_usd() {
    let mut t = task("t", 10);
    let mut a = attempt_with_failure(1, "loop:x");
    a.failure = Some(failure(FailureKind::Loop, "d", "loop:x"));
    t.fixture_data().attempts = vec![a];
    let s = run(&t, &[], &[("t", 1, vec![read("/a")])]);
    assert_eq!(s[0].wasted_calls, 1);
    assert_eq!(s[0].wasted_usd, 0.0);
    let s = run(&t, &[], &[]);
    assert_eq!((s[0].wasted_calls, s[0].wasted_usd), (0, 0.0));
    assert_eq!(s[0].excerpt_ref.file, "tasks/t/task.json");
}

#[test]
fn evolution_discovery_needs_history_and_twice_the_median() {
    let mut earlier = Vec::new();
    let mut evs: Vec<(&str, u32, Vec<Value>)> = Vec::new();
    for id in ["e1", "e2", "e3"] {
        let mut t = task(id, 1);
        t.fixture_data().attempts = vec![attempt_with_failure(1, "s")];
        earlier.push(t);
        evs.push((id, 1, vec![read("/x"), read("/y")]));
    }
    let mut t = task("t", 10);
    t.fixture_data().attempts = vec![attempt_with_failure(1, "s")];
    let many: Vec<Value> = (0..7).map(|i| read(&format!("/f{i}"))).collect();
    evs.push(("t", 1, many));
    let s = run(&t, &earlier, &evs);
    let d = of_kind(&s, SignalKind::Discovery);
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].wasted_calls, 5);
    assert_eq!(
        (d[0].excerpt_ref.from_line, d[0].excerpt_ref.to_line),
        (1, 7)
    );
    // too little history
    let s = run(&t, &earlier[..2], &evs);
    assert!(of_kind(&s, SignalKind::Discovery).is_empty());
    // 4 explore calls is not more than 2x a median of 2
    evs.pop();
    evs.push(("t", 1, (0..4).map(|i| read(&format!("/f{i}"))).collect()));
    assert!(of_kind(&run(&t, &earlier, &evs), SignalKind::Discovery).is_empty());
}

#[test]
fn evolution_verify_signature_seen_in_earlier_task() {
    let mut old = task("old", 1);
    old.fixture_data().attempts = vec![attempt_with_failure(1, "verify:boom")];
    let mut newer = task("new", 20);
    newer.fixture_data().attempts = vec![
        attempt_with_failure(1, "verify:boom"),
        attempt_with_failure(2, "verify:other"),
    ];
    let s = run(&newer, &[old.clone()], &[]);
    let v = of_kind(&s, SignalKind::VerifySignature);
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].attempt, 1);
    // a later task or another repo does not count
    old.fixture_data().created_at = 30;
    assert!(of_kind(
        &run(&newer, &[old.clone()], &[]),
        SignalKind::VerifySignature
    )
    .is_empty());
    old.fixture_data().created_at = 1;
    old.fixture_data().repo = "/elsewhere".into();
    assert!(of_kind(&run(&newer, &[old], &[]), SignalKind::VerifySignature).is_empty());
}

#[test]
fn evolution_review_finding_matches_normalised_earlier_finding() {
    let mut old = task("old", 1);
    old.fixture_data().attempts = vec![reviewed(
        attempt_with_failure(1, "s"),
        &["missing test at /a/b.rs:12"],
    )];
    let mut newer = task("new", 20);
    newer.fixture_data().attempts = vec![reviewed(
        attempt_with_failure(1, "t"),
        &["missing test at /c/d.rs:99", "unrelated"],
    )];
    let s = run(&newer, &[old], &[]);
    let r = of_kind(&s, SignalKind::ReviewFinding);
    assert_eq!(r.len(), 1);
    assert!(r[0].detail.contains("missing test at"));
}

#[test]
fn evolution_process_read_counts_matching_calls_of_the_repos_own_files() {
    let mut t = task("t", 10);
    t.fixture_data().attempts = vec![attempt_with_failure(1, "s")];
    let settings = process_settings(&t.repo, 2);
    let evs = vec![
        read("/wt/docs/LESSONS.md"),
        read("/wt/src/a.rs"),
        read("/wt/docs/LESSONS.md"),
    ];
    let events: AttemptEvents = [(("t".to_string(), 1), evs)].into();
    let s = Detection::for_task(&t, &events, &settings).finish(std::slice::from_ref(&t), &[], &[]);
    let p = of_kind(&s, SignalKind::ProcessRead);
    assert_eq!(p.len(), 1);
    assert_eq!(p[0].wasted_calls, 2);
    assert_eq!(
        (p[0].excerpt_ref.from_line, p[0].excerpt_ref.to_line),
        (1, 3)
    );
}

#[test]
fn evolution_process_read_needs_the_threshold_and_a_per_repo_list() {
    let mut t = task("t", 10);
    t.fixture_data().attempts = vec![attempt_with_failure(1, "s")];
    let one = vec![read("/wt/docs/LESSONS.md"), read("/wt/src/a.rs")];
    let events: AttemptEvents = [(("t".to_string(), 1), one)].into();
    let go = |settings: &Settings, events: &AttemptEvents| {
        of_kind(
            &Detection::for_task(&t, events, settings).finish(std::slice::from_ref(&t), &[], &[]),
            SignalKind::ProcessRead,
        )
        .len()
    };
    // One call is below the default threshold of three.
    assert_eq!(go(&process_settings(&t.repo, 3), &events), 0);
    assert_eq!(go(&process_settings(&t.repo, 1), &events), 1);
    // The list belongs to one repo: another repo's entry does not apply,
    // and the global work buckets no longer feed this signal.
    assert_eq!(go(&process_settings("/elsewhere", 1), &events), 0);
    let mut global = Settings::default();
    global.evolution.process_min_calls = 1;
    global.work_buckets.process = vec!["docs/LESSONS.md".into()];
    assert_eq!(go(&global, &events), 0);
}

#[test]
fn evolution_discovery_compares_first_attempts_with_first_attempts() {
    // Earlier tasks: a first attempt explores 10 files, a retry only 1.
    let mut earlier = Vec::new();
    let mut evs: Vec<(&str, u32, Vec<Value>)> = Vec::new();
    for id in ["e1", "e2", "e3"] {
        let mut t = task(id, 1);
        t.fixture_data().attempts =
            vec![attempt_with_failure(1, "s"), attempt_with_failure(2, "s")];
        earlier.push(t);
        evs.push((id, 1, (0..10).map(|i| read(&format!("/x{i}"))).collect()));
        evs.push((id, 2, vec![read("/y")]));
    }
    let mut t = task("t", 10);
    t.fixture_data().attempts = vec![attempt_with_failure(1, "s")];
    evs.push(("t", 1, (0..12).map(|i| read(&format!("/f{i}"))).collect()));
    // 12 explore calls against a first-attempt median of 10: normal.
    // (Pooled with the retries the median would be 1 and this would fire.)
    assert!(of_kind(&run(&t, &earlier, &evs), SignalKind::Discovery).is_empty());
    // A retry that explores 12 is compared with retries and does fire.
    let mut t2 = task("t2", 10);
    t2.fixture_data().attempts = vec![attempt_with_failure(1, "s"), attempt_with_failure(2, "s")];
    let mut evs2: Vec<(&str, u32, Vec<Value>)> = evs.clone();
    evs2.push(("t2", 1, vec![read("/a")]));
    evs2.push(("t2", 2, (0..12).map(|i| read(&format!("/g{i}"))).collect()));
    let s = run(&t2, &earlier, &evs2);
    let d = of_kind(&s, SignalKind::Discovery);
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].attempt, 2);
}

#[test]
fn evolution_the_owners_retry_answer_is_not_a_review_finding() {
    let mut old = task("old", 1);
    old.fixture_data().attempts = vec![reviewed(
        attempt_with_failure(1, "s"),
        &["Another attempt was asked for: retry"],
    )];
    let mut newer = task("new", 20);
    newer.fixture_data().attempts = vec![
        reviewed(
            attempt_with_failure(1, "t"),
            &["Another attempt was asked for: retry"],
        ),
        reviewed(
            attempt_with_failure(2, "u"),
            &["Another attempt was asked for: retry"],
        ),
    ];
    let s = run(&newer, &[old], &[]);
    assert!(of_kind(&s, SignalKind::ReviewFinding).is_empty());
    assert!(of_kind(&s, SignalKind::RepeatedReview).is_empty());
}

#[test]
fn evolution_repeated_review_on_consecutive_attempts() {
    let mut t = task("t", 10);
    t.fixture_data().attempts = vec![
        reviewed(attempt_with_failure(1, "a"), &["no test for x at line 4"]),
        reviewed(attempt_with_failure(2, "b"), &["no test for x at line 9"]),
        reviewed(attempt_with_failure(3, "c"), &["different"]),
    ];
    let s = run(&t, &[], &[]);
    let r = of_kind(&s, SignalKind::RepeatedReview);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].attempt, 2);
}

#[test]
fn evolution_preexisting_one_signal_per_base_failure() {
    let t = task("t", 10);
    let s = Detection::for_task(&t, &AttemptEvents::new(), &Settings::default()).finish(
        std::slice::from_ref(&t),
        &[],
        &["npm test".to_string(), "cargo clippy".to_string()],
    );
    let p = of_kind(&s, SignalKind::Preexisting);
    assert_eq!(p.len(), 2);
    assert!(p[0].detail.contains("npm test"));
    assert_eq!((p[0].attempt, p[0].wasted_calls), (0, 0));
}
