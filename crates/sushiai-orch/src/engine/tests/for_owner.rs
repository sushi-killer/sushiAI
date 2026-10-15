// `Question.for_owner`: orchd hands a waiting question to the owner only once
// nobody else (the overseer, triage, the answer policy, a precedent) will
// settle it, and never while the overseer holds it.

/// A test app with the fake overseer harness and a repo directory.
fn fo_app(auto_answer: bool) -> (Arc<App>, tempfile::TempDir, String) {
    let _ = crate::engine::chat::test_support::fake_overseer_harness();
    let (app, dir) = test_app();
    app.settings.write().unwrap().auto_answer = auto_answer;
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let repo = repo.to_string_lossy().to_string();
    (app, dir, repo)
}

/// The fake overseer sleeps until the turn times out after `ms`.
fn fo_slow_overseer(app: &App, repo: &str, ms: u64) {
    std::fs::write(Path::new(repo).join(".overseer-slow"), "").unwrap();
    app.overseer_turn_timeout_ms.store(ms, Ordering::SeqCst);
}

fn fo_waiting(repo: &str, kind: QuestionKind, options: &[&str]) -> Task {
    let mut task = task_with_status(TaskStatus::Waiting);
    task.fixture_data().repo = repo.to_string();
    task.fixture_data().question = Some(Question::new(
        "Which way?",
        options.iter().map(|o| o.to_string()).collect(),
        kind,
        AskedBy::Implement,
        crate::clock::now_ms(),
    ));
    task
}

fn fo_question(app: &App, id: &str) -> Question {
    app.store
        .load_task(id)
        .unwrap()
        .unwrap()
        .question
        .clone()
        .unwrap()
}

fn fo_for_owner(app: &App, id: &str) -> bool {
    fo_question(app, id).for_owner
}

/// A quiet-hours window around the current local minute.
fn fo_quiet_now() -> String {
    let m = owner_notice::minute_of_day(now_ms());
    let (from, to) = ((m + 1439) % 1440, (m + 3) % 1440);
    format!(
        "{:02}:{:02}-{:02}:{:02}",
        from / 60,
        from % 60,
        to / 60,
        to % 60
    )
}

struct FoParked {
    cancel: CancelToken,
    handle: tokio::task::JoinHandle<()>,
}

impl FoParked {
    async fn end(self) {
        self.cancel.cancel();
        self.handle.await.unwrap();
    }
}

/// Parks `task` the way its loop does; returns once the one-shot is installed
/// and the park-time wake has run.
async fn fo_park(app: &Arc<App>, mut task: Task) -> FoParked {
    app.store.save_task(&task).unwrap();
    let id = task.id.clone();
    let repo = task.repo.clone();
    let pending = Arc::new(StdMutex::new(None));
    let cancel = CancelToken::new();
    let handle = tokio::spawn({
        let (app, pending, cancel, _id) = (app.clone(), pending.clone(), cancel.clone(), id.clone());
        async move {
            park_for_answer(&app, &mut task, &pending, &cancel, &mut None).await;
        }
    });
    // The park-time wake runs right after the one-shot is installed and
    // either holds the question, hands it to the owner, or defers it to the
    // morning. A deferring turn holds and releases the question between two
    // polls, so its log entry is the only trace it leaves.
    let woken = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let installed = pending.lock().unwrap().is_some();
            let q = app
                .store
                .load_task(&id)
                .ok()
                .flatten()
                .and_then(|t| t.question.clone());
            let deferred = || {
                crate::overseer::read(&app.data_dir, Some(&repo), None)
                    .iter()
                    .any(|e| e.text == owner_notice::DEFERRED)
            };
            if installed && (q.is_some_and(|q| q.held_for_overseer || q.for_owner) || deferred()) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    if woken.is_err() {
        let task = app.store.load_task(&id).ok().flatten();
        panic!(
            "the park-time wake never held or handed over {id}: {:?}",
            task.map(|t| (t.status, t.question.clone()))
        );
    }
    FoParked { cancel, handle }
}

