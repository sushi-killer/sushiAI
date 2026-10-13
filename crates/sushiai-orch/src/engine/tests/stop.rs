fn stop_waiting_task() -> Task {
    let mut task = task_with_status(TaskStatus::Waiting);
    task.question = Some(Question::new(
        "Which implementation should I use?",
        vec!["continue".into(), "stop".into()],
        QuestionKind::AgentQuestion,
        AskedBy::Implement,
    ));
    task
}

fn stop_close_lines(task: &Task) -> Vec<&str> {
    task.decisions
        .iter()
        .filter(|line| line.starts_with("Stop: closed the open question"))
        .map(String::as_str)
        .collect()
}

fn hold_stop_control(app: &App, task: &Task) -> CancelToken {
    let cancel = CancelToken::new();
    app.controls.lock().unwrap().insert(
        task.id.clone(),
        TaskControl {
            cancel: cancel.clone(),
            pending_answer: Arc::new(StdMutex::new(None)),
            pending_amend: Arc::new(StdMutex::new(None)),
            handle: tokio::spawn(async {}),
        },
    );
    cancel
}

async fn start_stop_question_wait(app: &Arc<App>, mut task: Task) -> oneshot::Receiver<Task> {
    let cancel = CancelToken::new();
    let pending_answer = Arc::new(StdMutex::new(None));
    let (done, result) = oneshot::channel();
    let mut events = app.subscribe();
    let loop_app = app.clone();
    let loop_cancel = cancel.clone();
    let loop_pending = pending_answer.clone();
    let id = task.id.clone();
    let handle = tokio::spawn(async move {
        let id = task.id.clone();
        assert!(wait_for_answer(
            &loop_app,
            &id,
            &mut task,
            &loop_pending,
            &loop_cancel,
            &mut None,
        )
        .await
        .is_none());
        let _ = done.send(task);
    });
    app.controls.lock().unwrap().insert(
        id.clone(),
        TaskControl {
            cancel,
            pending_answer,
            pending_amend: Arc::new(StdMutex::new(None)),
            handle,
        },
    );
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Event::Task { task } = events.recv().await.unwrap() {
                if task.id == id && task.status == TaskStatus::Waiting {
                    break;
                }
            }
        }
    })
    .await
    .expect("the loop should park on its question");
    result
}

#[tokio::test]
async fn a_stopped_task_closes_its_question_on_owner_stop() {
    let (app, _dir) = test_app();
    let task = stop_waiting_task();
    let id = task.id.clone();
    app.store.save_task(&task).unwrap();
    let result = start_stop_question_wait(&app, task).await;

    app.handle_task_stop(json!({"id": id})).await.unwrap();
    let local = tokio::time::timeout(Duration::from_secs(2), result)
        .await
        .unwrap()
        .unwrap();
    let saved = app.store.load_task(&id).unwrap().unwrap();
    for task in [&local, &saved] {
        assert_eq!(task.status, TaskStatus::Stopped);
        assert!(task.question.is_none());
        assert_eq!(
            stop_close_lines(task),
            ["Stop: closed the open question \"Which implementation should I use?\""]
        );
        assert!(task.question_history.is_empty());
    }
}

#[tokio::test]
async fn a_stopped_task_without_a_loop_is_stopped_and_its_question_closed() {
    let (app, _dir) = test_app();
    let task = stop_waiting_task();
    app.store.save_task(&task).unwrap();
    assert!(!app.controls.lock().unwrap().contains_key(&task.id));
    assert!(task.parent.is_none() && task.depends_on.is_empty());
    assert!(task.queue.wait.is_none());

    let out = app.handle_task_stop(json!({"id": task.id})).await.unwrap();
    assert_eq!(out["status"], "stopped");
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert!(saved.question.is_none());
    assert_eq!(
        stop_close_lines(&saved),
        ["Stop: closed the open question \"Which implementation should I use?\""]
    );
    app.handle_task_stop(json!({"id": task.id})).await.unwrap();
    assert_eq!(
        app.store.load_task(&task.id).unwrap().unwrap().decisions,
        saved.decisions
    );
}

