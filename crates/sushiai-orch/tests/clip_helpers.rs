use sushiai_orch::clip::{ellipsize_chars, tail_chars, truncate_bytes, truncate_chars};

#[test]
fn truncate_chars_cuts_by_characters_without_a_marker() {
    assert_eq!(truncate_chars("h\u{e9}llo w\u{f6}rld", 5), "h\u{e9}llo");
    assert_eq!(truncate_chars("short", 10), "short");
    assert_eq!(truncate_chars("", 3), "");
    assert_eq!(truncate_chars("abc", 0), "");
}

#[test]
fn ellipsize_chars_marks_only_a_cut() {
    assert_eq!(
        ellipsize_chars("h\u{e9}llo w\u{f6}rld", 5),
        "h\u{e9}llo\u{2026}"
    );
    assert_eq!(ellipsize_chars("h\u{e9}llo", 5), "h\u{e9}llo");
}

#[test]
fn truncate_bytes_never_splits_a_character() {
    // each e-acute is two bytes: a limit of 4 bytes falls inside the second one
    assert_eq!(truncate_bytes("a\u{e9}\u{e9}b", 4), "a\u{e9}\u{2026}");
    assert_eq!(truncate_bytes("\u{e9}\u{e9}", 1), "\u{2026}");
    assert_eq!(truncate_bytes("abc", 3), "abc");
}

#[test]
fn tail_chars_keeps_the_last_characters() {
    assert_eq!(tail_chars("h\u{e9}llo w\u{f6}rld", 5), "w\u{f6}rld");
    assert_eq!(tail_chars("abc", 10), "abc");
    assert_eq!(tail_chars("abc", 0), "");
}
