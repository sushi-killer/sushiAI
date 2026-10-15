use crate::harness;
use crate::harness::compiler_cache;
use crate::policy::compiler_cache::agent_cache_block_reason;
use crate::policy::settings::SandboxMode;

pub(super) fn apply_agent_cache(
    cmd: &mut tokio::process::Command,
    cache: Option<&compiler_cache::CompilerCache>,
    req: &harness::RunRequest<'_>,
    sandbox: SandboxMode,
) -> Option<String> {
    let cache = cache?;
    if let Some(reason) = agent_cache_block_reason(
        req.harness,
        req.network_allowed,
        req.review,
        sandbox,
        cache.dir.starts_with(req.worktree),
    ) {
        return Some(format!(
            "Orchestrator: compiler cache disabled for agent: {reason}"
        ));
    }
    cache.apply(cmd);
    None
}