#[tokio::test]
async fn a_stopped_task_by_answer_releases_its_leases() {
    let (app, _dir) = test_app();
    for (source, answered_by) in [
        (None, AnsweredBy::Owner),
        (Some("lead"), AnsweredBy::Lead),
        (Some("mcp"), AnsweredBy::Lead),
        (Some("chat"), AnsweredBy::Orchestrator),
        (Some("overseer"), AnsweredBy::Overseer),
    ] {
        let mut task = stop_waiting_task();
        task.paths = vec!["src/declared.rs".into()];
        app.store.save_task(&task).unwrap();
        let result = start_stop_question_wait(&app, task.clone()).await;
        assert!(app.acquire_lease(&task, &task.paths, false).is_none());
        assert!(app.lease_file(&task.id, "src/edited.rs").is_none());
        let (tx, rx) = oneshot::channel();
        app.permission_waits
            .lock()
            .unwrap()
            .insert(task.id.clone(), tx);

        let mut params = json!({"id": task.id, "answer": "stop"});
        if let Some(source) = source {
            params["source"] = json!(source);
        }
        app.handle_task_answer(params).await.unwrap();
        assert!(!app.leases.lock().unwrap().contains_key(&task.id));
        assert!(!app.permission_waits.lock().unwrap().contains_key(&task.id));
        assert!(rx.await.is_err(), "the held tool call must be dropped");
        let local = tokio::time::timeout(Duration::from_secs(2), result)
            .await
            .unwrap()
            .unwrap();
        let saved = app.store.load_task(&task.id).unwrap().unwrap();
        for task in [&local, &saved] {
            assert_eq!(task.status, TaskStatus::Stopped);
            assert!(task.question.is_none());
            assert!(stop_close_lines(task).is_empty());
            assert_eq!(task.question_history.len(), 1);
            assert_eq!(task.question_history[0].answer, "stop");
            assert_eq!(
                task.question_history[0].answered_by,
                answered_by
            );
        }
    }
}

#[tokio::test]
async fn a_stopped_task_child_of_a_stopped_parent_has_no_question() {
    let (app, _dir) = test_app();
    let parent = task_with_status(TaskStatus::Running);
    let mut child = stop_waiting_task();
    child.parent = Some(parent.id.clone());
    app.store.save_task(&parent).unwrap();
    app.store.save_task(&child).unwrap();

    app.handle_task_stop(json!({"id": parent.id}))
        .await
        .unwrap();

    let saved = app.store.load_task(&child.id).unwrap().unwrap();
    assert_eq!(saved.status, TaskStatus::Stopped);
    assert!(saved.question.is_none());
    assert_eq!(
        stop_close_lines(&saved),
        ["Stop: closed the open question \"Which implementation should I use?\""]
    );
    assert!(stop_close_lines(&app.store.load_task(&parent.id).unwrap().unwrap()).is_empty());
}

#[tokio::test]
async fn a_stopped_task_dependency_keeps_its_parent_waiting_and_its_child_idle() {
    let (app, _dir) = test_app();
    app.settings.write().unwrap().answer_policy = false;
    let dependency = stop_waiting_task();
    let mut parent = task_with_status(TaskStatus::Queued);
    parent.depends_on = vec![dependency.id.clone()];
    let mut child = task_with_status(TaskStatus::Queued);
    child.parent = Some(parent.id.clone());
    for task in [&dependency, &parent, &child] {
        app.store.save_task(task).unwrap();
    }

    app.handle_task_stop(json!({"id": dependency.id}))
        .await
        .unwrap();
    app.advance_graph(&dependency.repo);

    let stopped = app.store.load_task(&dependency.id).unwrap().unwrap();
    assert_eq!(stopped.status, TaskStatus::Stopped);
    assert!(stopped.question.is_none());
    assert_eq!(stop_close_lines(&stopped).len(), 1);
    let waiting = app.store.load_task(&parent.id).unwrap().unwrap();
    assert_eq!(waiting.status, TaskStatus::Waiting);
    let question = waiting.question.as_ref().unwrap();
    assert_eq!(question.kind, QuestionKind::DependencyEnded);
    assert!(question.text.contains("(stopped)"));
    let idle = app.store.load_task(&child.id).unwrap().unwrap();
    assert_eq!(idle.status, TaskStatus::Queued);
    assert!(waiting.attempts.is_empty());
    assert!(idle.attempts.is_empty());
    assert!(!app.controls.lock().unwrap().contains_key(&parent.id));
    assert!(!app.controls.lock().unwrap().contains_key(&child.id));
}

