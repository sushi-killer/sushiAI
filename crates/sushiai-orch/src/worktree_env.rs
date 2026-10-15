//! The build environment of a task worktree. A repo describes it in a committed
//! `.sushiai/setup.json`; without one, a repo with a lockfile and no
//! `node_modules` gets its package manager's install. The setup runs once in a
//! fresh worktree, through the command builder the caller passes (the engine
//! hands it the verify sandbox wrapper, so setup has the same filesystem
//! limits as a check), with the daemon's own environment plus `PATH`,
//! `ORCHD_ROOT_PATH`, `ORCHD_WORKTREE_PATH`, the file's `env`,
//! `npm_config_cache` on macOS and the shared compiler cache when that is
//! installed. No project secret is added.

use std::collections::BTreeMap;
use std::future::Future;
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

const SETUP_FILE: &str = ".sushiai/setup.json";
const TAIL_CHARS: usize = 2000;

/// `"setup"`: a string runs through `sh -c`, a list runs as argv.
#[derive(Deserialize)]
#[serde(untagged)]
enum Step {
    Shell(String),
    Argv(Vec<String>),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetupFile {
    #[serde(default)]
    setup: Option<Step>,
    #[serde(default)]
    format: Option<Step>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default, rename = "compilerCache")]
    compiler_cache: Option<bool>,
    #[serde(default, rename = "scopedChecks")]
    scoped_checks: Vec<RepoScopedCheck>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RepoScopedCheck {
    command: String,
    paths: Vec<String>,
}

struct Plan {
    step: Step,
    env: BTreeMap<String, String>,
}

/// What to run in `worktree`: its `setup.json`, else the default install.
fn plan(worktree: &Path) -> Result<Option<Plan>, String> {
    let Some(parsed) = read_setup(worktree)? else {
        return Ok(default_install(worktree).map(|step| Plan {
            step,
            env: BTreeMap::new(),
        }));
    };
    match parsed.setup {
        Some(Step::Argv(argv)) if argv.is_empty() => {
            Err(format!("{SETUP_FILE}: \"setup\" is an empty list"))
        }
        Some(step) => Ok(Some(Plan {
            step,
            env: parsed.env,
        })),
        None => Ok(None),
    }
}

fn read_setup(worktree: &Path) -> Result<Option<SetupFile>, String> {
    let file = worktree.join(SETUP_FILE);
    if !file.is_file() {
        return Ok(None);
    }
    let text =
        std::fs::read_to_string(&file).map_err(|e| format!("cannot read {SETUP_FILE}: {e}"))?;
    parse_setup(&text).map(Some)
}

fn parse_setup(text: &str) -> Result<SetupFile, String> {
    serde_json::from_str(text).map_err(|e| format!("{SETUP_FILE} is not valid: {e}"))
}

/// Check policy comes from the base commit, never the task's edited file.
pub fn scoped_checks_at(
    repo: &Path,
    rev: &str,
) -> Result<Vec<crate::policy::settings::ScopedCheck>, String> {
    let Some(text) =
        crate::git::history::show_file(repo, rev, SETUP_FILE).map_err(|e| e.to_string())?
    else {
        return Ok(Vec::new());
    };
    Ok(parse_setup(&text)?
        .scoped_checks
        .into_iter()
        .map(|entry| crate::policy::settings::ScopedCheck {
            repo: Some(repo.to_string_lossy().into_owned()),
            command: entry.command,
            paths: entry.paths,
        })
        .collect())
}

/// A repo's `"format"` command and the file's `env`, read from the base commit
/// like the check policy.
pub struct FormatCommand {
    step: Step,
    env: BTreeMap<String, String>,
}

pub fn format_command_at(repo: &Path, rev: &str) -> Result<Option<FormatCommand>, String> {
    let Some(text) =
        crate::git::history::show_file(repo, rev, SETUP_FILE).map_err(|e| e.to_string())?
    else {
        return Ok(None);
    };
    let parsed = parse_setup(&text)?;
    match parsed.format {
        Some(Step::Argv(argv)) if argv.is_empty() => {
            Err(format!("{SETUP_FILE}: \"format\" is an empty list"))
        }
        Some(step) => Ok(Some(FormatCommand {
            step,
            env: parsed.env,
        })),
        None => Ok(None),
    }
}