/// Waits for the waiting broadcast the park ends with (after its wake).
async fn fo_until_waiting_broadcast(events: &mut broadcast::Receiver<Event>) -> Vec<Task> {
    let mut seen = Vec::new();
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let Ok(Event::Task { task }) = events.recv().await {
                let waiting = task.status == TaskStatus::Waiting;
                seen.push(*task.clone());
                if waiting {
                    // The park's own broadcast follows the wake's: settle.
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    while let Ok(Event::Task { task }) = events.try_recv() {
                        seen.push(*task);
                    }
                    return;
                }
            }
        }
    })
    .await
    .unwrap();
    seen
}

#[tokio::test]
async fn for_owner_is_set_at_once_for_an_owner_only_kind() {
    let (app, _dir, repo) = fo_app(true);
    // Also inside quiet hours: the owner hears of it at once.
    app.settings.write().unwrap().quiet_hours = fo_quiet_now();
    let task = fo_waiting(&repo, QuestionKind::Budget, &["raise", "stop"]);
    let id = task.id.clone();
    let parked = fo_park(&app, task).await;
    let q = fo_question(&app, &id);
    assert!(q.for_owner && !q.held_for_overseer);
    crate::engine::chat::test_support::wait_idle(&app, &repo).await;
    assert_eq!(crate::engine::chat::test_support::fake_overseer_calls(Path::new(&repo)), 0);
    parked.end().await;
}

#[tokio::test]
async fn for_owner_is_set_when_auto_answer_is_off() {
    let (app, _dir, repo) = fo_app(false);
    let task = fo_waiting(&repo, QuestionKind::AgentQuestion, &["A", "B"]);
    let id = task.id.clone();
    let parked = fo_park(&app, task).await;
    assert!(fo_for_owner(&app, &id));
    assert_eq!(crate::engine::chat::test_support::fake_overseer_calls(Path::new(&repo)), 0);
    parked.end().await;
}

#[tokio::test]
async fn for_owner_is_set_when_no_overseer_turn_can_start() {
    // No route for the orchestrator.
    let (app, _dir, repo) = fo_app(true);
    app.settings.write().unwrap().routes.clear();
    let task = fo_waiting(&repo, QuestionKind::AgentQuestion, &["A", "B"]);
    let id = task.id.clone();
    let parked = fo_park(&app, task).await;
    let q = fo_question(&app, &id);
    assert!(q.for_owner && !q.held_for_overseer);
    parked.end().await;

    // A repo that is not a directory.
    let (app, dir, _) = fo_app(true);
    let gone = dir.path().join("gone").to_string_lossy().to_string();
    let task = fo_waiting(&gone, QuestionKind::AgentQuestion, &["A", "B"]);
    let id = task.id.clone();
    let parked = fo_park(&app, task).await;
    assert!(fo_for_owner(&app, &id));
    parked.end().await;

    // A wake queued behind a busy turn that then finds no route.
    let (app, _dir, repo) = fo_app(true);
    fo_slow_overseer(&app, &repo, 1500);
    let first = fo_waiting(&repo, QuestionKind::AgentQuestion, &["A", "B"]);
    let second = fo_waiting(&repo, QuestionKind::AgentQuestion, &["C", "D"]);
    let second_id = second.id.clone();
    let one = fo_park(&app, first).await;
    let two = fo_park(&app, second).await;
    let q = fo_question(&app, &second_id);
    assert!(
        q.held_for_overseer && !q.for_owner,
        "queued is the overseer's"
    );
    app.settings.write().unwrap().routes.clear();
    crate::engine::chat::test_support::wait_idle(&app, &repo).await;
    let q = fo_question(&app, &second_id);
    assert!(q.for_owner && !q.held_for_overseer);
    one.end().await;
    two.end().await;
}

#[tokio::test]
async fn for_owner_is_set_when_the_overseer_turn_ends_with_the_question_open() {
    // A turn that replies and one that times out both leave it open.
    for slow in [false, true] {
        let (app, _dir, repo) = fo_app(true);
        if slow {
            fo_slow_overseer(&app, &repo, 400);
        }
        let task = fo_waiting(&repo, QuestionKind::AgentQuestion, &["A", "B"]);
        let id = task.id.clone();
        let parked = fo_park(&app, task).await;
        crate::engine::chat::test_support::wait_idle(&app, &repo).await;
        assert_eq!(crate::engine::chat::test_support::fake_overseer_calls(Path::new(&repo)), 1);
        let q = fo_question(&app, &id);
        assert!(q.for_owner && !q.held_for_overseer, "slow: {slow}");
        parked.end().await;
    }
}