#[tokio::test]
async fn a_stopped_task_holds_no_lease_and_another_task_can_lease_the_file() {
    let (app, _dir) = test_app();
    let mut task = task_with_status(TaskStatus::Running);
    task.base_ref = Some("main".into());
    task.paths = vec!["src/declared.rs".into()];
    app.store.save_task(&task).unwrap();
    let cancel = hold_stop_control(&app, &task);
    assert!(app.acquire_lease(&task, &task.paths, false).is_none());
    assert!(app.lease_file(&task.id, "src/edited.rs").is_none());
    let mut next = task_with_status(TaskStatus::Running);
    next.base_ref = task.base_ref.clone();
    app.store.save_task(&next).unwrap();
    hold_stop_control(&app, &next);
    for file in ["src/declared.rs", "src/edited.rs"] {
        assert_eq!(app.lease_file(&next.id, file).unwrap().task_id, task.id);
    }

    app.handle_task_stop(json!({"id": task.id})).await.unwrap();

    assert!(cancel.is_cancelled());
    assert!(app.controls.lock().unwrap().contains_key(&task.id));
    assert!(!app.leases.lock().unwrap().contains_key(&task.id));
    for file in ["src/declared.rs", "src/edited.rs"] {
        assert!(app.lease_file(&next.id, file).is_none());
    }
    assert!(app.lease_file(&task.id, "src/late-edit.rs").is_none());
    assert!(app.acquire_lease(&task, &task.paths, true).is_none());
    assert!(!app.leases.lock().unwrap().contains_key(&task.id));
    assert!(app.lease_file(&next.id, "src/late-edit.rs").is_none());

    mark_stopped_if_not_already(&app, &task.id).await;
    app.controls.lock().unwrap().remove(&task.id);
    assert!(app.lease_file(&task.id, "src/after-loop.rs").is_none());
    assert!(!app.leases.lock().unwrap().contains_key(&task.id));
}

#[tokio::test]
async fn a_stopped_task_keeps_no_question_after_a_restart() {
    let (app, _dir) = test_app();
    let mut task = stop_waiting_task();
    task.status = TaskStatus::Stopped;
    app.store.save_task(&task).unwrap();
    let mut archived = task.clone();
    archived.id = uuid::Uuid::new_v4().to_string();
    archived.archived = true;
    app.store.save_task(&archived).unwrap();
    let mut events = app.subscribe();

    app.recover_on_start().unwrap();

    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.status, TaskStatus::Stopped);
    assert!(saved.question.is_none());
    assert_eq!(
        stop_close_lines(&saved),
        ["Stop: closed the open question \"Which implementation should I use?\""]
    );
    assert!(!app.controls.lock().unwrap().contains_key(&task.id));
    assert!(!app.controls.lock().unwrap().contains_key(&archived.id));
    assert_eq!(
        serde_json::to_value(app.store.load_task(&archived.id).unwrap().unwrap()).unwrap(),
        serde_json::to_value(&archived).unwrap()
    );
    let mut broadcasts = 0;
    while let Ok(event) = events.try_recv() {
        if let Event::Task { task: broadcast } = event {
            if broadcast.id == task.id {
                assert!(broadcast.question.is_none());
                broadcasts += 1;
            }
        }
    }
    assert_eq!(broadcasts, 1);
    app.recover_on_start().unwrap();
    assert_eq!(
        app.store.load_task(&task.id).unwrap().unwrap().decisions,
        saved.decisions
    );
    app.shutdown();
}

