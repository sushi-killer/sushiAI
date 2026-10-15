use crate::clock::now_ms;
use crate::harness;
use crate::harness::watch::HookClock;
use crate::harness::watch::Stall;
use crate::loop_detect::LoopDetector;
use crate::policy::variant::Variant;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

/// What ends a run early: silence, a loop, spend past the cap, or the
/// wall-clock budget. The same guard watches a run this daemon started and
/// one it adopted.
pub(crate) struct Guard {
    stall: Option<Stall>,
    wall: Option<(tokio::time::Instant, WallLimit)>,
    detector: Option<LoopDetector>,
    /// Dollars one attempt may spend; `None` = no cap.
    cap: Option<f64>,
    last_output: tokio::time::Instant,
}

enum WallLimit {
    Attempt(f64),
    Helper(String, Duration),
}

pub(crate) fn helper_timeout_message(helper: &str, limit: Duration) -> String {
    let seconds = if limit.is_zero() {
        "0.0".to_string()
    } else {
        limit.as_secs_f64().to_string()
    };
    format!("{helper} timed out after {seconds}s")
}

/// Why a guard stopped a run.
pub(crate) enum Trip {
    Stalled(String),
    Looped(String),
    OverBudget(String),
    TimeBudget(String),
    TimedOut(String),
}

impl Trip {
    pub fn apply(self, outcome: &mut harness::RunOutcome) {
        match self {
            Trip::Stalled(error) => {
                outcome.stalled = true;
                outcome.error = Some(error);
            }
            Trip::Looped(note) => outcome.looped = Some(note),
            Trip::OverBudget(error) => {
                outcome.over_budget = true;
                outcome.error = Some(error);
            }
            Trip::TimeBudget(error) => {
                outcome.time_budget = true;
                outcome.error = Some(error);
            }
            Trip::TimedOut(error) => {
                outcome.timed_out = true;
                outcome.error = Some(error);
            }
        }
    }
}

impl Guard {
    /// No watch: an unguarded run.
    pub fn off() -> Self {
        Guard {
            stall: None,
            wall: None,
            detector: None,
            cap: None,
            last_output: tokio::time::Instant::now(),
        }
    }

    pub fn helper(name: &str, limit: Duration) -> Self {
        Self {
            wall: Some((
                tokio::time::Instant::now() + limit,
                WallLimit::Helper(name.to_string(), limit),
            )),
            ..Self::off()
        }
    }

    /// The watch a variant asks for; `hook_running` pauses the stall clock.
    pub fn for_variant(variant: &Variant, hook_running: Option<Arc<HookClock>>) -> Self {
        Guard {
            stall: (variant.stall_timeout_secs > 0).then(|| Stall {
                limit: Duration::from_secs(variant.stall_timeout_secs),
                paused: hook_running.unwrap_or_default(),
            }),
            wall: None,
            detector: variant.loop_detect.then(LoopDetector::new),
            cap: (variant.max_attempt_cost_usd > 0.0).then_some(variant.max_attempt_cost_usd),
            last_output: tokio::time::Instant::now(),
        }
    }

    /// Adds the wall-clock budget of an implement run: `mins` after the
    /// attempt started (`started_at_ms`), so a run a restarted daemon adopts
    /// does not get a fresh one. `0` (or less) = no budget.
    pub fn with_time_budget(mut self, mins: f64, started_at_ms: i64) -> Self {
        if mins > 0.0 {
            let left_ms = started_at_ms as f64 + mins * 60_000.0 - now_ms() as f64;
            let left = Duration::from_millis(left_ms.max(0.0) as u64);
            self.wall = Some((tokio::time::Instant::now() + left, WallLimit::Attempt(mins)));
        }
        self
    }

    pub(crate) fn armed(&self) -> bool {
        self.stall.is_some() || self.wall.is_some()
    }

    /// The earlier of the silence limit and the wall deadline. An unset
    /// limit still needs a deadline that does not overflow `Instant`
    /// (select! builds a disabled branch's future too).
    pub(crate) fn deadline(&self) -> tokio::time::Instant {
        let limit = self
            .stall
            .as_ref()
            .map_or(Duration::from_secs(365 * 24 * 3600), |s| s.limit);
        let silent = self.last_output + limit;
        self.wall
            .as_ref()
            .map_or(silent, |(wall, _)| (*wall).min(silent))
    }

    pub(crate) fn heard(&mut self) {
        self.last_output = tokio::time::Instant::now();
    }

    /// The wall deadline passed (`TimeBudget`), or the silence limit ran
    /// out. Silence is the only stall signal; a long
    /// Bash call or the Stop hook's verify (up to 540s) is silent too, so the
    /// limit must sit above them. While a hook runs no stall fires, and the
    /// moment it ends counts as activity: the window starts then.
    pub(crate) fn stall_expired(&mut self) -> Option<Trip> {
        if let Some((wall, limit)) = &self.wall {
            if tokio::time::Instant::now() >= *wall {
                return Some(match limit {
                    WallLimit::Attempt(mins) => Trip::TimeBudget(format!(
                        "The attempt ran past its {mins} minute time budget; the run was stopped."
                    )),
                    WallLimit::Helper(name, limit) => {
                        Trip::TimedOut(helper_timeout_message(name, *limit))
                    }
                });
            }
        }
        let stall = self.stall.as_ref()?;
        if stall.paused.running.load(Ordering::SeqCst) {
            self.heard();
            return None;
        }
        let ended = *stall.paused.ended.lock().unwrap();
        if let Some(ended) = ended.filter(|e| *e > self.last_output) {
            self.last_output = ended;
        }
        if tokio::time::Instant::now() < self.deadline() {
            return None;
        }
        Some(Trip::Stalled(format!(
            "No output for {}s; the run was stopped as stalled.",
            stall.limit.as_secs()
        )))
    }
}

impl Guard {
    pub(crate) fn observe_line(&mut self, line: &str) -> Option<Trip> {
        self.heard();
        self.detector
            .as_mut()
            .and_then(|d| d.feed(line))
            .map(|hit| Trip::Looped(format!("{} Do not repeat it; change approach.", hit.detail)))
    }

    pub(crate) fn has_cap(&self) -> bool {
        self.cap.is_some()
    }

    pub(crate) fn check_spend(
        &self,
        reported_over_budget: bool,
        spent: Option<f64>,
    ) -> Option<Trip> {
        let cap = self.cap?;
        (reported_over_budget || spent.is_some_and(|c| c > cap)).then(|| {
            Trip::OverBudget(format!(
                "The attempt spent about ${:.4}, past its ${cap} cap; the run was stopped.",
                spent.unwrap_or(cap)
            ))
        })
    }
}

#[cfg(test)]
mod tests;
