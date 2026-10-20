//! Screen rules for agents without usable hooks: gemini and cursor-agent
//! (shallow support, owner decision) and the Codex dialogs no hook reports.
//!
//! Table-driven. Rules are checked in order, first match wins, so blocked
//! rules come before working rules. A screen that matches nothing is read by
//! [`Screen::read`], which is where the agents differ.
//!
//! The Gemini and Cursor phrases come from the published manifests of the
//! session backend this daemon replaced (spec section 4). The Codex phrases
//! come from the same source; none of them has been checked against a captured
//! screen yet.

use crate::status::{BlockedKind, Detected};

/// One rule of one agent: the last `bottom` non-empty lines, every group
/// matching when any of its phrases is a substring (lowercase).
#[derive(Debug)]
pub struct Rule {
    pub result: Detected,
    /// Look at the last N non-empty lines.
    pub bottom: usize,
    /// Every group must match.
    pub all: &'static [&'static [&'static str]],
}

pub const PERM: Detected = Detected::Blocked(BlockedKind::Permission);
pub const INPUT: Detected = Detected::Blocked(BlockedKind::Input);

/// What a screen that matched no rule means to this agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenMode {
    /// Only a dialog changes the status. A quiet screen is Idle only when the
    /// agent is already blocked: Codex dialogs block, its spinner says nothing.
    DialogOnly,
    /// Any detection counts, and a quiet screen with text on it means the
    /// agent waits for the owner: gemini and cursor-agent.
    TextOnScreen,
}

/// An agent's screen: its rules, and how to read a screen that matched none.
#[derive(Debug, Clone, Copy)]
pub struct Screen {
    pub mode: ScreenMode,
    pub rules: &'static [Rule],
}

impl Screen {
    /// What the screen says about the agent's status, or `None` when it says
    /// nothing. `blocked` is the agent's current status, not its ask.
    pub fn read(&self, lines: &[&str], blocked: bool) -> Option<Detected> {
        match (detect(self.rules, lines), self.mode) {
            (Some(blocked @ Detected::Blocked(_)), _) => Some(blocked),
            (Some(_), ScreenMode::DialogOnly) => None,
            (Some(other), ScreenMode::TextOnScreen) => Some(other),
            (None, ScreenMode::DialogOnly) => blocked.then_some(Detected::Idle),
            (None, ScreenMode::TextOnScreen) => lines
                .iter()
                .any(|l| !l.trim().is_empty())
                .then_some(Detected::Idle),
        }
    }
}

pub fn detect<S: AsRef<str>>(rules: &[Rule], lines: &[S]) -> Option<Detected> {
    rules
        .iter()
        .find(|r| {
            let text = region_text(lines, r.bottom);
            r.all
                .iter()
                .all(|group| group.iter().any(|p| text.contains(p)))
        })
        .map(|r| r.result)
}

fn region_text<S: AsRef<str>>(lines: &[S], n: usize) -> String {
    let mut tail: Vec<&str> = lines
        .iter()
        .map(|l| l.as_ref())
        .filter(|l| !l.trim().is_empty())
        .rev()
        .take(n)
        .collect();
    tail.reverse();
    tail.join("\n").to_lowercase()
}
