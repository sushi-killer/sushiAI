use crate::harness;
use crate::harness::native::launch::wrap_task_harness;
use crate::policy::routing::Harness;
use crate::sandbox::TaskSandbox;

#[test]
fn codex_task_wrapper_preserves_host_and_non_task_runs() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let policy = TaskSandbox::build(crate::sandbox::Inputs {
        repo_root: root,
        worktree: root,
        run_dir: root,
        data_dir: &root.join("data"),
        sushiai_home: &root.join("sushiai"),
        worktree_root: "",
        home: &root.join("home"),
        daemon_socket: &root.join("daemon.sock"),
        cargo_home: None,
        rustup_home: None,
        tmp_dir: None,
        npm_cache: None,
        known_project_folders: &[],
        known_task_worktrees: &[],
    });
    let mut req = harness::RunRequest {
        harness: Harness::Codex,
        worktree: root,
        model: None,
        effort: None,
        max_budget_usd: None,
        review: false,
        task_run: true,
        task_sandbox: Some(&policy),
        mcp_config: None,
        settings_path: None,
        network_allowed: false,
        codex_mcp: &[],
        images: &[],
        repo_settings: true,
        resume: None,
    };
    let argv = harness::codex_argv(&req);
    let (bin, wrapped) = wrap_task_harness("codex".into(), argv.clone(), &req);
    #[cfg(target_os = "macos")]
    {
        assert_eq!(bin, "sandbox-exec");
        assert_eq!(
            &wrapped[..3],
            &["-p", crate::sandbox::to_sbpl(&policy).as_str(), "codex"]
        );
        assert_eq!(&wrapped[3..], argv);
    }
    #[cfg(not(target_os = "macos"))]
    assert_eq!((bin, wrapped), ("codex".into(), argv.clone()));
    req.task_sandbox = None;
    assert_eq!(
        wrap_task_harness("codex".into(), argv.clone(), &req),
        ("codex".into(), argv.clone())
    );
    req.task_sandbox = Some(&policy);
    req.review = true;
    assert_eq!(
        wrap_task_harness("codex".into(), argv.clone(), &req),
        ("codex".into(), argv.clone())
    );
    req.review = false;
    req.task_run = false;
    assert_eq!(
        wrap_task_harness("codex".into(), argv.clone(), &req),
        ("codex".into(), argv)
    );
}
