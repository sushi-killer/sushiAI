use super::{prompt_hash, stable_argv};
use crate::harness::replay_events;
use crate::policy::routing::Harness;

#[test]
fn claude_events_give_the_real_models_and_version() {
    let text = r#"{"type":"system","subtype":"init","session_id":"s","model":"claude-sonnet-5-5","claude_code_version":"2.1.284"}
{"type":"result","total_cost_usd":0.1,"modelUsage":{"claude-sonnet-5-5":{},"claude-haiku-4-5-20251001":{}},"result":"x"}"#;
    let outcome = replay_events(Harness::Claude, text);
    let fp = outcome.build_fingerprint(Harness::Claude, None, "h".into());
    assert_eq!(
        fp.models,
        ["claude-sonnet-5-5", "claude-haiku-4-5-20251001"]
    );
    assert_eq!(fp.harness_version.as_deref(), Some("2.1.284"));
    assert_eq!(
        fp.label(),
        "claude-sonnet-5-5+claude-haiku-4-5-20251001 (2.1.284)"
    );
}

#[test]
fn prompt_hash_ignores_codex_mcp_wiring() {
    let argv = |cmd: &str, key: &str| -> Vec<String> {
        [
            "exec",
            "-c",
            &format!("mcp_servers.m.command=\"{cmd}\""),
            "-c",
            &format!("mcp_servers.m.env.TOKEN=\"{key}\""),
            "--model",
            "gpt",
        ]
        .map(String::from)
        .to_vec()
    };
    assert_eq!(
        prompt_hash(&argv("/a", "k1"), None),
        prompt_hash(&argv("/b", "k2"), None)
    );
    assert!(!stable_argv(&argv("/a", "k1"))
        .iter()
        .any(|a| a.contains("mcp_servers")));
}

#[test]
fn prompt_hash_ignores_paths_ids_and_secrets_but_not_real_settings() {
    let argv = |mcp: &str, resume: &str| -> Vec<String> {
        [
            "-p",
            "--mcp-config",
            mcp,
            "--resume",
            resume,
            "--model",
            "sonnet",
        ]
        .map(String::from)
        .to_vec()
    };
    let settings = |key: &str, cap: bool| {
        serde_json::json!({
            "env": {"ANTHROPIC_API_KEY": key},
            "hooks": {"Stop": key},
            "permissions": {"allow": ["Bash"], "deny": ["Bash(git push:*)", format!("Read(//{key}/**)")]},
            "bashOutputMaxChars": if cap { 1 } else { 2 },
        })
    };
    let a = prompt_hash(&argv("/a/mcp.json", "s1"), Some(&settings("k1", true)));
    let b = prompt_hash(&argv("/b/mcp.json", "s2"), Some(&settings("k2", true)));
    assert_eq!(a, b);
    let c = prompt_hash(&argv("/a/mcp.json", "s1"), Some(&settings("k1", false)));
    assert_ne!(a, c);
    let mut other = argv("/a/mcp.json", "s1");
    other.push("--effort".into());
    assert_ne!(a, prompt_hash(&other, Some(&settings("k1", true))));
}
