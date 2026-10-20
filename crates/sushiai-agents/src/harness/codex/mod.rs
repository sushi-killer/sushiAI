//! Codex: the adapter, the argv that starts it, its `hooks.json` and its hook
//! trust in `config.toml`. Its hooks miss some dialogs, so its screen is read
//! too: a Codex dialog only ever blocks, and a quiet screen says nothing.

use std::path::{Path, PathBuf};

use super::catalog::{self, Catalog, Facts};
use super::launch::{shell_quote, AgentSession, LaunchError, LaunchParams};
use super::screen::{Rule, Screen, ScreenMode, INPUT, PERM};
use super::{
    Capabilities, DisabledBy, Harness, Homes, LaunchFn, Scope, SkillConfigs, SkillFacts, SkillRoot,
};

pub mod hooks;
pub mod trust;

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

/// The Codex flag (0.160 and later) that keeps the session in the `codex`
/// process itself. Codex otherwise runs interactive turns in a shared
/// background app server per CODEX_HOME, whose hooks inherit that server's
/// environment and would miss our session variables.
const CODEX_NO_DAEMON: &str = "--no-daemon";

/// The hook command line: `<bin> hook <event> --agent codex`. A Codex hook
/// entry names its agent in argv, unlike a Claude one.
pub fn hook_command(bin: &str, event: &str) -> String {
    format!("{} hook {} --agent codex", shell_quote(bin), event)
}

/// The argv that starts Codex in a session. `build` has checked the gates.
fn launch(p: &LaunchParams) -> Result<Vec<String>, LaunchError> {
    let mut argv = vec!["codex".to_owned()];
    if p.headless {
        argv.push("exec".to_owned());
    }
    if let AgentSession::Resume(id) = &p.session {
        argv.extend(["resume".to_owned(), (*id).to_owned()]);
    }
    if p.headless {
        argv.extend(["--json".to_owned(), "--skip-git-repo-check".to_owned()]);
        argv.extend(p.extra_args.iter().cloned());
        return Ok(argv);
    }
    argv.push(CODEX_NO_DAEMON.to_owned());
    argv.extend(p.extra_args.iter().cloned());
    Ok(argv)
}

impl Harness for Codex {
    fn id(&self) -> &'static str {
        "codex"
    }

    fn label(&self) -> &'static str {
        "Codex"
    }

    fn provider(&self) -> Option<&'static str> {
        Some("Codex")
    }

    fn launch(&self) -> Option<LaunchFn> {
        Some(launch)
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            session_id: false,
            hooks_in_home: true,
            settings_json: false,
        }
    }

    /// Models come from the CLI's own cache and the default from `config.toml`;
    /// both are per host, so the caller passes their text in.
    fn catalog(&self, facts: &Facts) -> Catalog {
        let models = catalog::codex_models(facts.codex_cache.as_deref(), catalog::CODEX_EFFORTS);
        let (default_model, default_model_label, default_context) =
            catalog::codex_default(facts.codex_config.as_deref(), &models);
        Catalog {
            id: self.id().to_owned(),
            label: self.label().to_owned(),
            models,
            efforts: catalog::words(catalog::CODEX_EFFORTS),
            builder_efforts: catalog::words(catalog::BUILDER_EFFORTS),
            permissions: catalog::words(catalog::PERMISSIONS),
            default_model,
            default_model_label,
            default_effort: catalog::toml_root(
                facts.codex_config.as_deref(),
                "model_reasoning_effort",
            ),
            default_context,
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::{find, HarnessRef};

    const BIN: &str = "/home/u/.sushiai/bin/sushiai";

    fn codex() -> HarnessRef {
        find("codex").expect("a registered agent")
    }

    fn argv(resume: bool, headless: bool) -> Vec<String> {
        let extra = vec!["--flag".to_string()];
        let session = if resume {
            AgentSession::Resume("abc-1")
        } else {
            AgentSession::New("abc-1")
        };
        crate::harness::launch::build(&LaunchParams {
            harness: codex(),
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

    fn words(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn codex_interactive_and_headless() {
        assert_eq!(
            argv(false, false),
            words(&["codex", "--no-daemon", "--flag", "--", "hi"])
        );
        assert_eq!(
            argv(true, false),
            words(&[
                "codex",
                "resume",
                "abc-1",
                "--no-daemon",
                "--flag",
                "--",
                "hi"
            ])
        );
        assert_eq!(
            argv(false, true),
            words(&[
                "codex",
                "exec",
                "--json",
                "--skip-git-repo-check",
                "--flag",
                "--",
                "hi"
            ])
        );
        assert_eq!(
            argv(true, true),
            words(&[
                "codex",
                "exec",
                "resume",
                "abc-1",
                "--json",
                "--skip-git-repo-check",
                "--flag",
                "--",
                "hi"
            ])
        );
    }
}
