use serde_json::Value;
use sushiai_protocol::catalog::{GroupsSync, ProjectsSync, SessionUpdate, SyncResult};

fn vector(name: &str) -> String {
    let path = format!(
        "{}/tests/catalog-vectors/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(path).expect("vector")
}

fn golden<T: serde::Serialize + serde::de::DeserializeOwned>(name: &str) -> T {
    let text = vector(name);
    let value: T = serde_json::from_str(&text).expect("parse");
    let again = serde_json::to_value(&value).expect("encode");
    assert_eq!(again, serde_json::from_str::<Value>(&text).unwrap());
    value
}

#[test]
fn projects_sync_vector() {
    let sync: ProjectsSync = golden("catalog-projects-sync.json");
    assert_eq!(sync.host, "devbox");
    assert!(sync.full);
    assert_eq!(sync.projects[0].updated_at, 1_700_000_000_000);
}

#[test]
fn groups_sync_vector() {
    let sync: GroupsSync = golden("catalog-groups-sync.json");
    assert_eq!(sync.groups[0].project_id, "p1");
    assert_eq!(sync.groups[0].order, -2);
}

#[test]
fn session_update_distinguishes_unset_from_clear() {
    let update: SessionUpdate = golden("catalog-session-update.json");
    assert_eq!(update.project, Some(None));
    assert_eq!(update.group, None);
    assert_eq!(update.title.as_deref(), Some("renamed"));

    let set: SessionUpdate = serde_json::from_str(r#"{"id":"s1","group":"g1"}"#).unwrap();
    assert_eq!(set.group, Some(Some("g1".into())));
    assert_eq!(set.project, None);
}

#[test]
fn sync_result_shape() {
    let r = SyncResult {
        applied: 1,
        ignored: 2,
        rev: 3,
    };
    assert_eq!(
        serde_json::to_string(&r).unwrap(),
        r#"{"applied":1,"ignored":2,"rev":3}"#
    );
}
