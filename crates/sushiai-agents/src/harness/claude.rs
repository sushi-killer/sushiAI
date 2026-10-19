//! Claude Code. Hooks report its status, so it has no screen rules.

use std::path::Path;

use super::screen::Screen;
use super::{
    id_of, Agent, DisabledBy, Harness, Homes, Scope, SkillConfigs, SkillFacts, SkillRoot, Source,
};

pub(super) struct Claude;

impl Harness for Claude {
    fn id(&self) -> &'static str {
        id_of(Agent::Claude)
    }

    fn label(&self) -> &'static str {
        "Claude Code"
    }

    fn provider(&self) -> Option<&'static str> {
        Some("Claude")
    }

    fn agent(&self) -> Option<Agent> {
        Some(Agent::Claude)
    }

    fn process_name(&self) -> Option<&'static str> {
        Some(self.id())
    }

    fn home<'a>(&self, homes: &'a Homes) -> Option<&'a Path> {
        Some(&homes.claude)
    }

    fn skills_roots(&self, homes: &Homes, project: Option<&Path>) -> Vec<SkillRoot> {
        let mut roots = vec![SkillRoot {
            scope: Scope::User,
            dir: homes.claude.join("skills"),
            hidden: false,
        }];
        if let Some(project) = project {
            roots.push(SkillRoot {
                scope: Scope::Project,
                dir: project.join(".claude").join("skills"),
                hidden: false,
            });
        }
        roots
    }

    fn disabled_by(&self, skill: &SkillFacts, configs: &SkillConfigs) -> Option<DisabledBy> {
        (skill.source == Source::Local && configs.claude.contains(skill.name))
            .then_some(DisabledBy::SkillOverride)
    }

    fn screen(&self) -> Option<Screen> {
        None
    }
}
