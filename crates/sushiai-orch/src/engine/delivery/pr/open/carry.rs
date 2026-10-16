use crate::delivery::pr::error::{PrError, PrRefusal};
use crate::engine::delivery::pr::admission::PrAdmission;
use crate::engine::delivery::pr::carry::{carry_for_pr, PrCarry};
use crate::engine::state::App;
use crate::task::{PrPlan, Task};
use std::sync::Arc;

pub(super) async fn carry_admission(
    app: &Arc<App>,
    task: &Task,
    plan: PrPlan,
    base: &str,
) -> Result<Box<PrAdmission>, PrError> {
    match carry_for_pr(app, task, Some(plan)).await? {
        PrCarry::Ready(admission) => Ok(admission),
        PrCarry::Contained => Err(PrRefusal::Contained(base.to_string()).into()),
        PrCarry::Resolving(files) => Err(PrRefusal::Resolving {
            base: base.to_string(),
            files,
        }
        .into()),
        PrCarry::Failed(reason) => Err(PrRefusal::CarryFailed(reason).into()),
        PrCarry::ChecksPending => Err(PrRefusal::CarryFailed(
            "the pull request is waiting for its check repair or the owner's answer".into(),
        )
        .into()),
        PrCarry::Deferred => Err(PrRefusal::TaskChanged.into()),
        PrCarry::Environment(reason) => Err(PrRefusal::CarryFailed(format!(
            "the setup of the carry worktree failed (environment): {reason}"
        ))
        .into()),
    }
}
