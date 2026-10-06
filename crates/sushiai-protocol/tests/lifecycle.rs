use serde_json::json;
use sushiai_protocol::{HelloResult, SessionCreate, SessionInfo, SessionRemoved};

#[test]
fn hello_result_host_is_optional_for_old_daemons() {
    let old: HelloResult =
        serde_json::from_value(json!({"protocol": 1, "capabilities": [], "daemon": "0.1.0"}))
            .unwrap();
    assert_eq!(old.host, "");
}

#[test]
fn create_takes_project_and_group_and_info_omits_unset_fields() {
    let create: SessionCreate = serde_json::from_value(
        json!({"cwd": "/w", "cols": 80, "rows": 24, "project": "p1", "group": "g1"}),
    )
    .unwrap();
    assert_eq!(
        (create.project.as_deref(), create.group.as_deref()),
        (Some("p1"), Some("g1"))
    );
    let info: SessionInfo = serde_json::from_value(json!({
        "id": "a", "cmd": [], "cwd": "/", "status": "running", "cols": 80, "rows": 24
    }))
    .unwrap();
    let value = serde_json::to_value(&info).unwrap();
    for key in ["project", "group"] {
        assert!(value.get(key).is_none(), "{key} was serialized while unset");
    }
}

#[test]
fn removed_params_shape() {
    assert_eq!(
        serde_json::to_value(SessionRemoved { id: "a".into() }).unwrap(),
        json!({"id": "a"})
    );
}
