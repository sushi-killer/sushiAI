use super::*;

#[tokio::test]
async fn decision_memory_owner_precedent_answers_without_asking_the_owner() {
    let (app, dir) = overseer_app();
    let repo = repo_root(&app, dir.path()).await;
    remember(&app, &repo, &base_q(), RETRY, AnsweredBy::Owner);
    remember(&app, &repo, &base_q(), RETRY, AnsweredBy::Owner);
    let (answer, task) = park(&app, waiting_task(&repo, base_q())).await;
    assert_eq!(answer.as_deref(), Some(RETRY));
    let line = task
        .decisions
        .iter()
        .find(|d| d.starts_with(LIKE_PREFIX))
        .cloned()
        .unwrap_or_default();
    let prefix = "Overseer: answered like owner precedent: retry after the base is fixed, ";
    assert!(line.starts_with(prefix), "{line}");
    assert!(
        line.ends_with(&format!(" -- {} -> {RETRY}", base_q().text)),
        "{line}"
    );
    let answered = task.question_history.last().unwrap();
    assert_eq!(answered.answered_by, AnsweredBy::Overseer);
    assert!(task.question.is_none());
    let kinds = log_kinds(&app);
    assert!(kinds.contains(&"answered".to_string()), "{kinds:?}");
    assert!(!kinds.contains(&"asked_owner".to_string()), "{kinds:?}");
    crate::engine::chat::test_support::wait_idle(&app, &repo).await;
    assert_eq!(
        crate::engine::chat::test_support::fake_overseer_calls(Path::new(&repo)),
        0
    );
}

#[tokio::test]
async fn decision_memory_other_sources_and_disagreeing_owners_answer_nothing() {
    for by in [
        AnsweredBy::Lead,
        AnsweredBy::Policy,
        AnsweredBy::Overseer,
        AnsweredBy::Orchestrator,
        AnsweredBy::Judge,
    ] {
        let (app, dir) = overseer_app();
        let repo = repo_root(&app, dir.path()).await;
        remember(&app, &repo, &base_q(), RETRY, by);
        let (answer, _) = park(&app, waiting_task(&repo, base_q())).await;
        assert_eq!(answer, None, "{by:?}");
        crate::engine::chat::test_support::wait_idle(&app, &repo).await;
        assert_eq!(
            crate::engine::chat::test_support::fake_overseer_calls(Path::new(&repo)),
            1,
            "{by:?}"
        );
    }
    let (app, dir) = overseer_app();
    let repo = repo_root(&app, dir.path()).await;
    remember(&app, &repo, &base_q(), RETRY, AnsweredBy::Owner);
    remember(&app, &repo, &base_q(), RETRY, AnsweredBy::Owner);
    remember(&app, &repo, &base_q(), "stop", AnsweredBy::Owner);
    let (answer, _) = park(&app, waiting_task(&repo, base_q())).await;
    assert_eq!(answer, None);
    crate::engine::chat::test_support::wait_idle(&app, &repo).await;
    assert_eq!(
        crate::engine::chat::test_support::fake_overseer_calls(Path::new(&repo)),
        1
    );
}

#[tokio::test]
async fn decision_memory_a_different_failure_or_a_stop_is_not_the_same_question() {
    let (app, dir) = overseer_app();
    let repo = repo_root(&app, dir.path()).await;
    remember(&app, &repo, &base_q(), RETRY, AnsweredBy::Owner);
    remember(&app, &repo, &base_q(), RETRY, AnsweredBy::Owner);
    let mut other = base_q();
    other.commands = vec!["npm test".into()];
    assert!(app
        .precedent_answer(&waiting_task(&repo, other.clone()), &other)
        .is_none());
    let q = base_q();
    assert!(app
        .precedent_answer(&waiting_task(&repo, q.clone()), &q)
        .is_some());
    // The owner's stop is never replayed.
    let stop = free_q("Keep going?", &["go on", "stop"]);
    remember(&app, &repo, &stop, "stop", AnsweredBy::Owner);
    assert!(app
        .precedent_answer(&waiting_task(&repo, stop.clone()), &stop)
        .is_none());
}

