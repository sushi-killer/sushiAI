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
#[test]
fn the_process_names_are_the_agents_that_have_one() {
    let names: Vec<&str> = process_names().collect();
    assert_eq!(names, ["claude", "codex", "gemini", "cursor-agent"]);
    for harness in registry() {
        if let Some(name) = harness.process_name() {
            assert_eq!(name, harness.id(), "a process name is the agent's id");
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

/// The whole catalog of every registered agent, as the wire carries it: the one
/// test copy of the values, so a dropped level or a renamed key fails here.
#[test]
fn the_catalog_of_every_agent_is_this() {
    let claude_levels = ["low", "medium", "high", "xhigh", "max"];
    let modes = ["default", "acceptEdits", "plan", "bypassPermissions"];
    let model = |id: &str, label: &str, description: &str| {
        serde_json::json!({
            "id": id, "label": label, "description": description,
            "efforts": claude_levels, "defaultEffort": "", "context": 200_000,
        })
    };
    let none = |id: &str, label: &str| {
        serde_json::json!({
            "id": id, "label": label, "models": [], "efforts": [], "builderEfforts": [],
            "permissions": [], "defaultModel": "", "defaultModelLabel": "",
            "defaultEffort": "", "defaultContext": 0,
        })
    };
    let expected = serde_json::json!([
        {
            "id": "claude", "label": "Claude Code",
            "models": [
                model("fable", "Fable", "Most capable for hard work."),
                model("opus", "Opus", "Deep reasoning and long tasks."),
                model("sonnet", "Sonnet", "Balanced everyday coding."),
                model("haiku", "Haiku", "Fastest, for light work."),
            ],
            "efforts": claude_levels, "builderEfforts": claude_levels,
            "permissions": modes, "defaultModel": "", "defaultModelLabel": "",
            "defaultEffort": "", "defaultContext": 200_000,
        },
        {
            "id": "codex", "label": "Codex", "models": [],
            "efforts": ["low", "medium", "high", "xhigh", "max", "ultra"],
            "builderEfforts": ["low", "medium", "high"],
            "permissions": modes, "defaultModel": "", "defaultModelLabel": "",
            "defaultEffort": "", "defaultContext": 0,
        },
        none("gemini", "Gemini CLI"),
        none("cursor-agent", "Cursor Agent"),
        none("shell", "Shell"),
    ]);
    assert_eq!(
        serde_json::to_value(super::catalogs(&Facts::default())).expect("it serializes"),
        expected
    );
}

/// Codex's cache and config reach the catalog through `Facts`, and the default
/// model is resolved against the listed ones.
#[test]
fn codexs_facts_become_its_models_and_default() {
    let cache = r#"{"models":[{"slug":"gpt-x","display_name":"GPT-X","visibility":"list",
        "priority":1,"context_window":400000,"default_reasoning_level":"high",
        "supported_reasoning_levels":[{"effort":"low"},{"effort":"high"}]}]}"#;
    let catalog = |config: &str| {
        agent("codex").catalog(&Facts {
            codex_cache: Some(cache.into()),
            codex_config: Some(config.into()),
        })
    };
    let listed = catalog("model = \"gpt-x\"\nmodel_reasoning_effort = \"high\"\n");
    assert_eq!(listed.models[0].label, "GPT-X");
    assert_eq!(
        (
            listed.default_model.as_str(),
            listed.default_model_label.as_str()
        ),
        ("gpt-x", "GPT-X")
    );
    assert_eq!(
        (listed.default_effort.as_str(), listed.default_context),
        ("high", 400_000)
    );
    // A slug the cache does not list is still the id, and shows itself.
    let unlisted = catalog("model = \"gpt-unknown\"\n");
    assert_eq!(unlisted.default_model_label, "gpt-unknown");
    assert_eq!(unlisted.default_context, 0);
}

fn agent(id: &str) -> HarnessRef {
    find(id).expect("a registered agent is findable")
}

fn homes() -> Homes {
    Homes::new(Path::new("/home"), None, None)
}