#[tokio::test]
async fn a_stopped_task_with_an_unfinished_attempt_is_not_requeued_after_a_restart() {
    let (app, _dir) = test_app();
    let mut task = stop_waiting_task();
    task.status = TaskStatus::Stopped;
    let mut attempt = attempt_with_failure(1, "unused");
    attempt.status = AttemptStatus::Running;
    attempt.failure = None;
    task.attempts.push(attempt);
    app.store.save_task(&task).unwrap();
    let mut events = app.subscribe();

    app.recover_on_start().unwrap();

    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.status, TaskStatus::Stopped);
    assert!(saved.question.is_none());
    assert_eq!(saved.attempts[0].status, AttemptStatus::Interrupted);
    assert_eq!(stop_close_lines(&saved).len(), 1);
    assert!(!app.controls.lock().unwrap().contains_key(&task.id));
    let mut broadcasts = 0;
    while let Ok(event) = events.try_recv() {
        if let Event::Task { task: broadcast } = event {
            if broadcast.id == task.id {
                assert!(broadcast.question.is_none());
                broadcasts += 1;
            }
        }
    }
    assert_eq!(broadcasts, 1);
    app.shutdown();
}

#[tokio::test]
async fn a_stopped_task_cleanup_does_not_close_a_question_on_daemon_shutdown() {
    let (app, _dir) = test_app();
    let task = stop_waiting_task();
    let id = task.id.clone();
    app.store.save_task(&task).unwrap();
    let result = start_stop_question_wait(&app, task.clone()).await;

    app.shutdown();

    let local = tokio::time::timeout(Duration::from_secs(2), result)
        .await
        .unwrap()
        .unwrap();
    let saved = app.store.load_task(&id).unwrap().unwrap();
    for saved in [&local, &saved] {
        assert_eq!(saved.status, TaskStatus::Waiting);
        assert_eq!(
            saved.question.as_ref().unwrap().text,
            task.question.as_ref().unwrap().text
        );
        assert!(saved.decisions.is_empty());
    }
}

#[tokio::test]
async fn a_stopped_task_with_a_cancelled_control_is_pruned_even_while_landing() {
    let (app, _dir) = test_app();
    let mut task = task_with_status(TaskStatus::Landing);
    task.paths = vec!["src/shared.rs".into()];
    app.store.save_task(&task).unwrap();
    let cancel = hold_stop_control(&app, &task);
    assert!(app.acquire_lease(&task, &task.paths, false).is_none());
    let next = task_with_status(TaskStatus::Running);
    app.store.save_task(&next).unwrap();
    hold_stop_control(&app, &next);
    assert!(app.lease_file(&next.id, "src/shared.rs").is_some());

    cancel.cancel();

    assert!(app.lease_file(&next.id, "src/shared.rs").is_none());
    assert!(!app.leases.lock().unwrap().contains_key(&task.id));
}

