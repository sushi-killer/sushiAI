//! The screen rules of the agents without usable hooks, driven through the
//! registry: `Screen::read` adds each agent's own meaning to a screen that
//! matched no rule (a quiet gemini/cursor screen is Idle; a quiet Codex screen
//! is Idle only when it is already blocked), and Claude is never read at all.

use sushiai_agents::harness::screen::{detect, Screen};
use sushiai_agents::harness::{self, HarnessRef};
use sushiai_agents::status::{BlockedKind, Detected};

const PERM: Option<Detected> = Some(Detected::Blocked(BlockedKind::Permission));
const INPUT: Option<Detected> = Some(Detected::Blocked(BlockedKind::Input));

/// The registered agent with this id. Every agent this test names is in the
/// registry.
fn agent(id: &str) -> HarnessRef {
    harness::find(id).expect("the agent is registered")
}

/// The screen an agent declares; a missing one is a test bug.
fn screen(id: &str) -> Screen {
    agent(id).screen().expect("the agent declares a screen")
}

#[test]
fn gemini() {
    let gemini = screen("gemini");
    let blocked = [
        "Apply this change?",
        "  1. Yes, allow once",
        "  3. No (esc)",
    ];
    assert_eq!(gemini.read(&blocked, false), PERM);
    assert_eq!(gemini.read(&["Allow execution of: 'ls'?"], false), PERM);
    let working = ["", "thinking...", "(esc to cancel, 12s)", ""];
    assert_eq!(gemini.read(&working, false), Some(Detected::Working));
    // A dialog beats the spinner line that stays on screen.
    let both = ["Do you want to proceed?", "(esc to cancel)"];
    assert_eq!(gemini.read(&both, false), PERM);
    // No rule matched and the screen has text on it: gemini waits for the owner.
    assert_eq!(
        gemini.read(&["> type your message"], false),
        Some(Detected::Idle)
    );
}

#[test]
fn cursor() {
    let cursor = screen("cursor-agent");
    let write = ["Write to this file?", "proceed (y)", "keep (n)"];
    assert_eq!(cursor.read(&write, false), PERM);
    let run = [
        "Waiting for approval...",
        "Run this command?",
        "Run (once) (y)",
        "Skip (esc or n)",
    ];
    assert_eq!(cursor.read(&run, false), PERM);
    assert_eq!(cursor.read(&["Keep (n)"], false), PERM);
    // The question alone, without its choices, is not a block: cursor waits.
    assert_eq!(
        cursor.read(&["Run this command?"], false),
        Some(Detected::Idle)
    );
    assert_eq!(
        cursor.read(&["x", "ctrl+c to stop"], false),
        Some(Detected::Working)
    );
}

#[test]
fn working_hint_only_counts_near_the_bottom() {
    let cursor = screen("cursor-agent");
    let mut lines: Vec<String> = vec!["ctrl+c to stop".into()];
    lines.extend((0..8).map(|i| format!("output {i}")));
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    // Too far up to match, so the screen is quiet and cursor waits.
    assert_eq!(detect(cursor.rules, &lines), None);
    assert_eq!(cursor.read(&refs, false), Some(Detected::Idle));
}

#[test]
fn codex_dialogs_without_a_hook() {
    let codex = screen("codex");
    assert_eq!(codex.read(&["Allow command?", "touch x"], false), PERM);
    assert_eq!(
        codex.read(&["Press Enter to confirm or Esc to cancel"], false),
        INPUT
    );
    assert_eq!(codex.read(&["enter to submit answer"], false), INPUT);
    assert_eq!(
        codex.read(&["Update available! 0.160 -> 0.161"], false),
        INPUT
    );
    assert_eq!(codex.read(&["> Ask Codex"], false), None);
}

/// A Codex dialog only ever blocks, and its spinner says nothing: a quiet screen
/// is Idle only when the agent is already blocked.
#[test]
fn a_codex_quiet_screen_is_idle_only_when_blocked() {
    let codex = screen("codex");
    assert_eq!(codex.read(&["> Ask Codex"], false), None);
    assert_eq!(codex.read(&["> Ask Codex"], true), Some(Detected::Idle));
    assert_eq!(codex.read(&[], false), None);
    assert_eq!(codex.read(&[], true), Some(Detected::Idle));
}

/// Claude's hooks cover it, so it declares no screen and is never read.
#[test]
fn claude_is_never_read() {
    assert!(harness::find("claude").and_then(|h| h.screen()).is_none());
}

#[test]
fn rules_do_not_leak_between_agents_and_empty_screens_are_none() {
    assert_eq!(detect(screen("codex").rules, &["Apply this change?"]), None);
    assert_eq!(detect(screen("gemini").rules, &["Allow command?"]), None);
    assert_eq!(
        detect(screen("cursor-agent").rules, &["Run this command?"]),
        None
    );
    let none: [&str; 0] = [];
    assert_eq!(detect(screen("gemini").rules, &none), None);
    assert_eq!(detect(screen("cursor-agent").rules, &["", "   "]), None);
}
