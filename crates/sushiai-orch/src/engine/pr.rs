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

const WAIVED_HEADING: &str = "## Waived base failures";

/// The section that tells the reviewer which failing base checks the task
/// was let through on (empty when none): put back by code after any drafted
/// body, so no turn can drop it.
pub(super) fn waived_section(task: &Task) -> String {
    let waived: Vec<&Assumption> = task
        .assumptions
        .iter()
        .filter(|a| a.by == WAIVER_BY && a.answer == WAIVED && !a.overturned)
        .collect();
    if waived.is_empty() {
        return String::new();
    }
    let mut section = format!("\n{WAIVED_HEADING}\n\n");
    for a in waived {
        let command = a.commands.join(", ");
        section.push_str(&format!("- {command}: {}\n", a.evidence.trim()));
    }
    section
}

/// `body` without any waived-failures section of its own, so the one `code`
/// writes is the only one.
pub(super) fn without_waived_section(body: &str) -> String {
    let mut kept = Vec::new();
    let mut skipping = false;
    for line in body.lines() {
        if line.trim_end() == WAIVED_HEADING {
            skipping = true;
        } else if skipping && line.starts_with("## ") {
            skipping = false;
        }
        if !skipping {
            kept.push(line);
        }
    }
    let mut text = kept.join("\n");
    if body.ends_with('\n') && !text.is_empty() {
        text.push('\n');
    }
    text
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
    body.push_str(&waived_section(task));
    body
}

/// Task ids with a `task.pr` running: a second call is refused.
static IN_FLIGHT: StdMutex<Option<HashSet<String>>> = StdMutex::new(None);

pub(super) struct InFlight(String);

impl InFlight {
    pub(super) fn take(id: &str) -> Result<InFlight, PrRefusal> {
        let mut guard = IN_FLIGHT.lock().unwrap();
        if !guard
            .get_or_insert_with(HashSet::new)
            .insert(id.to_string())
        {
            return Err(PrRefusal::InFlight);
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

/// A refusal orchd gives on its own. Whether a retry can help is decided by
/// the variant ([`PrRefusal::permanent`]), never by its text; `Display` is the
/// message the owner reads.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum PrRefusal {
    /// The request names no usable task or arguments; the text says what.
    Invalid(String),
    NotFound,
    NotTopLevel,
    NotDone,
    /// The overseer asked while orchd owns the pull request.
    OverseerOwns,
    Archived,
    Landed,
    /// Another `task.pr` for this task is still running.
    InFlight,
    /// An agent named a branch; it may only open the task's own.
    AgentOwnBranch,
    /// The remote branch name is not one git accepts.
    BadBranch(String),
    /// The task's own branch is gone.
    BranchGone(String),
    /// The repo has no default branch to open the pull request against.
    NoDefaultBranch,
    /// The remote branch name is a base branch.
    BaseBranch(String),
    /// An agent asked while automatic pull requests are off for the repo.
    AutomaticOff,
    /// `origin/<base>` already holds the work.
    Contained(String),
    /// The work conflicts with `origin/<base>`; a resolution attempt runs first.
    Resolving {
        base: String,
        files: Vec<String>,
    },
    /// The carry ended the task `failed`; the reason.
    CarryFailed(String),
    NoCommits {
        branch: String,
        base: String,
    },
    /// `origin` has a branch of this name that this task did not push.
    ForeignBranch(String),
    /// A pull request for this task is already open on `on`, at `url`.
    AlreadyOpen {
        on: String,
        url: String,
    },
}

impl PrRefusal {
    /// Whether a retry can only be refused again. A busy `task.pr` may pass later.
    pub(super) fn permanent(&self) -> bool {
        !matches!(self, Self::InFlight)
    }
}

impl std::fmt::Display for PrRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(why) => f.write_str(why),
            Self::NotFound => f.write_str("task not found"),
            Self::NotTopLevel => f.write_str("only a top-level task can open a pull request"),
            Self::NotDone => f.write_str("only a done task can open a pull request"),
            Self::OverseerOwns => f.write_str(
                "orchd opens this task's pull request itself; do not call task_pr while prPending is set",
            ),
            Self::Archived => f.write_str("task is archived; unarchive it first"),
            Self::Landed => f.write_str(
                "task is already landed; there is nothing to open a pull request for",
            ),
            Self::InFlight => f.write_str("a pull request is already being opened"),
            Self::AgentOwnBranch => {
                f.write_str("an agent opens the pull request for the task's own branch only")
            }
            Self::BadBranch(name) => write!(f, "{name:?} is not a valid branch name"),
            Self::BranchGone(branch) => write!(f, "the task branch {branch} no longer exists"),
            Self::NoDefaultBranch => f.write_str(
                "the repo has no default branch to open a pull request against",
            ),
            Self::AutomaticOff => f.write_str(
                "automatic pull requests are off for this repo: the owner opens the pull request",
            ),
            Self::BaseBranch(branch) => write!(
                f,
                "{branch} is a base branch; pick another remote branch name for the pull request"
            ),
            Self::Contained(base) => write!(
                f,
                "origin/{base} already contains this work; nothing to open a pull request for"
            ),
            Self::Resolving { base, files } => write!(
                f,
                "the work conflicts with origin/{base} in {}; one conflict-resolution attempt runs now and the pull request opens when it is done",
                files.join(", ")
            ),
            Self::CarryFailed(reason) => f.write_str(reason),
            Self::NoCommits { branch, base } => write!(
                f,
                "branch {branch} has no commits beyond origin/{base}; nothing to open a pull request for"
            ),
            Self::ForeignBranch(branch) => write!(
                f,
                "origin already has a branch {branch} that is not part of this task; pick another name"
            ),
            Self::AlreadyOpen { on, url } => {
                write!(f, "a pull request is already open for this task on {on}: {url}")
            }
        }
    }
}

