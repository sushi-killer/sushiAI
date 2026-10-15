use crate::engine::brief_check::run_judge;
use crate::engine::brief_check::JudgeRun;
use crate::engine::state::App;
use crate::engine::verification::healing::contract::amend_criterion;
use crate::engine::verification::healing::contract::judge_route;
use crate::engine::verification::healing::contract::overturnable;
use crate::harness::cancel::CancelToken;
use crate::harness::native::error::RunError;
use crate::task::review::ReviewResult;
use crate::task::state::Task;
use crate::verify::healing::reply::parse_ruling;
use crate::verify::healing::reply::repeated_unmet;
use crate::verify::healing::reply::ruling_prompt;
use crate::verify::healing::reply::Ruling;
use std::sync::Arc;

/// When the review marks a criterion unmet that an earlier review marked
/// unmet as well, a judge run decides between a code gap (the retry goes on
/// as before) and an infeasible criterion (amended, recorded, the owner told
/// without being blocked). `Ok(true)`: the criterion was amended, so the
/// same attempt is reviewed again against it.
pub(in crate::engine) async fn heal_repeated_unmet(
    app: &Arc<App>,
    task: &mut Task,
    idx: usize,
    review: &ReviewResult,
    cancel: &CancelToken,
) -> Result<bool, RunError> {
    let attempt_n = task.attempts[idx].n;
    let Some(rep) = repeated_unmet(task, idx, attempt_n, review) else {
        return Ok(false);
    };
    let Some(route) = judge_route(app, &task.variant()) else {
        return Ok(false);
    };
    let old = task.criteria[rep.criterion].clone();
    let prompt = ruling_prompt(task, &rep);
    let reply = run_judge(
        app,
        task,
        &route,
        Some(attempt_n),
        "brief_check",
        "criterion-judge",
        &prompt,
        cancel,
    )
    .await?;
    let JudgeRun {
        reply,
        cost,
        timeout_note,
    } = reply;
    task.record_decisions(timeout_note);
    task.charge(cost);
    let ruling = match reply {
        Ok(text) => parse_ruling(&text),
        Err(_) => None,
    };
    // Whatever it said, one ruling per criterion.
    task.mark_criterion_judged(old.clone());
    let (a, b) = (rep.earlier.0, rep.current.0);
    match ruling {
        Some(Ruling::Infeasible { criterion, reason }) => {
            let settings = app.settings.read().unwrap().clone();
            if let Err(why) = amend_criterion(&settings, task, rep.criterion, &criterion) {
                task.record_decision(format!(
                    "Orchestrator: criterion \"{old}\" stays as written: {why}"
                ));
                return Ok(false);
            }
            task.record_decision(format!(
                "Orchestrator: criterion \"{old}\" was unmet in attempts {a} and {b} and is infeasible as written ({reason}); amended to \"{criterion}\""
            ));
            overturnable(
                task,
                format!("Can the criterion \"{old}\" be met as written?"),
                format!("No: amended to \"{criterion}\""),
                reason,
            );
            app.broadcast_log(
                &task.id,
                attempt_n,
                format!("Amended an infeasible criterion (the owner can overturn it): {criterion}"),
            );
            Ok(true)
        }
        Some(Ruling::CodeGap) => {
            task.record_decision(format!(
                "Orchestrator: criterion \"{old}\" was unmet in attempts {a} and {b}; judged a code gap, retrying"
            ));
            Ok(false)
        }
        None => Ok(false),
    }
}
