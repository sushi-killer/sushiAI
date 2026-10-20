//! The contract every agent in [`registry`] must keep. It is what makes a new
//! agent one file: these properties hold for it without new test code.

use std::collections::HashSet;

use super::catalog::Facts;
use super::*;

#[test]
fn every_agent_has_a_unique_id_and_a_label() {
    let mut ids = HashSet::new();
    for harness in registry() {
        assert!(!harness.id().is_empty(), "{harness:?} has no id");
        assert!(ids.insert(harness.id()), "duplicate id {}", harness.id());
        assert!(!harness.label().is_empty(), "{} has no label", harness.id());
    }
}

#[test]
fn an_unknown_name_is_not_an_agent() {
    assert!(find("no-such-agent").is_none());
    assert!(find("").is_none());
}

/// An agent the daemon can launch is one `foreground` also sees as a process,
/// and there are exactly two of them.
#[test]
fn the_launchable_agents_are_claude_and_codex() {
    let launchable: HashSet<&str> = registry()
        .iter()
        .filter(|h| h.launch().is_some())
        .map(|h| h.id())
        .collect();
    assert_eq!(launchable, HashSet::from(["claude", "codex"]));
    for harness in registry() {
        if harness.launch().is_some() {
            assert!(
                harness.process_name().is_some(),
                "{} can be launched but has no process name",
                harness.id()
            );
        }
    }
}

/// What replaced `enum Agent`: the capabilities the daemon asks about. A
/// launchable agent with all of them off would break session minting and the
/// `claudeSettings` gate in silence.
#[test]
fn the_two_launchable_agents_declare_their_capabilities() {
    let claude = agent("claude").capabilities();
    assert!(claude.session_id && claude.settings_json && !claude.hooks_in_home);
    let codex = agent("codex").capabilities();
    assert!(codex.hooks_in_home && !codex.session_id && !codex.settings_json);
    for harness in registry() {
        let caps = harness.capabilities();
        assert!(
            harness.launch().is_some()
                || (!caps.session_id && !caps.hooks_in_home && !caps.settings_json),
            "{} has capabilities but cannot be launched",
            harness.id()
        );
    }
}

/// `foreground` matches a hand-started process by these names and no others.
/// They are the binaries a person types, so they are not always the agent's id:
/// `cursor` is started as `cursor-agent`.
#[test]
fn the_process_names_are_the_agents_that_have_one() {
    let names: Vec<&str> = process_names().collect();
    assert_eq!(names, ["claude", "codex", "gemini", "cursor-agent"]);
    for harness in registry() {
        if let Some(name) = harness.process_name() {
            assert!(!name.is_empty(), "{}: an empty process name", harness.id());
        }
    }
}

/// An agent that declares a screen must declare rules for it.
#[test]
fn a_screen_comes_with_rules() {
    for harness in registry() {
        if let Some(screen) = harness.screen() {
            assert!(
                !screen.rules.is_empty(),
                "{} declares a screen with no rules",
                harness.id()
            );
        }
    }
}

/// Only an agent with a config home has skills, in both directions: the daemon
/// and `skills` decide by `home()`.
#[test]
fn skills_belong_to_the_agents_that_have_a_home() {
    for harness in registry() {
        let roots = harness.skills_roots(&homes(), Some(Path::new("/checkout")));
        assert_eq!(
            roots.is_empty(),
            harness.home(&homes()).is_none(),
            "{}: folders and home disagree",
            harness.id()
        );
        assert_eq!(
            harness.provider().is_some(),
            harness.home(&homes()).is_some(),
            "{}: provider and home disagree",
            harness.id()
        );
        for root in &roots {
            assert!(root.dir.is_absolute(), "{}: relative root", harness.id());
        }
    }
}

/// The label a skill row prints is the string `sushiai skills --json` has
/// always printed, and the provider the app shows beside it.
#[test]
fn the_skills_agents_keep_their_labels() {
    assert_eq!(agent("claude").label(), "Claude Code");
    assert_eq!(agent("claude").provider(), Some("Claude"));
    assert_eq!(agent("codex").label(), "Codex");
    assert_eq!(agent("codex").provider(), Some("Codex"));
    assert!(agent("shell").provider().is_none(), "shell has no skills");
}

/// User folders first, in registry order, then the project's — the order the
/// scan and the install depend on.
#[test]
fn the_scan_order_is_user_folders_then_the_project() {
    let homes = homes();
    let project = Path::new("/checkout");
    for harness in registry() {
        let mut seen_project = false;
        for root in harness.skills_roots(&homes, Some(project)) {
            match root.scope {
                Scope::User => assert!(
                    !seen_project,
                    "{}: a user folder after a project one",
                    harness.id()
                ),
                Scope::Project => seen_project = true,
            }
        }
    }
}

/// Codex reads a `.system` folder at the top of every root it scans, not only
/// its own home; no other agent keeps a hidden top-level folder.
#[test]
fn codex_scans_a_hidden_system_folder_at_every_root() {
    let homes = homes();
    let project = Path::new("/checkout");
    for harness in registry() {
        let roots = harness.skills_roots(&homes, Some(project));
        let hidden: Vec<&Path> = roots
            .iter()
            .filter(|r| r.hidden)
            .map(|r| r.dir.as_path())
            .collect();
        if harness.id() == "codex" {
            assert_eq!(
                hidden,
                [
                    homes.codex.join("skills"),
                    homes.home.join(".agents").join("skills"),
                    project.join(".agents").join("skills"),
                ]
            );
        } else {
            assert!(hidden.is_empty(), "{} hides a folder", harness.id());
        }
    }
}

