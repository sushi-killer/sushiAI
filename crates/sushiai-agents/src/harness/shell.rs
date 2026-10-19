//! Shell: a session that runs a plain command, with no agent behind it.
//!
//! This is the template to copy for a new agent: id and label are what every
//! agent must have, everything else is added as it is needed. It has no
//! production caller yet — a terminal session is `session.create` with `cmd`
//! and no agent name.

use std::path::Path;

use super::screen::Screen;
use super::{Agent, DisabledBy, Harness, Homes, SkillConfigs, SkillFacts, SkillRoot};

pub(super) struct Shell;

impl Harness for Shell {
    fn id(&self) -> &'static str {
        "shell"
    }

    fn label(&self) -> &'static str {
        "Shell"
    }

    fn provider(&self) -> Option<&'static str> {
        None
    }

    fn agent(&self) -> Option<Agent> {
        None
    }

    fn process_name(&self) -> Option<&'static str> {
        None
    }

    fn home<'a>(&self, _homes: &'a Homes) -> Option<&'a Path> {
        None
    }

    fn skills_roots(&self, _homes: &Homes, _project: Option<&Path>) -> Vec<SkillRoot> {
        Vec::new()
    }

    fn disabled_by(&self, _skill: &SkillFacts, _configs: &SkillConfigs) -> Option<DisabledBy> {
        None
    }

    fn screen(&self) -> Option<Screen> {
        None
    }
}
