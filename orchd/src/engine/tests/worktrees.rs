
fn task_with_evidence(app: &App, archived: bool, age_days: i64) -> (Task, PathBuf) {
    let mut task = task_with_status(TaskStatus::Done);
    task.archived = archived;
    task.updated_at = now_ms() - age_days * 24 * 60 * 60 * 1000;
    let dir = app.store.run_dir(&task.id, 1).join("evidence");
    std::fs::create_dir_all(&dir).unwrap();
    let image = dir.join("panel.png");
    std::fs::write(&image, vec![0u8; 4096]).unwrap();
    let mut attempt = attempt_with_failure(1, "s");
    attempt.evidence = vec![image.display().to_string()];
    task.attempts = vec![attempt];
    app.store.save_task(&task).unwrap();
    (task, dir)
}

#[tokio::test]
async fn gc_prunes_the_evidence_of_old_archived_tasks_only() {
    let (app, _dir) = test_app();
    let (old, old_dir) = task_with_evidence(&app, true, 20);
    let (recent, recent_dir) = task_with_evidence(&app, true, 3);
    let (live, live_dir) = task_with_evidence(&app, false, 40);

    let dry = app
        .handle_worktrees_gc(json!({"dryRun": true}))
        .await
        .unwrap();
    let entries = dry["evidence"].as_array().unwrap();
    assert_eq!(entries.len(), 1, "{dry}");
    assert_eq!(entries[0]["task"], old.id);
    assert_eq!(entries[0]["action"], "would-remove");
    assert_eq!(entries[0]["path"], old_dir.to_string_lossy().as_ref());
    assert!(entries[0]["bytes"].as_u64().unwrap() >= 4096);
    assert!(dry["freedBytes"].as_u64().unwrap() >= 4096);
    assert!(old_dir.exists(), "a dry run changes nothing");
    let reloaded = app.store.load_task(&old.id).unwrap().unwrap();
    assert_eq!(reloaded.attempts[0].evidence.len(), 1);

    let real = app.handle_worktrees_gc(json!({})).await.unwrap();
    let entries = real["evidence"].as_array().unwrap();
    assert_eq!(entries.len(), 1, "{real}");
    assert_eq!(entries[0]["action"], "removed");
    assert_eq!(real["freedBytes"], dry["freedBytes"]);
    assert!(!old_dir.exists());
    assert!(recent_dir.join("panel.png").exists());
    assert!(live_dir.join("panel.png").exists());
    let reloaded = app.store.load_task(&old.id).unwrap().unwrap();
    assert!(reloaded.attempts[0].evidence.is_empty());
    for kept in [&recent, &live] {
        let t = app.store.load_task(&kept.id).unwrap().unwrap();
        assert_eq!(t.attempts[0].evidence.len(), 1);
    }
}
