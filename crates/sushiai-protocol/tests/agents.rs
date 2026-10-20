//! The wire shape of `agents.catalog`. The desktop, the link and the CLI decode
//! this same JSON, so a renamed key is a broken client.

use serde_json::json;
use sushiai_protocol::agents::AgentCatalog;
use sushiai_protocol::CAPABILITIES;

#[test]
fn the_catalog_carries_every_field_a_client_reads() {
    let catalog: AgentCatalog = serde_json::from_value(json!({
        "id": "claude",
        "label": "Claude Code",
        "models": [{
            "id": "opus",
            "label": "Opus",
            "description": "Deep reasoning and long tasks.",
            "efforts": ["low", "high"],
            "defaultEffort": "high",
            "context": 200000,
        }],
        "efforts": ["low", "high"],
        "builderEfforts": ["low"],
        "permissions": ["default", "plan"],
        "defaultModel": "opus",
        "defaultModelLabel": "Opus",
        "defaultEffort": "high",
        "defaultContext": 200000,
    }))
    .expect("parse");
    assert_eq!(catalog.models[0].id, "opus");
    assert_eq!(catalog.models[0].default_effort, "high");
    assert_eq!(catalog.builder_efforts, ["low"]);
    assert_eq!(catalog.default_model_label, "Opus");
    // And back out, with the same names.
    let back = serde_json::to_value(&catalog).expect("serialize");
    for key in [
        "id",
        "label",
        "models",
        "efforts",
        "builderEfforts",
        "permissions",
        "defaultModel",
        "defaultModelLabel",
        "defaultEffort",
        "defaultContext",
    ] {
        assert!(back.get(key).is_some(), "no {key}");
    }
    assert!(back["models"][0].get("defaultEffort").is_some());
}

/// The method is advertised, so a client can tell a daemon that serves it from
/// one that does not.
#[test]
fn the_capability_is_advertised() {
    assert!(CAPABILITIES.contains(&"agentsCatalog"));
}
