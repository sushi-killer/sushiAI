#[test]
fn resume_eligibility_requires_a_failed_check_session_on_the_same_route() {
    let mut task = task_with_status(TaskStatus::Running);
    let route = Route {
        id: "claude-sonnet".into(),
        label: "Claude".into(),
        harness: Harness::Claude,
        model: Some("sonnet".into()),
        effort: None,
        profile_id: None,
        account_id: None,
        strength: None,
    };
    let mut previous = attempt_with_failure(1, "verify");
    previous.session_id = Some("sess-1".into());
    task.attempts.push(previous);
    assert!(resume_attempt(&task, &route, false, false).is_some());
    assert!(resume_attempt(&task, &route, true, false).is_none());
    assert!(resume_attempt(&task, &route, false, true).is_none());
    let mut changed = route.clone();
    changed.id = "other".into();
    assert!(resume_attempt(&task, &changed, false, false).is_none());
    changed = route.clone();
    changed.harness = Harness::Codex;
    assert!(resume_attempt(&task, &changed, false, false).is_none());
    changed = route.clone();
    changed.model = Some("opus".into());
    assert!(resume_attempt(&task, &changed, false, false).is_none());
    task.attempts[0].session_id = None;
    assert!(resume_attempt(&task, &route, false, false).is_none());
    task.attempts[0].session_id = Some("sess-1".into());
    for kind in [
        FailureKind::Loop,
        FailureKind::Conflict,
        FailureKind::Review,
    ] {
        task.attempts[0].failure.as_mut().unwrap().kind = kind;
        assert!(
            resume_attempt(&task, &route, false, false).is_none(),
            "{kind:?}"
        );
    }
    task.attempts[0].review = Some(ReviewResult {
        verdict: Verdict::Fail,
        findings: vec!["[P1] wrong".into()],
        repeated: vec!["[P1] wrong".into()],
        severities: vec![],
        details: vec![],
        criteria: vec![],
    });
    assert!(last_review_repeated(&task));
    assert!(resume_attempt(&task, &route, false, false).is_none());
}

#[test]
fn a_resume_brief_keeps_failure_output_messages_and_new_owner_answers() {
    let mut task = task_with_status(TaskStatus::Running);
    let mut previous = attempt_with_failure(1, "verify");
    previous.started_at = 100;
    previous.failure.as_mut().unwrap().detail =
        "Final check check.sh exited 9.\nfailure tail".into();
    previous.verify = vec![VerifyOutcome {
        command: "check.sh".into(),
        code: Some(9),
        tail: "failure tail".into(),
        ms: 1,
        ..Default::default()
    }];
    for (answer, at, by) in [
        ("old answer", 99, AnsweredBy::Owner),
        ("new answer", 101, AnsweredBy::Owner),
        ("policy answer", 102, AnsweredBy::Policy),
    ] {
        task.question_history.push(AnsweredQuestion {
            question: "Which fix?".into(),
            options: vec![],
            kind: QuestionKind::AttemptsFailing,
            asked_by: Some(AskedBy::Verify),
            asked_at: Some(100),
            answer: answer.into(),
            answered_at: at,
            answered_by: by,
            failure_signature: None,
            commands: vec![],
        });
    }
    let brief = brief::build_resume_brief(
        &task,
        &previous,
        "\n## Messages\n\nA peer's update",
        Some("Fix check.sh"),
    );
    for text in [
        "check.sh exited 9.",
        "failure tail",
        "A peer's update",
        "new answer",
        "## Advisor",
        "Fix check.sh",
        "## Report format",
    ] {
        assert!(brief.contains(text), "{text}: {brief}");
    }
    assert!(!brief.contains("## Task"));
    assert!(!brief.contains("old answer") && !brief.contains("policy answer"));
    assert_eq!(brief.matches("failure tail").count(), 1, "{brief}");
}

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
    task.tier_fallback = Some("no planner tier".into());
    task.attempts = vec![attempt_with_failure(1, "sig-a")];
    assert!(advance_after_failure(&mut task, 4, &[]));
    assert_eq!(task.tier, Tier::Standard);
    assert_eq!(task.tier_fallback.as_deref(), Some("no planner tier"));
    // The same failure again moves the task up a tier: the note goes.
    task.attempts.push(attempt_with_failure(2, "sig-a"));
    assert!(advance_after_failure(&mut task, 4, &[]));
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
        repeated_reviews: 0,
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
        repeated_reviews: 0,
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
        repeated_reviews: 0,
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
        repeated_reviews: 0,
        attempt_n: 4,
        max_attempts: 4,
    };
    match decide_after_failure(&input) {
        FailureDecision::Waiting { .. } => {}
        _ => panic!("expected waiting"),
    }
}

