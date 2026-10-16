use super::{build_verify_command_in, VerifyEnvironment};
use crate::sandbox::{Inputs as SandboxInputs, TaskSandbox};
use std::path::Path;

#[test]
fn a_native_sandbox_check_overrides_a_configured_rustc_wrapper() {
    let root = Path::new("/");
    let policy = TaskSandbox::build(SandboxInputs {
        repo_root: root,
        worktree: root,
        run_dir: &root.join("run"),
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
    let wrapper = |sandbox: Option<&TaskSandbox>| {
        let environment = VerifyEnvironment {
            path: "/usr/bin:/bin".into(),
            compiler_cache: None,
        };
        let command = build_verify_command_in(root, "true", sandbox, "base", &environment);
        command
            .as_std()
            .get_envs()
            .find(|(key, _)| *key == "RUSTC_WRAPPER")
            .map(|(_, value)| value.map(|v| v.to_os_string()))
    };
    assert_eq!(wrapper(Some(&policy)), Some(Some("".into())));
    assert_eq!(wrapper(None), None);
}