/// Uses the same reader as setup, including files that have no setup step.
pub fn compiler_cache_enabled(worktree: &Path) -> Result<bool, String> {
    Ok(read_setup(worktree)?.is_none_or(|s| s.compiler_cache != Some(false)))
}

/// The install for the lockfile found, when `node_modules` is missing.
fn default_install(worktree: &Path) -> Option<Step> {
    if worktree.join("node_modules").exists() {
        return None;
    }
    let argv: &[&str] = if worktree.join("pnpm-lock.yaml").is_file() {
        &["pnpm", "install", "--frozen-lockfile"]
    } else if worktree.join("yarn.lock").is_file() {
        &["yarn", "install", "--frozen-lockfile"]
    } else if worktree.join("package-lock.json").is_file() {
        &["npm", "ci"]
    } else {
        return None;
    };
    Some(Step::Argv(argv.iter().map(|a| a.to_string()).collect()))
}

fn tail(text: &str) -> String {
    let text = text.trim();
    let skip = text.chars().count().saturating_sub(TAIL_CHARS);
    text.chars().skip(skip).collect()
}

fn shell_quote(arg: &str) -> String {
    format!("'{}'", arg.replace('\'', "'\\''"))
}

/// Runs the worktree's setup, if it has one. `build` turns the `sh -c` script
/// into the command to spawn (the engine's sandboxed builder). `path` is the
/// extended PATH the command gets. Stops at `timeout` or when `cancelled`
/// resolves. `Err` holds the reason with the output tail.
pub async fn run_setup(
    main_root: &Path,
    worktree: &Path,
    path: &str,
    timeout: Duration,
    build: impl FnOnce(&str) -> tokio::process::Command,
    cancelled: impl Future<Output = ()>,
) -> Result<(), String> {
    let Some(plan) = plan(worktree)? else {
        return Ok(());
    };
    let script = script_of(&plan.step, &[]);
    run_script(
        "worktree setup",
        &script,
        &plan.env,
        main_root,
        worktree,
        path,
        timeout,
        build,
        cancelled,
    )
    .await
}

/// Runs the repo's format command in `worktree` with the repo-relative
/// `paths` as its arguments (`"$@"` for a shell string, appended for argv),
/// in the same environment as setup.
#[allow(clippy::too_many_arguments)]
pub async fn run_format(
    command: &FormatCommand,
    paths: &[String],
    main_root: &Path,
    worktree: &Path,
    path: &str,
    timeout: Duration,
    build: impl FnOnce(&str) -> tokio::process::Command,
    cancelled: impl Future<Output = ()>,
) -> Result<(), String> {
    let script = script_of(&command.step, paths);
    run_script(
        "format command",
        &script,
        &command.env,
        main_root,
        worktree,
        path,
        timeout,
        build,
        cancelled,
    )
    .await
}

