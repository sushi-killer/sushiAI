use sushiai_core::{mark_exited, Screen, StateError, StateFile};
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
    let bytes = br#"{"schemaVersion":2,"sessions":[]}"#;
    assert!(matches!(
        StateFile::from_bytes(bytes),
        Err(StateError::UnsupportedSchema(Some(2)))
    ));
    assert!(matches!(
        StateFile::from_bytes(br#"{"sessions":[]}"#),
        Err(StateError::UnsupportedSchema(None))
    ));
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
