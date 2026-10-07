use std::fs;

use serde_json::{json, Value};
use sushiai_agents::codex_hooks::{install, merge};
use sushiai_agents::codex_trust::{
    apply_trust, handler_hash, remove_trust, trust, trust_entries, untrust,
};

const BIN: &str = "/opt/sushi/sushiai";
const HOOKS: &str = "/home/user/.codex/hooks.json";

// Golden hashes. They were computed ONCE with the independent Python
// implementation that reproduced 12 of 12 real Codex trust entries
// byte-for-byte (scratchpad codex-trust/verify.py, function `h`). If one of
// these fails, either this code or Codex's algorithm drifted: re-run the
// script against a real config before touching the constants.
const GOLDEN_STOP: &str = "sha256:5c388da9c301e8b00758aa214a75b08cc5f160d9728e6827db2c9127c3d36e9e";
const GOLDEN_MATCHER_DEFAULT_TIMEOUT: &str =
    "sha256:aa7c374c41d0189a6e1d474a99291e8c813f43c3a378666c81aac45c34cdcb1b";
const GOLDEN_SESSION_END_ASYNC: &str =
    "sha256:f9aa697f9fa43c49036041ac01764b6f7ca64bde9ce4fe741061a4577f64eaf8";
const GOLDEN_STATUS_NON_ASCII: &str =
    "sha256:99f1ca1270ad535b1b12918de42bedfd0678729f57ec4a3429258219f75cf1cf";

fn ours(command_arg: &str, timeout: u64) -> Value {
    json!({"type":"command","command":format!("{BIN} hook {command_arg} --agent codex"),"timeout":timeout})
}

#[test]
fn hash_matches_the_golden_vectors() {
    assert_eq!(handler_hash("stop", None, &ours("stop", 10)), GOLDEN_STOP);
    // Matcher is hashed for pre_tool_use; timeout defaults to 600.
    let h = json!({"type":"command","command":format!("{BIN} hook tool-start --agent codex")});
    assert_eq!(
        handler_hash("pre_tool_use", Some("Bash"), &h),
        GOLDEN_MATCHER_DEFAULT_TIMEOUT
    );
    // `async` is kept as written; session_end timeout is clamped to 1..=3.
    let mut h = ours("session-end", 10);
    h["async"] = json!(true);
    assert_eq!(
        handler_hash("session_end", None, &h),
        GOLDEN_SESSION_END_ASYNC
    );
    // stop drops the matcher.
    assert_eq!(
        handler_hash("stop", Some("ignored"), &ours("stop", 10)),
        GOLDEN_STOP
    );
    let h = json!({"type":"command","command":"echo \"é\" x","timeout":10,"statusMessage":"hi"});
    assert_eq!(handler_hash("stop", None, &h), GOLDEN_STATUS_NON_ASCII);
}

#[test]
fn hash_ignores_extra_fields_and_key_order() {
    let a = json!({"timeout":10,"command":"c","type":"command","env":{"A":"1"}});
    let b = json!({"type":"command","command":"c","timeout":10});
    assert_eq!(
        handler_hash("stop", None, &a),
        handler_hash("stop", None, &b)
    );
    assert_ne!(
        handler_hash("stop", None, &b),
        handler_hash(
            "stop",
            None,
            &json!({"type":"command","command":"c","timeout":11})
        )
    );
}

fn merged_with_user_group_first() -> Value {
    let user = json!({"hooks": {"Stop": [{"hooks": [{"type":"command","command":"/u/tool","timeout":5}]}]}});
    merge(&user, BIN, 600).unwrap()
}

