use serde_json::json;
use sushiai_protocol::{
    Overlap, ScheduleEvent, ScheduleEventKind, ScheduleSet, SessionCreate, CAPABILITIES,
};

#[test]
fn a_schedule_entry_takes_defaults_and_keeps_the_create_shape() {
    let set: ScheduleSet = serde_json::from_value(json!({
        "id": "nightly",
        "cron": "0 3 * * *",
        "tz": "Europe/Berlin",
        "create": { "cwd": "/work/my-app", "cols": 100, "rows": 30, "agent": "claude", "background": true },
    }))
    .expect("parse");
    assert!(set.enabled);
    assert_eq!(set.overlap, Overlap::Skip);
    assert_eq!(set.jitter_sec, None);
    assert!(set.create.background);
    let back = serde_json::to_value(&set).expect("serialize");
    assert_eq!(back["overlap"], "skip");
    assert_eq!(back["create"]["background"], true);
}

#[test]
fn background_is_left_out_when_false_and_defaults_to_false() {
    let create: SessionCreate = serde_json::from_value(json!({
        "cwd": "/work", "cols": 80, "rows": 24
    }))
    .expect("parse");
    assert!(!create.background);
    let value = serde_json::to_value(&create).expect("serialize");
    assert!(value.get("background").is_none());
}

#[test]
fn an_event_serializes_with_snake_case_kinds_and_camel_case_fields() {
    let event = ScheduleEvent {
        id: "nightly".into(),
        kind: ScheduleEventKind::CaughtUp,
        reason: None,
        session: Some("abc".into()),
        scheduled_for: 1_000,
        started_at: 2_000,
        ended_at: None,
    };
    assert_eq!(
        serde_json::to_value(&event).expect("serialize"),
        json!({
            "id": "nightly", "kind": "caught_up", "session": "abc",
            "scheduledFor": 1000, "startedAt": 2000
        })
    );
    assert!(CAPABILITIES.contains(&"schedules") && CAPABILITIES.contains(&"ownerAsk"));
}