#[tokio::test]
async fn for_owner_is_set_when_a_turn_outlives_the_hold_ttl() {
    let (app, _dir, repo) = fo_app(true);
    fo_slow_overseer(&app, &repo, 1500);
    let task = fo_waiting(&repo, QuestionKind::AgentQuestion, &["A", "B"]);
    let id = task.id.clone();
    let parked = fo_park(&app, task).await;
    assert!(fo_question(&app, &id).held_for_overseer);
    // The hold's TTL ends while the turn still runs: not the owner's yet.
    app.release_expired_holds(now_ms() + HOLD_TTL_MS + 1);
    let q = fo_question(&app, &id);
    assert!(!q.held_for_overseer && !q.for_owner);
    crate::engine::chat::test_support::wait_idle(&app, &repo).await;
    assert!(fo_for_owner(&app, &id), "handed on when the turn ends");
    parked.end().await;

    // A hold no turn or queued wake owns any more goes to the owner at once.
    let (app, _dir, repo) = fo_app(true);
    let mut task = fo_waiting(&repo, QuestionKind::AgentQuestion, &["A", "B"]);
    let q = task.fixture_data().question.as_mut().unwrap();
    q.held_for_overseer = true;
    q.held_at = Some(now_ms() - HOLD_TTL_MS - 1);
    app.store.save_task(&task).unwrap();
    app.release_expired_holds(now_ms());
    let q = fo_question(&app, &task.id);
    assert!(!q.held_for_overseer && q.for_owner);
}

#[tokio::test]
async fn for_owner_is_never_set_at_the_attempts_failing_cap() {
    // Past the overseer's continues the failure ladder decides, not the
    // owner: one changed approach, never an open question (`ladder.rs`).
    let (app, _dir, repo) = fo_app(true);
    app.settings.write().unwrap().answer_policy = false;
    let mut task = fo_waiting(
        &repo,
        QuestionKind::AttemptsFailing,
        &["continue", "change approach", "stop"],
    );
    task.fixture_data()
        .attempts
        .push(attempt_with_failure(1, "s"));
    task.fixture_data().loop_state.non_owner_continues = MAX_ORCHESTRATOR_CONTINUES as u32;
    app.store.save_task(&task).unwrap();
    let _id = task.id.clone();
    let pending = Arc::new(StdMutex::new(None));
    let cancel = CancelToken::new();
    let mut events = app.subscribe();
    let answered = wait_for_exhausted_answer(&app, &mut task, &pending, &cancel, &mut None).await;
    assert_eq!(answered.map(|a| a.text).as_deref(), Some("change approach"));
    assert!(task.question.is_none() && task.status == TaskStatus::Queued);
    while let Ok(event) = events.try_recv() {
        if let Event::Task { task } = event {
            assert!(
                task.question.clone().is_none_or(|q| !q.for_owner),
                "never the owner's"
            );
        }
    }

    // A hold on it that lapses steps the ladder too.
    let (app, _dir, repo) = fo_app(true);
    let mut task = fo_waiting(&repo, QuestionKind::AttemptsFailing, &["continue", "stop"]);
    task.fixture_data()
        .attempts
        .push(attempt_with_failure(1, "s"));
    let q = task.fixture_data().question.as_mut().unwrap();
    q.held_for_overseer = true;
    q.held_at = Some(now_ms() - HOLD_TTL_MS - 1);
    app.store.save_task(&task).unwrap();
    app.release_expired_holds(now_ms());
    let task = app.store.load_task(&task.id).unwrap().unwrap();
    assert!(task.question.clone().is_none_or(|q| !q.for_owner));
    assert_ne!(task.status, TaskStatus::Waiting);
}

