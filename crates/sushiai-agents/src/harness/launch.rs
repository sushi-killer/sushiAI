//! Build the argv and the child variables that start an agent in a session.
//!
//! The gates here are shared by every agent: the hook binary is absolute, a
//! caller's extra arguments cannot override a flag the daemon owns, and the
//! session id is safe to put in argv. The argv itself belongs to the agent
//! ([`Harness::launch`](super::Harness::launch)): Claude gets exactly one
//! `--settings` JSON holding the status hooks and the permission approver (a
//! second `--settings` flag is undocumented); Codex gets no per-launch hooks,
//! its entries live in `~/.codex/hooks.json` (see `super::codex::hooks`) and
//! stay inert without the session variables.

use serde_json::{Map, Value};

use super::HarnessRef;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum LaunchError {
    #[error("hook binary path must be absolute, got {0:?}")]
    RelativeHookPath(String),
    #[error("hook binary must be named `sushiai`, got {0:?}")]
    HookBinName(String),
    #[error("extra argument {0:?} would override what the daemon sets")]
    ForbiddenArg(String),
    #[error("invalid agent session id {0:?}")]
    BadSessionId(String),
    #[error("{0} is not an agent the daemon launches")]
    NotLaunchable(&'static str),
    #[error("claudeSettings must not contain a `hooks` key: the daemon owns the hooks")]
    SettingsHooks,
    #[error("claudeSettings key {0:?} is not allowed")]
    SettingsKey(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentSession<'a> {
    /// A new session. Claude is told this uuid (`--session-id`); Codex mints
    /// its own id and reports it through `SessionStart`.
    New(&'a str),
    Resume(&'a str),
}

#[derive(Debug, Clone)]
pub struct LaunchParams<'a> {
    /// The agent to start. Only a harness with a `launch()` builder gets here.
    pub harness: HarnessRef,
    /// Absolute path of `~/.sushiai/bin/sushiai`, expanded by the caller.
    pub hook_bin: &'a str,
    pub socket: &'a str,
    pub sushiai_session_id: &'a str,
    pub session_token: &'a str,
    pub session: AgentSession<'a>,
    /// Flags passed through before the prompt. A flag that takes several
    /// values (`--mcp-config`) must not be last when a prompt follows.
    pub extra_args: &'a [String],
    pub prompt: Option<&'a str>,
    /// How long the approver may wait for the owner (spec default 600).
    pub permission_wait_secs: u64,
    /// Extra top-level keys for Claude's `--settings` (may hold secrets). Our `hooks` win.
    pub claude_settings: Option<&'a Map<String, Value>>,
    /// Run one turn without a terminal UI. Claude gets `-p`. Codex runs `codex exec`, which
    /// has no `--no-daemon`, and prints JSON lines (`--json`); `--skip-git-repo-check` lets
    /// it start outside a git repository.
    pub headless: bool,
    /// Start with no permission prompts and no sandbox, by the agent's own flags
    /// ([`Capabilities::full_access`](super::Capabilities::full_access)). An agent without
    /// such flags ignores it.
    pub full_access: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchSpec {
    pub argv: Vec<String>,
    /// Variables to set on the PTY child (the hook reads them).
    pub vars: Vec<(String, String)>,
}

/// Quote one word for `sh -c`. Safe words pass through unchanged.
pub fn shell_quote(s: &str) -> String {
    let safe = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-./,:=@%+".contains(c));
    if safe {
        s.to_owned()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// Hook timeout that covers the approver's wait plus a margin. One owner for
/// Claude settings and the Codex entry.
pub fn permission_timeout(wait_secs: u64) -> u64 {
    wait_secs.saturating_add(30)
}

/// A uuid or a plain token (`[A-Za-z0-9._-]+`) that does not start with `-`.
fn valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('-')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

/// A flag a caller may not pass through: it would override what the daemon
/// sets. The set is Claude's (`--settings`, `--bare`, `--session-id`);
/// `--resume`/`-r` is both agents'. A third agent inherits the whole set.
fn check_extra_args(harness: HarnessRef, args: &[String]) -> Result<(), LaunchError> {
    // Full access has one way in, `LaunchParams::full_access`, so a caller cannot widen a
    // session by an argument the owner never ticked. `--permission-mode dontAsk` stays.
    let own = harness.capabilities().full_access;
    // `--flag=value` and `--flag value` are the same words.
    let words: Vec<&str> = args
        .iter()
        .flat_map(|a| {
            a.split_once('=')
                .map_or(vec![a.as_str()], |(n, v)| vec![n, v])
        })
        .collect();
    let widened = words.contains(&"--dangerously-skip-permissions")
        || (!own.is_empty() && words.windows(own.len()).any(|w| w == own));
    if widened {
        let flag = args.iter().find(|a| {
            let name = a.split('=').next().unwrap_or(a);
            name == "--dangerously-skip-permissions" || own.contains(&name)
        });
        return Err(LaunchError::ForbiddenArg(
            flag.cloned().unwrap_or_else(|| own.join(" ")),
        ));
    }
    for a in args {
        let name = a.split('=').next().unwrap_or(a);
        if matches!(
            name,
            "--settings" | "--bare" | "--session-id" | "--resume" | "-r"
        ) {
            return Err(LaunchError::ForbiddenArg(a.clone()));
        }
    }
    Ok(())
}

pub(crate) fn require_absolute(bin: &str) -> Result<(), LaunchError> {
    if bin.starts_with('/') {
        Ok(())
    } else {
        Err(LaunchError::RelativeHookPath(bin.to_owned()))
    }
}

pub fn build(p: &LaunchParams) -> Result<LaunchSpec, LaunchError> {
    require_absolute(p.hook_bin)?;
    check_extra_args(p.harness, p.extra_args)?;
    let id = match &p.session {
        AgentSession::New(id) | AgentSession::Resume(id) => *id,
    };
    // A resume names an id whatever the agent does with a new one.
    let id_used =
        p.harness.capabilities().session_id || matches!(p.session, AgentSession::Resume(_));
    if id_used && !valid_session_id(id) {
        return Err(LaunchError::BadSessionId(id.to_owned()));
    }
    let Some(launch) = p.harness.launch() else {
        return Err(LaunchError::NotLaunchable(p.harness.id()));
    };
    Ok(finish(p, launch(p)?))
}

/// The prompt and the child variables every agent's argv ends with. The full-access flags go
/// in front of the prompt, where a flag still parses.
fn finish(p: &LaunchParams, mut argv: Vec<String>) -> LaunchSpec {
    if p.full_access {
        argv.extend(
            p.harness
                .capabilities()
                .full_access
                .iter()
                .map(|f| (*f).to_owned()),
        );
    }
    if let Some(prompt) = p.prompt {
        argv.push("--".into());
        argv.push(prompt.into());
    }
    let vars = vec![
        ("SUSHIAI_SOCKET".into(), p.socket.into()),
        ("SUSHIAI_SESSION_ID".into(), p.sushiai_session_id.into()),
        ("SUSHIAI_SESSION_TOKEN".into(), p.session_token.into()),
        ("SUSHIAI_AGENT".into(), p.harness.id().into()),
    ];
    LaunchSpec { argv, vars }
}
