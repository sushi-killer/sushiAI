use crate::engine::state::App;
use crate::harness;
use crate::harness::guard::Guard;
use crate::harness::guard::Trip;
use crate::policy::pricing::Price;
use crate::policy::routing::Harness;
use crate::task::attempt::Attempt;
use crate::task::attempt::Stage;

/// The provider and session whose usage a native run reports.
#[derive(Clone, Copy)]
pub(crate) struct ProviderRun<'a> {
    pub(crate) harness: Harness,
    pub(crate) model: Option<&'a str>,
    pub(crate) resume: Option<&'a str>,
}

impl<'a> ProviderRun<'a> {
    pub(super) fn claude_resume(self) -> Option<&'a str> {
        self.resume.filter(|_| self.harness == Harness::Claude)
    }
}

impl<'a> From<&harness::RunRequest<'a>> for ProviderRun<'a> {
    fn from(request: &harness::RunRequest<'a>) -> Self {
        Self {
            harness: request.harness,
            model: request.model,
            resume: request.resume,
        }
    }
}

/// Prices a run the harness reported no cost for: Codex from its token
/// usage, Claude from its streamed messages (marked estimated).
pub(crate) fn finalize_cost(
    app: &App,
    harness: Harness,
    model: Option<&str>,
    outcome: &mut harness::RunOutcome,
) {
    if outcome.cost_usd.is_none() && harness == Harness::Codex {
        let price = model.and_then(|m| app.settings.read().unwrap().prices.get(m).copied());
        outcome.cost_usd = price.map(|p| {
            p.codex_cost(
                outcome.usage_input,
                outcome.usage_cached,
                outcome.usage_output,
            )
        });
    }
    if harness == Harness::Claude {
        outcome.estimate_cost(&app.settings.read().unwrap().prices);
    }
}

pub(crate) fn price_resumed_attempt(
    app: &App,
    task_id: &str,
    attempt_n: u32,
    provider: ProviderRun<'_>,
    outcome: &mut harness::RunOutcome,
) {
    let Some(session) = provider.claude_resume() else {
        return;
    };
    let task = app.store.load_task(task_id).ok().flatten();
    price_claude_resume(
        task.as_ref()
            .map_or(&[][..], |task| task.attempts.as_slice()),
        attempt_n,
        session,
        &app.settings.read().unwrap().prices,
        outcome,
    );
}

pub(crate) fn price_claude_resume(
    attempts: &[Attempt],
    attempt_n: u32,
    session: &str,
    prices: &std::collections::BTreeMap<String, Price>,
    outcome: &mut harness::RunOutcome,
) {
    if let Some(own) = outcome.streamed_cost(prices) {
        outcome.cost_usd = Some(own);
        outcome.cost_estimated = true;
    } else if let Some(total) = outcome.cost_usd {
        let earlier: f64 = attempts
            .iter()
            .filter(|a| {
                a.stage == Stage::Implement
                    && a.n < attempt_n
                    && a.session_id.as_deref() == Some(session)
            })
            .filter_map(|a| a.cost_usd)
            .sum();
        outcome.cost_usd = Some((total - earlier).max(0.0));
    }
}

/// What this run has cost so far, priced from its streamed message usage.
/// The CLI's own `total_cost_usd` is not used: on a resumed session it also
/// covers earlier attempts, which the cap must not count.
fn streamed_spend(
    app: &App,
    harness: Harness,
    model: Option<&str>,
    outcome: &harness::RunOutcome,
) -> Option<f64> {
    let settings = app.settings.read().unwrap();
    match harness {
        Harness::Claude => outcome.streamed_cost(&settings.prices),
        Harness::Codex => model.and_then(|m| settings.prices.get(m)).map(|p| {
            p.codex_cost(
                outcome.usage_input,
                outcome.usage_cached,
                outcome.usage_output,
            )
        }),
    }
}

pub(in crate::engine) fn guard_line(
    app: &App,
    guard: &mut Guard,
    run: (Harness, Option<&str>),
    line: &str,
    outcome: &harness::RunOutcome,
) -> Option<Trip> {
    if let Some(trip) = guard.observe_line(line) {
        return Some(trip);
    }
    if !guard.has_cap() {
        return None;
    }
    guard.check_spend(
        outcome.over_budget,
        streamed_spend(app, run.0, run.1, outcome),
    )
}
