use crate::engine::attempts::cache::apply_agent_cache;
use crate::harness;
use crate::harness::compiler_cache;
use crate::policy::compiler_cache::agent_cache_block_reason;
use crate::policy::routing::Harness;
use crate::policy::settings::SandboxMode;
use std::path::Path;

#[test]
fn codex_without_network_gets_no_wrapper_and_a_reason() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let tool = tmp.path().join("sccache");
    std::fs::write(&tool, "#!/bin/sh\nexit 99\n").unwrap();
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
    let cache = compiler_cache::CompilerCache::discover(
        &tmp.path().display().to_string(),
        true,
        false,
        tmp.path().join("cache"),
        4227,
    )
    .unwrap();
    let req = harness::RunRequest {
        harness: Harness::Codex,
        worktree: tmp.path(),
        model: None,
        effort: None,
        max_budget_usd: None,
        review: false,
        task_run: false,
        task_sandbox: None,
        mcp_config: None,
        settings_path: None,
        network_allowed: false,
        codex_mcp: &[],
        images: &[],
        repo_settings: false,
        resume: None,
    };
    let mut command = tokio::process::Command::new("codex");
    let line =
        apply_agent_cache(&mut command, Some(&cache), &req, SandboxMode::Host, false).unwrap();
    assert!(line.contains("codexNetwork false") && line.contains("localhost"));
    let var = |key: &str| {
        command
            .as_std()
            .get_envs()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.map(|v| v.to_os_string()))
    };
    assert_eq!(var("RUSTC_WRAPPER"), Some(Some("".into())));
    assert_eq!(var("RUSTC_WORKSPACE_WRAPPER"), Some(Some("".into())));
    // An inherited value shows as removed (None); orchd's own is never set.
    assert!(!matches!(var("SCCACHE_DIR"), Some(Some(_))));
    assert!(!matches!(var("SCCACHE_SERVER_PORT"), Some(Some(_))));

    // Without orchd's cache the configured wrapper is still turned off.
    let mut bare = tokio::process::Command::new("codex");
    assert_eq!(
        apply_agent_cache(&mut bare, None, &req, SandboxMode::Host, false),
        None
    );
    let wrapper = |command: &tokio::process::Command| {
        command
            .as_std()
            .get_envs()
            .find(|(k, _)| *k == "RUSTC_WRAPPER")
            .map(|(_, v)| v.map(|v| v.to_os_string()))
    };
    assert_eq!(wrapper(&bare), Some(Some("".into())));

    // A wrapper the owner exported stays.
    let mut owned = tokio::process::Command::new("codex");
    apply_agent_cache(&mut owned, None, &req, SandboxMode::Host, true);
    assert_eq!(wrapper(&owned), None);
}

#[test]
fn cache_write_access_is_required_even_with_agent_network() {
    let cache = Path::new("/shared/cache");
    let wt = Path::new("/worktree");
    assert!(agent_cache_block_reason(
        Harness::Codex,
        true,
        false,
        SandboxMode::Host,
        cache.starts_with(wt)
    )
    .is_some());
    assert!(agent_cache_block_reason(
        Harness::Claude,
        true,
        false,
        SandboxMode::Native,
        cache.starts_with(wt)
    )
    .is_some());
    assert_eq!(
        agent_cache_block_reason(
            Harness::Claude,
            true,
            false,
            SandboxMode::Host,
            cache.starts_with(wt)
        ),
        None
    );
}
