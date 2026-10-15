use super::{compiled, Catalog};

const YAML: &str = include_str!("../../../../pipelines/catalog.yaml");

#[test]
fn the_compiled_catalog_preserves_rpc_order_labels_and_kinds() {
    let actual = compiled()
        .stages()
        .iter()
        .map(|stage| (stage.id(), stage.label(), stage.kind()))
        .collect::<Vec<_>>();
    assert_eq!(
        actual,
        [
            ("brief", "Brief", "main"),
            ("plan", "Plan", "main"),
            ("brief-check", "Brief check", "main"),
            ("base-checks", "Base checks", "main"),
            ("implement", "Implement", "main"),
            ("checks", "Checks", "main"),
            ("held-out", "Held-out check", "main"),
            ("review", "Review", "main"),
            ("judge", "Judge", "main"),
            ("land", "Land", "main"),
            ("advisor", "Advisor", "side"),
            ("question", "Question", "side"),
        ]
    );
}

#[test]
fn catalog_labels_come_from_the_decoded_definition() {
    let catalog = Catalog::parse(&YAML.replace("label: Brief,", "label: Revised brief,")).unwrap();
    assert_eq!(catalog.stages()[0].label(), "Revised brief");
}

#[test]
fn malformed_unknown_missing_and_duplicate_catalog_entries_are_rejected() {
    let edge = "  - { id: brief, label: Brief, kind: main }\n";
    for text in [
        YAML.replace(edge, ""),
        format!("{YAML}{edge}"),
        YAML.replace("id: brief,", "id: unknown,"),
        YAML.replace(
            "id: brief, label: Brief, kind: main",
            "id: brief, label: Brief, kind: side",
        ),
        YAML.replace("label: Brief,", "label: '',"),
        format!("{YAML}command: something\n"),
    ] {
        assert!(Catalog::parse(&text).is_err());
    }
}
