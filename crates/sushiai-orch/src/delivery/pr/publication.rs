use super::branch::{check_pr_view, ls_remote_sha, pushed_head_mismatch};
use super::command::{gh_bin, run_cmd, GH_TIMEOUT, GIT_TIMEOUT, PUSH_TIMEOUT};
use super::error::{PrError, PrRefusal};
use std::path::PathBuf;
pub(crate) struct PrContent {
    pub(crate) branch: String,
    pub(crate) title: String,
    pub(crate) body: String,
}
pub(crate) struct Publication {
    pub(crate) content: PrContent,
    pub(crate) repo: PathBuf,
    pub(crate) base: String,
    pub(crate) local: String,
}
pub(crate) struct PublishedPr {
    pub(crate) url: String,
    pub(crate) verb: &'static str,
    pub(crate) head: String,
}
impl Publication {
    pub(crate) async fn push(&self, task_branch: &str) -> Result<String, PrError> {
        let Publication {
            content: PrContent { branch, .. },
            repo,
            base,
            local,
        } = self;
        // Read after the carry, which moves the task's branch.
        let local_sha = run_cmd(
            "git",
            &["rev-parse", "--verify", "-q", local],
            repo,
            GIT_TIMEOUT,
        )
        .await
        .map_err(|_| PrRefusal::BranchGone(task_branch.to_string()))?;
        let count = run_cmd(
            "git",
            &["rev-list", "--count", &format!("origin/{base}..{local}")],
            repo,
            GIT_TIMEOUT,
        )
        .await?;
        if count.trim() == "0" {
            return Err(PrRefusal::NoCommits {
                branch: task_branch.to_string(),
                base: base.clone(),
            }
            .into());
        }

        // Never overwrite a remote branch that is not an ancestor of ours.
        let remote_ref = format!("refs/heads/{branch}");
        let remote = run_cmd(
            "git",
            &["ls-remote", "--heads", "origin", &remote_ref],
            repo,
            GIT_TIMEOUT,
        )
        .await?;
        let remote_sha = ls_remote_sha(&remote);
        if let Some(sha) = &remote_sha {
            run_cmd(
                "git",
                &["merge-base", "--is-ancestor", sha, local],
                repo,
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
            repo,
            PUSH_TIMEOUT,
        )
        .await?;
        // Read back: a push origin did not take is a failed write, not a success.
        let after = run_cmd(
            "git",
            &["ls-remote", "--heads", "origin", &remote_ref],
            repo,
            GIT_TIMEOUT,
        )
        .await?;
        if let Some(why) = pushed_head_mismatch(branch, &local_sha, &after) {
            return Err(PrError::External(why));
        }

        Ok(local_sha)
    }
    pub(crate) async fn open(&self, local_sha: &str) -> Result<PublishedPr, PrError> {
        let Publication {
            content:
                PrContent {
                    branch,
                    title,
                    body,
                },
            repo,
            base,
            ..
        } = self;
        let gh = gh_bin();
        let open = run_cmd(
            &gh,
            &[
                "pr", "list", "--head", branch, "--state", "open", "--json", "url", "--jq",
                ".[0].url",
            ],
            repo,
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
                        "pr", "create", "--base", base, "--head", branch, "--title", title,
                        "--body", body,
                    ],
                    repo,
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
            repo,
            GH_TIMEOUT,
        )
        .await?;
        check_pr_view(&view, &url, local_sha, created.then_some(title.as_str()))?;

        Ok(PublishedPr {
            url,
            verb,
            head: local_sha.to_string(),
        })
    }
}
