use crate::engine::persistence::contract::ContractInput;
use crate::engine::questions::parking::WaitEnd;
use crate::engine::state::App;
use crate::engine::verification::context::CheckContext;
use crate::engine::verification::runner::run_verify_commands;
use crate::harness::cancel::CancelToken;
use crate::task::Task;
use crate::task::{FindingRecord, ReviewResult};
use crate::verify::command_gate::{command_refusal, no_check_line};
use std::path::Path;
use std::sync::Arc;

/// Review runs one implement attempt may have: the first and one more,
/// whatever asks for it (no verdict, a judge drop, a healed criterion, the
/// owner's `retry`).
pub(super) const MAX_REVIEW_RUNS: u32 = 2;

/// The decision line for a review loop that ran out of runs.
pub(super) fn review_stopped_note(attempt_n: u32) -> String {
    format!("Orchestrator: attempt {attempt_n} review stopped after {MAX_REVIEW_RUNS} runs")
}

/// Reviews of a finding after the first that make it a repeat to handle.
const HANDLE_AT_REPEATS: u32 = 2;

/// What the loop does about the findings that repeated for the second time:
/// a failing scenario becomes a criterion and a check. Every other one stays
/// open and blocking: the implementer may decline it with evidence, and the
/// next review rules on that (`apply_declines`). They no longer count as
/// repeated, so the next step is another attempt, not an owner question.
pub(super) async fn handle_repeated_findings(
    app: &Arc<App>,
    task: &mut Task,
    attempt_n: u32,
    r: &mut ReviewResult,
    cancel: &CancelToken,
) -> Result<(), WaitEnd> {
    let worktree = std::path::PathBuf::from(&task.worktree);
    let own = r.severities.len().min(r.findings.len());
    for i in 0..own {
        let finding = r.findings[i].clone();
        let Some(id) = r.details.get(i).and_then(|d| d.id.clone()) else {
            continue;
        };
        let Some(pos) = task.findings.iter().position(|f| f.id == id) else {
            continue;
        };
        if task.findings[pos].handled {
            r.repeated.retain(|t| *t != finding);
            continue;
        }
        if task.findings[pos].repeats < HANDLE_AT_REPEATS {
            continue;
        }
        let input = ContractInput::capture(task);
        let registered = task.findings[pos].clone();
        let run = r
            .details
            .get(i)
            .and_then(|d| d.scenario.as_ref())
            .map(|s| s.run.clone())
            .or_else(|| task.findings[pos].scenarios.last().cloned())
            .filter(|run| !run.trim().is_empty());
        let scenario = match run {
            None => ScenarioResult::Missing,
            Some(run) => match command_refusal(&worktree, &run) {
                Some(why) => ScenarioResult::Refused { run, why },
                None => {
                    let dir = app
                        .store
                        .run_dir(&task.id, attempt_n)
                        .join(format!("finding-scenario-{id}"));
                    let _ = std::fs::create_dir_all(&dir);
                    let fails = scenario_fails(app, task, &dir, &run, cancel)
                        .await
                        .ok_or(WaitEnd::Stopped)?;
                    ScenarioResult::Ran { run, fails }
                }
            },
        };
        let proposal = FindingScenario {
            finding: registered,
            scenario,
        };
        let publication = input
            .publish(app, task, cancel, |fresh| {
                proposal.apply(fresh);
                Ok(())
            })
            .map_err(|error| {
                eprintln!("orch: finding publication {}: {error}", task.id);
                WaitEnd::Stopped
            })?;
        if let Err(end) = publication.into_result() {
            let line = format!(
                "Orchestrator: deferred the repeated finding {} result after the task changed",
                proposal.finding.id
            );
            app.append_decision(&task.id, line.clone());
            task.record_decision(line);
            return Err(end);
        }
        proposal.apply(task);
        r.repeated.retain(|t| *t != finding);
    }
    Ok(())
}

enum ScenarioResult {
    Missing,
    Refused { run: String, why: String },
    Ran { run: String, fails: bool },
}

struct FindingScenario {
    finding: FindingRecord,
    scenario: ScenarioResult,
}

impl FindingScenario {
    fn apply(&self, task: &mut Task) {
        let id = &self.finding.id;
        task.record_handled_finding(&self.finding);
        let decision = match &self.scenario {
            ScenarioResult::Missing => format!(
                "Orchestrator: review finding {id} repeated twice with no scenario; it stays open"
            ),
            ScenarioResult::Refused { run, why } => {
                crate::task::checks::edits::append_regression(task, id, None);
                no_check_line(task.criteria.len() - 1, run, why)
            }
            ScenarioResult::Ran { run, fails: true } => {
                crate::task::checks::edits::append_regression(task, id, Some(run));
                format!("Orchestrator: review finding {id} repeated twice and its scenario fails in the worktree; added a criterion and a check: {run}")
            }
            ScenarioResult::Ran { fails: false, .. } => format!(
                "Orchestrator: review finding {id} repeated twice; its scenario passes in the worktree, so it stays open"
            ),
        };
        task.record_decision(decision);
    }
}

async fn scenario_fails(
    app: &App,
    task: &Task,
    dir: &Path,
    command: &str,
    cancel: &CancelToken,
) -> Option<bool> {
    let worktree = Path::new(&task.worktree);
    let sandbox = app.task_sandbox(&task.repo, worktree, dir);
    let runner = crate::engine::verification::runner::CheckRunner::from_context(
        CheckContext::new(app, Some(&task.id)),
        worktree,
        sandbox.as_ref(),
        &task.base_sha,
        cancel,
    );
    let results = run_verify_commands(runner, &[command.to_string()]).await;
    if cancel.is_cancelled() {
        None
    } else {
        Some(results.first().is_some_and(|result| result.code != Some(0)))
    }
}

#[cfg(test)]
mod tests;
