use crate::policy::routing::Harness;
use crate::policy::settings::SandboxMode;

pub(crate) fn agent_cache_block_reason(
    harness: Harness,
    network_allowed: bool,
    review: bool,
    sandbox: SandboxMode,
    cache_in_worktree: bool,
) -> Option<&'static str> {
    if harness == Harness::Codex && !network_allowed {
        return Some("Codex codexNetwork false blocks localhost access");
    }
    if harness == Harness::Codex && review {
        return Some("Codex read-only sandbox cannot write the cache directory");
    }
    // Codex always uses its own workspace sandbox, including in Host mode.
    // Native Claude runs grant the worktree, not the shared cache directory.
    if (harness == Harness::Codex || sandbox == SandboxMode::Native) && !cache_in_worktree {
        return Some("agent sandbox cannot write the shared cache directory");
    }
    None
}