#[tokio::test]
async fn a_stopped_task_closes_a_plan_question_but_shutdown_keeps_it() {
    for shutdown in [false, true] {
        let (app, _dir) = test_app();
        let task = task_with_status(TaskStatus::Drafting);
        app.store.save_task(&task).unwrap();
        let cancel = hold_stop_control(&app, &task);
        let pending = app.controls.lock().unwrap()[&task.id]
            .pending_answer
            .clone();
        let loop_app = app.clone();
        let id = task.id.clone();
        let mut events = app.subscribe();
        let waiter = tokio::spawn(async move {
            ask_plan_question(
                &loop_app,
                &id,
                "Which plan?",
                vec![],
                &pending,
                &cancel,
                &mut None,
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .unwrap()
            .unwrap();
        if shutdown {
            app.shutdown();
        } else {
            app.handle_task_stop(json!({"id": task.id})).await.unwrap();
        }
        assert!(tokio::time::timeout(Duration::from_secs(2), waiter)
            .await
            .unwrap()
            .unwrap()
            .is_none());
        let saved = app.store.load_task(&task.id).unwrap().unwrap();
        if shutdown {
            assert_eq!(saved.status, TaskStatus::Waiting);
            assert_eq!(saved.question.as_ref().unwrap().text, "Which plan?");
            assert!(saved.decisions.is_empty());
        } else {
            assert_eq!(saved.status, TaskStatus::Stopped);
            assert!(saved.question.is_none());
            assert_eq!(
                stop_close_lines(&saved),
                ["Stop: closed the open question \"Which plan?\""]
            );
        }
    }
}

#[tokio::test]
async fn a_stopped_task_close_line_truncates_the_question_at_character_boundaries() {
    let (app, _dir) = test_app();
    let mut task = stop_waiting_task();
    task.question.as_mut().unwrap().text = "é".repeat(250);
    app.store.save_task(&task).unwrap();

    app.handle_task_stop(json!({"id": task.id})).await.unwrap();

    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(
        stop_close_lines(&saved),
        [format!(
            "Stop: closed the open question \"{}\"",
            "é".repeat(200)
        )]
    );
}

#[tokio::test]
async fn a_stopped_task_cleanup_does_not_stop_a_live_loop_when_its_run_ends() {
    let (app, _dir) = test_app();
    let mut task = task_with_status(TaskStatus::Running);
    task.paths = vec!["src/declared.rs".into()];
    app.store.save_task(&task).unwrap();
    let cancel = hold_stop_control(&app, &task);
    assert!(app.acquire_lease(&task, &task.paths, false).is_none());
    assert!(app.lease_file(&task.id, "src/edited.rs").is_none());
    let ctx = HookContext::new(&task, 1, Path::new(&task.worktree), &task.base_sha);
    let run_ctx = ctx.clone();
    let run_app = app.clone();
    let mut events = app.subscribe();
    let call = tokio::spawn(async move {
        permissions::decide_call(
            &run_app,
            &run_ctx,
            &json!({"tool_name": "mcp__example__write", "tool_input": {}}),
        )
        .await
    });
    let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .unwrap()
        .unwrap();
    let Event::Task { task: waiting } = event else {
        panic!("a held tool call should show its question");
    };
    assert_eq!(waiting.status, TaskStatus::Waiting);
    assert!(waiting.question.is_some());

    ctx.cancel.cancel();

    let result = tokio::time::timeout(Duration::from_secs(2), call)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["hookSpecificOutput"]["permissionDecision"], "deny");
    assert!(!cancel.is_cancelled(), "the task loop is still live");
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.status, TaskStatus::Waiting);
    assert!(saved.question.is_none());
    assert!(saved.decisions.is_empty());
    assert!(saved.question_history.is_empty());
    assert!(!app.permission_waits.lock().unwrap().contains_key(&task.id));
    assert!(app.leases.lock().unwrap().contains_key(&task.id));
    let next = task_with_status(TaskStatus::Running);
    app.store.save_task(&next).unwrap();
    hold_stop_control(&app, &next);
    for file in ["src/declared.rs", "src/edited.rs"] {
        assert_eq!(app.lease_file(&next.id, file).unwrap().task_id, task.id);
    }
}

#[tokio::test]
async fn a_stopped_task_closes_a_held_tool_question_but_shutdown_keeps_it() {
    for shutdown in [false, true] {
        let (app, _dir) = test_app();
        let task = task_with_status(TaskStatus::Running);
        app.store.save_task(&task).unwrap();
        hold_stop_control(&app, &task);
        let ctx = HookContext::new(&task, 1, Path::new(&task.worktree), &task.base_sha);
        let loop_ctx = ctx.clone();
        let loop_app = app.clone();
        let mut events = app.subscribe();
        let call = tokio::spawn(async move {
            permissions::decide_call(
                &loop_app,
                &loop_ctx,
                &json!({"tool_name": "mcp__example__write", "tool_input": {}}),
            )
            .await
        });
        let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .unwrap()
            .unwrap();
        let Event::Task { task: waiting } = event else {
            panic!("a held tool call should show its question");
        };
        let question = waiting.question.as_ref().unwrap().text.clone();
        assert!(app.permission_waits.lock().unwrap().contains_key(&task.id));
        if shutdown {
            app.shutdown();
            ctx.cancel.cancel();
        } else {
            app.handle_task_stop(json!({"id": task.id})).await.unwrap();
        }
        let result = tokio::time::timeout(Duration::from_secs(2), call)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result["hookSpecificOutput"]["permissionDecision"], "deny");
        let saved = app.store.load_task(&task.id).unwrap().unwrap();
        if shutdown {
            assert_eq!(saved.status, TaskStatus::Waiting);
            assert_eq!(saved.question.as_ref().unwrap().text, question);
            assert!(saved.decisions.is_empty());
        } else {
            assert_eq!(saved.status, TaskStatus::Stopped);
            assert!(saved.question.is_none());
            assert_eq!(
                stop_close_lines(&saved),
                [format!("Stop: closed the open question \"{question}\"")]
            );
        }
        assert!(!app.permission_waits.lock().unwrap().contains_key(&task.id));
    }
}

