//! Codex. Its hooks miss some dialogs, so its screen is read too: a Codex
//! dialog only ever blocks, and a quiet screen says nothing.

use std::path::{Path, PathBuf};

use super::screen::{Rule, Screen, ScreenMode, INPUT, PERM};
use super::{id_of, Agent, DisabledBy, Harness, Homes, Scope, SkillConfigs, SkillFacts, SkillRoot};

static RULES: &[Rule] = &[
    Rule {
        result: PERM,
        bottom: 15,
        all: &[&["allow command?"]],
    },
    Rule {
        result: INPUT,
        bottom: 15,
        all: &[&[
            "press enter to confirm or esc to cancel",
            "enter to submit answer",
            "do you trust the contents of this directory",
            "update available!",
        ]],
    },
];

pub(super) struct Codex;

impl Harness for Codex {
    fn id(&self) -> &'static str {
        id_of(Agent::Codex)
    }

    fn label(&self) -> &'static str {
        "Codex"
    }

    fn provider(&self) -> Option<&'static str> {
        Some("Codex")
    }

    fn agent(&self) -> Option<Agent> {
        Some(Agent::Codex)
    }

    fn process_name(&self) -> Option<&'static str> {
        Some(self.id())
    }

    fn home<'a>(&self, homes: &'a Homes) -> Option<&'a Path> {
        Some(&homes.codex)
    }

    fn skills_roots(&self, homes: &Homes, project: Option<&Path>) -> Vec<SkillRoot> {
        let root = |scope, dir: PathBuf| SkillRoot {
            scope,
            dir,
            hidden: true,
        };
        // Codex reads both user folders; the first is where a skill is installed. Its `.system`
        // folder counts at the top of every root it reads, not only its own home.
        let mut roots = vec![
            root(Scope::User, homes.codex.join("skills")),
            root(Scope::User, homes.home.join(".agents").join("skills")),
        ];
        if let Some(project) = project {
            roots.push(root(Scope::Project, project.join(".agents").join("skills")));
        }
        roots
    }

    fn disabled_by(&self, skill: &SkillFacts, configs: &SkillConfigs) -> Option<DisabledBy> {
        let entry = configs
            .codex
            .get(Path::new(skill.file))
            .or_else(|| configs.codex.get(skill.real));
        (entry == Some(&false)).then_some(DisabledBy::SkillConfig)
    }

    fn screen(&self) -> Option<Screen> {
        Some(Screen {
            mode: ScreenMode::DialogOnly,
            rules: RULES,
        })
    }
}
