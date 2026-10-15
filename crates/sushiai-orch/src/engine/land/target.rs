use crate::engine::graph::landing::Landing;
use crate::engine::state::App;
use crate::task::graph::landing::LandingRedirect;
use crate::task::Task;
use std::path::PathBuf;
use std::sync::Arc;

pub(super) async fn resolve(
    app: &Arc<App>,
    task: &mut Task,
    land_on_default: bool,
) -> Result<LandingRedirect, Landing> {
    let Some(original_branch) = task.base_ref.clone() else {
        task.record_decision(
            "Land: refused, the task was not started from a branch; the work stays on its own branch"
                .to_string(),
        );
        return Err(Landing::NoParent);
    };
    let all = app.repo_tasks(&task.repo);
    let redirect = match crate::task::graph::landing::resolve_landing_branch(&original_branch, &all)
    {
        Ok(redirect) => redirect,
        Err(reason) => {
            task.record_decision(reason);
            return Err(Landing::NoParent);
        }
    };
    let branch = redirect.branch.clone();
    task.record_decisions(redirect.decisions.clone());
    let repo = PathBuf::from(&task.repo);
    if !land_on_default {
        let r = repo.clone();
        let default = tokio::task::spawn_blocking(move || crate::git::branches::default_branch(&r))
            .await
            .unwrap_or(None);
        if default.as_deref() == Some(branch.as_str()) {
            task.record_decision(format!(
                "Land: refused, {} The work stays on {}",
                crate::policy::delivery::land_on_default_refusal(&branch, &task.repo),
                task.branch
            ));
            return Err(Landing::DefaultBranchRefused);
        }
    }

    if !redirect.done_tasks.is_empty() {
        if redirect
            .done_tasks
            .iter()
            .any(|done| done.landed_sha.is_none())
        {
            let (r, start, target) = (repo.clone(), task.base_sha.clone(), branch.clone());
            match tokio::task::spawn_blocking(move || {
                crate::git::history::merge_base_of(&r, &start, &target)
            })
            .await
            .unwrap_or_else(|e| Err(crate::git::GitError(e.to_string())))
            {
                Ok(base) => crate::task::transitions::move_base(task, base),
                Err(e) => {
                    task.record_decision(format!(
                        "Land: refused, could not find the merge base for redirected branch {branch}: {e}; the work stays on its own branch"
                    ));
                    return Err(Landing::NoParent);
                }
            }
        }
        task.record_base_ref(Some(branch.clone()));
    }

    Ok(redirect)
}
