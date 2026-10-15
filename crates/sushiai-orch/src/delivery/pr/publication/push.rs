use super::{PreparedWrite, Publication};
use crate::delivery::pr::branch::{ls_remote_sha, pushed_head_mismatch};
use crate::delivery::pr::command::{run_cmd, start, CommandWrite, GIT_TIMEOUT, PUSH_TIMEOUT};
use crate::delivery::pr::error::{PrError, PrRefusal};
use std::path::PathBuf;

pub(crate) struct PreparedPush {
    repo: PathBuf,
    branch: String,
    head: String,
    lease: String,
}

pub(crate) struct PushWrite {
    command: CommandWrite,
    repo: PathBuf,
    branch: String,
    head: String,
}

impl Publication {
    pub(crate) async fn prepare_push(&self, task_branch: &str) -> Result<PreparedPush, PrError> {
        let local_sha = run_cmd(
            "git",
            &["rev-parse", "--verify", "-q", &self.local],
            &self.repo,
            GIT_TIMEOUT,
        )
        .await
        .map_err(|_| PrRefusal::BranchGone(task_branch.to_string()))?;
        let count = run_cmd(
            "git",
            &[
                "rev-list",
                "--count",
                &format!("origin/{}..{}", self.base, local_sha),
            ],
            &self.repo,
            GIT_TIMEOUT,
        )
        .await?;
        if count.trim() == "0" {
            return Err(PrRefusal::NoCommits {
                branch: task_branch.to_string(),
                base: self.base.clone(),
            }
            .into());
        }
        let remote_ref = format!("refs/heads/{}", self.content.branch);
        let remote = run_cmd(
            "git",
            &["ls-remote", "--heads", "origin", &remote_ref],
            &self.repo,
            GIT_TIMEOUT,
        )
        .await?;
        let remote_sha = ls_remote_sha(&remote);
        if let Some(sha) = &remote_sha {
            run_cmd(
                "git",
                &["merge-base", "--is-ancestor", sha, &local_sha],
                &self.repo,
                GIT_TIMEOUT,
            )
            .await
            .map_err(|_| PrRefusal::ForeignBranch(self.content.branch.clone()))?;
        }
        Ok(PreparedPush {
            repo: self.repo.clone(),
            branch: self.content.branch.clone(),
            head: local_sha,
            lease: format!(
                "--force-with-lease={remote_ref}:{}",
                remote_sha.as_deref().unwrap_or("")
            ),
        })
    }
}

impl PreparedWrite for PreparedPush {
    type Running = PushWrite;

    fn dispatch(self) -> Result<PushWrite, String> {
        let command = start(
            "git",
            &[
                "push",
                "-u",
                &self.lease,
                "origin",
                &format!("{}:refs/heads/{}", self.head, self.branch),
            ],
            &self.repo,
            PUSH_TIMEOUT,
        )?;
        Ok(PushWrite {
            command,
            repo: self.repo,
            branch: self.branch,
            head: self.head,
        })
    }
}

impl PushWrite {
    pub(crate) async fn finish(self) -> Result<String, PrError> {
        self.command.finish().await?;
        let remote_ref = format!("refs/heads/{}", self.branch);
        let after = run_cmd(
            "git",
            &["ls-remote", "--heads", "origin", &remote_ref],
            &self.repo,
            GIT_TIMEOUT,
        )
        .await?;
        if let Some(why) = pushed_head_mismatch(&self.branch, &self.head, &after) {
            return Err(PrError::External(why));
        }
        Ok(self.head)
    }
}