#[test]
fn a_risk_tag_adds_its_limit_to_a_question_without_the_keyword() {
    let mut q = free_q("Which approach, A or B?", &["A", "B"]);
    assert_eq!(hard_limit(&q, "A"), None);
    q.risk = vec![Risk::Security];
    assert_eq!(hard_limit(&q, "A"), Some(HardLimit::SandboxOrSecurity));
    // A tag only adds: a question the words already put under a limit stays there.
    let mut pushing = free_q("Push the branch?", &["push", "hold"]);
    pushing.risk = vec![Risk::Money];
    assert!(hard_limit(&pushing, "push").is_some());
}

#[tokio::test]
async fn decision_memory_every_hard_limit_reaches_the_owner() {
    let (app, dir) = overseer_app();
    let repo = repo_root(&app, dir.path()).await;
    let permission = Question::new(
        "Allow Bash for this task?",
        vec!["Allow once".into(), "Deny".into()],
        QuestionKind::Permission,
        AskedBy::Implement,
        crate::clock::now_ms(),
    );
    let cases: Vec<(HardLimit, Question, &str)> = vec![
        (HardLimit::OwnerOnlyKind, permission, "Allow once"),
        (HardLimit::SharedCheckDrop, base_q(), PRE_EXISTING_DROP),
        (
            HardLimit::SandboxOrSecurity,
            free_q("Disable the sandbox for this check?", &["disable", "keep"]),
            "disable",
        ),
        (
            HardLimit::PushPrMergeRelease,
            free_q("Push the branch to origin?", &["push", "hold"]),
            "push",
        ),
        (
            HardLimit::PushPrMergeRelease,
            free_q("Merge it now?", &["merge", "hold"]),
            "merge",
        ),
        (
            HardLimit::Money,
            free_q("Use the paid plan for the load test?", &["use it", "skip"]),
            "use it",
        ),
        (
            HardLimit::Destructive,
            free_q("Delete the old fixtures?", &["delete", "keep"]),
            "delete",
        ),
        (
            HardLimit::ProductionDeploy,
            free_q("Deploy to production?", &["deploy", "wait"]),
            "deploy",
        ),
    ];
    for (limit, q, answer) in cases {
        assert_eq!(hard_limit(&q, answer), Some(limit), "{}", q.text);
        remember(&app, &repo, &q, answer, AnsweredBy::Owner);
        remember(&app, &repo, &q, answer, AnsweredBy::Owner);
        let task = waiting_task(&repo, q.clone());
        assert!(app.precedent_answer(&task, &q).is_none(), "{}", q.text);
        assert!(
            checked_like(Some("owner precedent"), Some("overseer"), &task, answer).is_err(),
            "{}",
            q.text
        );
        // A plain overseer answer, no `like`, is refused as well; the
        // owner's own answer is not.
        app.store.save_task(&task).unwrap();
        let err = app
            .handle_task_answer(json!({"id": task.id, "answer": answer, "source": "overseer"}))
            .await;
        assert!(err.is_err(), "{}", q.text);
        let still = app.store.load_task(&task.id).unwrap().unwrap();
        assert_eq!(still.status, TaskStatus::Waiting, "{}", q.text);
    }
    // Whole words only: `improve` is not a PR, a routine question is free.
    let routine = free_q("Improve the naming of the helper?", &["yes", "no"]);
    assert_eq!(hard_limit(&routine, "yes"), None);
    // End to end: the question parks and the overseer is woken.
    let push = free_q("Push the branch to origin?", &["push", "hold"]);
    let (answer, _) = park(&app, waiting_task(&repo, push)).await;
    assert_eq!(answer, None);
    crate::engine::chat::test_support::wait_idle(&app, &repo).await;
    assert_eq!(
        crate::engine::chat::test_support::fake_overseer_calls(Path::new(&repo)),
        1
    );
}