#[tokio::test]
async fn for_owner_is_set_for_the_owner_batch_once_nobody_else_takes_it() {
    let (app, _dir, repo) = fo_app(true);
    fo_slow_overseer(&app, &repo, 1000);
    let mut task = task_with_status(TaskStatus::Drafting);
    task.fixture_data().repo = repo.clone();
    app.store.save_task(&task).unwrap();
    let id = task.id.clone();
    let pending = Arc::new(StdMutex::new(None));
    let cancel = CancelToken::new();
    let mut events = app.subscribe();
    let handle = tokio::spawn({
        let (app, pending, cancel, id) = (app.clone(), pending.clone(), cancel.clone(), id.clone());
        async move {
            ask_plan_question(&app, &id, crate::task::Question { risk: vec![], default_option: None, ..crate::task::Question::new("The planner has 2 blocking question(s); answer them all in one reply:\n\n1. A?\n2. B?", vec![], crate::task::QuestionKind::PlanQuestion, crate::task::AskedBy::Plan, crate::clock::now_ms()) }, &crate::engine::run_control::TaskSignals { cancel: cancel.clone(), pending_answer: pending.clone(), pending_amend: std::sync::Arc::new(std::sync::Mutex::new(None)) }, &mut None)
            .await;
        }
    });
    fo_until_waiting_broadcast(&mut events).await;
    let q = fo_question(&app, &id);
    assert!(
        q.held_for_overseer && !q.for_owner,
        "the overseer looks first"
    );
    crate::engine::chat::test_support::wait_idle(&app, &repo).await;
    let q = fo_question(&app, &id);
    assert!(q.for_owner && !q.held_for_overseer);
    cancel.cancel();
    handle.await.unwrap();

    // With auto-answer off nobody else looks: the owner's at once.
    let (app, _dir, repo) = fo_app(false);
    let mut task = task_with_status(TaskStatus::Drafting);
    task.fixture_data().repo = repo.clone();
    app.store.save_task(&task).unwrap();
    let id = task.id.clone();
    let pending = Arc::new(StdMutex::new(None));
    let cancel = CancelToken::new();
    let mut events = app.subscribe();
    let handle = tokio::spawn({
        let (app, pending, cancel, id) = (app.clone(), pending.clone(), cancel.clone(), id.clone());
        async move {
            ask_plan_question(
                &app,
                &id,
                crate::task::Question {
                    risk: vec![],
                    default_option: None,
                    ..crate::task::Question::new(
                        "The planner has 2 blocking question(s); answer them all in one reply:",
                        vec![],
                        crate::task::QuestionKind::PlanQuestion,
                        crate::task::AskedBy::Plan,
                        crate::clock::now_ms(),
                    )
                },
                &crate::engine::run_control::TaskSignals {
                    cancel: cancel.clone(),
                    pending_answer: pending.clone(),
                    pending_amend: std::sync::Arc::new(std::sync::Mutex::new(None)),
                },
                &mut None,
            )
            .await;
        }
    });
    fo_until_waiting_broadcast(&mut events).await;
    assert!(fo_for_owner(&app, &id));
    cancel.cancel();
    handle.await.unwrap();
}

#[tokio::test]
async fn for_owner_is_set_for_a_deferred_notice_when_quiet_hours_end() {
    for digest in [None, Some(8)] {
        let (app, _dir, repo) = fo_app(true);
        {
            let mut s = app.settings.write().unwrap();
            s.quiet_hours = fo_quiet_now();
            s.digest_hour = digest;
        }
        let task = fo_waiting(&repo, QuestionKind::AgentQuestion, &["A", "B"]);
        let id = task.id.clone();
        let parked = fo_park(&app, task).await;
        crate::engine::chat::test_support::wait_idle(&app, &repo).await;
        let log = crate::overseer::read(&app.data_dir, Some(&repo), None);
        assert!(
            log.iter().any(|e| e.text == owner_notice::DEFERRED),
            "{log:?}"
        );
        // Quiet hours held the notice: not the owner's yet.
        let q = fo_question(&app, &id);
        assert!(!q.for_owner && !q.held_for_overseer, "digest {digest:?}");
        app.wait_sla_tick(now_ms()).await;
        assert!(!fo_for_owner(&app, &id), "still quiet");
        app.settings.write().unwrap().quiet_hours = String::new();
        app.wait_sla_tick(now_ms()).await;
        assert!(fo_for_owner(&app, &id), "digest {digest:?}");
        parked.end().await;
    }
}

