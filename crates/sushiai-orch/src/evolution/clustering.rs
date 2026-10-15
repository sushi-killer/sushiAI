use crate::policy::evolution::EvolutionSettings;
use crate::policy::keys::{normalize_signature_line, simple_hash};
use crate::task::evolution::signals::{Signal, SignalKind, StoredSignal};
use std::collections::{HashMap, HashSet};

fn kind_name(kind: SignalKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default()
}

/// The first word of an owner's answer, lower-cased ("continue", "retry",
/// "drop"): the kind of decision, so a pattern of answers can add up where
/// the free text after it never repeats.
fn owner_answer_kind(detail: &str) -> String {
    detail
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase()
}

pub(super) fn cluster_key(signal: &Signal) -> String {
    let normalised = match signal.kind {
        SignalKind::OwnerQuestion => owner_answer_kind(&signal.detail),
        _ => normalize_signature_line(&signal.detail),
    };
    format!("{}:{}", kind_name(signal.kind), simple_hash(&normalised))
}

/// Signals of one kind and normalised detail.
pub(super) struct Cluster<'a> {
    pub(super) key: String,
    pub(super) kind: String,
    pub(super) occurrences: Vec<&'a StoredSignal>,
}

impl Cluster<'_> {
    pub(super) fn tasks(&self) -> usize {
        self.occurrences
            .iter()
            .map(|o| o.signal.task_id.as_str())
            .collect::<HashSet<_>>()
            .len()
    }

    pub(super) fn wasted_calls(&self) -> u32 {
        self.occurrences.iter().map(|o| o.signal.wasted_calls).sum()
    }

    pub(super) fn wasted_usd(&self) -> f64 {
        self.occurrences.iter().map(|o| o.signal.wasted_usd).sum()
    }

    /// The repo with the most occurrences; the smallest path on a tie.
    pub(super) fn main_repo(&self) -> &str {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for o in &self.occurrences {
            *counts.entry(o.signal.repo.as_str()).or_default() += 1;
        }
        counts
            .into_iter()
            .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(a.0)))
            .map(|(repo, _)| repo)
            .unwrap_or("")
    }

    /// Latest by `ts`, then by position in the file.
    pub(super) fn latest(&self) -> &Signal {
        let mut latest = &self.occurrences[0];
        for o in &self.occurrences {
            if o.ts >= latest.ts {
                latest = o;
            }
        }
        &latest.signal
    }

    pub(super) fn qualifies(&self, settings: &EvolutionSettings) -> bool {
        self.tasks() >= settings.min_tasks as usize
            || self.wasted_calls() >= settings.min_wasted_calls
            || self.wasted_usd() >= settings.min_wasted_usd
    }
}

/// Signals grouped by kind and normalised detail, most wasteful first.
pub(super) fn clusters(signals: &[StoredSignal]) -> Vec<Cluster<'_>> {
    let mut by_key: HashMap<String, Cluster> = HashMap::new();
    for s in signals {
        by_key
            .entry(cluster_key(&s.signal))
            .or_insert_with(|| Cluster {
                key: cluster_key(&s.signal),
                kind: kind_name(s.signal.kind),
                occurrences: Vec::new(),
            })
            .occurrences
            .push(s);
    }
    let mut out: Vec<Cluster> = by_key.into_values().collect();
    out.sort_by(|a, b| {
        b.wasted_usd()
            .total_cmp(&a.wasted_usd())
            .then_with(|| b.wasted_calls().cmp(&a.wasted_calls()))
            .then_with(|| b.tasks().cmp(&a.tasks()))
            .then_with(|| a.key.cmp(&b.key))
    });
    out
}

#[cfg(test)]
mod tests;
