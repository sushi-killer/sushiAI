//! Claude Code: the adapter, the argv that starts it and the `--settings` JSON
//! it carries. Hooks report its status, so it has no screen rules.

use std::path::Path;

use super::catalog::{self, Catalog, Facts, Model};
use super::launch::{AgentSession, LaunchError, LaunchParams};
use super::screen::Screen;
use super::{
    Capabilities, DisabledBy, Harness, Homes, LaunchFn, Scope, SkillConfigs, SkillFacts, SkillRoot,
    Source,
};

mod settings;

/// Public because `sushiai-daemon` builds its hook entries from it and the
/// integration tests check them; everything else goes through `launch`.
pub use settings::{claude_settings, hook_command};

pub(super) struct Claude;

/// Claude has no listing command; each alias resolves to that model's latest build.
static MODELS: &[(&str, &str, &str)] = &[
    ("fable", "Fable", "Most capable for hard work."),
    ("opus", "Opus", "Deep reasoning and long tasks."),
    ("sonnet", "Sonnet", "Balanced everyday coding."),
    ("haiku", "Haiku", "Fastest, for light work."),
];

/// The argv that starts Claude in a session. `build` has checked the gates.
fn launch(p: &LaunchParams) -> Result<Vec<String>, LaunchError> {
    let mut argv = vec!["claude".to_owned()];
    match &p.session {
        AgentSession::New(id) => argv.extend(["--session-id".to_owned(), (*id).to_owned()]),
        AgentSession::Resume(id) => argv.extend(["--resume".to_owned(), (*id).to_owned()]),
    }
    let mut settings = claude_settings(p.hook_bin, p.permission_wait_secs)?;
    if let Some(caller) = p.claude_settings {
        settings = settings::merge_claude_settings(settings, caller)?;
    }
    argv.extend(["--settings".to_owned(), settings.to_string()]);
    if p.headless {
        argv.push("-p".to_owned());
    }
    argv.extend(p.extra_args.iter().cloned());
    Ok(argv)
}

impl Harness for Claude {
    fn id(&self) -> &'static str {
        "claude"
    }

    fn label(&self) -> &'static str {
        "Claude Code"
    }

    fn provider(&self) -> Option<&'static str> {
        Some("Claude")
    }

    fn launch(&self) -> Option<LaunchFn> {
        Some(launch)
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            session_id: true,
            hooks_in_home: false,
            settings_json: true,
        }
    }

    /// Every alias takes every level; no default of ours, so the CLI's own stands.
    fn catalog(&self, _facts: &Facts) -> Catalog {
        let efforts = catalog::words(catalog::CLAUDE_EFFORTS);
        Catalog {
            id: self.id().to_owned(),
            label: self.label().to_owned(),
            models: MODELS
                .iter()
                .map(|(id, label, description)| Model {
                    id: (*id).to_owned(),
                    label: (*label).to_owned(),
                    description: (*description).to_owned(),
                    efforts: efforts.clone(),
                    default_effort: String::new(),
                    context: catalog::CLAUDE_CONTEXT,
                })
                .collect(),
            efforts: efforts.clone(),
            builder_efforts: efforts,
            permissions: catalog::words(catalog::PERMISSIONS),
            default_context: catalog::CLAUDE_CONTEXT,
            ..Catalog::default()
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::{find, HarnessRef};

    const BIN: &str = "/home/u/.sushiai/bin/sushiai";

    fn claude() -> HarnessRef {
        find("claude").expect("a registered agent")
    }

    fn argv(resume: bool, headless: bool) -> Vec<String> {
        let extra = vec!["--flag".to_string()];
        let session = if resume {
            AgentSession::Resume("abc-1")
        } else {
            AgentSession::New("abc-1")
        };
        crate::harness::launch::build(&LaunchParams {
            harness: claude(),
            hook_bin: BIN,
            socket: "/s",
            sushiai_session_id: "i",
            session_token: "t",
            session,
            extra_args: &extra,
            prompt: Some("hi"),
            permission_wait_secs: 600,
            claude_settings: None,
            headless,
        })
        .expect("a valid launch")
        .argv
    }

    /// Claude's argv with the long `--settings` JSON replaced.
    fn with_placeholder(resume: bool, headless: bool) -> Vec<String> {
        let mut v = argv(resume, headless);
        let at = v
            .iter()
            .position(|a| a == "--settings")
            .expect("a settings flag");
        v[at + 1] = "<settings>".into();
        v
    }

    fn words(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn claude_interactive_and_headless() {
        assert_eq!(
            with_placeholder(false, false),
            words(&[
                "claude",
                "--session-id",
                "abc-1",
                "--settings",
                "<settings>",
                "--flag",
                "--",
                "hi"
            ])
        );
        assert_eq!(
            with_placeholder(false, true),
            words(&[
                "claude",
                "--session-id",
                "abc-1",
                "--settings",
                "<settings>",
                "-p",
                "--flag",
                "--",
                "hi"
            ])
        );
        assert_eq!(
            with_placeholder(true, false),
            words(&[
                "claude",
                "--resume",
                "abc-1",
                "--settings",
                "<settings>",
                "--flag",
                "--",
                "hi"
            ])
        );
        assert_eq!(
            with_placeholder(true, true),
            words(&[
                "claude",
                "--resume",
                "abc-1",
                "--settings",
                "<settings>",
                "-p",
                "--flag",
                "--",
                "hi"
            ])
        );
    }
}