#[tokio::test]
async fn decision_memory_respects_the_continue_cap_and_the_base_retry_guard() {
    let (app, dir) = overseer_app();
    let repo = repo_root(&app, dir.path()).await;
    remember(&app, &repo, &exhausted_q(), "continue", AnsweredBy::Owner);
    remember(&app, &repo, &exhausted_q(), "continue", AnsweredBy::Owner);
    // One continue left: the precedent answers and its line counts.
    let mut task = waiting_task(&repo, exhausted_q());
    task.fixture_data()
        .decisions
        .push(format!("{EXHAUSTED_ANSWER_PREFIX} -> continue (r)"));
    task.fixture_data().loop_state.non_owner_continues = 1;
    let (answer, task) = park(&app, task).await;
    assert_eq!(answer.as_deref(), Some("continue"));
    assert_eq!(task.loop_state.non_owner_continues, 2);
    assert!(task.decisions.iter().any(|d| d.contains(" -> continue")));
    // One non-owner retry per base.
    remember(&app, &repo, &base_q(), RETRY, AnsweredBy::Owner);
    remember(&app, &repo, &base_q(), RETRY, AnsweredBy::Owner);
    let mut retried = waiting_task(&repo, base_q());
    let base_sha = retried.base_sha.clone();
    crate::engine::verification::base::answers::record_base_answer(
        &mut retried,
        &["cargo test".to_string()],
        &base_sha,
        "Lead",
        RETRY,
    );
    let (answer, _) = park(&app, retried).await;
    assert_eq!(answer, None);
    crate::engine::chat::test_support::wait_idle(&app, &repo).await;
    assert_eq!(
        crate::engine::chat::test_support::fake_overseer_calls(Path::new(&repo)),
        1
    );
}

