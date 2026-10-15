use crate::harness::{build_claude_settings, StopHook, HOOKED_TOOLS};
use crate::policy::settings::SandboxMode;

#[test]
fn claude_settings_merges_profile_and_omits_sandbox_for_host() {
    let profile =
        serde_json::json!({"env": {"ANTHROPIC_API_KEY_HELPER": "x"}, "apiKeyHelper": "helper.sh"});
    let domains = vec!["github.com".to_string()];
    let hook = StopHook {
        exe_path: "/usr/local/bin/sushiai",
        home: "/h/.sushiai",
        token: "tok-1",
    };
    let deny_read = vec!["/data".to_string()];
    let native = build_claude_settings(
        Some(&profile),
        SandboxMode::Native,
        &domains,
        &deny_read,
        Some(hook),
    );
    assert_eq!(native["apiKeyHelper"], "helper.sh");
    assert_eq!(native["sandbox"]["enabled"], true);
    assert_eq!(
        native["sandbox"]["network"]["allowedDomains"][0],
        "github.com"
    );
    assert!(native["sandbox"]["network"]
        .get("strictAllowlist")
        .is_none());
    assert_eq!(native["sandbox"]["filesystem"]["denyRead"][0], "/data");
    assert_eq!(native["hooks"]["Stop"][0]["hooks"][0]["timeout"], 600);
    assert_eq!(native["hooks"]["PreToolUse"][0]["matcher"], HOOKED_TOOLS);
    assert!(native["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .contains("SUSHIAI_HOME='/h/.sushiai' SUSHIAI_ORCH_TOKEN='tok-1' '/usr/local/bin/sushiai' orch hook edit"));
    assert!(native["hooks"]["Stop"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .contains("SUSHIAI_HOME='/h/.sushiai' SUSHIAI_ORCH_TOKEN='tok-1' '/usr/local/bin/sushiai' orch hook stop"));

    let hook2 = StopHook {
        exe_path: "/usr/local/bin/sushiai",
        home: "/h/.sushiai",
        token: "tok-1",
    };
    let host = build_claude_settings(None, SandboxMode::Host, &domains, &deny_read, Some(hook2));
    assert!(host.get("sandbox").is_none());
    assert!(host.get("hooks").is_some());
}

#[test]
fn claude_settings_omits_hooks_entirely_when_no_stop_hook_given() {
    let domains = vec!["github.com".to_string()];
    let review = build_claude_settings(None, SandboxMode::Native, &domains, &[], None);
    assert!(
        review.get("hooks").is_none(),
        "a review session has no registered token, so it must not get a Stop hook"
    );
    assert_eq!(review["sandbox"]["enabled"], true);
}

#[test]
fn claude_settings_allow_sandboxed_bash_and_forbid_commit_and_push() {
    let native = build_claude_settings(None, SandboxMode::Native, &[], &[], None);
    assert_eq!(native["sandbox"]["autoAllowBashIfSandboxed"], true);
    let deny = native["permissions"]["deny"].as_array().unwrap();
    assert!(deny.iter().any(|d| d == "Bash(git commit:*)"));
    assert!(deny.iter().any(|d| d == "Bash(git push:*)"));
    assert_eq!(native["permissions"]["allow"], serde_json::json!(["Bash"]));

    let host = build_claude_settings(None, SandboxMode::Host, &[], &[], None);
    assert_eq!(host["permissions"]["allow"], serde_json::json!(["Bash"]));
}
