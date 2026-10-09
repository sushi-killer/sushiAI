//! When an agent session may go to sleep. Pure: the daemon feeds it facts and acts on the verdict.

use std::collections::VecDeque;

use sushiai_protocol::{AgentStatus, SessionInfo};

/// Child processes of the agent that used more CPU than this ...
pub const BUSY_CPU_MS: u64 = 2_000;
/// ... within this window keep the session awake (a build, a test run). MCP servers idle at ~0.
pub const BUSY_WINDOW_MS: u64 = 10 * 60 * 1000;

/// True for a session that can sleep and be woken: an agent the daemon can resume
/// (`resumable`, decided by the daemon's agent list), with a conversation to resume and a
/// stored launch (it was launched by the app).
pub fn can_hibernate(info: &SessionInfo, has_launch: bool, resumable: bool) -> bool {
    has_launch && resumable && info.agent.agent_session.is_some()
}

/// What the daemon knows about one session at a tick.
#[derive(Debug, Clone, Copy)]
pub struct Candidate {
    /// `can_hibernate` held.
    pub eligible: bool,
    /// The session has a live process and is not waking or going to sleep.
    pub running: bool,
    pub agent_status: Option<AgentStatus>,
    pub open_asks: usize,
    pub pinned: bool,
    /// Some client looks at the session.
    pub focused: bool,
    /// The later of the last status change and the last input, unix ms.
    pub quiet_since_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Leave it awake; nothing to measure.
    Keep,
    /// Close to idle long enough: measure the CPU of the agent's children.
    Sample,
    Hibernate,
}

/// `threshold_ms` is the idle time that puts a session to sleep (never 0). `busy` is the
/// verdict of a `BusyMeter`; `None` while it has too few samples. Sampling starts one
/// `BUSY_WINDOW_MS` before the threshold, so the window is full when the threshold passes.
pub fn decide(c: &Candidate, now_ms: u64, threshold_ms: u64, busy: Option<bool>) -> Decision {
    let waiting = c.eligible
        && c.running
        && c.agent_status == Some(AgentStatus::Idle)
        && c.open_asks == 0
        && !c.pinned
        && !c.focused;
    if !waiting {
        return Decision::Keep;
    }
    let idle = now_ms.saturating_sub(c.quiet_since_ms);
    if idle.saturating_add(BUSY_WINDOW_MS) < threshold_ms {
        return Decision::Keep;
    }
    if idle >= threshold_ms && busy == Some(false) {
        return Decision::Hibernate;
    }
    Decision::Sample
}

/// CPU used by the agent's descendants, from samples of their summed CPU time.
#[derive(Debug, Default)]
pub struct BusyMeter {
    last_total_ms: Option<u64>,
    /// `(sample time, CPU ms used since the sample before)`.
    deltas: VecDeque<(u64, u64)>,
}

impl BusyMeter {
    /// Adds a sample: the CPU time all descendants have used in total so far. A total that
    /// went down (a process ended) counts as no use.
    pub fn observe(&mut self, now_ms: u64, total_ms: u64) {
        if let Some(last) = self.last_total_ms {
            self.deltas
                .push_back((now_ms, total_ms.saturating_sub(last)));
        }
        self.last_total_ms = Some(total_ms);
        while self
            .deltas
            .front()
            .is_some_and(|(at, _)| now_ms.saturating_sub(*at) > BUSY_WINDOW_MS)
        {
            self.deltas.pop_front();
        }
    }

    /// `None` until a second sample made a first difference.
    pub fn busy(&self, now_ms: u64) -> Option<bool> {
        let used: u64 = self
            .deltas
            .iter()
            .filter(|(at, _)| now_ms.saturating_sub(*at) <= BUSY_WINDOW_MS)
            .map(|(_, ms)| ms)
            .sum();
        (!self.deltas.is_empty()).then_some(used > BUSY_CPU_MS)
    }
}
