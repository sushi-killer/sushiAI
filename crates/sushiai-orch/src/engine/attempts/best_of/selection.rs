use crate::engine::attempts::best_of::evidence::Evaluated;
use crate::engine::attempts::best_of::outcome::BestOfExtra;
use crate::engine::attempts::best_of::pick::run_pick;
use crate::engine::attempts::best_of::sibling::Sibling;
use crate::engine::state::App;
use crate::harness::cancel::CancelToken;
use crate::harness::native::error::RunError;
use crate::policy::routing::Route;
use crate::policy::settings::Settings;
use crate::task::Task;
use std::path::Path;
use std::sync::Arc;

#[allow(clippy::too_many_arguments)]
pub(super) async fn select(
    app: &Arc<App>,
    task: &Task,
    task_id: &str,
    attempt_n: u32,
    settings: &Settings,
    a_route: &Route,
    b_route: &Route,
    worktree: &Path,
    sibling: &Sibling,
    evaluated: &Evaluated,
    extra: &mut BestOfExtra,
    deny_read: &[String],
    cancel: &CancelToken,
) -> Result<(bool, String), RunError> {
    let (id_a, id_b) = (a_route.id.as_str(), b_route.id.as_str());
    let pick_b;
    let decision;
    match (evaluated.a.passed, evaluated.b.passed) {
        (true, true) => {
            match run_pick(
                app,
                task,
                task_id,
                attempt_n,
                settings,
                a_route,
                b_route,
                worktree,
                &sibling.worktree,
                &task.own_diff(),
                &mut extra.decisions,
                &evaluated.a,
                &evaluated.b,
                deny_read,
                cancel,
            )
            .await
            {
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
                    decision = format!(
                        "Best-of: both passed; the pick run gave no answer, kept a ({id_a})"
                    );
                }
                Err(_) => {
                    return Err(RunError::Cancelled);
                }
            }
        }
        (true, false) => {
            pick_b = false;
            decision = format!("Best-of: only a ({id_a}) passed verify and the checks");
        }
        (false, true) => {
            pick_b = true;
            decision = format!("Best-of: only b ({id_b}) passed verify and the checks");
        }
        (false, false) => {
            pick_b = evaluated.b.failing < evaluated.a.failing;
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
                    evaluated.b.failing
                } else {
                    evaluated.a.failing
                }),
                fewer(if pick_b {
                    evaluated.a.failing
                } else {
                    evaluated.b.failing
                }),
            );
        }
    }

    Ok((pick_b, decision))
}