#[test]
fn a_second_loop_escalates_the_tier_even_with_a_different_signature() {
    let input = |loops| FailureDecisionInput {
        tier: Tier::Standard,
        signature: "loop:b",
        previous_signature: Some("loop:a"),
        consecutive_same: 1,
        loops,
        repeated_reviews: 0,
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

#[test]
fn a_repeated_review_finding_tiers_up_once_then_waits() {
    let input = |repeated_reviews| FailureDecisionInput {
        tier: Tier::Standard,
        signature: "review:x",
        previous_signature: Some("review:y"),
        consecutive_same: 1,
        loops: 0,
        repeated_reviews,
        attempt_n: 2,
        max_attempts: 4,
    };
    assert_eq!(
        decide_after_failure(&input(0)),
        FailureDecision::NextAttempt {
            tier: Tier::Standard
        }
    );
    assert_eq!(
        decide_after_failure(&input(1)),
        FailureDecision::NextAttempt { tier: Tier::Hard }
    );
    assert!(matches!(
        decide_after_failure(&input(2)),
        FailureDecision::Waiting { .. }
    ));
}

fn failed_with(kind: FailureKind, verify_codes: &[Option<i32>], changed: &[&str]) -> Task {
    let mut task = task_with_status(TaskStatus::Running);
    let mut a = attempt_with_failure(1, "sig");
    a.failure.as_mut().unwrap().kind = kind;
    a.verify = verify_codes
        .iter()
        .map(|c| VerifyOutcome {
            command: "cmd".into(),
            code: *c,
            tail: String::new(),
            ms: 1,
            ..Default::default()
        })
        .collect();
    a.changed_files = changed.iter().map(|f| f.to_string()).collect();
    task.attempts = vec![a];
    task
}

fn options_after(task: &mut Task, protected: &[&str]) -> Vec<String> {
    let protected: Vec<String> = protected.iter().map(|p| p.to_string()).collect();
    assert!(!advance_after_failure(task, 1, &protected));
    task.question.clone().unwrap().options
}

#[test]
fn accepting_is_offered_before_stop_after_review_and_evidence_with_clean_verify() {
    for kind in [FailureKind::Review, FailureKind::Evidence] {
        let mut task = failed_with(kind, &[Some(0), Some(0)], &["src/a.ts"]);
        assert_eq!(
            options_after(&mut task, &["src/app/**"]),
            vec!["continue", "change approach", brief::ACCEPT_LAST_ATTEMPT, "stop"]
        );
    }
    let mut task = failed_with(FailureKind::Review, &[], &[]);
    assert!(options_after(&mut task, &[]).contains(&brief::ACCEPT_LAST_ATTEMPT.to_string()));
}

#[test]
fn accepting_is_not_offered_after_other_failures_or_a_bad_verify() {
    for kind in [
        FailureKind::Verify,
        FailureKind::Heldout,
        FailureKind::Error,
        FailureKind::NoDeliverable,
    ] {
        let mut task = failed_with(kind, &[Some(0)], &["a.rs"]);
        assert_eq!(
            options_after(&mut task, &[]),
            vec!["continue", "change approach", "stop"]
        );
    }
    for codes in [vec![Some(0), Some(1)], vec![None]] {
        let mut task = failed_with(FailureKind::Review, &codes, &[]);
        assert!(!options_after(&mut task, &[]).contains(&brief::ACCEPT_LAST_ATTEMPT.to_string()));
    }
}

#[test]
fn accepting_is_not_offered_after_evidence_when_a_changed_file_is_protected() {
    let mut task = failed_with(FailureKind::Evidence, &[Some(0)], &["src/x.ts", "src/app/Shell.tsx"]);
    assert!(!options_after(&mut task, &["src/app/**"]).contains(&brief::ACCEPT_LAST_ATTEMPT.to_string()));
    // A review failure means the protected-path approval already happened.
    let mut task = failed_with(FailureKind::Review, &[Some(0)], &["src/app/Shell.tsx"]);
    assert!(options_after(&mut task, &["src/app/**"]).contains(&brief::ACCEPT_LAST_ATTEMPT.to_string()));
}

#[test]
fn a_pending_acceptance_is_the_failed_review_attempt_with_the_accept_line_last() {
    let mut task = failed_with(FailureKind::Review, &[Some(0)], &[]);
    assert_eq!(pending_acceptance(&task), None);
    task.decisions.push(accept_line(1));
    assert_eq!(pending_acceptance(&task), Some(0));
    task.decisions.push("Owner: continue".into());
    assert_eq!(pending_acceptance(&task), None);
    let mut task = failed_with(FailureKind::Verify, &[Some(0)], &[]);
    task.decisions.push(accept_line(1));
    assert_eq!(pending_acceptance(&task), None);
}

#[test]
fn the_exhausted_question_reads_the_signature_as_words() {
    let question = |signature: &str| match decide_after_failure(&FailureDecisionInput {
        tier: Tier::Standard,
        signature,
        previous_signature: None,
        consecutive_same: 1,
        loops: 0,
        repeated_reviews: 0,
        attempt_n: 4,
        max_attempts: 4,
    }) {
        FailureDecision::Waiting { question } => question,
        other => panic!("expected a question, got {other:?}"),
    };
    // What an exit with no stderr used to leave: "exit status: 1." with
    // its digit stripped.
    assert_eq!(
        question("error:Claude exited with exit status: ."),
        "Attempts keep failing with an error (Claude exited with exit status): continue, change approach, or stop?"
    );
    assert_eq!(
        question("error:Claude exited with an error."),
        "Attempts keep failing with an error (Claude exited with an error): continue, change approach, or stop?"
    );
    assert_eq!(
        question("no_deliverable:"),
        "Attempts keep failing with a no deliverable failure: continue, change approach, or stop?"
    );
}

#[test]
fn failure_signature_picks_the_real_failure_from_real_verify_tails() {
    let cargo = |name: &str| {
        format!(
            "--- stderr (tail) ---\nerror: test failed, to rerun pass `--bin orchd`\n\
             --- stdout (tail) ---\nrunning 2 tests\ntest engine::a::ok_one ... ok\n\
             test {name} ... FAILED\n\nfailures:\n\n---- {name} stdout ----\npanicked"
        )
    };
    let eslint_then_prettier = "--- stdout (tail) ---\n\u{2716} 42 problems (0 errors, 42 warnings)\n\
         [warn] src/orchestrator/helpers.ts\n[warn] Code style issues found in 2 files. Run Prettier with --write to fix.";
    let node = "\u{2714} worktreeBranchError accepts a valid branch table (0.4ms)\n\
         \u{2716} App.tsx stays a composition layer (1.2ms)\n\u{2139} fail 1";
    let table: Vec<(String, &str)> = vec![
        (
            cargo("engine::healing::tests::evidence_survives"),
            "engine::healing::tests::evidence_survives ... FAILED",
        ),
        (
            cargo("engine::chat::tests::claude_chat_reads"),
            "engine::chat::tests::claude_chat_reads ... FAILED",
        ),
        (
            eslint_then_prettier.to_string(),
            "Code style issues found in files",
        ),
        (node.to_string(), "App.tsx stays a composition layer"),
        (
            "error: could not compile `orchd`\nerror[E0432]: unresolved import `x`".to_string(),
            "error[E]: unresolved import `x`",
        ),
    ];
    for (detail, expect) in table {
        let sig = failure_signature(FailureKind::Verify, &detail);
        assert!(sig.contains(expect), "{sig} should contain {expect}");
        assert!(!sig.contains("to rerun pass"), "{sig}");
        assert!(!sig.contains("worktreeBranchError"), "{sig}");
        assert!(!sig.contains("problems"), "{sig}");
    }
    // Two different failing tests must not collapse into one signature.
    assert_ne!(
        failure_signature(FailureKind::Verify, &cargo("a::one")),
        failure_signature(FailureKind::Verify, &cargo("a::two"))
    );
}

#[test]
fn failure_signature_without_a_named_failure_skips_boilerplate_and_clean_summaries() {
    let sig = failure_signature(
        FailureKind::Verify,
        "to rerun pass `--bin orchd`\n\u{2714} handles Error cases\nerror: something broke",
    );
    assert!(sig.contains("something broke"), "{sig}");
    // Nothing usable falls back to the first non-empty line.
    let sig = failure_signature(FailureKind::Verify, "\n\nplain output");
    assert!(sig.contains("plain output"), "{sig}");
}

#[test]
fn a_harness_error_that_changed_nothing_does_not_spend_the_attempt_budget() {
    let mut task = failed_with(FailureKind::Error, &[Some(0)], &[]);
    assert!(advance_after_failure(&mut task, 1, &[]));
    assert_eq!(task.status, TaskStatus::Queued);
    // The same error three times in a row still reaches the owner.
    let mut task = failed_with(FailureKind::Error, &[Some(0)], &[]);
    task.attempts = (1..=3)
        .map(|n| {
            let mut a = attempt_with_failure(n, "error: boom");
            a.failure.as_mut().unwrap().kind = FailureKind::Error;
            a
        })
        .collect();
    assert!(!advance_after_failure(&mut task, 9, &[]));
}

fn error_attempt(n: u32, status: AttemptStatus) -> Attempt {
    // Each error carries a different id, so no two signatures match.
    let mut a = attempt_with_failure(n, &format!("error: request req_{n}x failed"));
    a.failure.as_mut().unwrap().kind = FailureKind::Error;
    a.status = status;
    a
}

#[test]
fn varying_id_errors_stop_at_twice_the_attempt_budget() {
    let mut task = task_with_status(TaskStatus::Running);
    task.attempts = (1..=7).map(|n| error_attempt(n, AttemptStatus::Failed)).collect();
    assert!(advance_after_failure(&mut task, 4, &[]));
    task.attempts.push(error_attempt(8, AttemptStatus::Failed));
    assert!(!advance_after_failure(&mut task, 4, &[]));
}

#[test]
fn errors_alternating_with_pauses_still_stop_but_pauses_alone_do_not_count() {
    let mut task = task_with_status(TaskStatus::Running);
    let mut attempts = Vec::new();
    for n in 1..=8 {
        let status = if n % 2 == 0 {
            AttemptStatus::Interrupted
        } else {
            AttemptStatus::Failed
        };
        attempts.push(error_attempt(n, status));
    }
    // Pauses that were real limit pauses stay out of the ceiling.
    task.queue.paused_attempts = 4;
    task.attempts = attempts.clone();
    assert!(advance_after_failure(&mut task, 4, &[]));
    // Interrupted attempts that were not limit pauses do count.
    task.queue.paused_attempts = 0;
    task.attempts = attempts;
    assert!(!advance_after_failure(&mut task, 4, &[]));
}

#[test]
fn ids_and_hashes_do_not_change_a_signature_and_interrupted_attempts_do_not_break_a_run() {
    let sig = |d: &str| failure_signature(FailureKind::Error, d);
    assert_eq!(
        sig("Error: request req_01AbCdEfGh failed at deadbeef12"),
        sig("Error: request req_09ZyXwVuTs failed at 0123abcd99")
    );
    assert_ne!(sig("Error: disk full"), sig("Error: out of memory"));
    let mut interrupted = attempt_with_failure(2, "x");
    interrupted.failure = None;
    interrupted.status = AttemptStatus::Interrupted;
    let attempts = vec![
        attempt_with_failure(1, "s"),
        interrupted,
        attempt_with_failure(3, "s"),
    ];
    assert_eq!(consecutive_same_signature(&attempts, "s"), 2);
}
