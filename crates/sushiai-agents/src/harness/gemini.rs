//! Gemini CLI: no hooks, so its status comes from the screen.

use std::path::Path;

use super::screen::{Rule, Screen, ScreenMode, PERM};
use super::{DisabledBy, Harness, Homes, SkillConfigs, SkillFacts, SkillRoot};
use crate::status::Detected;

static RULES: &[Rule] = &[
    Rule {
        result: PERM,
        bottom: 40,
        all: &[&[
            "apply this change",
            "allow execution",
            "waiting for user confirmation",
            "do you want to proceed?",
        ]],
    },
    Rule {
        result: Detected::Working,
        bottom: 12,
        all: &[&["esc to cancel"]],
    },
];

pub(super) struct Gemini;

impl Harness for Gemini {
    fn id(&self) -> &'static str {
        "gemini"
    }

    fn label(&self) -> &'static str {
        // The CLI's own name, which the app shows for a terminal panel too.
        "Gemini CLI"
    }

    fn provider(&self) -> Option<&'static str> {
        None
    }

    fn process_name(&self) -> Option<&'static str> {
        Some(self.id())
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
        Some(Screen {
            mode: ScreenMode::TextOnScreen,
            rules: RULES,
        })
    }
}