#[test]
fn entries_cover_only_our_handlers_with_key_format() {
    let v = merged_with_user_group_first();
    let e = trust_entries(&v, HOOKS);
    assert_eq!(e.len(), 8);
    // User group is index 0, ours index 1.
    let stop = e.iter().find(|(k, _)| k.contains(":stop:")).unwrap();
    assert_eq!(stop.0, format!("{HOOKS}:stop:1:0"));
    assert!(e.iter().all(|(k, h)| k.starts_with(&format!("{HOOKS}:"))
        && h.starts_with("sha256:")
        && h.len() == 71));
    assert!(e
        .iter()
        .any(|(k, _)| k == &format!("{HOOKS}:session_start:0:0")));
    assert!(trust_entries(
        &json!({"hooks": {"Stop": [{"hooks": [{"command": "/u/tool"}]}]}}),
        HOOKS
    )
    .is_empty());
    assert!(trust_entries(&Value::Null, HOOKS).is_empty());
}

#[test]
fn index_shift_changes_the_key_but_not_the_hash() {
    let before = trust_entries(&merge(&json!({}), BIN, 600).unwrap(), HOOKS);
    let after = trust_entries(&merged_with_user_group_first(), HOOKS);
    let b = before.iter().find(|(k, _)| k.contains(":stop:")).unwrap();
    let a = after.iter().find(|(k, _)| k.contains(":stop:")).unwrap();
    assert_eq!(b.0, format!("{HOOKS}:stop:0:0"));
    assert_eq!(a.0, format!("{HOOKS}:stop:1:0"));
    assert_eq!(b.1, a.1);
}

#[test]
fn apply_writes_only_trusted_hash_and_is_idempotent() {
    let e = vec![(format!("{HOOKS}:stop:0:0"), GOLDEN_STOP.to_owned())];
    let out = apply_trust("", HOOKS, &e).unwrap();
    let doc: toml_edit::DocumentMut = out.parse().unwrap();
    let t = &doc["hooks"]["state"][format!("{HOOKS}:stop:0:0").as_str()];
    assert_eq!(t["trusted_hash"].as_str(), Some(GOLDEN_STOP));
    assert_eq!(t.as_table().unwrap().len(), 1);
    assert!(out.contains(&format!("[hooks.state.\"{HOOKS}:stop:0:0\"]")));
    assert_eq!(apply_trust(&out, HOOKS, &e).unwrap(), out);
}

const USER_CONFIG: &str = r#"# my codex config
model = "x"   # inline comment

[features]
hooks = true

[hooks.state]

[hooks.state."/other/hooks.json:stop:0:0"]
trusted_hash = "sha256:aaaa"
enabled = false

[hooks.state."/home/user/.codex/hooks.json:stop:0:0"]
trusted_hash = "sha256:user-owned-different-hash"
# keep me
"#;

#[test]
fn formatting_comments_and_other_keys_survive() {
    let e = trust_entries(&merge(&json!({}), BIN, 600).unwrap(), HOOKS);
    let out = apply_trust(USER_CONFIG, HOOKS, &e).unwrap();
    for keep in [
        "# my codex config",
        "model = \"x\"   # inline comment",
        "[features]\nhooks = true",
        "[hooks.state.\"/other/hooks.json:stop:0:0\"]\ntrusted_hash = \"sha256:aaaa\"\nenabled = false",
    ] {
        assert!(out.contains(keep), "lost {keep:?}\n{out}");
    }
    // Our entry replaced the hash under the same key (it was not ours: a
    // different hash, same key: Codex would have flagged it Modified).
    let doc: toml_edit::DocumentMut = out.parse().unwrap();
    let k = format!("{HOOKS}:stop:0:0");
    assert_eq!(
        doc["hooks"]["state"][k.as_str()]["trusted_hash"].as_str(),
        Some(e.iter().find(|(x, _)| *x == k).unwrap().1.as_str())
    );
    assert!(out.contains("# keep me"));
}

