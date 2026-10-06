//! Daemon-side glue for agent sessions: launch preparation, permission answers and the
//! conversions between the pure `sushiai-agents` types and the wire types.

use std::io::Read;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use sushiai_agents::launch::{self, AgentSession, LaunchParams};
use sushiai_agents::status::{Status, StatusSource as AgentSource};
use sushiai_agents::Agent;
use sushiai_protocol::{code, AgentInfo, AgentStatus, Decision, SessionCreate, StatusSource};

use crate::error::Fail;
use crate::registry::hash_token;

/// How long the owner has to answer a permission ask before the agent falls back to its
/// own terminal prompt. `SUSHIAI_ASK_TIMEOUT_MS` overrides it (tests).
pub const ASK_WAIT_SECS: u64 = 600;

pub fn ask_timeout() -> std::time::Duration {
    std::env::var("SUSHIAI_ASK_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .map_or(std::time::Duration::from_secs(ASK_WAIT_SECS), |ms| {
            std::time::Duration::from_millis(ms)
        })
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

pub fn random_hex(len: usize) -> std::io::Result<String> {
    let mut bytes = vec![0u8; len];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// A random version 4 UUID (Claude insists on one for `--session-id`).
fn uuid_v4() -> std::io::Result<String> {
    let mut b = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut b)?;
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    ))
}

pub fn agent_of(name: Option<&str>) -> Option<Agent> {
    match name {
        Some("claude") => Some(Agent::Claude),
        Some("codex") => Some(Agent::Codex),
        _ => None,
    }
}

pub fn status_to_wire(s: Status) -> AgentStatus {
    match s {
        Status::Starting => AgentStatus::Starting,
        Status::Working => AgentStatus::Working,
        Status::Blocked => AgentStatus::Blocked,
        Status::Idle => AgentStatus::Idle,
        Status::Exited => AgentStatus::Exited,
    }
}

pub fn status_from_wire(s: AgentStatus) -> Status {
    match s {
        AgentStatus::Starting => Status::Starting,
        AgentStatus::Working => Status::Working,
        AgentStatus::Blocked => Status::Blocked,
        AgentStatus::Idle => Status::Idle,
        AgentStatus::Exited => Status::Exited,
    }
}

pub fn source_to_wire(s: AgentSource) -> StatusSource {
    match s {
        AgentSource::Hook => StatusSource::Hook,
        AgentSource::Heuristic => StatusSource::Heuristic,
    }
}

pub fn source_from_wire(s: StatusSource) -> AgentSource {
    match s {
        StatusSource::Hook => AgentSource::Hook,
        StatusSource::Heuristic => AgentSource::Heuristic,
    }
}

/// What `session.create` runs and records.
pub struct Prepared {
    /// What the catalog records and clients see: never holds `claudeSettings`.
    pub cmd: Vec<String>,
    /// What the holder runs. Differs from `cmd` only when `claudeSettings` is set.
    pub run_cmd: Vec<String>,
    pub env: Vec<(String, String)>,
    pub agent: AgentInfo,
}

/// Builds the command line, the child variables and the agent record for a new session.
/// Only `claude` and `codex` get hooks; any other agent name just labels a plain command.
pub fn prepare(p: &SessionCreate, socket: &str, id: &str) -> Result<Prepared, Fail> {
    let invalid = |m: &str| (code::INVALID_PARAMS, m.to_string());
    let internal = |e: std::io::Error| (code::INTERNAL, e.to_string());
    // User variables first, so a launch variable always wins.
    let mut env: Vec<(String, String)> = p.env.clone().into_iter().collect();
    let Some(agent) = agent_of(p.agent.as_deref()) else {
        if p.cmd.is_empty() {
            return Err(invalid("cmd, cols and rows are required"));
        }
        let agent = AgentInfo {
            name: p.agent.clone(),
            ..AgentInfo::default()
        };
        if p.claude_settings.is_some() {
            return Err(invalid("claudeSettings needs agent claude"));
        }
        return Ok(Prepared {
            cmd: p.cmd.clone(),
            run_cmd: p.cmd.clone(),
            env,
            agent,
        });
    };
    let token = random_hex(16).map_err(internal)?;
    let minted = match (&p.resume, agent) {
        (Some(resume), _) => Some(resume.clone()),
        (None, Agent::Claude) => Some(uuid_v4().map_err(internal)?),
        (None, Agent::Codex) => None,
    };
    let session = match (&p.resume, &minted) {
        (Some(r), _) => AgentSession::Resume(r),
        (None, Some(m)) => AgentSession::New(m),
        (None, None) => AgentSession::New(""),
    };
    let hook_bin = crate::binlink::real_exe()
        .map_err(internal)?
        .to_string_lossy()
        .into_owned();
    let mut extra_args: Vec<String> = Vec::new();
    if let Some(model) = &p.model {
        extra_args.extend(["--model".to_string(), model.clone()]);
    }
    extra_args.extend(p.extra_args.iter().cloned());
    if agent != Agent::Claude && p.claude_settings.is_some() {
        return Err(invalid("claudeSettings needs agent claude"));
    }
    let spec = launch::build(&LaunchParams {
        agent,
        hook_bin: &hook_bin,
        socket,
        sushiai_session_id: id,
        session_token: &token,
        session,
        extra_args: &extra_args,
        prompt: p.prompt.as_deref(),
        permission_wait_secs: ASK_WAIT_SECS,
        claude_settings: p.claude_settings.as_ref(),
    })
    .map_err(|e| invalid(&e.to_string()))?;
    env.extend(spec.vars);
    let agent_info = AgentInfo {
        name: Some(agent.as_str().into()),
        agent_session: minted,
        agent_status: Some(AgentStatus::Starting),
        status_source: Some(StatusSource::Heuristic),
        status_since: Some(now_ms()),
        token_hash: Some(hash_token(&token)),
        ..AgentInfo::default()
    };
    let mut cmd = spec.argv.clone();
    if p.claude_settings.is_some() {
        // The recorded command keeps our own settings only; the caller's keys may be secrets.
        let ours = launch::claude_settings(&hook_bin, ASK_WAIT_SECS)
            .map_err(|e| invalid(&e.to_string()))?
            .to_string();
        if let Some(at) = cmd.iter().position(|a| a == "--settings") {
            if let Some(value) = cmd.get_mut(at + 1) {
                *value = ours;
            }
        }
    }
    Ok(Prepared {
        cmd,
        run_cmd: spec.argv,
        env,
        agent: agent_info,
    })
}

