use sushiai_core::{mark_exited, mark_hibernated, Screen, StateError, StateFile};
use sushiai_protocol::{SessionInfo, SessionStatus};

fn info(id: &str) -> SessionInfo {
    SessionInfo {
        id: id.into(),
        cmd: vec!["/bin/sh".into()],
        cwd: "/work".into(),
        title: Some("demo".into()),
        status: SessionStatus::Running,
        exit_code: None,
        holder_pid: None,
        cols: 80,
        rows: 24,
        project: None,
        group: None,
        agent: Default::default(),
    }
}

#[test]
fn screen_snapshot_contains_fed_text() {
    let mut screen = Screen::new(24, 80);
    screen.feed(b"\x1b[31mhello\x1b[0m world\r\nsecond line");
    assert!(screen.text().contains("hello world"));
    let snapshot = screen.snapshot();
    let text = String::from_utf8_lossy(&snapshot);
    assert!(text.contains("hello") && text.contains("second line"));
    assert_eq!(screen.size(), (24, 80));
    screen.resize(10, 40);
    assert_eq!(screen.size(), (10, 40));
}

#[test]
fn state_file_round_trips() {
    let mut exited = info("b");
    mark_exited(&mut exited, Some(3));
    let state = StateFile::new(vec![info("a"), exited]);
    let back = StateFile::from_bytes(&state.to_bytes().expect("serialize")).expect("round trip");
    assert_eq!(back, state);
    assert_eq!(back.sessions[1].status, SessionStatus::Exited);
}

#[test]
fn unknown_schema_version_is_rejected() {
    let bytes = br#"{"schemaVersion":3,"sessions":[]}"#;
    assert!(matches!(
        StateFile::from_bytes(bytes),
        Err(StateError::UnsupportedSchema(Some(3)))
    ));
    assert!(matches!(
        StateFile::from_bytes(br#"{"sessions":[]}"#),
        Err(StateError::UnsupportedSchema(None))
    ));
}

#[test]
fn schema_two_keeps_the_sleep_fields_and_schema_one_still_loads() {
    let mut asleep = info("a");
    mark_hibernated(&mut asleep, 42);
    asleep.agent.incarnation = 2;
    asleep.agent.pinned = true;
    let state = StateFile::new(vec![asleep]);
    let bytes = state.to_bytes().expect("serialize");
    assert!(String::from_utf8_lossy(&bytes).contains("\"schemaVersion\": 2"));
    let back = StateFile::from_bytes(&bytes).expect("load");
    assert_eq!(back.sessions[0].status, SessionStatus::Hibernated);
    assert_eq!(back.sessions[0].agent.hibernated_at, Some(42));
    assert_eq!(back.sessions[0].agent.incarnation, 2);
    assert!(back.sessions[0].agent.pinned);
}

#[test]
fn a_state_file_from_before_agents_loads_with_empty_agent_fields() {
    let old = br#"{"schemaVersion":1,"sessions":[{"id":"a","cmd":["sh"],"cwd":"/","title":null,
        "status":"running","cols":80,"rows":24}]}"#;
    let state = StateFile::from_bytes(old).expect("old state loads");
    assert_eq!(state.sessions[0].agent, Default::default());
}

#[test]
fn a_state_file_keeps_the_binding_and_old_files_load_without_it() {
    let mut bound = info("a");
    bound.project = Some("p1".into());
    bound.group = Some("g1".into());
    let state = StateFile::new(vec![bound]);
    let back = StateFile::from_bytes(&state.to_bytes().expect("serialize")).expect("load");
    assert_eq!(back.sessions[0].project.as_deref(), Some("p1"));
    assert_eq!(back.sessions[0].group.as_deref(), Some("g1"));
    let old = br#"{"schemaVersion":1,"sessions":[{"id":"a","cmd":["sh"],"cwd":"/","status":"running","cols":80,"rows":24}]}"#;
    let loaded = StateFile::from_bytes(old).expect("old state loads");
    assert_eq!(loaded.sessions[0].project, None);
}

