//! The `sushiai-orchestrator` skill, so an agent in any repository knows how to drive the
//! orchestrator. `sushiai orch register` installs it through the bin's skill folders.

use std::path::Path;

const SKILL: &str = include_str!("../skills/sushiai-orchestrator/SKILL.md");
/// The skill's name, as its frontmatter declares it.
pub const NAME: &str = "sushiai-orchestrator";

/// The skill with this install's `sushiai` binary and data dir filled in.
pub fn render(sushiai: &Path, data_dir: &Path) -> String {
    SKILL
        .replace("{{SUSHIAI}}", &sushiai.to_string_lossy())
        .replace("{{DATA}}", &data_dir.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_its_name_and_the_paths_of_this_install() {
        let text = render(Path::new("/opt/sushiai"), Path::new("/d/orchestrator"));
        assert!(text.starts_with(&format!("---\nname: {NAME}\n")));
        assert!(text.contains("-- \"/opt/sushiai\" mcp\n"));
        assert!(!text.contains("{{"));
    }
}