/// Only one launch at a time edits an account home: read, merge and rename must not overlap.
static CODEX_HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// An account `CODEX_HOME` given in `env` is not the one `sushiai hooks install` served:
/// put our hooks and their trust there before Codex starts. Both steps are idempotent. The
/// directory must be absolute and free of `~`; nothing is written otherwise.
pub fn ensure_codex_home(p: &SessionCreate, home_dir: &std::path::Path) -> Result<(), Fail> {
    if agent_of(p.agent.as_deref()) != Some(Agent::Codex) {
        return Ok(());
    }
    let Some(dir) = p.env.get("CODEX_HOME").filter(|d| !d.is_empty()) else {
        return Ok(());
    };
    if !dir.starts_with('/') || dir.contains('~') {
        return Err((
            code::INVALID_PARAMS,
            "CODEX_HOME must be an absolute path without `~`".into(),
        ));
    }
    let fail = |what: &str, e: &dyn std::fmt::Display| {
        (
            code::SPAWN_FAILED,
            format!("cannot prepare CODEX_HOME: {what}: {e}"),
        )
    };
    let dir = std::path::Path::new(dir);
    let _one_at_a_time = CODEX_HOME_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    crate::binlink::ensure_bin_link(home_dir).map_err(|e| fail("bin link", &e))?;
    let bin = home_dir.join("bin/sushiai");
    let bin = bin.to_string_lossy();
    std::fs::create_dir_all(dir).map_err(|e| fail("directory", &e))?;
    // Codex resolves `$CODEX_HOME` and keys hook trust by the resolved path.
    let dir = dir.canonicalize().map_err(|e| fail("directory", &e))?;
    let (hooks, config) = (dir.join("hooks.json"), dir.join("config.toml"));
    let ts = now_ms() / 1000;
    sushiai_agents::codex_hooks::install(&hooks, &bin, ASK_WAIT_SECS, ts)
        .map_err(|e| fail("hooks.json", &e))?;
    sushiai_agents::codex_trust::trust(&config, &hooks, ts).map_err(|e| fail("config.toml", &e))?;
    Ok(())
}

/// The largest ask `input` sent to clients, as JSON text.
const MAX_ASK_INPUT: usize = 64 * 1024;

/// An oversize tool input becomes a truncated JSON text string, so one ask cannot flood
/// every client. The tool name travels separately and is kept.
pub fn clip_input(input: Value) -> Value {
    let text = input.to_string();
    if text.len() <= MAX_ASK_INPUT {
        return input;
    }
    let mut end = MAX_ASK_INPUT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Value::String(text[..end].to_string())
}

/// The JSON a `PermissionRequest` hook prints. Claude and Codex share the shape.
pub fn permission_answer(decision: Decision, message: Option<&str>) -> Value {
    let mut d = match decision {
        Decision::Allow => json!({ "behavior": "allow" }),
        Decision::Deny => json!({ "behavior": "deny" }),
    };
    if let (Decision::Deny, Some(m)) = (decision, message) {
        d["message"] = json!(m);
    }
    json!({ "hookSpecificOutput": { "hookEventName": "PermissionRequest", "decision": d } })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deny_carries_its_message_and_allow_does_not() {
        let deny = permission_answer(Decision::Deny, Some("no"));
        assert_eq!(deny["hookSpecificOutput"]["decision"]["behavior"], "deny");
        assert_eq!(deny["hookSpecificOutput"]["decision"]["message"], "no");
        let allow = permission_answer(Decision::Allow, Some("ignored"));
        assert_eq!(allow["hookSpecificOutput"]["decision"]["behavior"], "allow");
        assert!(allow["hookSpecificOutput"]["decision"]
            .get("message")
            .is_none());
    }

    #[test]
    fn an_oversize_input_is_cut_and_a_small_one_is_kept() {
        let small = json!({"command": "ls"});
        assert_eq!(clip_input(small.clone()), small);
        let big = json!({"command": "é".repeat(MAX_ASK_INPUT)});
        let Value::String(cut) = clip_input(big) else {
            panic!("expected a string");
        };
        assert!(cut.len() <= MAX_ASK_INPUT && cut.starts_with("{\"command\""));
    }

    #[test]
    fn a_minted_uuid_has_the_v4_shape() {
        let id = uuid_v4().expect("uuid");
        let parts: Vec<usize> = id.split('-').map(str::len).collect();
        assert_eq!(parts, [8, 4, 4, 4, 12]);
        assert!(id.split('-').nth(2).is_some_and(|p| p.starts_with('4')));
    }
}
