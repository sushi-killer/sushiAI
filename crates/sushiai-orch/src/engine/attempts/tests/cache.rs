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
    let line = apply_agent_cache(&mut command, Some(&cache), &req, SandboxMode::Host).unwrap();
    assert!(line.contains("codexNetwork false") && line.contains("localhost"));
    assert!(!command
        .as_std()
        .get_envs()
        .any(|(k, _)| k == "RUSTC_WRAPPER" || k.to_string_lossy().starts_with("SCCACHE_")));
    assert_eq!(
        apply_agent_cache(&mut command, None, &req, SandboxMode::Host),
        None
    );
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
