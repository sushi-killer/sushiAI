use std::process::Command;

use serde_json::Value;
use sushiai_agents::launch::{
    build, claude_settings, hook_command, shell_quote, AgentSession, LaunchError, LaunchParams,
};
use sushiai_agents::Agent;

const BIN: &str = "/home/user/.sushiai/bin/sushiai";

fn params<'a>(
    agent: Agent,
    session: AgentSession<'a>,
    bin: &'a str,
    extra: &'a [String],
) -> LaunchParams<'a> {
    LaunchParams {
        agent,
        hook_bin: bin,
        socket: "/run/sushiai.sock",
        sushiai_session_id: "sess-1",
        session_token: "tok",
        session,
        extra_args: extra,
        prompt: None,
        permission_wait_secs: 600,
    }
}

/// Run `sh -c "printf '%s\n' <quoted>"` and return what the shell saw.
fn through_sh(word: &str) -> String {
    let out = Command::new("sh")
        .arg("-c")
        .arg(format!("printf '%s' {}", shell_quote(word)))
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn quoting_round_trips_through_a_real_shell() {
    for w in [
        "plain",
        "/a b/c",
        "it's",
        "a\"b",
        "$HOME `x` $(y)",
        "semi;colon && pipe | x",
        "",
        "new\nline",
        "tab\tx",
        "*.txt",
        "~/x",
    ] {
        assert_eq!(through_sh(w), w, "word {w:?} quoted as {}", shell_quote(w));
    }
    assert_eq!(shell_quote("/safe/path-1.2_x"), "/safe/path-1.2_x");
}

#[test]
fn hook_command_is_quoted_when_the_path_has_spaces_and_quotes() {
    let bin = "/home/o'brien/my dir/sushiai";
    let cmd = hook_command(bin, "stop", Agent::Claude);
    let out = Command::new("sh")
        .arg("-c")
        .arg(format!("set -- {cmd}; printf '%s|' \"$@\""))
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        format!("{bin}|hook|stop|")
    );
    assert!(hook_command(BIN, "stop", Agent::Codex).ends_with("hook stop --agent codex"));
}

#[test]
fn claude_new_session_argv_has_one_settings_flag() {
    let spec = build(&params(
        Agent::Claude,
        AgentSession::New("11111111-2222-3333-4444-555555555555"),
        BIN,
        &[],
    ))
    .unwrap();
    assert_eq!(
        &spec.argv[..4],
        [
            "claude",
            "--session-id",
            "11111111-2222-3333-4444-555555555555",
            "--settings"
        ]
    );
    assert_eq!(spec.argv.iter().filter(|a| *a == "--settings").count(), 1);
    assert_eq!(spec.argv.len(), 5);
    assert!(!spec.argv.contains(&"--bare".to_owned()));
}

#[test]
fn claude_resume_uses_resume_and_no_session_id() {
    let spec = build(&params(
        Agent::Claude,
        AgentSession::Resume("abc"),
        BIN,
        &[],
    ))
    .unwrap();
    assert_eq!(&spec.argv[..3], ["claude", "--resume", "abc"]);
    assert!(!spec.argv.contains(&"--session-id".to_owned()));
}

#[test]
fn claude_settings_json_shape() {
    let v: Value = claude_settings(BIN, 600).unwrap();
    let hooks = v["hooks"].as_object().unwrap();
    for e in [
        "SessionStart",
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUse",
        "PostToolUseFailure",
        "PermissionRequest",
        "Notification",
        "Stop",
        "StopFailure",
        "SessionEnd",
    ] {
        assert_eq!(hooks[e].as_array().unwrap().len(), 1, "{e}");
    }
    let h = &hooks["PermissionRequest"][0]["hooks"][0];
    assert_eq!(h["command"], format!("{BIN} hook permission"));
    assert_eq!(h["timeout"], 630);
    assert!(h.get("async").is_none(), "the approver must be synchronous");
    assert_eq!(hooks["Stop"][0]["hooks"][0]["async"], true);
    assert_eq!(hooks["SessionEnd"][0]["hooks"][0]["timeout"], 2);
}

#[test]
fn settings_argument_survives_a_path_with_spaces_and_quotes() {
    let bin = "/home/o'brien/my dir/sushiai";
    let spec = build(&params(Agent::Claude, AgentSession::New("u"), bin, &[])).unwrap();
    let json: Value = serde_json::from_str(&spec.argv[4]).unwrap();
    let cmd = json["hooks"]["Stop"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    assert_eq!(cmd, hook_command(bin, "stop", Agent::Claude));
    assert!(cmd.starts_with("'/home/o'\\''brien/my dir/sushiai' hook stop"));
}

#[test]
fn codex_argv() {
    let spec = build(&params(
        Agent::Codex,
        AgentSession::New("ignored"),
        BIN,
        &[],
    ))
    .unwrap();
    assert_eq!(spec.argv, ["codex"]);
    let spec = build(&params(Agent::Codex, AgentSession::Resume("abc"), BIN, &[])).unwrap();
    assert_eq!(spec.argv, ["codex", "resume", "abc"]);
}

#[test]
fn extra_args_then_prompt_and_dash_prompt_is_guarded() {
    let extra = vec!["--model".to_owned(), "m".to_owned()];
    let mut p = params(Agent::Codex, AgentSession::New("x"), BIN, &extra);
    p.prompt = Some("-rf is not a flag");
    assert_eq!(
        build(&p).unwrap().argv,
        ["codex", "--model", "m", "--", "-rf is not a flag"]
    );
    p.prompt = Some("fix it");
    assert_eq!(build(&p).unwrap().argv.last().unwrap(), "fix it");
}

#[test]
fn vars_carry_socket_session_and_token() {
    let spec = build(&params(Agent::Claude, AgentSession::New("u"), BIN, &[])).unwrap();
    let get = |k: &str| {
        spec.vars
            .iter()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v.as_str())
    };
    assert_eq!(get("SUSHIAI_SOCKET"), Some("/run/sushiai.sock"));
    assert_eq!(get("SUSHIAI_SESSION_ID"), Some("sess-1"));
    assert_eq!(get("SUSHIAI_SESSION_TOKEN"), Some("tok"));
    assert_eq!(get("SUSHIAI_AGENT"), Some("claude"));
}

#[test]
fn a_relative_hook_path_is_rejected() {
    let err = build(&params(
        Agent::Claude,
        AgentSession::New("u"),
        "~/.sushiai/bin/sushiai",
        &[],
    ))
    .unwrap_err();
    assert_eq!(
        err,
        LaunchError::RelativeHookPath("~/.sushiai/bin/sushiai".into())
    );
}
