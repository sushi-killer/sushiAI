#[tokio::test]
async fn helpers_folder_task_log_and_timeline_read_helpers_without_a_legacy_fallback() {
    let (app, _dir) = test_app();
    let set_mtime = |path: &std::path::Path, ms: u64| {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(std::time::UNIX_EPOCH + Duration::from_millis(ms))
            .unwrap();
    };
    let mut task = task_with_status(TaskStatus::Done);
    task.fixture_data().attempts = vec![attempt_with_failure(1, "failure")];
    task.fixture_data().attempts[0].ended_at = Some(10_000);
    task.fixture_data().attempts[0].review_cost_usd = Some(0.5);
    task.fixture_data().attempts[0].advisor_cost_usd = Some(0.25);
    app.store.save_task(&task).unwrap();
    for stage in ["review", "advisor"] {
        let legacy = app.store.run_dir(&task.id, 1).join(stage);
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("brief.md"), "legacy prompt").unwrap();
        std::fs::write(legacy.join("events.jsonl"), "legacy").unwrap();
        set_mtime(&legacy.join("brief.md"), 500);
        set_mtime(&legacy.join("events.jsonl"), 5_000);
        let err = app
            .dispatch(
                "task.log",
                json!({"id":task.id, "attempt":1, "stage":stage}),
            )
            .await
            .unwrap_err();
        assert_eq!(err, "no stored log for this stage");
    }
    let timeline = crate::timeline::task_timeline(&app.store, &task, &Default::default(), now_ms());
    for (stage, cost) in [("review", 0.5), ("advisor", 0.25)] {
        let segments: Vec<_> = timeline.iter().filter(|s| s.stage == stage).collect();
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].attempt, 1);
        assert_eq!(segments[0].cost_usd, cost);
        assert_eq!(segments[0].started_at, 10_000);
        assert_eq!(segments[0].ended_at, 10_000);
    }
    for stage in ["review", "advisor"] {
        for (i, text) in ["first", "second"].into_iter().enumerate() {
            let dir = app
                .store
                .claim_helper_run_dir(&task.id, stage, Some(1))
                .unwrap();
            std::fs::write(dir.join("brief.md"), "prompt").unwrap();
            std::fs::write(dir.join("events.jsonl"), json!({"type":"assistant", "message":{"content":[{"type":"tool_use", "name":"Bash", "input":{"command":text}}]}}).to_string()).unwrap();
            let start = if stage == "review" { 6_000 } else { 11_000 } + i as u64 * 3_000;
            set_mtime(&dir.join("brief.md"), start);
            set_mtime(&dir.join("events.jsonl"), start + (i as u64 + 1) * 1_000);
        }
        let unrelated = app
            .store
            .claim_helper_run_dir(&task.id, stage, Some(2))
            .unwrap();
        std::fs::write(unrelated.join("brief.md"), "unrelated prompt").unwrap();
        std::fs::write(unrelated.join("events.jsonl"), json!({"type":"assistant", "message":{"content":[{"type":"tool_use", "name":"Bash", "input":{"command":"unrelated"}}]}}).to_string()).unwrap();
        set_mtime(&unrelated.join("brief.md"), 1_000);
        set_mtime(&unrelated.join("events.jsonl"), 6_000);
        let log = app
            .dispatch(
                "task.log",
                json!({"id":task.id, "attempt":1, "stage":stage}),
            )
            .await
            .unwrap();
        let lines = log["lines"].as_array().unwrap();
        assert_eq!(lines.len(), 2, "{log}");
        assert!(lines[0].as_str().unwrap().contains("first"), "{log}");
        assert!(lines[1].as_str().unwrap().contains("second"), "{log}");
    }
    let timeline = crate::timeline::task_timeline(&app.store, &task, &Default::default(), now_ms());
    for (stage, start, cost) in [("review", 7_000, 0.5), ("advisor", 11_000, 0.25)] {
        let segments: Vec<_> = timeline.iter().filter(|s| s.stage == stage).collect();
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].attempt, 1);
        assert_eq!(segments[0].cost_usd, cost);
        assert_eq!(segments[0].started_at, start);
        assert_eq!(segments[0].ended_at, start + 3_000);
    }
}
