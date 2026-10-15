use crate::delivery::pr::failure::classify_external;
use crate::delivery::pr::failure::PrFailure;
use crate::engine::delivery::pr::pending::record;
use crate::engine::state::App;
use crate::task::state::Task;
use std::path::PathBuf;
use std::sync::Arc;

pub(super) async fn remote_available(app: &Arc<App>, task: &Task) -> bool {
    let id = task.id.as_str();
    let repo = PathBuf::from(&task.repo);
    if crate::delivery::pr::command::run_cmd(
        "git",
        &["remote", "get-url", "origin"],
        &repo,
        crate::delivery::pr::command::GIT_TIMEOUT,
    )
    .await
    .is_err()
    {
        // Nothing to push to: no draft, no spend, no notice.
        let line = "PR: not opened: the repo has no origin remote".to_string();
        record(app, id, vec![line], 0.0, true);
        return false;
    }
    // A remote `gh` does not know (GitLab, a bare path): no draft, no push.
    if let Err(e) = crate::delivery::pr::command::run_cmd(
        &crate::delivery::pr::command::gh_bin(),
        &["repo", "view", "--json", "url"],
        &repo,
        crate::delivery::pr::command::GH_TIMEOUT,
    )
    .await
    {
        if matches!(classify_external(&e), PrFailure::NoGithubHost) {
            record(app, id, vec![format!("PR: not opened: {e}")], 0.0, true);
            return false;
        }
    }
    true
}