fn script_of(step: &Step, args: &[String]) -> String {
    let quoted = args
        .iter()
        .map(|a| shell_quote(a))
        .collect::<Vec<_>>()
        .join(" ");
    match step {
        Step::Shell(script) if args.is_empty() => script.clone(),
        Step::Shell(script) => format!("set -- {quoted}; {script}"),
        Step::Argv(argv) => argv
            .iter()
            .map(|a| shell_quote(a))
            .chain(args.iter().map(|a| shell_quote(a)))
            .collect::<Vec<_>>()
            .join(" "),
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_script(
    label: &str,
    script: &str,
    env: &BTreeMap<String, String>,
    main_root: &Path,
    worktree: &Path,
    path: &str,
    timeout: Duration,
    build: impl FnOnce(&str) -> tokio::process::Command,
    cancelled: impl Future<Output = ()>,
) -> Result<(), String> {
    let mut cmd = build(script);
    #[cfg(target_os = "macos")]
    if let Some(home) = std::env::var_os("HOME") {
        // Under the sandbox's writable ~/Library/Caches.
        cmd.env(
            "npm_config_cache",
            Path::new(&home).join("Library/Caches/sushiai/npm"),
        );
    }
    // A sandboxed `build` already set RUSTC_WRAPPER (empty): the policy owns it.
    let sandboxed = cmd
        .as_std()
        .get_envs()
        .any(|(key, _)| key == "RUSTC_WRAPPER");
    cmd.envs(env);
    if !sandboxed && !env.contains_key("RUSTC_WRAPPER") {
        if let Some(cache) =
            crate::harness::compiler_cache::CompilerCache::for_worktree(worktree, path)
        {
            cache.apply(&mut cmd);
        }
    }
    cmd.env("PATH", path)
        .env("ORCHD_ROOT_PATH", main_root)
        .env("ORCHD_WORKTREE_PATH", worktree)
        .current_dir(worktree)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let running = crate::procs::running_in(worktree);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("could not start the {label}: {e}"))?;
    let pgid = child.id().and_then(|p| i32::try_from(p).ok());
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    // Drain the pipes while waiting, without moving `child`, so its group
    // can still be stopped when the wait is dropped.
    let read = async {
        use tokio::io::AsyncReadExt;
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let (status, _, _) = tokio::try_join!(
            child.wait(),
            async {
                match stdout.as_mut() {
                    Some(s) => s.read_to_end(&mut out).await,
                    None => Ok(0),
                }
            },
            async {
                match stderr.as_mut() {
                    Some(s) => s.read_to_end(&mut err).await,
                    None => Ok(0),
                }
            },
        )?;
        Ok::<_, std::io::Error>((status, out, err))
    };
    enum Ended {
        Done(std::io::Result<(std::process::ExitStatus, Vec<u8>, Vec<u8>)>),
        TimedOut,
        Cancelled,
    }
    let ended = tokio::select! {
        r = read => Ended::Done(r),
        _ = tokio::time::sleep(timeout) => Ended::TimedOut,
        _ = cancelled => Ended::Cancelled,
    };
    drop((stdout, stderr));
    crate::procs::kill_group(pgid, &mut child).await;
    let dir = worktree.to_path_buf();
    if matches!(ended, Ended::Done(_)) {
        if running.alone() {
            let _ =
                tokio::task::spawn_blocking(move || crate::procs::stop_orphans_under(&dir)).await;
        }
    } else {
        let _ = tokio::task::spawn_blocking(move || crate::procs::stop_processes_under(&dir)).await;
    }
    let (status, stdout, stderr) = match ended {
        Ended::Done(r) => r.map_err(|e| format!("{label} failed: {e}"))?,
        Ended::TimedOut => {
            return Err(format!("{label} timed out after {}s", timeout.as_secs()));
        }
        Ended::Cancelled => return Err(format!("{label} stopped")),
    };
    if status.success() {
        return Ok(());
    }
    let output = format!(
        "{}\n{}",
        String::from_utf8_lossy(&stdout),
        String::from_utf8_lossy(&stderr)
    );
    Err(format!("{label} failed ({}): {}", status, tail(&output)))
}

#[cfg(test)]
mod tests {
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
        rt.block_on(run_setup(
            root,
            wt,
            path,
            Duration::from_secs(30),
            sh,
            std::future::pending(),
        ))
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
        let run = |command: FormatCommand| {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let paths = vec!["a b.rs".to_string(), "-x'y.cjs".to_string()];
            rt.block_on(run_format(
                &command,
                &paths,
                tmp.path(),
                &wt,
                SYSTEM_PATH,
                Duration::from_secs(30),
                sh,
                std::future::pending(),
            ))
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
            .block_on(run_setup(
                tmp.path(),
                &wt,
                SYSTEM_PATH,
                Duration::from_millis(200),
                sh,
                std::future::pending(),
            ))
            .unwrap_err();
        assert!(err.contains("timed out"), "{err}");
    }

    fn gone(pid: i32) -> bool {
        for _ in 0..200 {
            if unsafe { libc::kill(pid, 0) } != 0 {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
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
            .block_on(run_setup(
                tmp.path(),
                &wt,
                "/usr/bin:/bin",
                timeout,
                sh,
                async move { tokio::time::sleep(cancel).await },
            ))
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
}
