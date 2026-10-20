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

// A cargo config can name a compiler wrapper (sccache) whose server cannot
// start inside the profile: every compile then fails with EPERM. The empty
// RUSTC_WRAPPER of the policy must win over that config on the real path.
#[cfg(target_os = "macos")]
#[tokio::test]
async fn a_sandboxed_cargo_check_skips_the_configured_rustc_wrapper() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let marker = root.join("wrapper-ran");
    let wrapper = root.join("wrapper");
    std::fs::write(
        &wrapper,
        format!("#!/bin/sh\ntouch '{}'\nexit 7\n", marker.display()),
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let worktree = root.join("wt");
    std::fs::create_dir_all(worktree.join("src")).unwrap();
    std::fs::create_dir_all(worktree.join(".cargo")).unwrap();
    std::fs::write(
        worktree.join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n",
    )
    .unwrap();
    std::fs::write(worktree.join("src/lib.rs"), "pub fn probe() {}\n").unwrap();
    std::fs::write(
        worktree.join(".cargo/config.toml"),
        format!("[build]\nrustc-wrapper = \"{}\"\n", wrapper.display()),
    )
    .unwrap();
    let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
    let cargo_home = std::env::var_os("CARGO_HOME").map(std::path::PathBuf::from);
    let rustup_home = std::env::var_os("RUSTUP_HOME").map(std::path::PathBuf::from);
    let policy = TaskSandbox::build(SandboxInputs {
        repo_root: &worktree,
        worktree: &worktree,
        run_dir: &root.join("run"),
        data_dir: &root.join("data"),
        sushiai_home: &root.join("sushiai"),
        worktree_root: "",
        home: &home,
        daemon_socket: &root.join("daemon.sock"),
        cargo_home: cargo_home.as_deref(),
        rustup_home: rustup_home.as_deref(),
        tmp_dir: None,
        npm_cache: None,
        known_project_folders: &[],
        known_task_worktrees: &[],
    });
    let output = super::build_verify_command(
        &worktree,
        "cargo build --offline --quiet",
        Some(&policy),
        "base",
    )
    .output()
    .await
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!marker.exists());
}
