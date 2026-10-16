use crate::engine::attempts::best_of::candidate::EvaluatedCandidates;
use crate::harness;
use crate::harness::native::error::RunError;
use crate::policy::routing::Route;
use crate::task::attempt::Candidate;

/// What a best-of attempt adds to the task beside the winner's own run.
#[derive(Default)]
pub(in crate::engine) struct BestOfExtra {
    /// The loser's cost and the pick run's, on top of the winner's.
    pub cost: f64,
    pub candidates: Vec<Candidate>,
    pub decisions: Vec<String>,
    /// The route that produced the winning changes, when it is not the one
    /// the attempt started on.
    pub winner: Option<Route>,
    /// Protected files the winning candidate changed and orchd restored.
    pub discarded: Vec<String>,
}

pub(super) fn finish(
    mut extra: BestOfExtra,
    evaluated: EvaluatedCandidates,
    pick_b: bool,
) -> (Result<harness::RunOutcome, RunError>, BestOfExtra) {
    extra.discarded = if pick_b {
        evaluated.b.restored.clone()
    } else {
        evaluated.a.restored.clone()
    };
    extra.candidates = [
        (
            &evaluated.a.run.route,
            evaluated.a.run.cost,
            &evaluated.a.checks,
            !pick_b,
        ),
        (
            &evaluated.b.run.route,
            evaluated.b.run.cost,
            &evaluated.b.checks,
            pick_b,
        ),
    ]
    .into_iter()
    .map(|(route, cost, eval, picked)| Candidate {
        route: route.id.clone(),
        cost,
        verify: eval.verify_ok,
        checks: eval.tally,
        picked,
    })
    .collect();
    if pick_b {
        extra.cost += evaluated.a.run.cost;
        extra.winner = Some(evaluated.b.run.route);
        (evaluated.b.run.outcome, extra)
    } else {
        extra.cost += evaluated.b.run.cost;
        (evaluated.a.run.outcome, extra)
    }
}

#[cfg(test)]
mod tests {
    use super::{finish, BestOfExtra};
    use crate::engine::attempts::best_of::candidate::{
        CandidateEvidence, CandidateRun, EvaluatedCandidates,
    };
    use crate::engine::attempts::best_of::evaluate::Eval;
    use crate::harness::RunOutcome;
    use crate::policy::settings::Settings;
    use crate::task::attempt::CheckTally;

    fn candidate(id: &str, cost: f64) -> CandidateEvidence {
        let mut route = Settings::default().routes[0].clone();
        route.id = id.into();
        CandidateEvidence {
            run: CandidateRun {
                route,
                worktree: id.into(),
                run_dir: id.into(),
                outcome: Ok(RunOutcome {
                    session_id: Some(id.into()),
                    final_text: Some(format!("{id} report")),
                    cost_usd: Some(cost),
                    ..Default::default()
                }),
                cost,
            },
            checks: Eval {
                passed: true,
                verify_ok: true,
                failing: 0,
                verify: vec![],
                tally: CheckTally {
                    passed: 2,
                    failed: 0,
                },
            },
            restored: vec![format!("{id}-protected")],
        }
    }

    #[test]
    fn selecting_b_keeps_its_session_report_and_cost_and_charges_a_once() {
        let candidates = EvaluatedCandidates {
            a: candidate("a", 0.4),
            b: candidate("b", 0.8),
            decisions: vec![],
        };
        let (outcome, extra) = finish(
            BestOfExtra {
                cost: 0.07,
                ..Default::default()
            },
            candidates,
            true,
        );
        let outcome = outcome.ok().expect("selected candidate completed");
        assert_eq!(outcome.session_id.as_deref(), Some("b"));
        assert_eq!(outcome.final_text.as_deref(), Some("b report"));
        assert_eq!(outcome.cost_usd, Some(0.8));
        assert_eq!(
            extra.winner.as_ref().map(|route| route.id.as_str()),
            Some("b")
        );
        assert!((extra.cost - 0.47).abs() < 1e-9);
        assert_eq!(extra.discarded, ["b-protected"]);
        assert_eq!(
            extra
                .candidates
                .iter()
                .map(|candidate| (candidate.route.as_str(), candidate.cost, candidate.picked))
                .collect::<Vec<_>>(),
            [("a", 0.4, false), ("b", 0.8, true)]
        );
        assert!(extra
            .candidates
            .iter()
            .all(|candidate| candidate.verify && candidate.checks.passed == 2));
    }

    #[test]
    fn retaining_a_charges_b_once_without_changing_the_primary_route() {
        let candidates = EvaluatedCandidates {
            a: candidate("a", 0.4),
            b: candidate("b", 0.8),
            decisions: vec![],
        };
        let (outcome, extra) = finish(
            BestOfExtra {
                cost: 0.07,
                ..Default::default()
            },
            candidates,
            false,
        );
        let outcome = outcome.ok().expect("selected candidate completed");
        assert_eq!(outcome.session_id.as_deref(), Some("a"));
        assert_eq!(outcome.final_text.as_deref(), Some("a report"));
        assert_eq!(outcome.cost_usd, Some(0.4));
        assert!(extra.winner.is_none());
        assert!((extra.cost - 0.87).abs() < 1e-9);
        assert_eq!(extra.discarded, ["a-protected"]);
        assert_eq!(
            extra
                .candidates
                .iter()
                .map(|candidate| candidate.picked)
                .collect::<Vec<_>>(),
            [true, false]
        );
    }
}
