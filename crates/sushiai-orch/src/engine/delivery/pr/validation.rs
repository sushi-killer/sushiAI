use crate::delivery::pr::command::{run_cmd, GIT_TIMEOUT};
use crate::task::Task;
use std::collections::BTreeMap;
use std::path::Path;

/// Match actual recorded trees to reachable commits without assigning HEAD to older checks.
pub(super) async fn recorded_commits(task: &Task) -> BTreeMap<String, String> {
    let trees: std::collections::BTreeSet<&str> = task
        .attempts
        .iter()
        .flat_map(|attempt| &attempt.verify)
        .chain(&task.delivery_checks)
        .filter_map(|result| result.tree.as_deref())
        .collect();
    if trees.is_empty() {
        return BTreeMap::new();
    }
    let history = run_cmd(
        "git",
        &["log", "--all", "--format=%H%x09%T"],
        Path::new(&task.repo),
        GIT_TIMEOUT,
    )
    .await
    .unwrap_or_default();
    let mut commits = BTreeMap::new();
    for line in history.lines() {
        if let Some((commit, tree)) = line.split_once('\t') {
            if trees.contains(tree) {
                commits
                    .entry(tree.to_string())
                    .or_insert_with(|| commit.to_string());
            }
        }
    }
    commits
}

/// Rebuilt at publication as a carried checkout may have added newer observations.
pub(super) async fn body(
    app: &crate::engine::state::App,
    task: &Task,
    drafted: String,
) -> Result<String, String> {
    let secrets = crate::engine::attempts::secrets::run_secrets(app, &task.id)?.redaction;
    let home = std::env::var("HOME").ok();
    let commits = recorded_commits(task).await;
    Ok(crate::delivery::pr::body::final_body(
        task,
        drafted,
        &secrets,
        home.as_deref(),
        &commits,
    ))
}
