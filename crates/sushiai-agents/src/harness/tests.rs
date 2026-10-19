//! The contract every agent in [`registry`] must keep. It is what makes a new
//! agent one file: these properties hold for it without new test code.

use std::collections::HashSet;

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

/// The launch id and the name are one fact: an adapter that can be launched
/// takes its id from [`id_of`], so the two cannot drift apart.
#[test]
fn a_launchable_agent_agrees_with_its_launch_id() {
    for harness in registry() {
        if let Some(agent) = harness.agent() {
            assert_eq!(harness.id(), id_of(agent), "{agent:?} id mismatch");
        }
    }
    let launchable: HashSet<&str> = registry()
        .iter()
        .filter_map(|h| h.agent())
        .map(|a| a.as_str())
        .collect();
    assert_eq!(launchable, HashSet::from(["claude", "codex"]));
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

fn agent(id: &str) -> HarnessRef {
    find(id).expect("a registered agent is findable")
}

fn homes() -> Homes {
    Homes::new(Path::new("/home"), None, None)
}
