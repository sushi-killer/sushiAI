
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

fn sh_git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "{args:?}: {out:?}");
}

/// A repo ignoring `artifacts/` with a task worktree holding an ignored
/// deliverable, an untracked one and a committed one.
fn deliverable_fixture() -> (tempfile::TempDir, Task) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    sh_git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join(".gitignore"), "artifacts/\n").unwrap();
    std::fs::write(repo.join("kept.md"), "tracked").unwrap();
    sh_git(&repo, &["add", "-A"]);
    sh_git(&repo, &["commit", "-q", "-m", "init"]);
    let wt = tmp.path().join("wt");
    sh_git(
        &repo,
        &["worktree", "add", "-q", "-b", "task", wt.to_str().unwrap()],
    );
    std::fs::create_dir_all(wt.join("artifacts")).unwrap();
    std::fs::write(wt.join("artifacts/inventory.md"), "report").unwrap();
    std::fs::write(wt.join("notes.md"), "untracked").unwrap();
    let mut task = task_with_status(TaskStatus::Done);
    task.repo = repo.display().to_string();
    task.worktree = wt.display().to_string();
    task.goal = "Write `artifacts/inventory.md` and notes.md".into();
    task.criteria = vec!["kept.md stays as it is".into()];
    task.verify = vec!["test -s artifacts/inventory.md".into()];
    (tmp, task)
}

#[test]
fn deliverables_outside_git_are_kept_before_the_worktree_goes() {
    let (tmp, task) = deliverable_fixture();
    let dest = tmp.path().join("data/deliverables");
    let list = deliverables::collect(&task, &dest);
    let paths: Vec<&str> = list.iter().map(|d| d.path.as_str()).collect();
    assert_eq!(paths, ["artifacts/inventory.md", "notes.md"], "{list:?}");
    assert_eq!(
        std::fs::read_to_string(dest.join("artifacts/inventory.md")).unwrap(),
        "report"
    );
    assert!(list.iter().all(|d| d.placed.is_none()), "not landed yet");
    assert!(!tmp.path().join("repo/artifacts").exists());
}

#[test]
fn a_landed_deliverable_is_placed_in_the_main_copy_without_overwriting() {
    let (tmp, mut task) = deliverable_fixture();
    task.landed_sha = Some("abc".into());
    let repo = tmp.path().join("repo");
    let dest = tmp.path().join("data/deliverables");
    let list = deliverables::collect(&task, &dest);
    let inventory = list.iter().find(|d| d.path == "artifacts/inventory.md");
    assert_eq!(
        inventory.and_then(|d| d.placed.clone()),
        Some(repo.join("artifacts/inventory.md").display().to_string())
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("artifacts/inventory.md")).unwrap(),
        "report"
    );
    // notes.md is not gitignored in the main copy: not placed.
    assert!(list.iter().find(|d| d.path == "notes.md").unwrap().placed.is_none());
    assert!(!repo.join("notes.md").exists());

    // An existing file is never overwritten.
    std::fs::write(repo.join("artifacts/inventory.md"), "mine").unwrap();
    std::fs::write(
        Path::new(&task.worktree).join("artifacts/inventory.md"),
        "newer",
    )
    .unwrap();
    task.deliverables = Vec::new();
    let again = deliverables::collect(&task, &dest);
    assert!(again.iter().all(|d| d.placed.is_none()), "{again:?}");
    assert_eq!(
        std::fs::read_to_string(repo.join("artifacts/inventory.md")).unwrap(),
        "mine"
    );
}

#[tokio::test]
async fn releasing_a_failed_tasks_worktree_keeps_its_deliverables() {
    let (app, _dir) = test_app();
    let (_tmp, mut task) = deliverable_fixture();
    task.status = TaskStatus::Failed;
    app.release_worktree(&mut task, "test").await;
    assert!(!Path::new(&task.worktree).exists());
    assert_eq!(task.deliverables.len(), 2);
    let saved = app
        .store
        .task_dir(&task.id)
        .join("deliverables/artifacts/inventory.md");
    assert_eq!(std::fs::read_to_string(saved).unwrap(), "report");
}

#[test]
fn the_report_lists_deliverables() {
    let (_tmp, mut task) = deliverable_fixture();
    task.deliverables = vec![Deliverable {
        path: "artifacts/inventory.md".into(),
        saved: "/data/deliverables/artifacts/inventory.md".into(),
        placed: Some("/repo/artifacts/inventory.md".into()),
    }];
    let text = crate::report::build(&task, &[&task], &[], &Default::default());
    assert!(text.contains("## Deliverables"), "{text}");
    assert!(text.contains("placed at `/repo/artifacts/inventory.md`"), "{text}");
}
