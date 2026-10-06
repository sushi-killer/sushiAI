//! `orchd serve --install-skill`: puts the `sushiai-orchestrator` skill into
//! the owner's Claude Code and Codex skill folders, so an agent in any
//! repository knows how to drive orchd. Written only when its content
//! changed; a folder whose tool is not installed is left alone. Skipped for
//! a test launch and for a binary under a cargo `target/` dir, so only the
//! installed app's orchd writes it.

use std::path::Path;

const SKILL: &str = include_str!("../skills/sushiai-orchestrator/SKILL.md");
const NAME: &str = "sushiai-orchestrator";

/// The skill with this install's orchd binary and data dir filled in.
fn render(orchd: &Path, data_dir: &Path) -> String {
    SKILL
        .replace("{{ORCHD}}", &orchd.to_string_lossy())
        .replace("{{DATA}}", &data_dir.to_string_lossy())
}

/// Whether this process may install: not a test launch, not a dev build.
pub fn allowed(orchd: &Path) -> bool {
    let test = ["SUSHIAI_TEST_WINDOW", "SUSHIAI_MCP_HOME"]
        .iter()
        .any(|k| std::env::var_os(k).is_some());
    !test && !orchd.components().any(|c| c.as_os_str() == "target")
}

/// Installs under `<home>/{.claude,.codex}/skills` for each tool folder that
/// exists; returns the files it wrote.
pub fn install(home: &Path, orchd: &Path, data_dir: &Path) -> Vec<std::path::PathBuf> {
    let text = render(orchd, data_dir);
    let mut written = Vec::new();
    for tool in [".claude", ".codex"] {
        if !home.join(tool).is_dir() {
            continue;
        }
        let path = home.join(tool).join("skills").join(NAME).join("SKILL.md");
        if std::fs::read_to_string(&path).ok().as_deref() == Some(text.as_str()) {
            continue;
        }
        let saved = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&path, &text));
        match saved {
            Ok(()) => written.push(path),
            Err(e) => eprintln!("orchd: could not install {}: {e}", path.display()),
        }
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installs_into_each_present_tool_folder_once_with_paths_filled_in() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join(".claude")).unwrap();
        std::fs::create_dir(home.path().join(".codex")).unwrap();
        std::fs::create_dir(home.path().join(".agents")).unwrap();
        let (orchd, data) = (Path::new("/opt/orchd"), Path::new("/d/orchestrator"));

        let written = install(home.path(), orchd, data);
        assert_eq!(written.len(), 2);
        // One Codex folder only, so a Codex that reads both never loads it twice.
        assert!(!home.path().join(".agents/skills").exists());
        let text = std::fs::read_to_string(&written[0]).unwrap();
        assert!(text.starts_with("---\nname: sushiai-orchestrator\n"));
        assert!(text.contains("-- \"/opt/orchd\" mcp --data \"/d/orchestrator\""));
        assert!(!text.contains("{{"));

        assert!(install(home.path(), orchd, data).is_empty());
    }

    #[test]
    fn a_dev_build_never_installs() {
        assert!(!allowed(Path::new("/w/orchd/target/release/orchd")));
    }
}
