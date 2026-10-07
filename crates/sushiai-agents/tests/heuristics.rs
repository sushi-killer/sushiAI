use sushiai_agents::heuristic::{detect, ScreenAgent};
use sushiai_agents::status::{BlockedKind, Detected};

const PERM: Option<Detected> = Some(Detected::Blocked(BlockedKind::Permission));
const INPUT: Option<Detected> = Some(Detected::Blocked(BlockedKind::Input));

#[test]
fn gemini() {
    let blocked = [
        "Apply this change?",
        "  1. Yes, allow once",
        "  3. No (esc)",
    ];
    assert_eq!(detect(ScreenAgent::Gemini, &blocked), PERM);
    assert_eq!(
        detect(ScreenAgent::Gemini, &["Allow execution of: 'ls'?"]),
        PERM
    );
    let working = ["", "thinking...", "(esc to cancel, 12s)", ""];
    assert_eq!(
        detect(ScreenAgent::Gemini, &working),
        Some(Detected::Working)
    );
    // A dialog beats the spinner line that stays on screen.
    let both = ["Do you want to proceed?", "(esc to cancel)"];
    assert_eq!(detect(ScreenAgent::Gemini, &both), PERM);
    assert_eq!(detect(ScreenAgent::Gemini, &["> type your message"]), None);
}

#[test]
fn cursor() {
    let write = ["Write to this file?", "proceed (y)", "keep (n)"];
    assert_eq!(detect(ScreenAgent::Cursor, &write), PERM);
    let run = [
        "Waiting for approval...",
        "Run this command?",
        "Run (once) (y)",
        "Skip (esc or n)",
    ];
    assert_eq!(detect(ScreenAgent::Cursor, &run), PERM);
    assert_eq!(detect(ScreenAgent::Cursor, &["Keep (n)"]), PERM);
    // The question alone, without its choices, is not a block.
    assert_eq!(detect(ScreenAgent::Cursor, &["Run this command?"]), None);
    assert_eq!(
        detect(ScreenAgent::Cursor, &["x", "ctrl+c to stop"]),
        Some(Detected::Working)
    );
}

#[test]
fn working_hint_only_counts_near_the_bottom() {
    let mut lines: Vec<String> = vec!["ctrl+c to stop".into()];
    lines.extend((0..8).map(|i| format!("output {i}")));
    assert_eq!(detect(ScreenAgent::Cursor, &lines), None);
}

#[test]
fn codex_dialogs_without_a_hook() {
    assert_eq!(
        detect(ScreenAgent::Codex, &["Allow command?", "touch x"]),
        PERM
    );
    assert_eq!(
        detect(
            ScreenAgent::Codex,
            &["Press Enter to confirm or Esc to cancel"]
        ),
        INPUT
    );
    assert_eq!(
        detect(ScreenAgent::Codex, &["enter to submit answer"]),
        INPUT
    );
    assert_eq!(
        detect(ScreenAgent::Codex, &["Update available! 0.160 -> 0.161"]),
        INPUT
    );
    assert_eq!(detect(ScreenAgent::Codex, &["> Ask Codex"]), None);
}

#[test]
fn rules_do_not_leak_between_agents_and_empty_screens_are_none() {
    assert_eq!(detect(ScreenAgent::Codex, &["Apply this change?"]), None);
    assert_eq!(detect(ScreenAgent::Gemini, &["Allow command?"]), None);
    let none: [&str; 0] = [];
    assert_eq!(detect(ScreenAgent::Gemini, &none), None);
    assert_eq!(detect(ScreenAgent::Cursor, &["", "   "]), None);
}
