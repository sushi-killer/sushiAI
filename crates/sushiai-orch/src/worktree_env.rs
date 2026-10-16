//! The build environment of a task worktree. A repo describes it in a committed
//! `.sushiai/setup.json`; without one, a repo with a lockfile and no
//! `node_modules` gets its package manager's install. The setup runs once in a
//! fresh worktree, through the command builder the caller passes (the engine
//! hands it the verify sandbox wrapper, so setup has the same filesystem
//! limits as a check), with the daemon's own environment plus `PATH`,
//! `ORCHD_ROOT_PATH`, `ORCHD_WORKTREE_PATH`, the file's `env`,
//! `npm_config_cache` on macOS and the shared compiler cache when that is
//! installed. No project secret is added.

use crate::clip::tail_chars;
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

fn shell_quote(arg: &str) -> String {
    format!("'{}'", arg.replace('\'', "'\\''"))
}

/// Process environment shared by worktree setup and repository formatting.
pub struct WorktreeEnvironment<'a> {
    main_root: &'a Path,
    worktree: &'a Path,
    path: &'a str,
    timeout: Duration,
}

struct Script<'a> {
    label: &'static str,
    text: String,
    env: &'a BTreeMap<String, String>,
}

impl<'a> WorktreeEnvironment<'a> {
    pub fn new(main_root: &'a Path, worktree: &'a Path, path: &'a str, timeout: Duration) -> Self {
        Self {
            main_root,
            worktree,
            path,
            timeout,
        }
    }

    /// Runs the worktree's setup through the caller's command builder.
    /// Stops at the environment's timeout or when `cancelled` resolves.
    pub async fn setup(
        &self,
        build: impl FnOnce(&str) -> tokio::process::Command,
        cancelled: impl Future<Output = ()>,
    ) -> Result<(), String> {
        let Some(plan) = plan(self.worktree)? else {
            return Ok(());
        };
        self.run_script(
            Script {
                label: "worktree setup",
                text: script_of(&plan.step, &[]),
                env: &plan.env,
            },
            build,
            cancelled,
        )
        .await
    }

    /// Runs the repo's format command with repo-relative paths as arguments.
    pub async fn format(
        &self,
        command: &FormatCommand,
        paths: &[String],
        build: impl FnOnce(&str) -> tokio::process::Command,
        cancelled: impl Future<Output = ()>,
    ) -> Result<(), String> {
        self.run_script(
            Script {
                label: "format command",
                text: script_of(&command.step, paths),
                env: &command.env,
            },
            build,
            cancelled,
        )
        .await
    }
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

impl WorktreeEnvironment<'_> {
    fn command(
        &self,
        script: &Script<'_>,
        build: impl FnOnce(&str) -> tokio::process::Command,
    ) -> tokio::process::Command {
        let mut cmd = build(&script.text);
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
        cmd.envs(script.env);
        if !sandboxed && !script.env.contains_key("RUSTC_WRAPPER") {
            if let Some(cache) = crate::harness::compiler_cache::CompilerCache::for_worktree(
                self.worktree,
                self.path,
            ) {
                cache.apply(&mut cmd);
            }
        }
        cmd.env("PATH", self.path)
            .env("ORCHD_ROOT_PATH", self.main_root)
            .env("ORCHD_WORKTREE_PATH", self.worktree)
            .current_dir(self.worktree)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        cmd
    }

    async fn run_script(
        &self,
        script: Script<'_>,
        build: impl FnOnce(&str) -> tokio::process::Command,
        cancelled: impl Future<Output = ()>,
    ) -> Result<(), String> {
        let worktree = self.worktree;
        let timeout = self.timeout;
        let label = script.label;
        let mut cmd = self.command(&script, build);
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
                let _ = tokio::task::spawn_blocking(move || crate::procs::stop_orphans_under(&dir))
                    .await;
            }
        } else {
            let _ =
                tokio::task::spawn_blocking(move || crate::procs::stop_processes_under(&dir)).await;
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
        Err(format!(
            "{label} failed ({}): {}",
            status,
            tail_chars(output.trim(), TAIL_CHARS)
        ))
    }
}

#[cfg(test)]
mod tests;
