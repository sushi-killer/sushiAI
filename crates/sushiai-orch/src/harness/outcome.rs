use super::failure::ProviderFailure;
use crate::policy::pricing::{price_for, Price};
use crate::policy::routing::Harness;
use crate::task::attempt::Fingerprint;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default)]
pub struct RunOutcome {
    pub session_id: Option<String>,
    pub cost_usd: Option<f64>,
    pub usage_input: u64,
    pub usage_output: u64,
    pub usage_cached: u64,
    pub final_text: Option<String>,
    /// A harness-reported failure (Claude `is_error` result, Codex
    /// `turn.failed`/`error`), used as the failure detail when the run
    /// produced nothing.
    pub error: Option<String>,
    /// The failure class the harness reported in its structured output (Claude's
    /// error category or HTTP status). Read through `provider_failure`.
    pub structured_failure: Option<ProviderFailure>,
    /// The run printed nothing for its stall timeout and was killed.
    pub stalled: bool,
    /// A helper reached its wall-clock deadline and was stopped.
    pub timed_out: bool,
    /// The loop detector killed the run; the failure detail.
    pub looped: Option<String>,
    /// The run's estimated cost passed `RunRequest::max_budget_usd` and it
    /// was stopped (or the CLI reported `error_max_budget_usd`).
    pub over_budget: bool,
    /// The run was still going at the attempt's wall-clock budget and was
    /// stopped.
    pub time_budget: bool,
    /// Claude only: input + cache creation + cache read tokens of the first
    /// `assistant` event, i.e. the whole prompt the first turn was sent,
    /// whether or not an earlier run left it in the prompt cache.
    pub first_turn_tokens: Option<u64>,
    /// Claude only: the usage of each top-level assistant message, by
    /// message id -- the stream repeats a message once per content block.
    pub messages: BTreeMap<String, MessageUsage>,
    /// `cost_usd` was priced from `messages`: the run printed no `result`.
    pub cost_estimated: bool,
    /// What the harness reported it ran: Claude's init `model` and
    /// `claude_code_version`, the `modelUsage` keys of its result (every
    /// real model id, subagents included), Codex's reported `model`.
    pub init_model: Option<String>,
    pub harness_version: Option<String>,
    pub usage_models: Vec<String>,
    /// Set by the engine once the run ends.
    pub fingerprint: Option<Fingerprint>,
    /// Claude only: the calls the run was not allowed to make
    /// (`permission_denials` of its result).
    pub permission_denials: Vec<PermissionDenial>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PermissionDenial {
    pub tool: String,
    pub input: serde_json::Value,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MessageUsage {
    pub model: String,
    pub input: u64,
    pub cache_write: u64,
    pub cache_read: u64,
    pub output: u64,
}

impl RunOutcome {
    /// The model ids that really ran: init model, then every `modelUsage`
    /// key, then any top-level message's model, without repeats.
    pub fn real_models(&self) -> Vec<String> {
        let mut models: Vec<String> = Vec::new();
        let all = self
            .init_model
            .iter()
            .chain(self.usage_models.iter())
            .chain(self.messages.values().map(|m| &m.model))
            .filter(|m| !m.is_empty());
        for m in all {
            if !models.contains(m) {
                models.push(m.clone());
            }
        }
        models
    }

    /// A Claude run that ended without its `result` event (killed, stalled,
    /// crashed): price the messages it did stream, fill in their token
    /// usage, and mark the cost estimated. No-op once a cost is known, or
    /// when no message has a price.
    pub fn estimate_cost(&mut self, prices: &BTreeMap<String, Price>) {
        if self.cost_usd.is_some() {
            return;
        }
        if let Some(cost) = self.streamed_cost(prices) {
            self.cost_usd = Some(cost);
            self.cost_estimated = true;
            let sum = |f: fn(&MessageUsage) -> u64| self.messages.values().map(f).sum::<u64>();
            self.usage_input = sum(|m| m.input);
            self.usage_cached = sum(|m| m.cache_read);
            self.usage_output = sum(|m| m.output);
        }
    }

    /// The streamed messages priced so far; `None` when none has a price.
    pub fn streamed_cost(&self, prices: &BTreeMap<String, Price>) -> Option<f64> {
        let priced: Vec<(&MessageUsage, Price)> = self
            .messages
            .values()
            .filter_map(|m| price_for(prices, &m.model).map(|p| (m, p)))
            .collect();
        if priced.is_empty() {
            return None;
        }
        Some(
            priced
                .iter()
                .map(|(m, p)| p.claude_cost(m.input, m.cache_write, m.cache_read, m.output))
                .sum(),
        )
    }
}

/// The error a harness run leaves when it exits non-zero with nothing on
/// stderr. No exit code in the text: failure signatures strip digits, and
/// "exit status: ." then reached the owner's question verbatim.
pub fn exit_error(harness: Harness, status: std::process::ExitStatus) -> String {
    let name = format!("{harness:?}");
    if status.code().is_some() {
        format!("{name} exited with an error.")
    } else {
        format!("{name} was stopped by a signal.")
    }
}
