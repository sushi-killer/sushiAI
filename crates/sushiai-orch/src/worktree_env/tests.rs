use super::*;
use std::path::PathBuf;

const SYSTEM_PATH: &str = "/usr/bin:/bin";

fn write_script(dir: &Path, name: &str, body: &str) {
    let p = dir.join(name);
    std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn worktree() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let wt = tmp.path().join("wt");
    std::fs::create_dir_all(wt.join(".sushiai")).unwrap();
    (tmp, wt)
}

fn sh(script: &str) -> tokio::process::Command {
    let mut c = tokio::process::Command::new("/bin/sh");
    c.arg("-c").arg(script);
    #[cfg(unix)]
    unsafe {
        c.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    c
}

fn run(root: &Path, wt: &Path, path: &str) -> Result<(), String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(
        WorktreeEnvironment::new(root, wt, path, Duration::from_secs(30))
            .setup(sh, std::future::pending()),
    )
}

#[test]
fn setup_json_accepts_a_shell_string_or_an_argv_list_and_rejects_bad_input() {
    let (_tmp, wt) = worktree();
    let write = |text: &str| std::fs::write(wt.join(SETUP_FILE), text).unwrap();
    write(r#"{"setup": "echo hi", "env": {"A": "1"}}"#);
    let p = plan(&wt).unwrap().unwrap();
    assert!(matches!(p.step, Step::Shell(ref s) if s == "echo hi"));
    assert_eq!(p.env["A"], "1");
    write(r#"{"setup": ["make", "deps"]}"#);
    assert!(matches!(plan(&wt).unwrap().unwrap().step, Step::Argv(ref v) if v.len() == 2));
    write("{ not json");
    assert!(plan(&wt).err().unwrap().contains("setup.json is not valid"));
    write(r#"{"setup": []}"#);
    assert!(plan(&wt).is_err());
    write(r#"{"env": {}}"#);
    assert!(plan(&wt).unwrap().is_none(), "no setup key, nothing to run");
    assert!(compiler_cache_enabled(&wt).unwrap());
    write(r#"{"compilerCache": false}"#);
    assert!(plan(&wt).unwrap().is_none());
    assert!(!compiler_cache_enabled(&wt).unwrap());
    write(r#"{"compilerCache": true}"#);
    assert!(compiler_cache_enabled(&wt).unwrap());
    write(r#"{"compilerCache": "false"}"#);
    assert!(plan(&wt).is_err(), "compilerCache must be a boolean");
}

#[test]
fn setup_json_accepts_a_format_command_as_a_string_or_a_list_and_rejects_bad_types() {
    let (_tmp, wt) = worktree();
    let write = |text: &str| std::fs::write(wt.join(SETUP_FILE), text).unwrap();
    let format = || read_setup(&wt).map(|s| s.unwrap().format);
    write(r#"{"format": "fmt \"$@\""}"#);
    assert!(matches!(format().unwrap(), Some(Step::Shell(ref s)) if s == "fmt \"$@\""));
    write(r#"{"setup": "true", "format": ["fmt", "--write"]}"#);
    assert!(matches!(format().unwrap(), Some(Step::Argv(ref v)) if v.len() == 2));
    assert!(
        plan(&wt).unwrap().is_some(),
        "format does not replace setup"
    );
    write(r#"{"env": {}}"#);
    assert!(format().unwrap().is_none());
    write(r#"{"format": 3}"#);
    assert!(format().is_err());
    write(r#"{"format": {"cmd": "fmt"}}"#);
    assert!(format().is_err());
}

#[test]
fn a_format_command_gets_the_paths_as_arguments_in_the_worktree() {
    let (tmp, wt) = worktree();
    let run =
        |command: FormatCommand| {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let paths = vec!["a b.rs".to_string(), "-x'y.cjs".to_string()];
            rt.block_on(
                WorktreeEnvironment::new(tmp.path(), &wt, SYSTEM_PATH, Duration::from_secs(30))
                    .format(&command, &paths, sh, std::future::pending()),
            )
        };
    let shell = FormatCommand {
        step: Step::Shell("for f; do echo \"$f\"; done > out.txt".into()),
        env: BTreeMap::new(),
    };
    run(shell).unwrap();
    assert_eq!(
        std::fs::read_to_string(wt.join("out.txt")).unwrap(),
        "a b.rs\n-x'y.cjs\n"
    );
    let argv = FormatCommand {
        step: Step::Argv(vec![
            "sh".into(),
            "-c".into(),
            "echo \"$@\" > out2.txt".into(),
            "sh".into(),
        ]),
        env: BTreeMap::new(),
    };
    run(argv).unwrap();
    assert_eq!(
        std::fs::read_to_string(wt.join("out2.txt")).unwrap(),
        "a b.rs -x'y.cjs\n"
    );
    let failing = FormatCommand {
        step: Step::Shell("echo boom >&2; exit 4".into()),
        env: BTreeMap::new(),
    };
    let err = run(failing).unwrap_err();
    assert!(
        err.starts_with("format command failed") && err.contains("boom"),
        "{err}"
    );
}

#[test]
fn setup_gets_the_roots_and_the_file_env_and_runs_in_the_worktree() {
    let (tmp, wt) = worktree();
    let root = tmp.path().join("main");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
            wt.join(SETUP_FILE),
            r#"{"setup": "echo \"$ORCHD_ROOT_PATH|$ORCHD_WORKTREE_PATH|$FROM_FILE|$PWD\" > out.txt", "env": {"FROM_FILE": "yes"}}"#,
        )
        .unwrap();
    run(&root, &wt, SYSTEM_PATH).unwrap();
    let got = std::fs::read_to_string(wt.join("out.txt")).unwrap();
    let wt_real = wt.canonicalize().unwrap();
    assert_eq!(
        got.trim(),
        format!(
            "{}|{}|yes|{}",
            root.display(),
            wt.display(),
            wt_real.display()
        )
    );
}

#[test]
fn a_failing_setup_reports_the_output_tail() {
    let (tmp, wt) = worktree();
    std::fs::write(
        wt.join(SETUP_FILE),
        r#"{"setup": "echo to-out; echo to-err >&2; exit 3"}"#,
    )
    .unwrap();
    let err = run(tmp.path(), &wt, SYSTEM_PATH).unwrap_err();
    assert!(err.contains("to-out") && err.contains("to-err"), "{err}");
}

#[test]
fn the_default_install_runs_only_with_a_lockfile_and_no_node_modules() {
    let (tmp, wt) = worktree();
    let bin = tmp.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let log = tmp.path().join("calls.log");
    for tool in ["npm", "pnpm", "yarn"] {
        write_script(
            &bin,
            tool,
            &format!("echo \"{tool} $@\" >> {}", log.display()),
        );
    }
    let path = format!("{}:{SYSTEM_PATH}", bin.display());
    let calls = || std::fs::read_to_string(&log).unwrap_or_default();

    run(tmp.path(), &wt, &path).unwrap();
    assert_eq!(calls(), "", "no lockfile, no install");

    std::fs::write(wt.join("package-lock.json"), "{}").unwrap();
    std::fs::create_dir_all(wt.join("node_modules")).unwrap();
    run(tmp.path(), &wt, &path).unwrap();
    assert_eq!(calls(), "", "node_modules present, no install");
    std::fs::remove_dir_all(wt.join("node_modules")).unwrap();

    run(tmp.path(), &wt, &path).unwrap();
    assert_eq!(calls(), "npm ci\n");

    std::fs::write(wt.join("yarn.lock"), "").unwrap();
    run(tmp.path(), &wt, &path).unwrap();
    std::fs::write(wt.join("pnpm-lock.yaml"), "").unwrap();
    run(tmp.path(), &wt, &path).unwrap();
    assert_eq!(
        calls(),
        "npm ci\nyarn install --frozen-lockfile\npnpm install --frozen-lockfile\n"
    );
}

#[test]
fn a_setup_json_replaces_the_default_install() {
    let (tmp, wt) = worktree();
    std::fs::write(wt.join("package-lock.json"), "{}").unwrap();
    std::fs::write(wt.join(SETUP_FILE), r#"{"setup": "true"}"#).unwrap();
    let bin = tmp.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    write_script(&bin, "npm", "exit 9");
    let path = format!("{}:{SYSTEM_PATH}", bin.display());
    run(tmp.path(), &wt, &path).unwrap();
}

#[test]
fn a_setup_stops_at_the_timeout() {
    let (tmp, wt) = worktree();
    std::fs::write(wt.join(SETUP_FILE), r#"{"setup": "sleep 30"}"#).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let err = rt
        .block_on(
            WorktreeEnvironment::new(tmp.path(), &wt, SYSTEM_PATH, Duration::from_millis(200))
                .setup(sh, std::future::pending()),
        )
        .unwrap_err();
    assert!(err.contains("timed out"), "{err}");
}

fn gone(pid: i32) -> bool {
    crate::procs::testing::wait_until("the process to be gone", || unsafe {
        libc::kill(pid, 0) != 0
    });
    true
}

fn stop_detached_grandchild(timeout: Duration, cancel: Duration) {
    let (tmp, wt) = worktree();
    let exe = crate::procs::testing::place(&wt);
    let pidfile = tmp.path().join("pid");
    let marker = tmp.path().join("marker");
    let script = format!(
        "trap 'echo t > {marker}; exit 0' TERM; {detach}; wait",
        marker = shell_quote(&marker.display().to_string()),
        detach = crate::procs::testing::detach(&exe, &pidfile),
    );
    std::fs::write(
        wt.join(SETUP_FILE),
        serde_json::json!({ "setup": script }).to_string(),
    )
    .unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let err = rt
        .block_on(
            WorktreeEnvironment::new(tmp.path(), &wt, "/usr/bin:/bin", timeout)
                .setup(sh, async move { tokio::time::sleep(cancel).await }),
        )
        .unwrap_err();
    assert!(
        err.contains("timed out") || err.contains("stopped"),
        "{err}"
    );
    let pid: i32 = std::fs::read_to_string(&pidfile)
        .expect("the setup wrote the pid")
        .trim()
        .parse()
        .unwrap();
    assert!(marker.exists(), "the group was not sent SIGTERM first");
    assert!(gone(pid), "the detached grandchild {pid} is still alive");
}

#[test]
fn a_timed_out_setup_gets_sigterm_and_loses_its_detached_grandchild() {
    stop_detached_grandchild(Duration::from_millis(1500), Duration::from_secs(60));
}

#[test]
fn a_cancelled_setup_gets_sigterm_and_loses_its_detached_grandchild() {
    stop_detached_grandchild(Duration::from_secs(60), Duration::from_millis(1500));
}
