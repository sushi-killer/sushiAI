//! `task.pr`: pushes a finished task's branch to `origin` and opens a GitHub
//! pull request against the repo's default branch. The owner starts it from the
//! app; an agent may call it as the `task_pr` MCP tool only where automatic pull
//! requests are on (`may_open_pr_on_done`). It pushes
//! the task's own branch only, never the default branch, and never merges.

use super::*;
use std::collections::HashSet;
use tokio::process::Command;

pub(super) const GIT_TIMEOUT: Duration = Duration::from_secs(60);
const PUSH_TIMEOUT: Duration = Duration::from_secs(120);
pub(super) const GH_TIMEOUT: Duration = Duration::from_secs(60);

/// The `gh` binary; `ORCHD_GH_BIN` lets a test point it at a fake script.
pub(super) fn gh_bin() -> String {
    std::env::var("ORCHD_GH_BIN")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "gh".to_string())
}

/// Runs a command in `cwd` with a timeout; stdout on success, stderr in the error.
pub(super) async fn run_cmd(
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

/// First line of the body of a pull request for work no review read.
pub(super) const UNREVIEWED_LINE: &str = "Unreviewed: no review route gave a verdict";

/// The PR body: goal, criteria as a checklist, then the report summary.
/// What the pull request and the decision say of a subtask that landed
/// unreviewed.
fn unreviewed_part_text(id: &str, title: &str) -> String {
    format!("subtask \"{title}\" ({id}) landed unreviewed: no review route gave a verdict")
}

/// The decision a parent records for a subtask that landed unreviewed.
pub(super) fn unreviewed_part_line(child: &Task) -> String {
    format!(
        "Orchestrator: {}",
        unreviewed_part_text(&child.id, &child.title)
    )
}

pub(super) fn pr_body(task: &Task) -> String {
    let mark = if task.unreviewed { " " } else { "x" };
    let mut body = String::new();
    if task.unreviewed {
        body.push_str(&format!("{UNREVIEWED_LINE}\n\n"));
        for part in &task.loop_state.unreviewed_parts {
            body.push_str(&format!(
                "- Orchestrator: {}\n",
                unreviewed_part_text(&part.id, &part.title)
            ));
        }
        if !task.loop_state.unreviewed_parts.is_empty() {
            body.push('\n');
        }
    }
    body.push_str(&format!("{}\n", task.goal.trim()));
    if !task.criteria.is_empty() {
        body.push_str("\n## Criteria\n\n");
        for c in &task.criteria {
            body.push_str(&format!("- [{mark}] {}\n", c.trim()));
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

pub(super) struct InFlight(String);

impl InFlight {
    pub(super) fn take(id: &str) -> Result<InFlight, String> {
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
pub(super) fn check_branch_syntax(name: &str) -> Result<(), String> {
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

/// Whether the drafted remote branch name `branch` is safe to push for
/// `task`: well formed, no base branch, and not yet on origin.
pub(super) async fn check_draft_branch(task: &Task, branch: &str) -> Result<(), String> {
    check_branch_syntax(branch)?;
    let repo = PathBuf::from(&task.repo);
    run_cmd(
        "git",
        &["check-ref-format", "--branch", branch],
        &repo,
        GIT_TIMEOUT,
    )
    .await
    .map_err(|_| format!("{branch:?} is not a valid branch name"))?;
    let default = {
        let repo = repo.clone();
        tokio::task::spawn_blocking(move || git::default_branch(&repo))
            .await
            .map_err(|e| e.to_string())?
    };
    if [default.as_deref(), task.base_ref.as_deref()].contains(&Some(branch)) {
        return Err("it is a base branch".to_string());
    }
    let remote = run_cmd(
        "git",
        &[
            "ls-remote",
            "--heads",
            "origin",
            &format!("refs/heads/{branch}"),
        ],
        &repo,
        GIT_TIMEOUT,
    )
    .await?;
    if ls_remote_sha(&remote).is_some() {
        return Err("origin already has it".to_string());
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

/// The base a pull request targets: the branch the task started from when
/// origin has it, else the repo's default branch.
pub(super) async fn pr_base(
    task: &Task,
    repo: &Path,
    default: Option<String>,
) -> Result<String, String> {
    if let Some(b) = task.base_ref.as_deref().filter(|b| !b.is_empty()) {
        if check_branch_syntax(b).is_ok() {
            let on_origin = run_cmd(
                "git",
                &["ls-remote", "--heads", "origin", &format!("refs/heads/{b}")],
                repo,
                GIT_TIMEOUT,
            )
            .await?;
            if ls_remote_sha(&on_origin).is_some() {
                return Ok(b.to_string());
            }
        }
    }
    default
        .ok_or_else(|| "the repo has no default branch to open a pull request against".to_string())
}

impl App {
    /// The RPC entry: a branch that needs the newest base is carried off the
    /// request path, so a slow carry never stalls the connection.
    pub(super) async fn handle_task_pr(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        self.open_pr(params, false).await
    }

    /// `inline`: only the in-process pull request flow sets it; it carries
    /// onto the newest base before the push, as part of its own background run.
    pub(super) async fn open_pr(
        &self,
        params: serde_json::Value,
        inline: bool,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        struct P {
            id: String,
            title: Option<String>,
            branch: Option<String>,
            /// Replaces the generated body when not empty.
            body: Option<String>,
            /// Set by the MCP bridge; the app sends none.
            #[serde(default)]
            source: Option<String>,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        if p.source.as_deref().is_some_and(|s| !s.is_empty()) && p.branch.is_some() {
            return Err(
                "an agent opens the pull request for the task's own branch only".to_string(),
            );
        }
        validate_task_id(&self.store, &p.id)?;
        let _guard = InFlight::take(&p.id)?;
        let task = self
            .store
            .load_task(&p.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        // The owner's own calls (the app, or the owner's chat) always open one;
        // an agent opens one only where automatic pull requests are on.
        let by_owner = matches!(p.source.as_deref(), None | Some("") | Some("chat"));
        let automatic = self
            .settings
            .read()
            .unwrap()
            .may_open_pr_on_done(&task.repo);
        if !by_owner && !automatic {
            return Err(
                "automatic pull requests are off for this repo: the owner opens the pull request"
                    .to_string(),
            );
        }
        if task.parent.is_some() {
            return Err("only a top-level task can open a pull request".to_string());
        }
        if task.status != TaskStatus::Done {
            return Err("only a done task can open a pull request".to_string());
        }
        if task.pr_pending && p.source.as_deref() == Some("overseer") {
            return Err(
                "orchd opens this task's pull request itself; do not call task_pr while prPending is set"
                    .to_string(),
            );
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
        if let Some(url) = &task.pr_url {
            let open = task.pr_branch.as_deref().unwrap_or(&task.branch);
            if open != branch {
                return Err(format!(
                    "a pull request is already open for this task on {open}: {url}"
                ));
            }
        }
        let title = p
            .title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .unwrap_or(&task.title)
            .to_string();
        let body = p
            .body
            .as_deref()
            .map(str::trim)
            .filter(|b| !b.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| pr_body(&task));
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

        let default = {
            let repo = repo.clone();
            tokio::task::spawn_blocking(move || git::default_branch(&repo))
                .await
                .map_err(|e| e.to_string())?
        };
        let base = pr_base(&task, &repo, default.clone()).await?;
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
        // The same carry as the automatic pull request: never push a stale base.
        let owner_plan = PrPlan {
            branch: branch.clone(),
            title: p.title.clone().unwrap_or_default(),
            body: p.body.clone().unwrap_or_default(),
        };
        run_cmd("git", &["fetch", "origin", &base], &repo, GIT_TIMEOUT).await?;
        let up_to_date = run_cmd(
            "git",
            &[
                "merge-base",
                "--is-ancestor",
                &format!("origin/{base}"),
                &local,
            ],
            &repo,
            GIT_TIMEOUT,
        )
        .await
        .is_ok();
        if !inline && !up_to_date {
            // Stored with the owner's choices; the flow carries, resolves,
            // pushes and opens, then the task shows the pull request.
            let queued = self.update_task(&p.id, move |t| {
                t.pr_pending = true;
                t.pr_plan = Some(owner_plan);
                t.decisions.push(
                    "PR: carrying onto the newest base; the pull request opens when done"
                        .to_string(),
                );
            })?;
            super::pr_draft::spawn_open_pr(self, &p.id);
            return serde_json::to_value(&queued).map_err(|e| e.to_string());
        }
        match super::pr_draft::carry_for_pr(&self.arc(), &task, Some(owner_plan)).await? {
            super::pr_draft::PrCarry::Ready => {}
            super::pr_draft::PrCarry::Contained => {
                return Err(format!(
                    "origin/{base} already contains this work; nothing to open a pull request for"
                ));
            }
            super::pr_draft::PrCarry::Resolving(files) => {
                return Err(format!(
                    "the work conflicts with origin/{base} in {}; one conflict-resolution attempt runs now and the pull request opens when it is done",
                    files.join(", ")
                ));
            }
            super::pr_draft::PrCarry::Failed(reason) => return Err(reason),
        }
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
                        "pr", "create", "--base", &base, "--head", &branch, "--title", &title,
                        "--body", &body,
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
        task.queue.pr_conflict_tried = false;
        task.pr_branch = Some(branch.clone());
        task.decisions.push(format!("PR: {verb} {url}"));
        task.updated_at = now_ms();
        self.store.save_task(&task).map_err(|e| e.to_string())?;
        self.broadcast_task(&task);
        if p.source.as_deref() == Some("overseer") {
            self.record_overseer(
                overseer::Entry::new("pr", &task.repo, Some(&task.id), &url).with_detail(&url),
            );
        }
        serde_json::to_value(&task).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod unreviewed_tests {
    use super::*;
    use crate::engine::test_support::task_with_status;

    #[test]
    fn an_unreviewed_task_opens_with_the_warning_and_unchecked_criteria() {
        let mut task = task_with_status(TaskStatus::Done);
        task.goal = "Add the toggle".into();
        task.criteria = vec!["It toggles".into()];
        assert!(pr_body(&task).contains("- [x] It toggles"));
        task.unreviewed = true;
        let body = pr_body(&task);
        assert!(
            body.starts_with("Unreviewed: no review route gave a verdict\n"),
            "{body}"
        );
        assert!(body.contains("- [ ] It toggles"), "{body}");
    }

    #[test]
    fn a_parent_pr_names_its_unreviewed_subtasks() {
        let mut child = task_with_status(TaskStatus::Done);
        child.title = "Part".into();
        let mut parent = task_with_status(TaskStatus::Done);
        parent.goal = "Whole".into();
        parent.unreviewed = true;
        parent.loop_state.unreviewed_parts.push(UnreviewedPart {
            id: child.id.clone(),
            title: child.title.clone(),
        });
        let body = pr_body(&parent);
        assert!(body.starts_with(UNREVIEWED_LINE), "{body}");
        assert!(body.contains("subtask \"Part\""), "{body}");
    }
}
