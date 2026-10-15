use super::calls::{bucket_rules, calls_of, explore_of, throwaways, Call};
use super::Detection;
use crate::ab::Bucket;
use crate::policy::keys::normalize_signature_line;
use crate::task::evolution::signals::SignalKind;
use crate::task::{Attempt, FailureKind, Stage, Task};
use std::collections::HashSet;

impl Detection<'_> {
    /// loop, throwaway_script, verify_signature, review_finding, process_read.
    pub(super) fn per_attempt(&mut self, earlier: &[&Task]) {
        let task = self.task;
        let known_signatures: HashSet<&str> = earlier
            .iter()
            .flat_map(|t| &t.attempts)
            .filter_map(|a| a.failure.as_ref())
            .map(|f| f.signature.as_str())
            .filter(|s| !s.is_empty())
            .collect();
        let known_findings: HashSet<String> = earlier
            .iter()
            .flat_map(|t| &t.attempts)
            .flat_map(findings)
            .collect();
        for a in &task.attempts {
            let total = self.total_calls(a);
            if let Some(f) = &a.failure {
                if f.kind == FailureKind::Loop {
                    self.source(a.n, None).record(
                        SignalKind::Loop,
                        format!("attempt {} was stopped by the loop detector", a.n),
                        total,
                    );
                }
                if known_signatures.contains(f.signature.as_str()) {
                    self.source(a.n, None).record(
                        SignalKind::VerifySignature,
                        format!(
                            "failure `{}` already happened in an earlier task",
                            f.signature
                        ),
                        total,
                    );
                }
            }
            let repeated: Vec<String> = findings(a)
                .into_iter()
                .filter(|f| known_findings.contains(f))
                .collect();
            if !repeated.is_empty() {
                self.source(a.n, None).record(
                    SignalKind::ReviewFinding,
                    format!(
                        "review finding seen in an earlier task: {}",
                        repeated.join("; ")
                    ),
                    total,
                );
            }
            if a.stage != Stage::Implement {
                continue;
            }
            let calls = self.calls(a);
            let process: Vec<&Call> = calls
                .iter()
                .filter(|c| c.bucket == Bucket::Process)
                .collect();
            let enough = process.len() as u32 >= self.settings.evolution.process_min_calls.max(1);
            if let (true, Some(first), Some(last)) = (enough, process.first(), process.last()) {
                self.source(a.n, Some((first.line, last.line))).record(
                    SignalKind::ProcessRead,
                    format!("{} tool call(s) went into process files", process.len()),
                    process.len() as u32,
                );
            }
            for (path, matching) in throwaways(&calls, &a.changed_files) {
                self.source(
                    a.n,
                    matching.first().zip(matching.last()).map(|(f, l)| (*f, *l)),
                )
                .record(
                    SignalKind::ThrowawayScript,
                    format!("{path} was created and then deleted"),
                    matching.len() as u32,
                );
            }
        }
    }

    /// Compares each implement attempt with earlier tasks' implement attempts
    /// of the same ordinal, so a first attempt (which explores) is never
    /// measured against retries (which barely do).
    pub(super) fn discovery(&mut self, earlier: &[&Task]) {
        let rules = bucket_rules(self.settings, &self.task.repo);
        let mut samples: Vec<Vec<f64>> = Vec::new();
        for t in earlier {
            let implement = t.attempts.iter().filter(|a| a.stage == Stage::Implement);
            for (ordinal, a) in implement.enumerate() {
                let Some(evs) = self.events.get(&(t.id.clone(), a.n)) else {
                    continue;
                };
                let calls = calls_of(t, a, evs, &rules);
                if calls.is_empty() {
                    continue;
                }
                if samples.len() <= ordinal {
                    samples.resize(ordinal + 1, Vec::new());
                }
                samples[ordinal].push(explore_of(&calls).len() as f64);
            }
        }
        let implement = self
            .task
            .attempts
            .iter()
            .filter(|a| a.stage == Stage::Implement);
        for (ordinal, a) in implement.enumerate() {
            let Some(sample) = samples.get_mut(ordinal) else {
                continue;
            };
            if (sample.len() as u32) < self.settings.evolution.min_tasks {
                continue;
            }
            sample.sort_by(|a, b| a.total_cmp(b));
            let Some(median) = median_sorted(sample) else {
                continue;
            };
            let calls = self.calls(a);
            let explore = explore_of(&calls);
            if (explore.len() as f64) > 2.0 * median {
                let wasted = (explore.len() as f64 - median) as u32;
                let lines = explore.first().zip(explore.last()).map(|(f, l)| (*f, *l));
                self.source(a.n, lines).record(
                    SignalKind::Discovery,
                    format!(
                        "{} explore calls, the repo median is {median}",
                        explore.len()
                    ),
                    wasted,
                );
            }
        }
    }

    pub(super) fn repeated_review(&mut self) {
        for pair in self.task.attempts.windows(2) {
            let before: HashSet<String> = findings(&pair[0]).into_iter().collect();
            let same: Vec<String> = findings(&pair[1])
                .into_iter()
                .filter(|f| before.contains(f))
                .collect();
            if !same.is_empty() {
                let total = self.total_calls(&pair[1]);
                self.source(pair[1].n, None).record(
                    SignalKind::RepeatedReview,
                    format!(
                        "review raised the same finding again on attempt {}: {}",
                        pair[1].n,
                        same.join("; ")
                    ),
                    total,
                );
            }
        }
    }
}

fn median_sorted(xs: &[f64]) -> Option<f64> {
    if xs.is_empty() {
        return None;
    }
    let mid = xs.len() / 2;
    Some(if xs.len().is_multiple_of(2) {
        (xs[mid - 1] + xs[mid]) / 2.0
    } else {
        xs[mid]
    })
}

pub(super) fn findings(a: &Attempt) -> Vec<String> {
    let mut out: Vec<String> = a
        .review
        .iter()
        .flat_map(|r| &r.findings)
        // Synthetic: the owner's answer, not something the reviewer found.
        .filter(|f| !f.starts_with("Another attempt was asked for:"))
        .map(|f| normalize_signature_line(f.trim()))
        .filter(|f| !f.is_empty())
        .collect();
    out.dedup();
    out
}
