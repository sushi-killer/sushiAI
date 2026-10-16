use crate::delivery::pr::body::pr_body;
use crate::delivery::pr::branch::check_branch_syntax;
use crate::delivery::pr::branch::pr_base;
use crate::delivery::pr::command::run_cmd;
use crate::delivery::pr::command::GIT_TIMEOUT;
use crate::delivery::pr::error::PrError;
use crate::delivery::pr::error::PrRefusal;
use crate::delivery::pr::request::PrParams;
use crate::engine::state::App;
use crate::task::state::Task;
use crate::task::state::TaskStatus;
use std::path::PathBuf;

use crate::delivery::pr::publication::{PrContent, Publication};
fn validate_request(app: &App, p: &PrParams, task: &Task) -> Result<PrContent, PrError> {
    // The owner's own calls (the app, or the owner's chat) always open one;
    // an agent opens one only where automatic pull requests are on.
    let by_owner = matches!(p.source.as_deref(), None | Some("") | Some("chat"));
    let automatic = app.settings.read().unwrap().may_open_pr_on_done(&task.repo);
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
        .unwrap_or_else(|| pr_body(task));
    check_branch_syntax(&branch)?;
    Ok(PrContent {
        branch,
        title,
        body,
    })
}
pub(super) async fn prepare_pr(app: &App, p: &PrParams) -> Result<(Task, Publication), PrError> {
    let task = app
        .store
        .load_task(&p.id)
        .map_err(|e| e.to_string())?
        .ok_or(PrRefusal::NotFound)?;
    let content = validate_request(app, p, &task)?;
    let branch = &content.branch;
    let repo = PathBuf::from(&task.repo);
    run_cmd(
        "git",
        &["check-ref-format", "--branch", branch],
        &repo,
        GIT_TIMEOUT,
    )
    .await
    .map_err(|_| PrRefusal::BadBranch(branch.clone()))?;

    let default = {
        let repo = repo.clone();
        tokio::task::spawn_blocking(move || crate::git::branches::default_branch(&repo))
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
    Ok((
        task,
        Publication {
            content,
            repo,
            base,
            local,
        },
    ))
}