#[tokio::test]
async fn a_cancelled_loop_saving_its_old_copy_does_not_undo_a_stop() {
    let (app, _dir) = test_app();
    let mut task = task_with_status(TaskStatus::Running);
    app.store.save_task(&task).unwrap();
    let stale = task.clone();
    app.handle_task_stop(json!({"id": task.id})).await.unwrap();

    app.save_task_logged(&stale);
    task = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Stopped);
    assert!(task.queue.stopped_explicitly);
    app.handle_task_archive(json!({"id": task.id}))
        .await
        .unwrap();
}

#[tokio::test]
async fn a_stale_save_after_a_stop_keeps_the_stop_lines_and_the_closed_question() {
    let (app, _dir) = test_app();
    let task = stop_waiting_task();
    app.store.save_task(&task).unwrap();
    let stale = task.clone();
    app.handle_task_stop(json!({"id": task.id})).await.unwrap();

    app.save_task_logged(&stale);
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.status, TaskStatus::Stopped);
    assert!(saved.question.is_none());
    assert_eq!(
        stop_close_lines(&saved),
        ["Stop: closed the open question \"Which implementation should I use?\""]
    );
    assert!(saved.decisions.iter().any(|l| l.contains("stop")));
}

#[tokio::test]
async fn a_start_after_a_stop_is_not_frozen_by_the_stale_save_guard() {
    let (app, _dir) = test_app();
    let task = task_with_status(TaskStatus::Running);
    app.store.save_task(&task).unwrap();
    app.handle_task_stop(json!({"id": task.id})).await.unwrap();
    app.update_task(&task.id, |t| {
        t.status = TaskStatus::Queued;
        t.queue.stopped_explicitly = false;
    })
    .unwrap();

    let mut running = task.clone();
    running.status = TaskStatus::Running;
    app.save_task_logged(&running);
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.status, TaskStatus::Running);
}

#[tokio::test]
async fn a_stale_save_over_a_parked_task_keeps_it_queued_with_its_park() {
    let (app, _dir) = test_app();
    let task = task_with_status(TaskStatus::Running);
    app.store.save_task(&task).unwrap();
    app.handle_task_park(json!({"id": task.id, "reason": "later"}))
        .await
        .unwrap();
    let parked = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(parked.status, TaskStatus::Queued);
    assert!(parked.queue.park.is_some());

    let mut stale = task.clone();
    stale.status = TaskStatus::Waiting;
    stale.question = stop_waiting_task().question;
    app.save_task_logged(&stale);
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.status, TaskStatus::Queued);
    assert!(saved.question.is_none());
    assert_eq!(saved.queue.park, parked.queue.park);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_stale_saves_never_leave_a_live_loop_task_running_after_a_stop() {
    let (app, _dir) = test_app();
    let task = task_with_status(TaskStatus::Running);
    app.store.save_task(&task).unwrap();
    let cancel = hold_stop_control(&app, &task);
    let savers: Vec<_> = (0..4)
        .map(|_| {
            let app = app.clone();
            let stale = task.clone();
            std::thread::spawn(move || {
                for _ in 0..200 {
                    app.save_task_logged(&stale);
                }
            })
        })
        .collect();
    app.handle_task_stop(json!({"id": task.id})).await.unwrap();
    assert!(cancel.is_cancelled());
    for saver in savers {
        saver.join().unwrap();
    }
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.status, TaskStatus::Stopped);
    assert!(saved.queue.stopped_explicitly);
}