/// Every agent answers a catalog, named like itself; the levels the routine
/// builder offers are always a subset of the chat's, which is the drift this
/// table exists to stop.
#[test]
fn every_agent_has_a_catalog_and_the_builder_set_is_smaller() {
    let catalogs = super::catalogs(&Facts::default());
    assert_eq!(catalogs.len(), registry().len());
    for (catalog, harness) in catalogs.iter().zip(registry()) {
        assert_eq!(catalog.id, harness.id());
        assert_eq!(catalog.label, harness.label());
        for level in &catalog.builder_efforts {
            assert!(
                catalog.efforts.contains(level),
                "{}: builder level {level} is not a chat level",
                catalog.id
            );
        }
        for model in &catalog.models {
            for level in &model.efforts {
                assert!(
                    catalog.efforts.contains(level),
                    "{}: model {} offers {level}",
                    catalog.id,
                    model.id
                );
            }
        }
    }
}

/// An agent with no models of its own offers nothing: only its label.
#[test]
fn an_agent_without_models_offers_no_levels() {
    for id in ["gemini", "cursor", "shell"] {
        let catalog = agent(id).catalog(&Facts::default());
        assert!(catalog.models.is_empty(), "{id} lists models");
        assert!(catalog.efforts.is_empty(), "{id} lists levels");
        assert!(catalog.permissions.is_empty(), "{id} lists modes");
        assert_eq!(catalog.default_model, "", "{id} has a default model");
    }
}

/// Claude's four aliases, each taking every level, and no default of its own:
/// the CLI's own default stands until the caller names a model.
#[test]
fn claudes_catalog_is_the_four_aliases() {
    let catalog = agent("claude").catalog(&Facts::default());
    let ids: Vec<&str> = catalog.models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["fable", "opus", "sonnet", "haiku"]);
    assert_eq!(catalog.efforts, ["low", "medium", "high", "xhigh", "max"]);
    assert_eq!(catalog.builder_efforts, catalog.efforts);
    assert_eq!(
        catalog.permissions,
        ["default", "acceptEdits", "plan", "bypassPermissions"]
    );
    assert_eq!(catalog.default_context, 200_000);
    assert_eq!(catalog.default_model, "");
    for model in &catalog.models {
        assert_eq!(model.efforts, catalog.efforts);
        assert_eq!(model.context, 200_000);
        assert!(!model.description.is_empty(), "{} has no note", model.id);
    }
}

/// Codex takes `ultra` on top of Claude's levels, and its builder stays on the
/// three both CLIs share. Without its cache it still names its levels.
#[test]
fn codexs_catalog_adds_ultra_and_keeps_the_builder_at_three() {
    let catalog = agent("codex").catalog(&Facts::default());
    assert_eq!(
        catalog.efforts,
        ["low", "medium", "high", "xhigh", "max", "ultra"]
    );
    assert_eq!(catalog.builder_efforts, ["low", "medium", "high"]);
    assert!(catalog.models.is_empty(), "no cache, no models");
    assert_eq!(catalog.default_model, "");
}

/// Codex's cache and config reach the catalog through `Facts`, and the default
/// model is resolved against the listed ones.
#[test]
fn codexs_facts_become_its_models_and_default() {
    let facts = Facts {
        codex_cache: Some(
            r#"{"models":[{"slug":"gpt-x","display_name":"GPT-X","visibility":"list",
                 "priority":1,"context_window":400000,"default_reasoning_level":"high",
                 "supported_reasoning_levels":[{"effort":"low"},{"effort":"high"}]}]}"#
                .into(),
        ),
        codex_config: Some("model = \"gpt-x\"\nmodel_reasoning_effort = \"high\"\n".into()),
    };
    let catalog = agent("codex").catalog(&facts);
    assert_eq!(catalog.models.len(), 1);
    assert_eq!(catalog.models[0].label, "GPT-X");
    assert_eq!(catalog.default_model, "gpt-x");
    assert_eq!(catalog.default_model_label, "GPT-X");
    assert_eq!(catalog.default_effort, "high");
    assert_eq!(catalog.default_context, 400_000);
    // A slug the cache does not list is still the id, and shows itself.
    let unlisted = Facts {
        codex_config: Some("model = \"gpt-unknown\"\n".into()),
        ..facts.clone()
    };
    let catalog = agent("codex").catalog(&unlisted);
    assert_eq!(catalog.default_model, "gpt-unknown");
    assert_eq!(catalog.default_model_label, "gpt-unknown");
    assert_eq!(catalog.default_context, 0);
}

/// The names the desktop reads. They are camelCase and complete: the app and
/// the CLI decode this same JSON.
#[test]
fn the_catalog_wire_names_are_camel_case() {
    let value = serde_json::to_value(super::catalogs(&Facts::default())).expect("it serializes");
    let claude = &value[0];
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
        assert!(claude.get(key).is_some(), "claude has no {key}");
    }
    for key in [
        "id",
        "label",
        "description",
        "efforts",
        "defaultEffort",
        "context",
    ] {
        assert!(
            claude["models"][0].get(key).is_some(),
            "a model has no {key}"
        );
    }
}

fn agent(id: &str) -> HarnessRef {
    find(id).expect("a registered agent is findable")
}

fn homes() -> Homes {
    Homes::new(Path::new("/home"), None, None)
}
