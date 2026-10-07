//! Scrollback across a resize: a shell's history must survive the layout changing its pty.
//! vt100 keeps scrollback rows as they are on a width change, but a shorter screen used to
//! drop its bottom rows (the newest output) instead of moving the top rows into scrollback.

use sushiai_core::Screen;

fn feed_lines(screen: &mut Screen, from: usize, to: usize) {
    for i in from..=to {
        screen.feed(format!("line-{i:03}\r\n").as_bytes());
    }
}

/// History plus the live screen, as `session.read` returns it.
fn all_text(screen: &mut Screen) -> String {
    screen.history_text(usize::MAX) + &screen.text()
}

/// The fed lines `from..=to` that are not in `text`, each matched as a whole line.
fn missing(text: &str, from: usize, to: usize) -> Vec<usize> {
    (from..=to)
        .filter(|i| !text.lines().any(|l| l == format!("line-{i:03}")))
        .collect()
}

#[test]
fn history_survives_narrow_to_wide_resize() {
    let mut screen = Screen::new(24, 27);
    feed_lines(&mut screen, 1, 80);
    screen.resize(24, 138);
    assert_eq!(missing(&all_text(&mut screen), 1, 80), Vec::<usize>::new());
    let lines = screen.history_formatted(usize::MAX);
    assert!(String::from_utf8_lossy(&lines[0]).contains("line-001"));
}

#[test]
fn history_survives_wide_to_narrow_to_wide_resize() {
    let mut screen = Screen::new(24, 138);
    feed_lines(&mut screen, 1, 80);
    screen.resize(24, 104);
    screen.resize(24, 138);
    assert_eq!(missing(&all_text(&mut screen), 1, 80), Vec::<usize>::new());
}

#[test]
fn narrow_to_wide_and_shorter_keeps_every_line_in_order() {
    let mut screen = Screen::new(30, 27);
    feed_lines(&mut screen, 1, 80);
    screen.resize(20, 138);
    let text = all_text(&mut screen);
    assert_eq!(missing(&text, 1, 80), Vec::<usize>::new(), "{text}");
    // The newest lines stay on screen, right above the cursor's (empty) line.
    let live = screen.text();
    assert!(live.trim_end().ends_with("line-080"), "{live}");
    // Output after the resize continues below them, not over them.
    feed_lines(&mut screen, 81, 90);
    let text = all_text(&mut screen);
    assert_eq!(missing(&text, 1, 90), Vec::<usize>::new(), "{text}");
    let at = |n: usize| text.find(&format!("line-{n:03}")).expect("line");
    assert!(
        at(1) < at(80) && at(80) < at(81) && at(81) < at(90),
        "{text}"
    );
}

#[test]
fn a_very_short_intermediate_size_loses_nothing() {
    let mut screen = Screen::new(24, 27);
    feed_lines(&mut screen, 1, 80);
    screen.resize(1, 27);
    screen.resize(40, 138);
    let text = all_text(&mut screen);
    assert_eq!(missing(&text, 1, 80), Vec::<usize>::new(), "{text}");
}

#[test]
fn rows_below_the_cursor_go_first() {
    // The cursor is near the top: shrinking drops the blank rows below it, no history moves.
    let mut screen = Screen::new(24, 40);
    screen.feed(b"one\r\ntwo\r\n");
    screen.resize(10, 40);
    assert_eq!(screen.history_text(usize::MAX), "");
    assert!(screen.text().starts_with("one\ntwo"));
}

#[test]
fn the_alternate_screen_is_left_to_its_program() {
    let mut screen = Screen::new(24, 40);
    feed_lines(&mut screen, 1, 10);
    screen.feed(b"\x1b[?1049h\x1b[24;1Hbottom");
    screen.resize(10, 40);
    assert_eq!(screen.size(), (10, 40));
    screen.feed(b"\x1b[?1049l");
    let text = all_text(&mut screen);
    assert_eq!(missing(&text, 1, 10), Vec::<usize>::new(), "{text}");
}
