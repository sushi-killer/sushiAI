use crate::delivery::pr::command::run_cmd;
use crate::delivery::pr::command::GIT_TIMEOUT;
use crate::delivery::pr::error::PrError;
use crate::delivery::pr::error::PrRefusal;
use crate::task::state::Task;
use serde::Deserialize;
use std::path::Path;
use std::path::PathBuf;

/// Syntax rules for the remote branch name, before git sees it.
pub(crate) fn check_branch_syntax(name: &str) -> Result<(), PrRefusal> {
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
pub(crate) fn ls_remote_sha(out: &str) -> Option<String> {
    out.lines()
        .next()
        .and_then(|l| l.split_whitespace().next())
        .map(str::to_string)
}

/// Read-back of a push: origin must hold the sha that was pushed. A mismatch
/// is a failed write, so the push is retried and never recorded as done.
pub(crate) fn pushed_head_mismatch(branch: &str, pushed: &str, ls_remote: &str) -> Option<String> {
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
pub(crate) fn check_pr_view(
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
pub(crate) async fn pr_base(
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

/// Gates the branch name the drafter chose: a valid ref name that is not the
/// default or base branch, and not a branch that already exists on the push
/// remote or locally (the task's own excepted). `Err` is the reason the
/// task's own branch is used instead.
pub(crate) async fn choose_branch(task: &Task, drafted: &str) -> Result<String, String> {
    // A pull request already open keeps its head.
    if let Some(open) = task.pr_branch.as_deref().filter(|_| task.pr_url.is_some()) {
        return Ok(open.to_string());
    }
    let name = drafted.trim();
    if name.is_empty() {
        return Err("the draft named none".to_string());
    }
    let bad = |why: &str| format!("the drafted name {name} {why}");
    if crate::delivery::pr::branch::check_branch_syntax(name).is_err() {
        return Err(bad("is not a usable branch name"));
    }
    if name == task.branch {
        return Ok(task.branch.clone());
    }
    let repo = PathBuf::from(&task.repo);
    async fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
        crate::delivery::pr::command::run_cmd(
            "git",
            args,
            repo,
            crate::delivery::pr::command::GIT_TIMEOUT,
        )
        .await
    }
    if git(&repo, &["check-ref-format", "--branch", name])
        .await
        .is_err()
    {
        return Err(bad("is not a valid branch name"));
    }
    let default = {
        let repo = repo.clone();
        tokio::task::spawn_blocking(move || crate::git::branches::default_branch(&repo))
            .await
            .map_err(|e| e.to_string())?
    };
    if [default.as_deref(), task.base_ref.as_deref()].contains(&Some(name)) {
        return Err(bad("is the base branch"));
    }
    let full = format!("refs/heads/{name}");
    if git(&repo, &["show-ref", "--verify", "-q", &full])
        .await
        .is_ok()
    {
        return Err(bad("is a local branch of another task"));
    }
    match git(&repo, &["ls-remote", "--heads", "origin", &full]).await {
        Ok(out) if out.trim().is_empty() => Ok(name.to_string()),
        Ok(_) => Err(bad("exists on the remote")),
        Err(e) => Err(format!("the remote could not be checked ({e})")),
    }
}
