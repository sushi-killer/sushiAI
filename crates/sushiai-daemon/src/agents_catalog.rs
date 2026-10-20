//! `agents.catalog`: what each agent CLI on this host offers. The table itself
//! lives in `sushiai-agents`; this module only finds and reads the files that
//! table parses.

use std::path::{Path, PathBuf};

use sushiai_agents::harness;
use sushiai_agents::harness::catalog::Facts;
use sushiai_protocol::agents::AgentCatalog as Catalog;

/// Codex's config home: this process's own `CODEX_HOME` (the one a Codex it
/// launches would read), else `~/.codex`.
fn codex_home() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CODEX_HOME").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
    Some(PathBuf::from(home).join(".codex"))
}

/// A file's text, or `None` when it is missing or unreadable: a host without a
/// Codex cache still has a catalog.
fn read(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// The catalog of every agent, in registry order.
pub fn agents() -> Vec<Catalog> {
    agents_in(codex_home().as_deref())
}

/// The catalog with Codex's files read from `codex`.
fn agents_in(codex: Option<&Path>) -> Vec<Catalog> {
    let file = |name: &str| codex.and_then(|d| read(&d.join(name)));
    harness::catalogs(&Facts {
        codex_cache: file("models_cache.json"),
        codex_config: file("config.toml"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, text: &str) {
        std::fs::write(dir.join(name), text).expect("the fixture is writable");
    }

    /// The home's two files reach the table: the cache lists the models, the
    /// config names the default.
    #[test]
    fn a_codex_home_is_read() {
        let dir = tempfile::tempdir().expect("a temp dir");
        write(
            dir.path(),
            "models_cache.json",
            r#"{"models":[{"slug":"gpt-x","display_name":"GPT-X","visibility":"list",
                 "priority":1,"context_window":400000,"default_reasoning_level":"high",
                 "supported_reasoning_levels":[{"effort":"high"}]}]}"#,
        );
        write(dir.path(), "config.toml", "model = \"gpt-x\"\n");
        let codex = agents_in(Some(dir.path()))
            .into_iter()
            .find(|c| c.id == "codex")
            .expect("codex is registered");
        assert_eq!(codex.models.len(), 1);
        assert_eq!(codex.default_model, "gpt-x");
        assert_eq!(codex.default_context, 400_000);
        // Every other agent is unaffected by a Codex home.
        let claude = agents_in(Some(dir.path()))
            .into_iter()
            .find(|c| c.id == "claude")
            .expect("claude is registered");
        assert_eq!(claude.models.len(), 4);
    }

    /// A home with neither file is not an error: the levels are still named.
    #[test]
    fn a_home_without_files_still_answers() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let codex = agents_in(Some(dir.path()))
            .into_iter()
            .find(|c| c.id == "codex")
            .expect("codex is registered");
        assert!(codex.models.is_empty());
        assert_eq!(codex.default_model, "");
        assert!(!codex.efforts.is_empty());
    }
}
