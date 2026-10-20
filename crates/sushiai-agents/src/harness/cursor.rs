//! cursor-agent: no hooks, so its status comes from the screen.

use std::path::Path;

use super::screen::{Rule, Screen, ScreenMode, PERM};
use super::{DisabledBy, Harness, Homes, SkillConfigs, SkillFacts, SkillRoot};
use crate::status::Detected;

static RULES: &[Rule] = &[
    Rule {
        result: PERM,
        bottom: 40,
        all: &[&["write to this file?"], &["proceed (y)"]],
    },
    Rule {
        result: PERM,
        bottom: 40,
        all: &[
            &["waiting for approval", "run this command?"],
            &["run (once) (y)", "skip (esc or n)"],
        ],
    },
    Rule {
        result: PERM,
        bottom: 40,
        all: &[&["keep (n)"]],
    },
    Rule {
        result: Detected::Working,
        bottom: 6,
        all: &[&["ctrl+c to stop"]],
    },
];

pub(super) struct Cursor;

impl Harness for Cursor {
    fn id(&self) -> &'static str {
        "cursor-agent"
    }

    fn label(&self) -> &'static str {
        "Cursor Agent"
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