#[test]
fn history_returns_the_newest_scrolled_off_lines_oldest_first() {
    let mut screen = Screen::new(5, 20);
    for i in 1..=30 {
        screen.feed(format!("line-{i}\r\n").as_bytes());
    }
    let before = screen.snapshot();
    // Rows 5 hold lines 27..30 and the cursor row; lines 1..=26 scrolled off.
    let text = screen.history_text(12);
    let expected: String = (15..=26).map(|i| format!("line-{i}\n")).collect();
    assert_eq!(text, expected);
    let formatted = screen.history_formatted(3);
    assert_eq!(formatted.len(), 3);
    assert!(String::from_utf8_lossy(&formatted[0]).contains("line-24"));
    assert_eq!(screen.history_text(0), "");
    assert_eq!(
        screen.history_text(10_000).lines().count(),
        26,
        "clamped to what exists"
    );
    // The live view is untouched, cursor and attributes included.
    assert_eq!(screen.snapshot(), before);
    assert_eq!(screen.size(), (5, 20));
    assert!(screen.text().contains("line-30"));
}

#[test]
fn reading_history_keeps_a_pending_wrap_cursor_and_the_pen() {
    // Both screens end a line exactly in the last column: the cursor waits to wrap.
    let setup: &[u8] = b"a\r\nb\r\nc\r\nd\r\ne\r\nf\r\n\x1b[1;31m0123456789";
    let (mut read, mut control) = (Screen::new(5, 10), Screen::new(5, 10));
    read.feed(setup);
    control.feed(setup);
    assert!(!read.history_text(3).is_empty());
    assert_eq!(read.snapshot(), control.snapshot());
    // The next character wraps; the old last column must survive and the pen must too.
    read.feed(b"XY");
    control.feed(b"XY");
    assert_eq!(read.text(), control.text());
    assert!(read.text().contains("0123456789XY"));
    assert_eq!(read.snapshot(), control.snapshot());
}

#[test]
fn history_text_joins_wrapped_rows_like_text_does() {
    let mut screen = Screen::new(3, 10);
    screen.feed(b"abcdefghijklmnopqrstuvwxy\r\none\r\ntwo\r\nthree\r\nfour");
    let all = screen.history_text(100) + &screen.text();
    assert!(
        all.starts_with("abcdefghijklmnopqrstuvwxy\none\n"),
        "{all:?}"
    );
}

/// Feeds `chunks` to a screen that reads history between the first chunk and the rest, and
/// the same bytes in one piece to a control; both must end identical, and stay so after more
/// output.
fn history_between_chunks_matches_control(chunks: &[&[u8]]) {
    let (mut read, mut control) = (Screen::new(5, 10), Screen::new(5, 10));
    read.feed(chunks[0]);
    assert!(!read.history_text(3).is_empty(), "the test needs history");
    for chunk in &chunks[1..] {
        read.feed(chunk);
    }
    control.feed(&chunks.concat());
    assert_eq!(read.text(), control.text());
    assert_eq!(read.snapshot(), control.snapshot());
    read.feed(b"+tail");
    control.feed(b"+tail");
    assert_eq!(read.text(), control.text());
    assert_eq!(read.snapshot(), control.snapshot());
}

const SCROLLED: &[u8] = b"a\r\nb\r\nc\r\nd\r\ne\r\nf\r\n";

#[test]
fn history_between_the_halves_of_an_escape_sequence() {
    let first = [SCROLLED, b"0123456789\x1b[3"].concat();
    history_between_chunks_matches_control(&[&first, b"1mXY"]);
}

#[test]
fn history_between_the_halves_of_a_utf8_character() {
    let first = [SCROLLED, b"0123456789\xc3"].concat();
    history_between_chunks_matches_control(&[&first, b"\xa9Z"]);
}

#[test]
fn history_with_the_cursor_waiting_to_wrap_in_origin_mode() {
    let first = [SCROLLED, b"\x1b[2;4r\x1b[?6h\x1b[2;1H0123456789"].concat();
    history_between_chunks_matches_control(&[&first, b"XY"]);
}

#[test]
fn history_with_a_wide_character_in_the_last_columns() {
    let first = [SCROLLED, "01234567\u{4e2d}".as_bytes()].concat();
    history_between_chunks_matches_control(&[&first, b"XY"]);
}

#[test]
fn a_snapshot_ends_with_the_bytes_the_screen_still_holds() {
    let mut screen = Screen::new(5, 20);
    screen.feed(b"abc\x1b[3");
    assert!(screen.snapshot().ends_with(b"\x1b[3"));
    // The rest of the sequence arrives: nothing is held, nothing is repeated.
    screen.feed(b"1mred");
    assert!(!screen.snapshot().ends_with(b"\x1b[31m"));
    assert!(String::from_utf8_lossy(&screen.snapshot()).contains("red"));
}