#[tokio::test]
async fn for_owner_is_never_set_while_the_overseer_holds_the_question() {
    let (app, _dir, repo) = fo_app(true);
    fo_slow_overseer(&app, &repo, 1500);
    let mut events = app.subscribe();
    let task = fo_waiting(&repo, QuestionKind::AgentQuestion, &["A", "B"]);
    let id = task.id.clone();
    let parked = fo_park(&app, task).await;
    let q = fo_question(&app, &id);
    assert!(q.held_for_overseer && !q.for_owner);
    // Nothing that runs meanwhile hands it over: an unexpired TTL, the
    // end of quiet hours, the minute tick.
    app.release_expired_holds(now_ms());
    app.hand_over_deferred(now_ms());
    app.wait_sla_tick(now_ms()).await;
    let q = fo_question(&app, &id);
    assert!(q.held_for_overseer && !q.for_owner);
    crate::engine::chat::test_support::wait_idle(&app, &repo).await;
    assert!(fo_for_owner(&app, &id));
    let mut seen = Vec::new();
    while let Ok(Event::Task { task }) = events.try_recv() {
        seen.push(*task);
    }
    let ours: Vec<&Question> = seen
        .iter()
        .filter(|t| t.id == id)
        .filter_map(|t| t.question.as_ref())
        .collect();
    assert!(ours.iter().any(|q| q.held_for_overseer));
    assert!(
        ours.iter().all(|q| !(q.held_for_overseer && q.for_owner)),
        "no broadcast is both held and the owner's"
    );
    parked.end().await;
}

#[tokio::test]
async fn for_owner_is_never_set_when_the_policy_or_a_precedent_answers() {
    // The answer policy answers before the task parks.
    let (app, _dir, repo) = fo_app(true);
    let mut task = fo_waiting(
        &repo,
        QuestionKind::DependencyEnded,
        &["retry the dependency", "stop"],
    );
    app.store.save_task(&task).unwrap();
    let mut events = app.subscribe();
    let _id = task.id.clone();
    let pending = Arc::new(StdMutex::new(None));
    let answer = wait_for_answer(&app, &mut task, &pending, &CancelToken::new(), &mut None).await;
    assert_eq!(answer.as_deref(), Some("retry the dependency"));
    while let Ok(event) = events.try_recv() {
        if let Event::Task { task } = event {
            assert!(task.question.clone().is_none_or(|q| !q.for_owner));
        }
    }

    // An owner precedent answers it as the overseer, with no turn.
    let (app, dir, _) = fo_app(true);
    let root = dir.path().join("git");
    std::fs::create_dir_all(&root).unwrap();
    init_git_repo(&root);
    let repo = app.resolve_repo(root.to_str().unwrap()).await.unwrap();
    let mut base = Question::new(
        "cargo test fails on base deadbee: 3 failed",
        vec![
            PRE_EXISTING_RETRY.into(),
            PRE_EXISTING_DROP.into(),
            "stop".into(),
        ],
        QuestionKind::PreexistingFailure,
        AskedBy::Implement,
        crate::clock::now_ms(),
    );
    base.commands = vec!["cargo test".into()];
    for _ in 0..2 {
        let mut answered = task_with_status(TaskStatus::Queued);
        answered.fixture_data().repo = repo.clone();
        record_answer_for(
            &mut answered,
            Some(&base),
            &crate::engine::questions::reply::Reply {
                text: (PRE_EXISTING_RETRY).to_string(),
                default_evidence: (None).map(str::to_string),
                like: (None).map(str::to_string),
            },
            AnsweredBy::Owner,
        );
        app.store.save_task(&answered).unwrap();
    }
    let mut task = task_with_status(TaskStatus::Waiting);
    task.fixture_data().repo = repo.clone();
    task.fixture_data().question = Some(base);
    let id = task.id.clone();
    app.store.save_task(&task).unwrap();
    let pending = Arc::new(StdMutex::new(None));
    let cancel = CancelToken::new();
    app.controls.lock().unwrap().insert(
        id.clone(),
        TaskControl {
            signals: crate::engine::run_control::TaskSignals {
                cancel: cancel.clone(),
                pending_answer: pending.clone(),
                pending_amend: Arc::new(StdMutex::new(None)),
            },
            handle: tokio::spawn(async {}),
        },
    );
    let mut events = app.subscribe();
    let parked = tokio::spawn({
        let (app, pending, cancel, _id) = (app.clone(), pending.clone(), cancel.clone(), id.clone());
        async move { park_for_answer(&app, &mut task, &pending, &cancel, &mut None).await }
    });
    let answer = tokio::time::timeout(Duration::from_secs(5), parked)
        .await
        .expect("the precedent answers")
        .unwrap();
    assert_eq!(answer.as_deref(), Some(PRE_EXISTING_RETRY));
    while let Ok(event) = events.try_recv() {
        if let Event::Task { task } = event {
            assert!(task.question.clone().is_none_or(|q| !q.for_owner));
        }
    }
    let answered = app.store.load_task(&id).unwrap().unwrap();
    assert!(answered.question.is_none());
    assert_eq!(
        answered.question_history.last().unwrap().answered_by,
        AnsweredBy::Overseer
    );
}