#[test]
fn stale_keys_with_our_hash_are_removed_after_a_shift() {
    let old = trust_entries(&merge(&json!({}), BIN, 600).unwrap(), HOOKS);
    let cfg = apply_trust(USER_CONFIG, HOOKS, &old).unwrap();
    let new = trust_entries(&merged_with_user_group_first(), HOOKS);
    let out = apply_trust(&cfg, HOOKS, &new).unwrap();
    let doc: toml_edit::DocumentMut = out.parse().unwrap();
    let state = doc["hooks"]["state"].as_table().unwrap();
    // Old stop key (0:0) is not ours any more at that index? It is the user's
    // group now, and its hash is the user's: it must NOT be removed unless
    // it holds one of our hashes. The old key held our stop hash: removed.
    assert!(state.get(&format!("{HOOKS}:stop:0:0")).is_none());
    assert!(state.get(&format!("{HOOKS}:stop:1:0")).is_some());
    // Keys of other files and the foreign-hash entry are untouched.
    assert!(state.get("/other/hooks.json:stop:0:0").is_some());
    // Idempotent once settled.
    assert_eq!(apply_trust(&out, HOOKS, &new).unwrap(), out);
}

#[test]
fn a_user_entry_with_a_foreign_hash_is_never_removed() {
    let cfg = format!("[hooks.state.\"{HOOKS}:stop:5:0\"]\ntrusted_hash = \"sha256:not-ours\"\n");
    let e = trust_entries(&merge(&json!({}), BIN, 600).unwrap(), HOOKS);
    let out = apply_trust(&cfg, HOOKS, &e).unwrap();
    assert!(out.contains("sha256:not-ours"));
}

#[test]
fn remove_trust_removes_only_the_named_keys() {
    let e = trust_entries(&merge(&json!({}), BIN, 600).unwrap(), HOOKS);
    let cfg = apply_trust(USER_CONFIG, HOOKS, &e).unwrap();
    let keys: Vec<String> = e.iter().map(|(k, _)| k.clone()).collect();
    let out = remove_trust(&cfg, &keys).unwrap();
    let doc: toml_edit::DocumentMut = out.parse().unwrap();
    let state = doc["hooks"]["state"].as_table().unwrap();
    assert!(state.get("/other/hooks.json:stop:0:0").is_some());
    assert!(keys.iter().all(|k| state.get(k).is_none()));
    assert!(out.contains("# my codex config"));
}

#[test]
fn bad_toml_and_wrong_shapes_are_errors() {
    assert!(apply_trust("= nope", HOOKS, &[]).is_err());
    assert!(apply_trust("hooks = 3\n", HOOKS, &[]).is_err());
}

#[test]
fn fs_trust_backs_up_writes_verifies_and_untrusts() {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    let hooks = root.join("hooks.json");
    let config = root.join("config.toml");
    fs::write(&config, USER_CONFIG).unwrap();
    assert!(install(&hooks, BIN, 600, 1).unwrap().changed);

    let out = trust(&config, &hooks, 10).unwrap();
    assert!(out.changed);
    assert_eq!(
        fs::read_to_string(out.backup.unwrap()).unwrap(),
        USER_CONFIG
    );
    let text = fs::read_to_string(&config).unwrap();
    let key = format!("{}:stop:0:0", hooks.display());
    assert!(text.contains(&key));
    assert!(text.contains("# my codex config"));
    // Idempotent: no change, no new backup.
    let again = trust(&config, &hooks, 11).unwrap();
    assert!(!again.changed && again.backup.is_none());

    let out = untrust(&config, &hooks, 12).unwrap();
    assert!(out.changed);
    assert!(!fs::read_to_string(&config).unwrap().contains(&key));
}

#[test]
fn fs_trust_creates_config_and_writes_through_a_symlink() {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    let hooks = root.join("hooks.json");
    install(&hooks, BIN, 600, 1).unwrap();
    // Missing config: created, no backup.
    let config = root.join("config.toml");
    let out = trust(&config, &hooks, 1).unwrap();
    assert!(out.changed && out.backup.is_none() && config.exists());
    // Symlinked config: the link stays.
    let target = root.join("real.toml");
    let link = root.join("link.toml");
    fs::write(&target, "").unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();
    trust(&link, &hooks, 2).unwrap();
    assert!(fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(fs::read_to_string(&target)
        .unwrap()
        .contains("trusted_hash"));
}

#[test]
fn relative_hooks_path_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let r = trust(
        &dir.path().join("c.toml"),
        std::path::Path::new("hooks.json"),
        1,
    );
    assert!(r.is_err());
}
