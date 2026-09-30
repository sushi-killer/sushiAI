//! The orchestrator's prompts: defaults ship in `prompts/orchestrator.yaml`,
//! and `<data dir>/prompts.yaml` overrides any key. The override is read on
//! every use, so an edit applies to the next turn without a restart.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const DEFAULTS: &str = include_str!("../prompts/orchestrator.yaml");

static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Where `prompts.yaml` is looked up; set once by `orchd serve` / `orchd mcp`.
pub fn set_data_dir(dir: &Path) {
    let _ = DATA_DIR.set(dir.to_path_buf());
}

/// The prompt for `key`: the override's when it has one, else the default.
pub fn get(key: &str) -> String {
    lookup(DATA_DIR.get().map(PathBuf::as_path), key)
}

fn lookup(dir: Option<&Path>, key: &str) -> String {
    if let Some(dir) = dir {
        let path = dir.join("prompts.yaml");
        if let Ok(text) = std::fs::read_to_string(&path) {
            match parse(&text) {
                Ok(mut over) => {
                    // An empty block would blank the prompt: keep the default.
                    if let Some(prompt) = over.remove(key).filter(|p| !p.is_empty()) {
                        return prompt;
                    }
                }
                Err(e) => eprintln!("orchd: ignoring {}: {e}", path.display()),
            }
        }
    }
    default(key)
}

pub fn default(key: &str) -> String {
    parse(DEFAULTS)
        .expect("prompts/orchestrator.yaml parses")
        .remove(key)
        .unwrap_or_else(|| panic!("prompts/orchestrator.yaml has no {key}"))
}

/// ponytail: only the YAML this file needs -- top-level `key: |` block
/// scalars indented by two spaces, `#` comments and blank lines. Anything
/// else is an error, not a guess; switch to a YAML crate if a prompt ever
/// needs more.
fn parse(text: &str) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    let mut current: Option<(String, Vec<&str>)> = None;
    let mut finish = |current: &mut Option<(String, Vec<&str>)>| {
        if let Some((key, lines)) = current.take() {
            out.insert(key, lines.join("\n").trim_end().to_string());
        }
    };
    let mut seen = std::collections::BTreeSet::new();
    for (n, line) in text.lines().enumerate() {
        if let Some(rest) = line.strip_prefix("  ") {
            match current.as_mut() {
                Some((_, lines)) => lines.push(rest),
                None => return Err(format!("line {}: indented text outside a key", n + 1)),
            }
        } else if line.trim().is_empty() {
            if let Some((_, lines)) = current.as_mut() {
                lines.push("");
            }
        } else if line.starts_with('#') {
            finish(&mut current);
        } else if let Some(key) = line.strip_suffix(": |") {
            finish(&mut current);
            let key = key.trim();
            if key.is_empty() || key.contains(char::is_whitespace) {
                return Err(format!("line {}: bad key {key:?}", n + 1));
            }
            if !seen.insert(key.to_string()) {
                return Err(format!("line {}: {key} appears twice", n + 1));
            }
            current = Some((key.to_string(), Vec::new()));
        } else {
            return Err(format!(
                "line {}: expected `key: |`, a comment or text indented by two spaces",
                n + 1
            ));
        }
    }
    finish(&mut current);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_default_prompt_parses() {
        let all = parse(DEFAULTS).unwrap();
        for key in [
            "chat",
            "brainstorm",
            "plan",
            "mcp_instructions",
            "mcp_task_instructions",
        ] {
            assert!(all[key].len() > 100, "{key}");
            assert!(!all[key].ends_with('\n'), "{key}");
        }
    }

    #[test]
    fn a_block_keeps_its_blank_lines_and_ends_at_the_next_key() {
        let text = "# c\nfirst: |\n  one\n\n  two\n\nsecond: |\n  three\n";
        let all = parse(text).unwrap();
        assert_eq!(all["first"], "one\n\ntwo");
        assert_eq!(all["second"], "three");
    }

    #[test]
    fn an_override_replaces_only_its_own_keys() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("prompts.yaml"),
            "plan: |\n  Plan in haiku.\n",
        )
        .unwrap();
        assert_eq!(lookup(Some(dir.path()), "plan"), "Plan in haiku.");
        assert_eq!(lookup(Some(dir.path()), "chat"), default("chat"));
        std::fs::write(dir.path().join("prompts.yaml"), "plan: oops\n").unwrap();
        assert_eq!(lookup(Some(dir.path()), "plan"), default("plan"));
        std::fs::write(dir.path().join("prompts.yaml"), "plan: |\nchat: |\n  x\n").unwrap();
        assert_eq!(lookup(Some(dir.path()), "plan"), default("plan"));
    }

    #[test]
    fn anything_but_the_supported_shape_is_an_error() {
        assert!(parse("key: value\n").is_err());
        assert!(parse("  stray\n").is_err());
        assert!(parse("two words: |\n  x\n").is_err());
        assert!(parse("a: |\n  x\na: |\n  y\n").is_err());
    }
}