#[tokio::test]
async fn for_owner_is_never_on_the_waiting_broadcast_before_the_hold() {
    // A plan question and an agent question: each first shows as waiting
    // already held, never unheld or handed over first.
    let (app, _dir, repo) = fo_app(true);
    fo_slow_overseer(&app, &repo, 800);
    let mut task = task_with_status(TaskStatus::Drafting);
    task.fixture_data().repo = repo.clone();
    app.store.save_task(&task).unwrap();
    let id = task.id.clone();
    let pending = Arc::new(StdMutex::new(None));
    let cancel = CancelToken::new();
    let mut events = app.subscribe();
    let handle = tokio::spawn({
        let (app, pending, cancel, id) = (app.clone(), pending.clone(), cancel.clone(), id.clone());
        async move {
            ask_plan_question(
                &app,
                &id,
                crate::task::Question {
                    risk: vec![],
                    default_option: None,
                    ..crate::task::Question::new(
                        "Which plan?",
                        vec!["A".into(), "B".into()],
                        crate::task::QuestionKind::PlanQuestion,
                        crate::task::AskedBy::Plan,
                        crate::clock::now_ms(),
                    )
                },
                &crate::engine::run_control::TaskSignals {
                    cancel: cancel.clone(),
                    pending_answer: pending.clone(),
                    pending_amend: std::sync::Arc::new(std::sync::Mutex::new(None)),
                },
                &mut None,
            )
            .await;
        }
    });
    let seen = fo_until_waiting_broadcast(&mut events).await;
    let first = seen
        .iter()
        .find(|t| t.id == id && t.status == TaskStatus::Waiting)
        .and_then(|t| t.question.clone())
        .unwrap();
    assert!(first.held_for_overseer && !first.for_owner, "{first:?}");
    crate::engine::chat::test_support::wait_idle(&app, &repo).await;
    cancel.cancel();
    handle.await.unwrap();

    let (app, _dir, repo) = fo_app(true);
    fo_slow_overseer(&app, &repo, 800);
    let task = fo_waiting(&repo, QuestionKind::AgentQuestion, &["A", "B"]);
    let id = task.id.clone();
    let mut events = app.subscribe();
    let parked = fo_park(&app, task).await;
    let mut seen = Vec::new();
    while let Ok(Event::Task { task }) = events.try_recv() {
        seen.push(*task);
    }
    let waiting: Vec<&Question> = seen
        .iter()
        .filter(|t| t.id == id && t.status == TaskStatus::Waiting)
        .filter_map(|t| t.question.as_ref())
        .collect();
    assert!(!waiting.is_empty());
    assert!(
        waiting.iter().all(|q| q.held_for_overseer && !q.for_owner),
        "{waiting:?}"
    );
    crate::engine::chat::test_support::wait_idle(&app, &repo).await;
    parked.end().await;
}
