#[tokio::test]
async fn recover_on_start_skips_archived_tasks() {
    let (app, _dir) = test_app();
    let mut task = task_with_status(TaskStatus::Queued);
    task.archived = true;
    app.store.save_task(&task).unwrap();

    app.recover_on_start().unwrap();

    assert!(
        !app.controls.lock().unwrap().contains_key(&task.id),
        "an archived task must not get a loop started on recovery"
    );
}

fn running_implement_task(app: &App, dir: &Path) -> (Task, PathBuf) {
    let mut task = task_with_status(TaskStatus::Running);
    task.attempts.push(attempt_with_failure(1, "x"));
    let attempt = &mut task.attempts[0];
    attempt.status = AttemptStatus::Running;
    attempt.failure = None;
    // A process group that is gone: a short child, reaped.
    let mut child = std::process::Command::new("true").spawn().unwrap();
    attempt.pgid = Some(child.id() as i32);
    child.wait().unwrap();
    app.store.save_task(&task).unwrap();
    let run_dir = dir.join("data/tasks").join(&task.id).join("runs/1");
    std::fs::create_dir_all(&run_dir).unwrap();
    (task, run_dir)
}

#[tokio::test]
async fn a_run_that_ended_while_the_daemon_was_down_is_resumed_from_its_files() {
    let (app, dir) = test_app();
    let (task, run_dir) = running_implement_task(&app, dir.path());
    std::fs::write(
        run_dir.join("events.raw"),
        "{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s1\"}\n{\"type\":\"result\",\"total_cost_usd\":0.5,\"usage\":{\"input_tokens\":3,\"output_tokens\":4}}\n",
    )
    .unwrap();
    std::fs::write(run_dir.join("events.exit"), "1\n").unwrap();
    std::fs::write(run_dir.join("events.stderr.log"), "boom\n").unwrap();

    let recovered = app
        .store
        .recover_interrupted_with(
            |task, idx| app.run_fate(task, idx),
            |_, _| panic!("a run with its files is not interrupted"),
        )
        .unwrap();
    assert!(recovered.is_empty());
    let task = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(task.attempts[0].status, AttemptStatus::Running);
    assert_eq!(recovery::resumable_attempt(&app, &task), Some(0));

    // The loop follows it and gets the outcome run_harness would have given.
    let outcome = recovery::follow_run(&app.arc(), &task, 0, &CancelToken::new())
        .await
        .ok()
        .expect("outcome");
    assert_eq!(outcome.error.as_deref(), Some("boom"));
    assert_eq!(outcome.session_id.as_deref(), Some("s1"));
    assert_eq!(outcome.cost_usd, Some(0.5));
    let log = std::fs::read_to_string(run_dir.join("events.jsonl")).unwrap();
    assert!(log.contains("total_cost_usd"), "{log}");
}

#[tokio::test]
async fn a_dead_run_without_an_exit_file_is_interrupted_and_its_spend_kept() {
    let (app, dir) = test_app();
    let (task, run_dir) = running_implement_task(&app, dir.path());
    std::fs::write(
        run_dir.join("events.raw"),
        "{\"type\":\"result\",\"total_cost_usd\":0.25,\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}\n",
    )
    .unwrap();

    let recovered = app
        .store
        .recover_interrupted_with(
            |task, idx| app.run_fate(task, idx),
            |task, idx| {
                let run_dir = app.store.run_dir(&task.id, task.attempts[idx].n);
                settle_unfinished_cost(task, idx, &run_dir, &Default::default());
            },
        )
        .unwrap();

    assert_eq!(recovered.len(), 1);
    let task = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Queued);
    assert_eq!(task.attempts[0].status, AttemptStatus::Interrupted);
    assert_eq!(task.attempts[0].cost_usd, Some(0.25));
}
