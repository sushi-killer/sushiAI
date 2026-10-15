use crate::clock::now_ms;
use crate::engine::attempts::pricing::ProviderRun;
use crate::engine::attempts::run::RunIdentity;
use crate::engine::state::App;
use crate::harness;

/// What a run is for, so its cost record names the stage. `repo` is set for
/// a run that is no task's (audit, evolution proposal, chat): its `task_id`
/// argument is then the audit or proposal id, kept as the record's `runId`.
pub(crate) struct CostTag {
    pub stage: &'static str,
    pub route_id: String,
    pub repo: Option<String>,
    pub has_attempt: bool,
}

/// Appends the run's cost record. A resumed Claude session's own total
/// covers its earlier runs too, so its record is priced from this run's
/// streamed messages when it has any.
pub(in crate::engine) fn record_run(
    app: &App,
    identity: &RunIdentity<'_>,
    provider: ProviderRun<'_>,
    outcome: &harness::RunOutcome,
    started_at: i64,
) {
    let RunIdentity {
        id: task_id,
        attempt_n,
        tag,
        ..
    } = identity;
    let harness = provider.harness;
    let (mut cost, mut estimated) = (outcome.cost_usd.unwrap_or(0.0), outcome.cost_estimated);
    if provider.claude_resume().is_some() {
        if let Some(own) = outcome.streamed_cost(&app.settings.read().unwrap().prices) {
            cost = own;
            estimated = true;
        }
    }
    let is_task = tag.repo.is_none();
    let repo = tag.repo.clone().unwrap_or_else(|| {
        app.store
            .load_task(task_id)
            .ok()
            .flatten()
            .map(|t| t.repo.clone())
            .unwrap_or_default()
    });
    let model = outcome
        .fingerprint
        .as_ref()
        .and_then(|f| f.models.first().cloned())
        .or_else(|| provider.model.map(str::to_string))
        .unwrap_or_else(|| "unknown".into());
    let ended = now_ms();
    crate::store::costs::append(
        &app.data_dir,
        &crate::store::costs::CostRecord {
            ts: ended,
            repo,
            task_id: is_task.then(|| task_id.to_string()),
            run_id: (!is_task && !task_id.is_empty()).then(|| task_id.to_string()),
            stage: tag.stage.to_string(),
            attempt: (is_task && tag.has_attempt).then_some(*attempt_n),
            route_id: tag.route_id.clone(),
            harness,
            model,
            cost_usd: cost,
            estimated,
            input_tokens: outcome.usage_input,
            cached_tokens: outcome.usage_cached,
            output_tokens: outcome.usage_output,
            ms: (ended - started_at).max(0) as u64,
            backfilled: false,
        },
    );
}

impl CostTag {
    pub fn task(stage: &'static str, route_id: &str) -> Self {
        CostTag {
            stage,
            route_id: route_id.to_string(),
            repo: None,
            has_attempt: true,
        }
    }

    pub fn task_without_attempt(stage: &'static str, route_id: &str) -> Self {
        Self {
            has_attempt: false,
            ..Self::task(stage, route_id)
        }
    }

    pub fn repo(stage: &'static str, route_id: &str, repo: &str) -> Self {
        CostTag {
            stage,
            route_id: route_id.to_string(),
            repo: Some(repo.to_string()),
            has_attempt: false,
        }
    }
}
