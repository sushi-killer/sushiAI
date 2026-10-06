//! Screen heuristics for agents without usable hooks: gemini and cursor-agent
//! (shallow support, owner decision) and the Codex dialogs no hook reports.
//!
//! Table-driven. Rules are checked in order, first match wins, so blocked
//! rules come before working rules. A screen that matches nothing returns
//! `None` (the caller decides what a quiet screen means).
//!
//! The Gemini and Cursor phrases come from Herdr's published manifests (spec
//! section 4). The Codex phrases come from the same source; none of them has
//! been checked against a captured screen yet.

use crate::status::{BlockedKind, Detected};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenAgent {
    Gemini,
    Cursor,
    Codex,
}

struct Rule {
    agent: ScreenAgent,
    result: Detected,
    /// Look at the last N non-empty lines.
    bottom: usize,
    /// Every group must match; a group matches when any of its phrases is a
    /// substring (lowercase) of the region text.
    all: &'static [&'static [&'static str]],
}

const PERM: Detected = Detected::Blocked(BlockedKind::Permission);
const INPUT: Detected = Detected::Blocked(BlockedKind::Input);

static RULES: &[Rule] = &[
    Rule {
        agent: ScreenAgent::Gemini,
        result: PERM,
        bottom: 40,
        all: &[&[
            "apply this change",
            "allow execution",
            "waiting for user confirmation",
            "do you want to proceed?",
        ]],
    },
    Rule {
        agent: ScreenAgent::Gemini,
        result: Detected::Working,
        bottom: 12,
        all: &[&["esc to cancel"]],
    },
    Rule {
        agent: ScreenAgent::Cursor,
        result: PERM,
        bottom: 40,
        all: &[&["write to this file?"], &["proceed (y)"]],
    },
    Rule {
        agent: ScreenAgent::Cursor,
        result: PERM,
        bottom: 40,
        all: &[
            &["waiting for approval", "run this command?"],
            &["run (once) (y)", "skip (esc or n)"],
        ],
    },
    Rule {
        agent: ScreenAgent::Cursor,
        result: PERM,
        bottom: 40,
        all: &[&["keep (n)"]],
    },
    Rule {
        agent: ScreenAgent::Cursor,
        result: Detected::Working,
        bottom: 6,
        all: &[&["ctrl+c to stop"]],
    },
    Rule {
        agent: ScreenAgent::Codex,
        result: PERM,
        bottom: 15,
        all: &[&["allow command?"]],
    },
    Rule {
        agent: ScreenAgent::Codex,
        result: INPUT,
        bottom: 15,
        all: &[&[
            "press enter to confirm or esc to cancel",
            "enter to submit answer",
            "do you trust the contents of this directory",
            "update available!",
        ]],
    },
];

fn region_text<S: AsRef<str>>(lines: &[S], n: usize) -> String {
    let mut tail: Vec<&str> = lines
        .iter()
        .map(AsRef::as_ref)
        .filter(|l| !l.trim().is_empty())
        .rev()
        .take(n)
        .collect();
    tail.reverse();
    tail.join("\n").to_lowercase()
}

pub fn detect<S: AsRef<str>>(agent: ScreenAgent, lines: &[S]) -> Option<Detected> {
    RULES
        .iter()
        .filter(|r| r.agent == agent)
        .find(|r| {
            let text = region_text(lines, r.bottom);
            r.all
                .iter()
                .all(|group| group.iter().any(|p| text.contains(p)))
        })
        .map(|r| r.result)
}
