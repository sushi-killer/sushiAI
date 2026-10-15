use crate::clock::now_ms;
use crate::delivery::pr::error::PrError;
use crate::delivery::pr::error::PrRefusal;
use crate::delivery::pr::request::PrParams;
use crate::engine::state::App;
use crate::overseer;

use crate::delivery::pr::publication::{Publication, PublishedPr};
pub(super) fn record_publication(
    app: &App,
    p: &PrParams,
    publication: &Publication,
    receipt: PublishedPr,
) -> Result<serde_json::Value, PrError> {
    // Reload: the task may have changed while the commands ran.
    let mut task = app
        .store
        .load_task(&p.id)
        .map_err(|e| e.to_string())?
        .ok_or(PrRefusal::NotFound)?;
    let PublishedPr {
        url,
        verb,
        head: local_sha,
    } = receipt;
    let branch = &publication.content.branch;
    let short = local_sha.get(..7).unwrap_or(&local_sha);
    task.record_pr_opened(url.clone(), branch.clone());
    task.record_decision(format!("PR: {verb} {url} (head {short})"));
    task.touch(now_ms());
    app.store.save_task(&task).map_err(|e| e.to_string())?;
    app.broadcast_task(&task);
    if p.source.as_deref() == Some("overseer") {
        app.record_overseer(
            overseer::Entry::new("pr", &task.repo, Some(&task.id), &url).with_detail(&url),
        );
    }
    Ok(serde_json::to_value(&task).map_err(|e| e.to_string())?)
}
