use super::routing::Route;
use serde::{Deserialize, Serialize};

/// Per-million-token list prices for a model whose harness reports tokens
/// but no cost (Codex), or a Claude run that ended before its `result`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Price {
    pub input: f64,
    /// Cache reads.
    pub cached_input: f64,
    pub output: f64,
    /// Cache writes (Claude's `cache_creation_input_tokens`); `None` bills
    /// them at the input price.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<f64>,
}

/// The price of `model`: an exact key, else the longest key it extends by a
/// `-suffix` (`claude-haiku-4-5` prices `claude-haiku-4-5-20251001`).
pub fn price_for(prices: &std::collections::BTreeMap<String, Price>, model: &str) -> Option<Price> {
    if let Some(p) = prices.get(model) {
        return Some(*p);
    }
    prices
        .iter()
        .filter(|(key, _)| {
            model
                .strip_prefix(key.as_str())
                .is_some_and(|rest| rest.starts_with('-'))
        })
        .max_by_key(|(key, _)| key.len())
        .map(|(_, p)| *p)
}

/// Input + output price per million tokens of the route's model: `price_for`,
/// else the cheapest price key holding the bare alias (`sonnet`) as a
/// dash-separated segment. `None` without a model or a match.
pub fn route_cost(
    prices: &std::collections::BTreeMap<String, Price>,
    route: &Route,
) -> Option<f64> {
    let model = route.model.as_deref()?;
    let cost = |p: &Price| p.input + p.output;
    if let Some(p) = price_for(prices, model) {
        return Some(cost(&p));
    }
    prices
        .iter()
        .filter(|(key, _)| key.split('-').any(|seg| seg == model))
        .map(|(_, p)| cost(p))
        .min_by(|a, b| a.total_cmp(b))
}

impl Price {
    /// Claude's `input_tokens` excludes cache writes and reads.
    pub fn claude_cost(&self, input: u64, cache_write: u64, cache_read: u64, output: u64) -> f64 {
        (input as f64 * self.input
            + cache_write as f64 * self.cache_write.unwrap_or(self.input)
            + cache_read as f64 * self.cached_input
            + output as f64 * self.output)
            / 1_000_000.0
    }

    /// Codex's `input_tokens` already includes the cached ones.
    pub fn codex_cost(&self, input: u64, cached: u64, output: u64) -> f64 {
        let fresh = input.saturating_sub(cached) as f64;
        (fresh * self.input + cached as f64 * self.cached_input + output as f64 * self.output)
            / 1_000_000.0
    }
}

// ponytail: list prices as of 2026-09; `settings.prices` overrides them.

pub(super) fn default_prices() -> std::collections::BTreeMap<String, Price> {
    let codex = [
        ("gpt-5.3-codex", 1.75, 0.175, 14.0),
        ("gpt-5.6-luna", 0.20, 0.02, 1.20),
    ]
    .map(|(m, input, cached_input, output)| (m, input, None, cached_input, output));
    let claude = [
        ("claude-opus-5-5", 4.0, 5.0, 0.20, 20.0),
        ("claude-sonnet-5-5", 2.0, 2.50, 0.20, 10.0),
        ("claude-sonnet-5", 2.0, 2.50, 0.20, 10.0),
        ("claude-haiku-4-5", 1.0, 1.25, 0.10, 5.0),
    ]
    .map(|(m, input, write, read, output)| (m, input, Some(write), read, output));
    codex
        .into_iter()
        .chain(claude)
        .map(|(m, input, cache_write, cached_input, output)| {
            (
                m.to_string(),
                Price {
                    input,
                    cached_input,
                    output,
                    cache_write,
                },
            )
        })
        .collect()
}

#[cfg(test)]
#[path = "tests/pricing.rs"]
mod tests;