/// What a `task.pr` attempt failed with: orchd's own refusal, or a git, gh or
/// store failure whose text the retry reads once (`pr_draft::classify_external`).
#[derive(Debug)]
pub(super) enum PrError {
    Refused(PrRefusal),
    External(String),
}

impl From<PrRefusal> for PrError {
    fn from(refusal: PrRefusal) -> Self {
        Self::Refused(refusal)
    }
}

impl From<String> for PrError {
    fn from(error: String) -> Self {
        Self::External(error)
    }
}

impl std::fmt::Display for PrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(refusal) => write!(f, "{refusal}"),
            Self::External(error) => f.write_str(error),
        }
    }
}

/// The arguments of `task.pr`. The owner may name a branch; an agent names
/// none, and the pull request is always the task's own branch.
#[derive(Deserialize)]
pub(super) struct PrParams {
    pub id: String,
    pub title: Option<String>,
    pub branch: Option<String>,
    /// Replaces the generated body when not empty.
    pub body: Option<String>,
    /// Set by the MCP bridge; the app sends none.
    #[serde(default)]
    pub source: Option<String>,
}

/// Syntax rules for the remote branch name, before git sees it.
pub(super) fn check_branch_syntax(name: &str) -> Result<(), PrRefusal> {
    let bad = name.is_empty()
        || name.starts_with('-')
        || name.starts_with("refs/")
        || name.contains(':')
        || name.contains('+')
        || name.contains("@{")
        || name == "HEAD";
    if bad {
        return Err(PrRefusal::BadBranch(name.to_string()));
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

/// Read-back of a push: origin must hold the sha that was pushed. A mismatch
/// is a failed write, so the push is retried and never recorded as done.
pub(super) fn pushed_head_mismatch(branch: &str, pushed: &str, ls_remote: &str) -> Option<String> {
    let remote = ls_remote_sha(ls_remote);
    (remote.as_deref() != Some(pushed)).then(|| match remote {
        Some(sha) => format!("the push of {branch} did not land: origin has {sha}, not {pushed}"),
        None => format!("the push of {branch} did not land: origin has no {branch}"),
    })
}

#[derive(Deserialize)]
struct PrView {
    url: String,
    #[serde(rename = "headRefOid")]
    head_ref_oid: String,
    title: String,
}

/// Read-back of a pull request from `gh pr view`: it must be the URL orchd
/// wrote, with the pushed sha as its head, and for a pull request orchd just
/// created, the title it sent.
pub(super) fn check_pr_view(
    json: &str,
    url: &str,
    pushed: &str,
    created_title: Option<&str>,
) -> Result<(), String> {
    let view: PrView = serde_json::from_str(json)
        .map_err(|e| format!("gh pr view did not return a pull request: {e}"))?;
    if view.url != url {
        return Err(format!("gh pr view shows {} for {url}", view.url));
    }
    if view.head_ref_oid != pushed {
        return Err(format!(
            "the pull request {url} has head {}, not the pushed {pushed}",
            view.head_ref_oid
        ));
    }
    if let Some(title) = created_title.filter(|t| *t != view.title) {
        return Err(format!(
            "the pull request {url} has title {:?}, not {title:?}",
            view.title
        ));
    }
    Ok(())
}

/// The base a pull request targets: the branch the task started from when
/// origin has it, else the repo's default branch.
pub(super) async fn pr_base(
    task: &Task,
    repo: &Path,
    default: Option<String>,
) -> Result<String, PrError> {
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
        .ok_or(PrRefusal::NoDefaultBranch)
        .map_err(PrError::from)
}

impl App {
    /// The RPC entry: a branch that needs the newest base is carried off the
    /// request path, so a slow carry never stalls the connection.
    pub(super) async fn handle_task_pr(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let p: PrParams = serde_json::from_value(params).map_err(|e| e.to_string())?;
        self.open_pr(p, false).await.map_err(|e| e.to_string())
    }

    /// `inline`: only the in-process pull request flow sets it; it carries
    /// onto the newest base before the push, as part of its own background run.
    pub(super) async fn open_pr(
        &self,
        p: PrParams,
        inline: bool,
    ) -> Result<serde_json::Value, PrError> {
        if p.source.as_deref().is_some_and(|s| !s.is_empty()) && p.branch.is_some() {
            return Err(PrRefusal::AgentOwnBranch.into());
        }
        validate_task_id(&self.store, &p.id).map_err(PrRefusal::Invalid)?;
        let _guard = InFlight::take(&p.id)?;
        let task = self
            .store
            .load_task(&p.id)
            .map_err(|e| e.to_string())?
            .ok_or(PrRefusal::NotFound)?;
        // The owner's own calls (the app, or the owner's chat) always open one;
        // an agent opens one only where automatic pull requests are on.
        let by_owner = matches!(p.source.as_deref(), None | Some("") | Some("chat"));
        let automatic = self
            .settings
            .read()
            .unwrap()
            .may_open_pr_on_done(&task.repo);
        if !by_owner && !automatic {
            return Err(PrRefusal::AutomaticOff.into());
        }
        if task.parent.is_some() {
            return Err(PrRefusal::NotTopLevel.into());
        }
        if task.status != TaskStatus::Done {
            return Err(PrRefusal::NotDone.into());
        }
        if task.pr_pending && p.source.as_deref() == Some("overseer") {
            return Err(PrRefusal::OverseerOwns.into());
        }
        if task.archived {
            return Err(PrRefusal::Archived.into());
        }
        if task.landed_sha.is_some() {
            return Err(PrRefusal::Landed.into());
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
                return Err(PrRefusal::AlreadyOpen {
                    on: open.to_string(),
                    url: url.clone(),
                }
                .into());
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
        .map_err(|_| PrRefusal::BadBranch(branch.clone()))?;

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
            return Err(PrRefusal::BaseBranch(branch.clone()).into());
        }
        let local = format!("refs/heads/{}", task.branch);
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
            return Ok(serde_json::to_value(&queued).map_err(|e| e.to_string())?);
        }
        match super::pr_draft::carry_for_pr(&self.arc(), &task, Some(owner_plan)).await? {
            super::pr_draft::PrCarry::Ready => {}
            super::pr_draft::PrCarry::Contained => {
                return Err(PrRefusal::Contained(base).into());
            }
            super::pr_draft::PrCarry::Resolving(files) => {
                return Err(PrRefusal::Resolving { base, files }.into());
            }
            super::pr_draft::PrCarry::Failed(reason) => {
                return Err(PrRefusal::CarryFailed(reason).into());
            }
            super::pr_draft::PrCarry::Environment(reason) => {
                return Err(PrRefusal::CarryFailed(format!(
                    "the setup of the carry worktree failed (environment): {reason}"
                ))
                .into());
            }
        }
        // Read after the carry, which moves the task's branch.
        let local_sha = run_cmd(
            "git",
            &["rev-parse", "--verify", "-q", &local],
            &repo,
            GIT_TIMEOUT,
        )
        .await
        .map_err(|_| PrRefusal::BranchGone(task.branch.clone()))?;
        let count = run_cmd(
            "git",
            &["rev-list", "--count", &format!("origin/{base}..{local}")],
            &repo,
            GIT_TIMEOUT,
        )
        .await?;
        if count.trim() == "0" {
            return Err(PrRefusal::NoCommits {
                branch: task.branch.clone(),
                base: base.clone(),
            }
            .into());
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
            .map_err(|_| PrRefusal::ForeignBranch(branch.clone()))?;
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
        // Read back: a push origin did not take is a failed write, not a success.
        let after = run_cmd(
            "git",
            &["ls-remote", "--heads", "origin", &remote_ref],
            &repo,
            GIT_TIMEOUT,
        )
        .await?;
        if let Some(why) = pushed_head_mismatch(&branch, &local_sha, &after) {
            return Err(PrError::External(why));
        }

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
        let (url, verb, created) = match existing {
            Some(url) => (url.to_string(), "updated", false),
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
                (url, "opened", true)
            }
        };
        // Read back what GitHub holds: the URL, the pushed head, the title sent.
        let view = run_cmd(
            &gh,
            &["pr", "view", &url, "--json", "url,headRefOid,title"],
            &repo,
            GH_TIMEOUT,
        )
        .await?;
        check_pr_view(&view, &url, &local_sha, created.then_some(title.as_str()))?;

        // Reload: the task may have changed while the commands ran.
        let mut task = self
            .store
            .load_task(&p.id)
            .map_err(|e| e.to_string())?
            .ok_or(PrRefusal::NotFound)?;
        let short = local_sha.get(..7).unwrap_or(&local_sha);
        task.pr_url = Some(url.clone());
        task.queue.pr_conflict_tried = false;
        task.pr_branch = Some(branch.clone());
        task.decisions
            .push(format!("PR: {verb} {url} (head {short})"));
        task.updated_at = now_ms();
        self.store.save_task(&task).map_err(|e| e.to_string())?;
        self.broadcast_task(&task);
        if p.source.as_deref() == Some("overseer") {
            self.record_overseer(
                overseer::Entry::new("pr", &task.repo, Some(&task.id), &url).with_detail(&url),
            );
        }
        Ok(serde_json::to_value(&task).map_err(|e| e.to_string())?)
    }
}

#[cfg(test)]
mod refusal_tests {
    use super::*;

    #[test]
    fn an_orchd_refusal_is_permanent_even_when_its_text_holds_a_transient_marker() {
        let refusal = PrRefusal::CarryFailed("could not reach origin; try again later".into());
        assert!(refusal.permanent());
        assert!(PrRefusal::BadBranch("timed out".into()).permanent());
        assert!(PrRefusal::Contained("rate limit".into()).permanent());
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

    #[test]
    fn only_a_busy_pull_request_may_pass_on_its_own() {
        assert!(!PrRefusal::InFlight.permanent());
        assert!(PrRefusal::NotDone.permanent());
        assert!(PrRefusal::OverseerOwns.permanent());
    }

    #[test]
    fn a_refusal_keeps_the_owner_message() {
        let open = PrRefusal::AlreadyOpen {
            on: "feature/a".into(),
            url: "https://github.example/acme/app/pull/7".into(),
        };
        assert_eq!(
            open.to_string(),
            "a pull request is already open for this task on feature/a: https://github.example/acme/app/pull/7"
        );
        assert_eq!(
            PrError::from(PrRefusal::NotDone).to_string(),
            "only a done task can open a pull request"
        );
    }

    #[test]
    fn a_push_whose_remote_sha_differs_is_a_mismatch() {
        let pushed = "a".repeat(40);
        let same = format!("{pushed}\trefs/heads/feature/x\n");
        assert_eq!(pushed_head_mismatch("feature/x", &pushed, &same), None);
        let other = format!("{}\trefs/heads/feature/x\n", "b".repeat(40));
        let why = pushed_head_mismatch("feature/x", &pushed, &other).unwrap();
        assert!(why.contains("did not land"), "{why}");
        let gone = pushed_head_mismatch("feature/x", &pushed, "").unwrap();
        assert!(gone.contains("origin has no feature/x"), "{gone}");
    }

    #[test]
    fn a_pull_request_must_read_back_at_the_pushed_head_and_the_sent_title() {
        let url = "https://github.example/acme/app/pull/7";
        let view = |head: &str, title: &str| {
            format!(r#"{{"url":"{url}","headRefOid":"{head}","title":"{title}"}}"#)
        };
        assert!(check_pr_view(&view("abc", "t"), url, "abc", Some("t")).is_ok());
        assert!(check_pr_view(&view("abc", "old"), url, "abc", None).is_ok());
        let head = check_pr_view(&view("zzz", "t"), url, "abc", None).unwrap_err();
        assert!(head.contains("not the pushed abc"), "{head}");
        let title = check_pr_view(&view("abc", "old"), url, "abc", Some("t")).unwrap_err();
        assert!(title.contains("title"), "{title}");
        assert!(check_pr_view("", url, "abc", None).is_err());
    }

    #[test]
    fn a_waived_base_failure_is_listed_until_the_owner_overturns_it() {
        let mut task = task_with_status(TaskStatus::Done);
        task.assumptions.push(Assumption {
            question: "q".into(),
            answer: WAIVED.into(),
            evidence: "alpha fails on the base".into(),
            by: WAIVER_BY.into(),
            kind: Some(QuestionKind::PreexistingFailure),
            attempt: Some(1),
            commands: vec!["check".into()],
            overturned: false,
            owner_answer: None,
        });
        let body = pr_body(&task);
        assert!(body.contains("## Waived base failures"), "{body}");
        assert!(body.contains("- check: alpha fails on the base"), "{body}");
        task.assumptions[0].overturned = true;
        assert!(!pr_body(&task).contains("Waived"));
    }

    #[test]
    fn a_drafted_body_gets_the_waived_section_by_code_and_only_once() {
        let mut task = task_with_status(TaskStatus::Done);
        task.assumptions.push(Assumption {
            question: "q".into(),
            answer: WAIVED.into(),
            evidence: "alpha fails on the base".into(),
            by: WAIVER_BY.into(),
            kind: Some(QuestionKind::PreexistingFailure),
            attempt: Some(1),
            commands: vec!["check".into()],
            overturned: false,
            owner_answer: None,
        });
        let section = waived_section(&task);
        assert!(section.contains("- check: alpha fails on the base"));
        let own = "Intro\n\n## Waived base failures\n\n- invented\n\n## Notes\n\nkept\n";
        let stripped = without_waived_section(own);
        assert!(!stripped.contains("invented") && !stripped.contains("Waived"));
        assert!(stripped.contains("Intro") && stripped.contains("## Notes\n\nkept"));
        assert_eq!(without_waived_section("plain\n"), "plain\n");
    }
}
