use super::Detection;
use crate::task::evolution::signals::{ExcerptRef, Signal, SignalKind};
use crate::task::Task;

pub(super) struct SignalSource<'a> {
    task: &'a Task,
    attempt: u32,
    calls: usize,
    excerpt: ExcerptRef,
    out: &'a mut Vec<Signal>,
}

impl SignalSource<'_> {
    pub(super) fn record(self, kind: SignalKind, detail: String, wasted_calls: u32) {
        let cost = self
            .task
            .attempts
            .iter()
            .find(|a| a.n == self.attempt)
            .and_then(|a| a.cost_usd);
        self.out.push(Signal {
            kind,
            task_id: self.task.id.clone(),
            repo: self.task.repo.clone(),
            attempt: self.attempt,
            detail,
            wasted_calls,
            wasted_usd: wasted_usd(wasted_calls, self.calls, cost),
            excerpt_ref: self.excerpt,
        });
    }
}

impl Detection<'_> {
    pub(super) fn source(
        &mut self,
        attempt: u32,
        lines: Option<(usize, usize)>,
    ) -> SignalSource<'_> {
        let calls = self
            .task
            .attempts
            .iter()
            .find(|a| a.n == attempt)
            .map_or(0, |a| self.calls(a).len());
        let excerpt = self.excerpt(attempt, lines);
        SignalSource {
            task: self.task,
            attempt,
            calls,
            excerpt,
            out: &mut self.out,
        }
    }

    fn excerpt(&self, n: u32, lines: Option<(usize, usize)>) -> ExcerptRef {
        let len = self.attempt_events(n).len();
        if n == 0 || len == 0 {
            return ExcerptRef {
                file: format!("tasks/{}/task.json", self.task.id),
                from_line: 1,
                to_line: 1,
            };
        }
        let (from, to) = lines.unwrap_or((1, len));
        ExcerptRef {
            file: format!("tasks/{}/runs/{n}/events.jsonl", self.task.id),
            from_line: from,
            to_line: to,
        }
    }
}

fn wasted_usd(wasted: u32, total: usize, cost: Option<f64>) -> f64 {
    match cost {
        Some(c) if total > 0 && wasted > 0 => wasted as f64 / total as f64 * c,
        _ => 0.0,
    }
}
