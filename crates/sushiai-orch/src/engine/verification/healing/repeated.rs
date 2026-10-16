use crate::engine::brief_check::judge::JudgeRun;
use crate::engine::persistence::contract::{ContractInput, Publication};
use crate::engine::state::App;
use crate::engine::verification::healing::contract::amend_criterion;
use crate::engine::verification::healing::contract::judge_route;
use crate::engine::verification::healing::contract::overturnable;
use crate::harness::cancel::CancelToken;
use crate::harness::native::error::RunError;
use crate::policy::settings::Settings;
use crate::task::review::ReviewResult;
use crate::task::state::Task;
use crate::verify::healing::reply::parse_ruling;
use crate::verify::healing::reply::repeated_unmet;
use crate::verify::healing::reply::ruling_prompt;
use crate::verify::healing::reply::{Repeated, Ruling};
use std::sync::Arc;

/// When the review marks a criterion unmet that an earlier review marked
/// unmet as well, a judge run decides between a code gap (the retry goes on
/// as before) and an infeasible criterion (amended, recorded, the owner told
/// without being blocked). `Applied(true)` means the criterion was amended, so the
/// same attempt is reviewed again against it.
pub(in crate::engine) async fn heal_repeated_unmet(
    app: &Arc<App>,
    task: &mut Task,
    idx: usize,
    review: &ReviewResult,
    cancel: &CancelToken,
) -> Result<Publication<bool>, RunError> {
    let attempt_n = task.attempts[idx].n;
    let Some(rep) = repeated_unmet(task, idx, attempt_n, review) else {
        return Ok(Publication::Applied(false));
    };
    let Some(route) = judge_route(app, &task.variant()) else {
        return Ok(Publication::Applied(false));
    };
    let old = task.criteria[rep.criterion].clone();
    let input = ContractInput::capture(task);
    let prompt = ruling_prompt(task, &rep);
    let reply = crate::engine::brief_check::judge::Judge::new(
        &route,
        crate::engine::brief_check::judge::JudgePurpose::CriterionJudge(attempt_n),
    )
    .run(app, task, &prompt, cancel)
    .await?;
    let JudgeRun {
        reply,
        cost,
        timeout_note,
    } = reply;
    app.update_task(&task.id, |fresh| {
        fresh.charge(cost);
        fresh.record_decisions(timeout_note.clone());
    })
    .map_err(RunError::Io)?;
    task.record_decisions(timeout_note);
    task.charge(cost);
    let ruling = reply.ok().and_then(|text| parse_ruling(&text));
    let proposal = CriterionProposal {
        old,
        repeated: rep,
        ruling,
    };
    proposal.publish(app, task, &input, cancel)
}

struct CriterionProposal {
    old: String,
    repeated: Repeated,
    ruling: Option<Ruling>,
}

impl CriterionProposal {
    fn publish(
        &self,
        app: &App,
        task: &mut Task,
        input: &ContractInput,
        cancel: &CancelToken,
    ) -> Result<Publication<bool>, RunError> {
        let settings = app.settings.read().unwrap().clone();
        let publication = input
            .publish(app, task, cancel, |fresh| Ok(self.apply(&settings, fresh)))
            .map_err(RunError::Io)?;
        match publication {
            Publication::Applied(changed) => {
                self.apply(&settings, task);
                if changed {
                    app.broadcast_log(
                        &task.id,
                        self.repeated.current.0,
                        format!(
                            "Amended an infeasible criterion (the owner can overturn it): {}",
                            task.criteria[self.repeated.criterion]
                        ),
                    );
                }
                Ok(Publication::Applied(changed))
            }
            Publication::Deferred(end) => {
                let line =
                    "Orchestrator: deferred the criterion judge result after the task changed"
                        .to_string();
                app.append_decision(&task.id, line.clone());
                task.record_decision(line);
                Ok(Publication::Deferred(end))
            }
        }
    }

    fn apply(&self, settings: &Settings, task: &mut Task) -> bool {
        let old = &self.old;
        task.mark_criterion_judged(old.clone());
        let (a, b) = (self.repeated.earlier.0, self.repeated.current.0);
        match &self.ruling {
            Some(Ruling::Infeasible { criterion, reason }) => {
                if let Err(why) =
                    amend_criterion(settings, task, self.repeated.criterion, criterion)
                {
                    task.record_decision(format!(
                        "Orchestrator: criterion \"{old}\" stays as written: {why}"
                    ));
                    return false;
                }
                task.record_decision(format!(
                    "Orchestrator: criterion \"{old}\" was unmet in attempts {a} and {b} and is infeasible as written ({reason}); amended to \"{criterion}\""
                ));
                overturnable(
                    task,
                    format!("Can the criterion \"{old}\" be met as written?"),
                    format!("No: amended to \"{criterion}\""),
                    reason.clone(),
                );
                true
            }
            Some(Ruling::CodeGap) => {
                task.record_decision(format!(
                    "Orchestrator: criterion \"{old}\" was unmet in attempts {a} and {b}; judged a code gap, retrying"
                ));
                false
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests;
