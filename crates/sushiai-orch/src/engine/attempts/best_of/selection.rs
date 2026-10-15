use crate::engine::attempts::best_of::candidate::EvaluatedCandidates;
use crate::engine::attempts::best_of::outcome::BestOfExtra;
use crate::engine::attempts::best_of::pick::run_pick;
use crate::engine::attempts::best_of::run::BestOfRun;
use crate::engine::state::App;
use crate::harness::cancel::CancelToken;
use crate::harness::native::error::RunError;
use std::sync::Arc;

pub(super) async fn select(
    app: &Arc<App>,
    run: &BestOfRun<'_>,
    evaluated: &EvaluatedCandidates,
    extra: &mut BestOfExtra,
    cancel: &CancelToken,
) -> Result<(bool, String), RunError> {
    let (a_route, b_route) = (&evaluated.a.run.route, &evaluated.b.run.route);
    let (id_a, id_b) = (a_route.id.as_str(), b_route.id.as_str());
    let pick_b;
    let decision;
    match (evaluated.a.checks.passed, evaluated.b.checks.passed) {
        (true, true) => match run_pick(app, run, evaluated, &mut extra.decisions, cancel).await {
            Ok((Some((pick, why)), cost)) => {
                extra.cost += cost;
                pick_b = pick;
                decision = format!(
                    "Best-of: both passed; picked {} ({}) - {why}",
                    if pick_b { "b" } else { "a" },
                    if pick_b { id_b } else { id_a }
                );
            }
            Ok((None, cost)) => {
                extra.cost += cost;
                pick_b = false;
                decision =
                    format!("Best-of: both passed; the pick run gave no answer, kept a ({id_a})");
            }
            Err(_) => {
                return Err(RunError::Cancelled);
            }
        },
        (true, false) => {
            pick_b = false;
            decision = format!("Best-of: only a ({id_a}) passed verify and the checks");
        }
        (false, true) => {
            pick_b = true;
            decision = format!("Best-of: only b ({id_b}) passed verify and the checks");
        }
        (false, false) => {
            pick_b = evaluated.b.checks.failing < evaluated.a.checks.failing;
            let fewer = |n: u32| {
                if n == u32::MAX {
                    "unfinished".to_string()
                } else {
                    format!("{n} failing")
                }
            };
            decision = format!(
                "Best-of: neither passed; continuing from {} ({} vs {})",
                if pick_b {
                    format!("b ({id_b})")
                } else {
                    format!("a ({id_a})")
                },
                fewer(if pick_b {
                    evaluated.b.checks.failing
                } else {
                    evaluated.a.checks.failing
                }),
                fewer(if pick_b {
                    evaluated.a.checks.failing
                } else {
                    evaluated.b.checks.failing
                }),
            );
        }
    }

    Ok((pick_b, decision))
}
