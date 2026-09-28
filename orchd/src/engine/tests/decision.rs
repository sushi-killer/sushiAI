#[test]
fn failure_signature_prefers_an_error_looking_line_over_the_first_line() {
    let detail =
        "Compiling...\nWarning: unused variable\nError: assertion failed at line 42\nmore noise";
    let sig = failure_signature(FailureKind::Verify, detail);
    assert!(sig.contains("assertion failed at line"));
    assert!(!sig.contains("Compiling"));
    // Digits and the line number are stripped so two runs at different
    // line numbers still normalize to the same signature.
    assert!(!sig.contains("42"));
}

#[test]
fn failure_signature_strips_absolute_paths() {
    let detail = "Error at /Users/me/repo/src/foo.ts:10: boom";
    let sig = failure_signature(FailureKind::Verify, detail);
    assert!(!sig.contains("/Users/me/repo"));
    assert!(sig.contains("boom"));
}

#[test]
fn failure_signature_falls_back_to_first_line_without_error_keywords() {
    let sig = failure_signature(FailureKind::NoDeliverable, "nothing changed\nsecond line");
    assert!(sig.contains("nothing changed"));
}

#[test]
fn failure_signature_uses_kind_and_truncates_to_120_chars() {
    let long = "error: ".to_string() + &"x".repeat(200);
    let sig = failure_signature(FailureKind::NoDeliverable, &long);
    assert!(sig.starts_with("no_deliverable:"));
    assert_eq!(sig.len(), "no_deliverable:".len() + 120);
}

#[test]
fn a_retry_on_the_same_tier_keeps_the_fallback_note() {
    let mut task = task_with_status(TaskStatus::Running);
    task.tier_fallback = Some("no classifier key".into());
    task.attempts = vec![attempt_with_failure(1, "sig-a")];
    assert!(advance_after_failure(&mut task, 4));
    assert_eq!(task.tier, Tier::Standard);
    assert_eq!(task.tier_fallback.as_deref(), Some("no classifier key"));
    // The same failure again moves the task up a tier: the note goes.
    task.attempts.push(attempt_with_failure(2, "sig-a"));
    assert!(advance_after_failure(&mut task, 4));
    assert_eq!(task.tier, Tier::Hard);
    assert_eq!(task.tier_fallback, None);
}

#[test]
fn consecutive_same_signature_counts_trailing_run_only() {
    let attempts = vec![
        attempt_with_failure(1, "sig-a"),
        attempt_with_failure(2, "sig-b"),
        attempt_with_failure(3, "sig-b"),
        attempt_with_failure(4, "sig-b"),
    ];
    assert_eq!(consecutive_same_signature(&attempts, "sig-b"), 3);
    assert_eq!(consecutive_same_signature(&attempts, "sig-a"), 0);
}

#[test]
fn decide_after_failure_ties_up_on_repeat_signature() {
    let input = FailureDecisionInput {
        tier: Tier::Mechanical,
        signature: "sig-b",
        previous_signature: Some("sig-b"),
        consecutive_same: 2,
        loops: 0,
        attempt_n: 2,
        max_attempts: 4,
    };
    match decide_after_failure(&input) {
        FailureDecision::NextAttempt { tier } => assert_eq!(tier, Tier::Standard),
        _ => panic!("expected next attempt"),
    }
}

#[test]
fn decide_after_failure_stays_same_tier_on_new_signature() {
    let input = FailureDecisionInput {
        tier: Tier::Standard,
        signature: "sig-c",
        previous_signature: Some("sig-b"),
        consecutive_same: 1,
        loops: 0,
        attempt_n: 2,
        max_attempts: 4,
    };
    match decide_after_failure(&input) {
        FailureDecision::NextAttempt { tier } => assert_eq!(tier, Tier::Standard),
        _ => panic!("expected next attempt"),
    }
}

#[test]
fn decide_after_failure_waits_after_three_consecutive() {
    let input = FailureDecisionInput {
        tier: Tier::Standard,
        signature: "sig-b",
        previous_signature: Some("sig-b"),
        consecutive_same: 3,
        loops: 0,
        attempt_n: 3,
        max_attempts: 10,
    };
    match decide_after_failure(&input) {
        FailureDecision::Waiting { question } => assert!(question.contains("sig-b")),
        _ => panic!("expected waiting"),
    }
}

#[test]
fn decide_after_failure_waits_when_attempts_exhausted() {
    let input = FailureDecisionInput {
        tier: Tier::Standard,
        signature: "sig-x",
        previous_signature: None,
        consecutive_same: 1,
        loops: 0,
        attempt_n: 4,
        max_attempts: 4,
    };
    match decide_after_failure(&input) {
        FailureDecision::Waiting { .. } => {}
        _ => panic!("expected waiting"),
    }
}

#[test]
fn decide_blocked_question_answers_self_above_threshold() {
    assert_eq!(
        decide_blocked_question(Some(0.7)),
        BlockedDecision::AnswerSelf
    );
    assert_eq!(
        decide_blocked_question(Some(0.9)),
        BlockedDecision::AnswerSelf
    );
    assert_eq!(
        decide_blocked_question(Some(0.69)),
        BlockedDecision::Waiting
    );
    assert_eq!(decide_blocked_question(None), BlockedDecision::Waiting);
}

#[test]
fn a_second_loop_escalates_the_tier_even_with_a_different_signature() {
    let input = |loops| FailureDecisionInput {
        tier: Tier::Standard,
        signature: "loop:b",
        previous_signature: Some("loop:a"),
        consecutive_same: 1,
        loops,
        attempt_n: 2,
        max_attempts: 4,
    };
    assert_eq!(
        decide_after_failure(&input(1)),
        FailureDecision::NextAttempt {
            tier: Tier::Standard
        }
    );
    assert_eq!(
        decide_after_failure(&input(2)),
        FailureDecision::NextAttempt { tier: Tier::Hard }
    );
}
