use super::super::Store;
use crate::policy::settings::Settings;

#[test]
fn settings_round_trips_and_defaults_when_absent() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let defaulted = store.load_settings().unwrap();
    assert_eq!(defaulted.max_attempts, 4);

    let custom = Settings {
        max_attempts: 7,
        ..Settings::default()
    };
    store.save_settings(&custom).unwrap();
    let loaded = store.load_settings().unwrap();
    assert_eq!(loaded.max_attempts, 7);
}

#[test]
fn settings_saved_before_presets_load_with_none() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut value = serde_json::to_value(Settings::default()).unwrap();
    let o = value.as_object_mut().unwrap();
    assert!(o.remove("presets").is_some());
    assert!(o.remove("repoPresets").is_some());
    std::fs::write(store.settings_path(), value.to_string()).unwrap();
    let loaded = store.load_settings().unwrap();
    assert!(loaded.presets.is_empty());
    assert!(loaded.repo_presets.is_empty());
    assert!(loaded.check_presets().is_ok());
}

#[test]
fn old_settings_naming_the_retired_haiku_route_load_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut value = serde_json::to_value(Settings::default()).unwrap();
    value["briefCheckRoute"] = serde_json::json!("claude-haiku");
    std::fs::write(store.settings_path(), value.to_string()).unwrap();
    let loaded = store.load_settings().unwrap();
    assert_eq!(loaded.brief_check_route, "claude-haiku");
    assert!(!loaded.routes.iter().any(|r| r.id == "claude-haiku"));
}
