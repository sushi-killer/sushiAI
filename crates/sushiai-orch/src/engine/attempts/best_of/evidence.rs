use crate::engine::attempts::best_of::candidate::{
    CandidateEvidence, CompletedCandidates, EvaluatedCandidates,
};
use crate::engine::attempts::best_of::evaluate::{disqualify_restored, evaluate};
use crate::engine::protected::restore::{grown_line, guard_protected};
use crate::engine::state::App;
use crate::engine::verification::scoping::scoped_checks_for;
use crate::harness::cancel::CancelToken;
use crate::policy::settings::Settings;
use crate::task::state::Task;

pub(super) async fn evaluate_pair(
    app: &App,
    task: &Task,
    candidates: CompletedCandidates,
    settings: &Settings,
    cancel: &CancelToken,
) -> EvaluatedCandidates {
    let (guarded_a, guarded_b) = tokio::join!(
        guard_protected(task, &candidates.a.worktree, &[]),
        guard_protected(task, &candidates.b.worktree, &[])
    );
    let mut decisions = Vec::new();
    for (label, guarded) in [("a", &guarded_a), ("b", &guarded_b)] {
        if let Some(line) = grown_line(&guarded.grown) {
            decisions.push(format!("Best-of: candidate {label}: {line}"));
        }
    }
    let (restored_a, restored_b) = (guarded_a.restored, guarded_b.restored);
    let scoped = scoped_checks_for(settings, task);
    let (mut eval_a, mut eval_b) = tokio::join!(
        evaluate(app, task, &candidates.a, &scoped, cancel),
        evaluate(app, task, &candidates.b, &scoped, cancel),
    );
    disqualify_restored(&mut eval_a, &restored_a, "a", &mut decisions);
    disqualify_restored(&mut eval_b, &restored_b, "b", &mut decisions);
    decisions.extend(
        eval_a
            .verify
            .iter()
            .chain(&eval_b.verify)
            .flat_map(|result| result.decisions.iter().cloned()),
    );
    EvaluatedCandidates {
        a: CandidateEvidence {
            run: candidates.a,
            checks: eval_a,
            restored: restored_a,
        },
        b: CandidateEvidence {
            run: candidates.b,
            checks: eval_b,
            restored: restored_b,
        },
        decisions,
    }
}
