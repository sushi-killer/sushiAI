use crate::task::{Task, TaskStatus};

pub(crate) struct LandingRedirect {
    pub(crate) branch: String,
    pub(crate) done_tasks: Vec<Task>,
    pub(crate) decisions: Vec<String>,
}

pub(crate) fn resolve_landing_branch(
    branch: &str,
    tasks: &[Task],
) -> Result<LandingRedirect, String> {
    let mut branch = branch.to_string();
    let mut seen = std::collections::HashSet::new();
    let mut done_tasks = Vec::new();
    let mut decisions = Vec::new();
    loop {
        if !seen.insert(branch.clone()) {
            return Err(format!("Land: refused, a cycle was found while following done task branches from {branch}; the work stays on its own branch"));
        }
        let Some(done) = tasks
            .iter()
            .find(|t| t.status == TaskStatus::Done && t.branch == branch)
        else {
            break;
        };
        let Some(base) = done.base_ref.clone() else {
            return Err(format!("Land: refused, done task {} has no base branch to redirect to; the work stays on its own branch", done.title));
        };
        decisions.push(format!(
            "Land: redirected from done task {} ({})'s branch {branch} to {base}",
            done.title, done.id
        ));
        done_tasks.push(done.clone());
        branch = base;
    }
    Ok(LandingRedirect {
        branch,
        done_tasks,
        decisions,
    })
}
