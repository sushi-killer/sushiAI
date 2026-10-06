//! `task.pr`: the owner pushes a finished task's branch to `origin` and opens
//! a GitHub pull request against the repo's default branch. An owner action
//! only: it is deliberately not an MCP tool, so an agent never pushes.

use super::*;
use std::collections::HashSet;
use tokio::process::Command;

const GIT_TIMEOUT: Duration = Duration::from_secs(60);
const PUSH_TIMEOUT: Duration = Duration::from_secs(120);
const GH_TIMEOUT: Duration = Duration::from_secs(60);

/// The `gh` binary; `ORCHD_GH_BIN` lets a test point it at a fake script.
fn gh_bin() -> String {
    std::env::var("ORCHD_GH_BIN")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "gh".to_string())
}

/// Runs a command in `cwd` with a timeout; stdout on success, stderr in the error.
async fn run_cmd(
    program: &str,
    args: &[&str],
    cwd: &Path,
    timeout: Duration,
) -> Result<String, String> {
    let shown = format!("{program} {}", args.first().copied().unwrap_or(""));
    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GH_PROMPT_DISABLED", "1")
        .kill_on_drop(true);
    if std::env::var_os("GIT_SSH_COMMAND").is_none() {
        cmd.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
    }
    let out = tokio::time::timeout(timeout, cmd.output())
        .await
        .map_err(|_| format!("{shown} timed out after {}s", timeout.as_secs()))?
        .map_err(|e| format!("{shown}: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("{shown} failed: {}", err.trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The PR body: goal, criteria as a checklist, then the report summary.
fn pr_body(task: &Task) -> String {
    let mut body = format!("{}\n", task.goal.trim());
    if !task.criteria.is_empty() {
        body.push_str("\n## Criteria\n\n");
        for c in &task.criteria {
            body.push_str(&format!("- [x] {}\n", c.trim()));
        }
    }
    if let Some(report) = task.report.as_deref().filter(|r| !r.trim().is_empty()) {
        body.push_str("\n## Report\n\n");
        body.push_str(report.trim());
        body.push('\n');
    }
    body
}

/// Task ids with a `task.pr` running: a second call is refused.
static IN_FLIGHT: StdMutex<Option<HashSet<String>>> = StdMutex::new(None);

struct InFlight(String);

impl InFlight {
    fn take(id: &str) -> Result<InFlight, String> {
        let mut guard = IN_FLIGHT.lock().unwrap();
        if !guard
            .get_or_insert_with(HashSet::new)
            .insert(id.to_string())
        {
            return Err("a pull request is already being opened".to_string());
        }
        Ok(InFlight(id.to_string()))
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        if let Some(set) = IN_FLIGHT.lock().unwrap().as_mut() {
            set.remove(&self.0);
        }
    }
}

/// Syntax rules for the remote branch name the owner typed, before git sees it.
fn check_branch_syntax(name: &str) -> Result<(), String> {
    let bad = name.is_empty()
        || name.starts_with('-')
        || name.starts_with("refs/")
        || name.contains(':')
        || name.contains('+')
        || name.contains("@{")
        || name == "HEAD";
    if bad {
        return Err(format!("{name:?} is not a usable remote branch name"));
    }
    Ok(())
}

/// First field of the first `ls-remote` line: the sha of a remote ref.
fn ls_remote_sha(out: &str) -> Option<String> {
    out.lines()
        .next()
        .and_then(|l| l.split_whitespace().next())
        .map(str::to_string)
}

impl App {
    pub(super) async fn handle_task_pr(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        struct P {
            id: String,
            title: Option<String>,
            branch: Option<String>,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        validate_task_id(&self.store, &p.id)?;
        let _guard = InFlight::take(&p.id)?;
        let task = self
            .store
            .load_task(&p.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        if task.parent.is_some() {
            return Err("only a top-level task can open a pull request".to_string());
        }
        if task.status != TaskStatus::Done {
            return Err("only a done task can open a pull request".to_string());
        }
        if task.archived {
            return Err("task is archived; unarchive it first".to_string());
        }
        if task.landed_sha.is_some() {
            return Err(
                "task is already landed; there is nothing to open a pull request for".to_string(),
            );
        }
        let branch = p
            .branch
            .as_deref()
            .map(str::trim)
            .filter(|b| !b.is_empty())
            .unwrap_or(&task.branch)
            .to_string();
        let title = p
            .title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .unwrap_or(&task.title)
            .to_string();
        check_branch_syntax(&branch)?;
        let repo = PathBuf::from(&task.repo);
        run_cmd(
            "git",
            &["check-ref-format", "--branch", &branch],
            &repo,
            GIT_TIMEOUT,
        )
        .await
        .map_err(|_| format!("{branch:?} is not a valid branch name"))?;
        let local = format!("refs/heads/{}", task.branch);
        run_cmd(
            "git",
            &["rev-parse", "--verify", "-q", &local],
            &repo,
            GIT_TIMEOUT,
        )
        .await
        .map_err(|_| format!("the task branch {} no longer exists", task.branch))?;

        // The base: the branch the task started from when origin has it,
        // else the repo's default branch.
        let default = {
            let repo = repo.clone();
            tokio::task::spawn_blocking(move || git::default_branch(&repo))
                .await
                .map_err(|e| e.to_string())?
        };
        let mut base = None;
        if let Some(b) = task.base_ref.as_deref().filter(|b| !b.is_empty()) {
            if check_branch_syntax(b).is_ok() {
                let on_origin = run_cmd(
                    "git",
                    &["ls-remote", "--heads", "origin", &format!("refs/heads/{b}")],
                    &repo,
                    GIT_TIMEOUT,
                )
                .await?;
                if ls_remote_sha(&on_origin).is_some() {
                    base = Some(b.to_string());
                }
            }
        }
        let base = base.or(default.clone()).ok_or_else(|| {
            "the repo has no default branch to open a pull request against".to_string()
        })?;
        if [
            Some(base.as_str()),
            default.as_deref(),
            task.base_ref.as_deref(),
        ]
        .contains(&Some(branch.as_str()))
        {
            return Err(format!(
                "{branch} is a base branch; pick another remote branch name for the pull request"
            ));
        }
        run_cmd("git", &["fetch", "origin", &base], &repo, GIT_TIMEOUT).await?;
        let count = run_cmd(
            "git",
            &["rev-list", "--count", &format!("origin/{base}..{local}")],
            &repo,
            GIT_TIMEOUT,
        )
        .await?;
        if count.trim() == "0" {
            return Err(format!(
                "branch {} has no commits beyond origin/{base}; nothing to open a pull request for",
                task.branch
            ));
        }

        // Never overwrite a remote branch that is not an ancestor of ours.
        let remote_ref = format!("refs/heads/{branch}");
        let remote = run_cmd(
            "git",
            &["ls-remote", "--heads", "origin", &remote_ref],
            &repo,
            GIT_TIMEOUT,
        )
        .await?;
        let remote_sha = ls_remote_sha(&remote);
        if let Some(sha) = &remote_sha {
            run_cmd(
                "git",
                &["merge-base", "--is-ancestor", sha, &local],
                &repo,
                GIT_TIMEOUT,
            )
            .await
            .map_err(|_| {
                format!("origin already has a branch {branch} that is not part of this task; pick another name")
            })?;
        }
        let lease = format!(
            "--force-with-lease={remote_ref}:{}",
            remote_sha.as_deref().unwrap_or("")
        );
        run_cmd(
            "git",
            &[
                "push",
                "-u",
                &lease,
                "origin",
                &format!("{local}:{remote_ref}"),
            ],
            &repo,
            PUSH_TIMEOUT,
        )
        .await?;

        let gh = gh_bin();
        let open = run_cmd(
            &gh,
            &[
                "pr", "list", "--head", &branch, "--state", "open", "--json", "url", "--jq",
                ".[0].url",
            ],
            &repo,
            GH_TIMEOUT,
        )
        .await?;
        let existing = open
            .lines()
            .last()
            .map(str::trim)
            .filter(|u| u.starts_with("http"));
        let (url, verb) = match existing {
            Some(url) => (url.to_string(), "updated"),
            None => {
                let out = run_cmd(
                    &gh,
                    &[
                        "pr",
                        "create",
                        "--base",
                        &base,
                        "--head",
                        &branch,
                        "--title",
                        &title,
                        "--body",
                        &pr_body(&task),
                    ],
                    &repo,
                    GH_TIMEOUT,
                )
                .await?;
                let url = out
                    .lines()
                    .rev()
                    .find(|l| l.trim().starts_with("http"))
                    .map(|l| l.trim().to_string())
                    .ok_or_else(|| format!("gh did not print a pull request URL: {out}"))?;
                (url, "opened")
            }
        };
        // Reload: the task may have changed while the commands ran.
        let mut task = self
            .store
            .load_task(&p.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        task.pr_url = Some(url.clone());
        task.decisions.push(format!("PR: {verb} {url}"));
        task.updated_at = now_ms();
        self.store.save_task(&task).map_err(|e| e.to_string())?;
        self.broadcast_task(&task);
        serde_json::to_value(&task).map_err(|e| e.to_string())
    }
}
