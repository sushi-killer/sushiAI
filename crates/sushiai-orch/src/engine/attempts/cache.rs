use crate::harness;
use crate::harness::compiler_cache;
use crate::policy::compiler_cache::agent_cache_block_reason;
use crate::policy::settings::SandboxMode;

/// Gives an agent orchd's compiler cache when its sandbox can reach it. When it
/// cannot, a wrapper the owner's cargo config names (sccache) fails there too,
/// so it is turned off, unless the owner set `RUSTC_WRAPPER` on purpose.
pub(super) fn apply_agent_cache(
    cmd: &mut tokio::process::Command,
    cache: Option<&compiler_cache::CompilerCache>,
    req: &harness::RunRequest<'_>,
    sandbox: SandboxMode,
    owner_wrapper: bool,
) -> Option<String> {
    let Some(reason) = agent_cache_block_reason(
        req.harness,
        req.network_allowed,
        req.review,
        sandbox,
        cache.is_some_and(|cache| cache.dir.starts_with(req.worktree)),
    ) else {
        if let Some(cache) = cache {
            cache.apply(cmd);
        }
        return None;
    };
    if !owner_wrapper {
        crate::sandbox::disable_compiler_wrapper(cmd.as_std_mut());
    }
    cache.map(|_| format!("Orchestrator: compiler cache disabled for agent: {reason}"))
}
