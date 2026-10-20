//! `agents.catalog`: reads this host's Codex files and hands them to the table in
//! `sushiai-agents`.

use std::path::{Path, PathBuf};

use sushiai_agents::harness;
use sushiai_agents::harness::catalog::Facts;
use sushiai_protocol::agents::AgentCatalog as Catalog;

/// `CODEX_HOME` as a Codex launched by this process would see it, else `~/.codex`.
fn codex_home() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CODEX_HOME").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
    Some(PathBuf::from(home).join(".codex"))
}

/// A missing or unreadable file is `None`: a host without a Codex cache still has
/// a catalog.
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

    fn codex(dir: &Path) -> Catalog {
        agents_in(Some(dir))
            .into_iter()
            .find(|c| c.id == "codex")
            .expect("codex is registered")
    }

    /// A home without files still names the levels; once the cache and config
    /// are there, the models and the default are read.
    #[test]
    fn a_codex_home_is_read() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let empty = codex(dir.path());
        assert!(empty.models.is_empty());
        assert_eq!(empty.default_model, "");
        assert!(!empty.efforts.is_empty());

        let write = |name: &str, text: &str| {
            std::fs::write(dir.path().join(name), text).expect("the fixture is writable");
        };
        write(
            "models_cache.json",
            r#"{"models":[{"slug":"gpt-x","display_name":"GPT-X","visibility":"list",
                 "priority":1,"context_window":400000,"default_reasoning_level":"high",
                 "supported_reasoning_levels":[{"effort":"high"}]}]}"#,
        );
        write("config.toml", "model = \"gpt-x\"\n");
        let read = codex(dir.path());
        assert_eq!(read.models.len(), 1);
        assert_eq!(read.default_model, "gpt-x");
        assert_eq!(read.default_context, 400_000);
    }
}