#[tokio::test]
async fn decision_memory_two_owner_answers_propose_a_rule_the_owner_accepts() {
    let (app, dir) = test_app();
    let repo = repo_root(&app, dir.path()).await;
    let q = base_q();
    remember(&app, &repo, &q, RETRY, AnsweredBy::Owner);
    assert!(precedents::list(&app.data_dir, &repo).1.is_empty());
    remember(&app, &repo, &q, RETRY, AnsweredBy::Owner);
    remember(&app, &repo, &q, RETRY, AnsweredBy::Owner);
    let listed = app
        .handle_precedents_list(json!({"repo": repo}))
        .await
        .unwrap();
    let proposals = listed["proposals"].as_array().unwrap();
    assert_eq!(proposals.len(), 1, "{listed}");
    let rule = proposals[0]["rule"].as_str().unwrap().to_string();
    assert!(rule.chars().count() <= 160 && !rule.contains('\n'));
    assert_eq!(listed["precedents"].as_array().unwrap().len(), 1);
    assert!(app.digest_prompt(&repo).contains(&rule));
    assert!(!app.store.repo_notes_path().exists());
    // Another repo's digest does not carry it.
    assert!(!app.digest_prompt("/elsewhere").contains(&rule));

    let id = proposals[0]["id"].as_str().unwrap();
    let err = app
        .handle_precedents_rule_accept(json!({"repo": repo, "id": "nope"}))
        .await
        .unwrap_err();
    assert!(err.contains("unknown proposal"), "{err}");
    app.handle_precedents_rule_accept(json!({"repo": repo, "id": id}))
        .await
        .unwrap();
    let notes = app
        .handle_repo_notes_list(json!({"repo": repo}))
        .await
        .unwrap();
    assert_eq!(notes["notes"][0]["text"], rule.as_str());
    assert_eq!(notes["notes"][0]["source"], "owner");
    assert!(precedents::list(&app.data_dir, &repo).1.is_empty());
    remember(&app, &repo, &q, RETRY, AnsweredBy::Owner);
    assert!(precedents::list(&app.data_dir, &repo).1.is_empty());

    // A dismissed key is not proposed again.
    let other = free_q("Rename the helper?", &["yes", "no"]);
    remember(&app, &repo, &other, "yes", AnsweredBy::Owner);
    remember(&app, &repo, &other, "yes", AnsweredBy::Owner);
    let open = precedents::list(&app.data_dir, &repo).1;
    assert_eq!(open.len(), 1);
    app.handle_precedents_rule_dismiss(json!({"repo": repo, "id": open[0].id}))
        .await
        .unwrap();
    remember(&app, &repo, &other, "yes", AnsweredBy::Owner);
    assert!(precedents::list(&app.data_dir, &repo).1.is_empty());
    assert_eq!(
        app.handle_repo_notes_list(json!({"repo": repo}))
            .await
            .unwrap()["notes"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // Remove: an unknown id is an error.
    let list = precedents::list(&app.data_dir, &repo).0;
    let err = app
        .handle_precedents_remove(json!({"repo": repo, "id": "nope"}))
        .await
        .unwrap_err();
    assert!(err.contains("unknown precedent"), "{err}");
    app.handle_precedents_remove(json!({"repo": repo, "id": list[0].id}))
        .await
        .unwrap();
    assert_eq!(
        precedents::list(&app.data_dir, &repo).0.len(),
        list.len() - 1
    );
}

#[tokio::test]
async fn decision_memory_the_overseer_cannot_write_repo_notes() {
    let (app, dir) = test_app();
    let repo = repo_root(&app, dir.path()).await;
    let note = app
        .handle_repo_notes_add(json!({"repo": repo, "text": "Use tabs"}))
        .await
        .unwrap();
    for result in [
        app.handle_repo_notes_add(json!({"repo": repo, "text": "x", "source": "overseer"}))
            .await,
        app.handle_repo_notes_remove(json!({"repo": repo, "id": note["id"], "source": "overseer"}))
            .await,
    ] {
        assert!(result.unwrap_err().contains("owner's"));
    }
    let notes = app
        .handle_repo_notes_list(json!({"repo": repo}))
        .await
        .unwrap();
    assert_eq!(notes["notes"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn decision_memory_like_is_the_overseers_and_cut_to_one_line() {
    let (app, _dir) = test_app();
    let task = waiting_task("/r1", free_q("Which way?", &["A", "B"]));
    app.store.save_task(&task).unwrap();
    let err = app
        .handle_task_answer(json!({"id": task.id, "answer": "A", "source": "lead", "like": "x"}))
        .await
        .unwrap_err();
    assert!(err.contains("overseer"), "{err}");
    let long = format!("repo note\n{}", "y".repeat(300));
    app.handle_task_answer(
        json!({"id": task.id, "answer": "A", "source": "overseer", "like": long}),
    )
    .await
    .unwrap();
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    let line = saved.decisions.last().unwrap();
    let like = line
        .strip_prefix("Overseer: answered like ")
        .and_then(|l| l.split_once(" -- "))
        .unwrap()
        .0;
    assert_eq!(like.chars().count(), 120);
    assert!(!like.contains('\n'));
    assert_eq!(saved.question_history[0].answered_by, AnsweredBy::Overseer);
}

#[tokio::test]
async fn decision_memory_the_overseer_prompt_carries_the_matching_owner_precedents() {
    let (app, _dir) = test_app();
    let q = base_q();
    let task = waiting_task("/r1", q.clone());
    assert_eq!(app.with_precedents("Prompt.".into(), &[&task]), "Prompt.");
    remember(&app, "/r1", &q, RETRY, AnsweredBy::Owner);
    remember(&app, "/r1", &q, RETRY, AnsweredBy::Lead);
    let text = app.with_precedents("Prompt.".into(), &[&task]);
    assert!(text.starts_with("Prompt."));
    assert!(
        text.contains("task_answer {like}") && text.contains(RETRY),
        "{text}"
    );
    assert!(!text.contains("lead"), "{text}");
    assert_eq!(text.matches("the owner answered").count(), 1);
}

#[tokio::test]
async fn decision_memory_the_prompt_hides_precedents_that_hit_a_hard_limit() {
    let (app, _dir) = test_app();
    let q = free_q("Push the branch to origin?", &["push", "hold"]);
    remember(&app, "/r1", &q, "push", AnsweredBy::Owner);
    let task = waiting_task("/r1", q);
    assert_eq!(app.with_precedents("Prompt.".into(), &[&task]), "Prompt.");
}

#[tokio::test]
async fn decision_memory_a_like_must_name_a_precedent_that_exists() {
    let (app, dir) = overseer_app();
    let repo = repo_root(&app, dir.path()).await;
    let q = free_q("Which naming style?", &["snake", "camel"]);
    let task = waiting_task(&repo, q.clone());
    app.store.save_task(&task).unwrap();
    let err = app
        .handle_task_answer(json!({
            "id": task.id, "answer": "snake", "source": "overseer",
            "like": "owner precedent: snake, 2026-10-01"
        }))
        .await
        .unwrap_err();
    assert!(err.contains("no matching answer"), "{err}");
    remember(&app, &repo, &q, "snake", AnsweredBy::Owner);
    // A precedent for another answer does not back this one.
    let err = app
        .handle_task_answer(json!({
            "id": task.id, "answer": "camel", "source": "overseer",
            "like": "Owner precedent: camel"
        }))
        .await;
    assert!(err.is_err());
    app.handle_task_answer(json!({
        "id": task.id, "answer": "snake", "source": "overseer",
        "like": "owner precedent: snake"
    }))
    .await
    .unwrap();
}

/// An owner answer to `q` given `ago` ms before now, on its own task.
fn remember_at(app: &App, repo: &str, q: &Question, answer: &str, ago: i64) {
    let mut task = answered_task(repo, q, answer, AnsweredBy::Owner);
    task.fixture_data().question_history[0].answered_at = crate::clock::now_ms() - ago;
    app.store.save_task(&task).unwrap();
}

#[tokio::test]
async fn decision_memory_the_newest_owner_answers_must_agree() {
    let (app, dir) = overseer_app();
    let repo = repo_root(&app, dir.path()).await;
    let q = free_q("Which naming style?", &["snake", "camel"]);
    let key = precedents::question_key(&q, &waiting_task(&repo, q.clone()));
    let agreed = |app: &App| precedents::owner_answer(&app.data_dir, &repo, &key);
    // One answer is not enough.
    remember_at(&app, &repo, &q, "camel", 9000);
    assert_eq!(agreed(&app), None);
    // One old different answer, then two newer ones: the window holds
    // camel, snake, snake and blocks.
    remember_at(&app, &repo, &q, "snake", 5000);
    remember_at(&app, &repo, &q, "snake", 4000);
    assert_eq!(agreed(&app), None);
    // A third newer answer fills the window: the old one stops blocking.
    remember_at(&app, &repo, &q, "snake", 3000);
    assert_eq!(agreed(&app).map(|a| a.0), Some("snake".to_string()));
    // A newer different answer blocks again.
    remember_at(&app, &repo, &q, "camel", 1000);
    assert_eq!(agreed(&app), None);
}

#[tokio::test]
async fn decision_memory_a_resave_after_eviction_counts_nothing_twice() {
    let dir = tempfile::tempdir().unwrap();
    let q = free_q("Which naming style?", &["snake", "camel"]);
    let now = crate::clock::now_ms();
    let mut first = answered_task("/r1", &q, "snake", AnsweredBy::Owner);
    first.fixture_data().question_history[0].answered_at = now - 30_000;
    precedents::sync(dir.path(), &first);
    for i in 0..520 {
        let mut other = answered_task("/r2", &q, "camel", AnsweredBy::Owner);
        other.fixture_data().question_history[0].answered_at = now - 20_000 + i;
        precedents::sync(dir.path(), &other);
    }
    precedents::forget_synced();
    precedents::sync(dir.path(), &first);
    let (list, _) = precedents::list(dir.path(), "/r1");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].count, 1);
}
