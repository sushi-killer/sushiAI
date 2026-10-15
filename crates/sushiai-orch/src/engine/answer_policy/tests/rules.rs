use super::*;

fn opts(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn the_wait_default_never_takes_an_option_under_a_hard_limit() {
    let (app, _dir) = crate::engine::test_support::test_app();
    app.settings.write().unwrap().answer_policy = true;
    let task = task_with_status(TaskStatus::Waiting);
    let mut question = Question::new(
        "Which layout?",
        opts(&["wide", "narrow"]),
        QuestionKind::AgentQuestion,
        AskedBy::Implement,
        crate::clock::now_ms(),
    );
    question.default_option = Some(1);
    assert_eq!(
        default_choice(&app, &task, &question).as_deref(),
        Some("narrow")
    );
    question.text = "Clean up the old data?".into();
    question.options = opts(&["delete the old data", "keep the old data"]);
    assert_eq!(default_choice(&app, &task, &question), None);
    question.default_option = Some(0);
    assert_eq!(default_choice(&app, &task, &question), None);
    // The agent's own tag limits an option the words do not.
    question.options = opts(&["go ahead", "wait"]);
    question.text = "How should it go?".into();
    question.risk = vec![Risk::Destructive];
    assert_eq!(default_choice(&app, &task, &question), None);
}

#[test]
fn a_check_is_the_owners_unless_the_planner_wrote_it_for_a_file_of_its_own() {
    let settings = Settings::default();
    let mut task = task_with_status(TaskStatus::Waiting);
    planner_made(&mut task, "pytest tests/test_new.py", "tests/test_new.py");
    planner_made(
        &mut task,
        "node --test tests/a.test.cjs README.md",
        "tests/a.test.cjs",
    );
    for command in [
        "go test ./...",
        "pytest",
        "cargo nextest run",
        "just ci",
        "./gradlew test",
        "npx vitest run",
        "npm run test:daemon",
        "pytest tests/test_other.py",
        "",
    ] {
        assert_eq!(
            is_shared_check(&settings, &task, command),
            !command.is_empty(),
            "{command}"
        );
    }
    assert!(!is_shared_check(
        &settings,
        &task,
        "pytest   tests/test_new.py"
    ));
}

#[test]
fn a_planner_command_that_the_owner_or_the_settings_name_stays_the_owners() {
    let mut settings = Settings::default();
    let mut task = task_with_status(TaskStatus::Waiting);
    planner_made(&mut task, "pytest tests/test_new.py", "tests/test_new.py");
    assert!(!is_shared_check(
        &settings,
        &task,
        "pytest tests/test_new.py"
    ));
    task.fixture_data().planner_tests.sources[0].source = CheckSource::Owner;
    assert!(is_shared_check(
        &settings,
        &task,
        "pytest tests/test_new.py"
    ));
    task.fixture_data().planner_tests.sources[0].source = CheckSource::Planner;
    settings.after_land.push(AfterLand {
        repo: task.repo.clone(),
        run: "pytest tests/test_new.py".into(),
    });
    assert!(is_shared_check(
        &settings,
        &task,
        "pytest tests/test_new.py"
    ));
}

fn sourced(task: &mut Task, command: &str, source: CheckSource) {
    task.fixture_data()
        .planner_tests
        .sources
        .push(SourcedCheck {
            command: command.into(),
            source,
        });
}

#[test]
fn a_whole_suite_in_verify_is_the_repos_and_a_planner_file_check_is_not() {
    let repo = tempfile::tempdir().unwrap();
    init_git_repo(repo.path());
    let mut task = task_with_status(TaskStatus::Waiting);
    task.fixture_data().repo = repo.path().to_string_lossy().into_owned();
    task.fixture_data().base_sha = git_in(repo.path(), &["rev-parse", "HEAD"]);
    let suites = [
        "go test ./...",
        "pytest",
        "just ci",
        "./gradlew test",
        "tox",
        "make check",
        "cargo nextest run",
        "cargo test --workspace",
        "pytest tests tests/new_test.py",
    ];
    task.fixture_data().verify = suites.iter().map(|c| c.to_string()).collect();
    task.fixture_data().verify.push("test -f new/a.txt".into());
    task.fixture_data().criteria = vec!["x -- check: go test ./...".into()];
    record_plan_check_sources(&mut task, repo.path());
    let settings = Settings::default();
    for command in suites {
        assert!(is_shared_check(&settings, &task, command), "{command}");
    }
    assert!(!is_shared_check(&settings, &task, "test -f new/a.txt"));
}

#[test]
fn a_planner_check_that_cannot_pass_on_a_moved_base_is_droppable() {
    let settings = Settings::default();
    let mut task = task_with_status(TaskStatus::Waiting);
    task.fixture_data().criteria = vec![
        "The version is 9 -- check: grep -qF 'pub const V: u32 = 9;' src/brief.rs && git diff --quiet".into(),
    ];
    let command = "grep -qF 'pub const V: u32 = 9;' src/brief.rs && git diff --quiet";
    sourced(&mut task, command, CheckSource::Planner);
    assert!(!is_shared_check(&settings, &task, command));
    assert_eq!(
        removal_refusal(&settings, &task, &[command.into()], Origin::Other),
        None
    );
}

#[test]
fn a_repo_or_owner_check_stays_shared_for_a_non_owner() {
    let mut settings = Settings::default();
    let mut task = task_with_status(TaskStatus::Waiting);
    sourced(&mut task, "npm test", CheckSource::Repo);
    sourced(&mut task, "cargo test --workspace", CheckSource::Repo);
    sourced(&mut task, "grep -q x a.txt", CheckSource::Owner);
    for command in [
        "npm test",
        "cargo test --workspace",
        "grep -q x a.txt",
        "make unknown",
    ] {
        assert!(is_shared_check(&settings, &task, command), "{command}");
        assert!(
            removal_refusal(&settings, &task, &[command.into()], Origin::Other).is_some(),
            "{command}"
        );
        assert_eq!(
            removal_refusal(&settings, &task, &[command.into()], Origin::Owner),
            None
        );
    }
    // A setting that names a planner-recorded command keeps it shared.
    sourced(&mut task, "pytest tests/x.py", CheckSource::Planner);
    assert!(!is_shared_check(&settings, &task, "pytest tests/x.py"));
    settings.after_land.push(AfterLand {
        repo: task.repo.clone(),
        run: "pytest tests/x.py".into(),
    });
    assert!(is_shared_check(&settings, &task, "pytest tests/x.py"));
}

#[test]
fn a_cautious_or_reversible_option_is_found() {
    let o = opts(&["rewrite it", "keep the old API (cautious)"]);
    assert_eq!(
        cautious_option(&o).map(String::as_str),
        Some("keep the old API (cautious)")
    );
    let o = opts(&["a reversible migration", "drop the table"]);
    assert_eq!(
        cautious_option(&o).map(String::as_str),
        Some("a reversible migration")
    );
}

#[test]
fn a_negated_option_is_not_cautious() {
    assert!(cautious_option(&opts(&["an irreversible migration", "not cautious"])).is_none());
    assert!(cautious_option(&opts(&["yes", "no"])).is_none());
}

#[test]
fn the_impossible_question_yields_its_criterion_and_evidence() {
    let mut task = task_with_status(TaskStatus::Running);
    task.fixture_data().criteria = vec!["zero".into(), "one. Evidence: nope".into()];
    // The criterion and the evidence hold the old separator: nothing is parsed back.
    let evidence = "see a. Evidence: b";
    let claim = crate::engine::attempts::report::Impossible {
        criterion: 1,
        evidence: evidence.into(),
    };
    let q = impossible_question(&task, &claim);
    assert_eq!(impossible_parts(&q), Some((1, evidence.to_string())));
}

#[test]
fn a_judgement_reads_its_flag() {
    assert!(matches!(
        parse_judgement("```json\n{\"verified\": true, \"reason\": \"seen\"}\n```"),
        Some(Judgement::Verified(r)) if r == "seen"
    ));
    assert!(matches!(
        parse_judgement("{\"verified\": false}"),
        Some(Judgement::Refuted(_))
    ));
    assert!(parse_judgement("no idea").is_none());
}

#[test]
fn attempts_keep_failing_has_no_blind_continue_rule() {
    let mut task = task_with_status(TaskStatus::Waiting);
    task.fixture_data().attempts = vec![attempt_with_failure(1, "review:x")];
    let q = Question::new(
        "Attempts keep failing",
        opts(&["continue", "stop"]),
        QuestionKind::AttemptsFailing,
        AskedBy::Implement,
        crate::clock::now_ms(),
    );
    // The failure ladder owns this kind: a changed approach, never a
    // plain continue.
    assert!(rule_answer(&task, &q, 1).is_none());
}

fn base_check_question(commands: &[&str], options: &[&str]) -> Question {
    let failed: Vec<VerifyOutcome> = commands
        .iter()
        .map(|c| VerifyOutcome {
            command: c.to_string(),
            code: Some(1),
            tail: "boom".to_string(),
            ms: 1,
            ..Default::default()
        })
        .collect();
    let mut q =
        crate::engine::verification::base::questions::pre_existing_question(&failed, "0123456789");
    q.options = opts(options);
    q
}

#[test]
fn drop_all_with_a_shared_check_is_refused_for_a_non_owner() {
    let settings = Settings::default();
    let task = task_with_status(TaskStatus::Waiting);
    let q = base_check_question(
        &["node --test tests/x.test.cjs", "npm run ci"],
        &["retry after the base is fixed", "drop all", "stop"],
    );
    let mut task = task;
    planner_made(
        &mut task,
        "node --test tests/x.test.cjs",
        "tests/x.test.cjs",
    );
    let why = shared_check_drop_refusal(&settings, &task, &q, "drop all").unwrap();
    assert!(
        why.contains("`npm run ci`") && why.contains("only the owner"),
        "{why}"
    );
    // Retry and stop are not removals.
    assert!(shared_check_drop_refusal(&settings, &task, &q, "stop").is_none());
    // Only task-specific checks: droppable by anyone.
    let q = base_check_question(&["node --test tests/x.test.cjs"], &["drop all"]);
    assert!(shared_check_drop_refusal(&settings, &task, &q, "drop all").is_none());
    // Nothing is removed on a refusal: the owner's route is the only one.
    task.fixture_data().verify = vec!["npm run ci".into(), "node --test tests/x.test.cjs".into()];
    let q = base_check_question(
        &["npm run ci", "node --test tests/x.test.cjs"],
        &["drop all"],
    );
    assert!(removal_refusal(&settings, &task, &q.commands, Origin::Other).is_some());
    assert_eq!(task.verify.len(), 2);
}

#[test]
fn a_base_check_rule_fires_once_per_command() {
    let mut task = task_with_status(TaskStatus::Waiting);
    let keep = crate::engine::verification::base::answers::KEEP_ALL;
    let first = base_check_question(&["a.sh", "b.sh"], &[keep, "drop all", "stop"]);
    let (answer, _) = rule_answer_with_context(&task, &first, 1, &[true, true], false).unwrap();
    assert_eq!(answer, keep);
    record_policy_answer(
        &mut task,
        &first,
        &AutomaticAnswer::Policy {
            text: answer.to_string(),
            evidence: ("e").to_string(),
        },
        1,
    );
    // The same commands again: covered.
    assert!(rule_answer_with_context(&task, &first, 1, &[true, true], false).is_none());
    // A new command in the list: the rule fires again.
    let second = base_check_question(&["b.sh", "c.sh"], &[keep, "drop all", "stop"]);
    assert!(rule_answer_with_context(&task, &second, 1, &[true, true], false).is_some());
    record_policy_answer(
        &mut task,
        &second,
        &AutomaticAnswer::Policy {
            text: (keep).to_string(),
            evidence: ("e").to_string(),
        },
        1,
    );
    assert!(rule_answer_with_context(&task, &second, 1, &[true, true], false).is_none());

    // The retry rule of a single task is per command too.
    let retry = PRE_EXISTING_RETRY;
    let mut task = task_with_status(TaskStatus::Waiting);
    let one = base_check_question(&["a.sh"], &[retry, "drop all", "stop"]);
    let (answer, _) = rule_answer_with_context(&task, &one, 1, &[false], true).unwrap();
    assert_eq!(answer, retry);
    record_policy_answer(
        &mut task,
        &one,
        &AutomaticAnswer::Policy {
            text: answer.to_string(),
            evidence: ("e").to_string(),
        },
        1,
    );
    assert!(rule_answer_with_context(&task, &one, 1, &[false], true).is_none());
    let other = base_check_question(&["a.sh", "d.sh"], &[retry, "drop all", "stop"]);
    assert!(rule_answer_with_context(&task, &other, 1, &[false, false], true).is_some());
}

#[test]
fn preexisting_policy_keeps_targets_retries_only_moving_bases_and_otherwise_defers() {
    let mut task = task_with_status(TaskStatus::Waiting);
    let command = "node --test tests/focus.test.cjs";
    task.fixture_data().request = Some(format!("Make {command} pass"));
    let question = crate::engine::verification::base::questions::pre_existing_question(
        &[VerifyOutcome {
            command: command.into(),
            code: Some(1),
            tail: "test a_base_retry_carries_the_planner_test_commit ... FAILED".into(),
            ..Default::default()
        }],
        &task.base_sha,
    );
    let targeted = crate::engine::verification::base::answers::is_task_target(&task, command, &[]);
    let (answer, evidence) =
        rule_answer_with_context(&task, &question, 1, &[targeted], false).unwrap();
    assert_eq!(answer, crate::engine::verification::base::answers::KEEP_ALL);
    assert!(evidence.contains("task is meant to make the check pass"));

    task.fixture_data().request = None;
    let path_command = "node --test crates/sushiai-orch/tests/base_retry.rs";
    let path_output = "error in crates/sushiai-orch/tests/base_retry.rs:17:2";
    let path_target = crate::engine::verification::base::answers::is_task_target(
        &task,
        path_command,
        &["crates/sushiai-orch/tests/base_retry.rs".into()],
    );
    assert!(path_target);
    let path_question = crate::engine::verification::base::questions::pre_existing_question(
        &[VerifyOutcome {
            command: path_command.into(),
            code: Some(1),
            tail: path_output.into(),
            ..Default::default()
        }],
        &task.base_sha,
    );
    let (answer, _) =
        rule_answer_with_context(&task, &path_question, 1, &[path_target], false).unwrap();
    assert_eq!(answer, crate::engine::verification::base::answers::KEEP_ALL);

    let control = base_check_question(&["make verify"], &[PRE_EXISTING_RETRY, "drop all", "stop"]);
    assert!(rule_answer_with_context(&task, &control, 1, &[false], false).is_none());
    let (answer, _) = rule_answer_with_context(&task, &control, 1, &[false], true).unwrap();
    assert_eq!(answer, PRE_EXISTING_RETRY);

    let parent_options = base_check_question(
        &["make verify"],
        &[
            crate::engine::verification::base::answers::KEEP_ALL,
            "drop all",
            "stop",
        ],
    );
    for base_moves in [false, true] {
        let (answer, evidence) =
            rule_answer_with_context(&task, &parent_options, 1, &[false], base_moves).unwrap();
        assert_eq!(answer, crate::engine::verification::base::answers::KEEP_ALL);
        assert!(evidence.contains("subtasks are meant to make the check pass"));
    }

    let mixed = base_check_question(
        &[
            "cargo test -p sushiai-orch --test base_retry",
            "make verify",
        ],
        &[PRE_EXISTING_RETRY, "drop all", "stop"],
    );
    let (answer, _) = rule_answer_with_context(&task, &mixed, 1, &[true, false], true).unwrap();
    assert_eq!(answer, PRE_EXISTING_RETRY);
    assert!(rule_answer_with_context(&task, &mixed, 1, &[true, false], false).is_none());
}
