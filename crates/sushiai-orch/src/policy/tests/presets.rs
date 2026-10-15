use crate::policy::presets::{Preset, PresetValues};
use crate::policy::settings::Settings;
use crate::policy::variant::Variant;

#[test]
fn preset_apply_copies_judge_route_to_variant() {
    let values = PresetValues {
        judge: Some("claude-opus".into()),
        ..PresetValues::default()
    };
    let mut variant = Variant::default();
    values.apply(&mut variant);
    assert_eq!(variant.judge_route.as_deref(), Some("claude-opus"));
}

#[test]
fn a_preset_applies_to_a_variant_like_the_equivalent_fields() {
    let mut v = Variant::default();
    Preset::builtins()[0].values.apply(&mut v);
    assert_eq!(v.brief_check_route.as_deref(), Some(""));
    assert_eq!(v.max_attempts, Some(2));
    assert!(!v.advisor && !v.held_out);
    // Careful hands the brief check back to the settings' route.
    Preset::builtins()[1].values.apply(&mut v);
    assert_eq!(v.brief_check_route, None);
    assert!(v.advisor && v.held_out);
    let s = Settings::default();
    assert_eq!(v.max_attempts(&s), 4);
    assert_eq!(v.review_route_id(&s), "auto");
    assert_eq!(v.brief_check_route_id(&s), "auto");
}

#[test]
fn presets_do_not_restore_the_retired_check_switch() {
    for preset in Preset::builtins() {
        let mut variant = Variant::default();
        preset.values.apply(&mut variant);
        let variant = serde_json::to_value(variant).unwrap();
        assert!(variant.get("groundedChecks").is_none());
    }
}
