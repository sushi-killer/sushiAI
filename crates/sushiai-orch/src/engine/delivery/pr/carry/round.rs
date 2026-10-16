use super::PrCarry;
use crate::engine::delivery::pr::admission::PrAdmission;
use crate::engine::delivery::pr::{
    carried_work::carry_work,
    checkout::{prepare_checkout, CarryCheckout},
    conflict::resolve_conflict,
    observations::{publish, CarryPublication},
};
use crate::engine::state::App;
use crate::engine::verification::base::answers::owner_revoked_waiver;
use crate::task::checks::admission::CheckAdmission;
use crate::task::{PrPlan, Task, TaskStatus};
use std::path::Path;
use std::sync::Arc;

pub(super) enum RoundOutcome {
    Finished(PrCarry),
    Changed(Box<Task>),
}

pub(super) async fn carry_round(
    app: &Arc<App>,
    task: &Task,
    base: &str,
    plan: Option<PrPlan>,
) -> Result<RoundOutcome, String> {
    let repo = Path::new(&task.repo);
    let (upstream, local) = (
        format!("origin/{base}"),
        format!("refs/heads/{}", task.branch),
    );
    let contains = crate::delivery::pr::command::run_cmd(
        "git",
        &["merge-base", "--is-ancestor", &upstream, &local],
        repo,
        crate::delivery::pr::command::GIT_TIMEOUT,
    )
    .await
    .is_ok();
    let revoked = task
        .verify
        .iter()
        .chain(&task.final_verify)
        .any(|command| owner_revoked_waiver(task, command));
    if contains && !revoked {
        return Ok(RoundOutcome::Finished(PrCarry::Ready(Box::new(
            PrAdmission::capture(task),
        ))));
    }
    let old = crate::delivery::pr::command::run_cmd(
        "git",
        &["rev-parse", &local],
        repo,
        crate::delivery::pr::command::GIT_TIMEOUT,
    )
    .await?;
    let (temp, run_dir) = match prepare_checkout(app, task, &old).await? {
        CarryCheckout::Ready(temp, run_dir) => (temp, run_dir),
        CarryCheckout::Environment(error) => {
            return Ok(RoundOutcome::Finished(PrCarry::Environment(error)))
        }
    };
    let carried = carry_work(app, task, &temp, &run_dir, &upstream).await?;
    let publication = publish(app, task, &carried, &old)?;
    drop(temp);
    let fresh = match publication {
        CarryPublication::Current(fresh) => fresh,
        CarryPublication::Changed(fresh) => return Ok(RoundOutcome::Changed(fresh)),
        CarryPublication::Deferred => return Ok(RoundOutcome::Finished(PrCarry::Deferred)),
    };
    let result = if carried.contained {
        PrCarry::Contained
    } else if carried.conflicted {
        resolve_conflict(app, &task.id, base, plan).await?
    } else if let Some(failed) = carried.failed_check {
        crate::engine::delivery::pr::repair::resolve_checks(
            app,
            &fresh,
            base,
            &failed,
            crate::engine::delivery::pr::repair::Origin::Carry(plan),
        )
        .await?
    } else {
        carried
            .committed
            .map(|_| PrCarry::Ready(Box::new(PrAdmission::capture(&fresh))))?
    };
    if matches!(result, PrCarry::Deferred) {
        let fresh = app
            .store
            .load_task(&task.id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        if fresh.status == TaskStatus::Done
            && !fresh.archived
            && fresh.queue.park.is_none()
            && fresh.pending_amendment.is_none()
            && fresh.pr_push_owed()
            && fresh.landed_sha.is_none()
            && !CheckAdmission::capture(task).matches(&fresh)
        {
            return Ok(RoundOutcome::Changed(Box::new(fresh)));
        }
    }
    Ok(RoundOutcome::Finished(result))
}
