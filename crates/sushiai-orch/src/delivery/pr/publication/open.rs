use super::{PreparedWrite, Publication, PublishedPr};
use crate::delivery::pr::branch::check_pr_view;
use crate::delivery::pr::command::{gh_bin, run_cmd, start, CommandWrite, GH_TIMEOUT};
use crate::delivery::pr::error::PrError;
use std::path::PathBuf;

pub(crate) enum PreparedPr {
    Existing {
        gh: String,
        repo: PathBuf,
        url: String,
        head: String,
    },
    Create {
        gh: String,
        repo: PathBuf,
        base: String,
        branch: String,
        title: String,
        body: String,
        head: String,
    },
}

pub(crate) enum PrWrite {
    Existing {
        gh: String,
        repo: PathBuf,
        url: String,
        head: String,
    },
    Creating {
        command: Box<CommandWrite>,
        gh: String,
        repo: PathBuf,
        head: String,
        title: String,
    },
}

impl Publication {
    pub(crate) async fn prepare_open(&self, head: &str) -> Result<PreparedPr, PrError> {
        let gh = gh_bin();
        let open = run_cmd(
            &gh,
            &[
                "pr",
                "list",
                "--head",
                &self.content.branch,
                "--state",
                "open",
                "--json",
                "url",
                "--jq",
                ".[0].url",
            ],
            &self.repo,
            GH_TIMEOUT,
        )
        .await?;
        if let Some(url) = open
            .lines()
            .last()
            .map(str::trim)
            .filter(|url| url.starts_with("http"))
        {
            return Ok(PreparedPr::Existing {
                gh,
                repo: self.repo.clone(),
                url: url.to_string(),
                head: head.to_string(),
            });
        }
        Ok(PreparedPr::Create {
            gh,
            repo: self.repo.clone(),
            base: self.base.clone(),
            branch: self.content.branch.clone(),
            title: self.content.title.clone(),
            body: self.content.body.clone(),
            head: head.to_string(),
        })
    }
}

impl PreparedWrite for PreparedPr {
    type Running = PrWrite;

    fn dispatch(self) -> Result<PrWrite, String> {
        Ok(match self {
            Self::Existing {
                gh,
                repo,
                url,
                head,
            } => PrWrite::Existing {
                gh,
                repo,
                url,
                head,
            },
            Self::Create {
                gh,
                repo,
                base,
                branch,
                title,
                body,
                head,
            } => {
                let command = start(
                    &gh,
                    &[
                        "pr", "create", "--base", &base, "--head", &branch, "--title", &title,
                        "--body", &body,
                    ],
                    &repo,
                    GH_TIMEOUT,
                )?;
                PrWrite::Creating {
                    command: Box::new(command),
                    gh,
                    repo,
                    head,
                    title,
                }
            }
        })
    }
}

impl PrWrite {
    pub(crate) async fn finish(self) -> Result<PublishedPr, PrError> {
        let (gh, repo, url, head, created_title) = match self {
            Self::Existing {
                gh,
                repo,
                url,
                head,
            } => (gh, repo, url, head, None),
            Self::Creating {
                command,
                gh,
                repo,
                head,
                title,
            } => {
                let out = (*command).finish().await?;
                let url = out
                    .lines()
                    .rev()
                    .find(|line| line.trim().starts_with("http"))
                    .map(|line| line.trim().to_string())
                    .ok_or_else(|| format!("gh did not print a pull request URL: {out}"))?;
                (gh, repo, url, head, Some(title))
            }
        };
        let view = run_cmd(
            &gh,
            &["pr", "view", &url, "--json", "url,headRefOid,title"],
            &repo,
            GH_TIMEOUT,
        )
        .await?;
        check_pr_view(&view, &url, &head, created_title.as_deref())?;
        Ok(PublishedPr {
            url,
            verb: if created_title.is_some() {
                "opened"
            } else {
                "updated"
            },
            head,
        })
    }
}
