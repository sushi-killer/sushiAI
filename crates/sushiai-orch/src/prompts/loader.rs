//! The orchestrator's prompts: defaults ship in `prompts/orchestrator.yaml`,
//! and `<data dir>/prompts.yaml` overrides any key. The override is read on
//! every use, so an edit applies to the next turn without a restart.
//!
//! The task agents' brief texts ship in `prompts/brief.yaml`: compiled in,
//! parsed once and never overridden (see [`brief_text`]). The engine's and the
//! chat agent's turn texts do the same from `prompts/engine.yaml` (see
//! [`engine_text`]).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const DEFAULTS: &str = include_str!("../../prompts/orchestrator.yaml");
const ATTRIBUTION: &str = include_str!("../../prompts/attribution.yaml");
const BRIEF: &str = include_str!("../../prompts/brief.yaml");
const ENGINE: &str = include_str!("../../prompts/engine.yaml");

static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Where `prompts.yaml` is looked up; set once by `sushiai mcp`.
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
                Err(e) => eprintln!("sushiai: ignoring {}: {e}", path.display()),
            }
        }
    }
    default(key)
}

pub fn default(key: &str) -> String {
    parse(DEFAULTS)
        .expect("prompts/orchestrator.yaml parses")
        .remove(key)
        .or_else(|| {
            parse(ATTRIBUTION)
                .expect("prompts/attribution.yaml parses")
                .remove(key)
        })
        .unwrap_or_else(|| panic!("prompts/orchestrator.yaml has no {key}"))
}

/// The brief text for `key` from `prompts/brief.yaml`. Never read at run
/// time and never overridden by `<data dir>/prompts.yaml`: the briefs are
/// part of every run's `promptHash`. A missing key panics.
pub fn brief_text(key: &str) -> &'static str {
    static TEXTS: OnceLock<BTreeMap<String, String>> = OnceLock::new();
    TEXTS
        .get_or_init(|| parse(BRIEF).expect("prompts/brief.yaml parses"))
        .get(key)
        .unwrap_or_else(|| panic!("prompts/brief.yaml has no {key}"))
}

/// The engine and chat agent-turn text for `key` from `prompts/engine.yaml`:
/// compiled in, parsed once, never overridden. A missing key panics.
pub fn engine_text(key: &str) -> &'static str {
    static TEXTS: OnceLock<BTreeMap<String, String>> = OnceLock::new();
    TEXTS
        .get_or_init(|| parse(ENGINE).expect("prompts/engine.yaml parses"))
        .get(key)
        .unwrap_or_else(|| panic!("prompts/engine.yaml has no {key}"))
}

/// `text` with every `{name}` that `values` names replaced by its value, in
/// one pass: an inserted value is never scanned again, and a brace that names
/// no value (JSON in a report format) stays as written.
pub fn fill(text: &str, values: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let value = after.find('}').and_then(|close| {
            let name = &after[..close];
            values
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (close, *value))
        });
        match value {
            Some((close, value)) => {
                out.push_str(value);
                rest = &after[close + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
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

/// `actual` against `tests/fixtures/prompt-snapshots/engine/<kind>.txt`, byte
/// for byte. A missing golden fails; `UPDATE_PROMPT_SNAPSHOTS=1` writes it.
///
/// The goldens hold JSON objects in insertion order, as the shipped binary
/// renders them: `sushiai-agents` turns on serde_json's `preserve_order`, so
/// it is on in every workspace build. A build of this crate alone sorts the
/// keys instead, and there the golden's compact JSON lines are sorted too
/// before the comparison.
#[cfg(test)]
pub(crate) fn assert_engine_snapshot(kind: &str, actual: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/prompt-snapshots/engine")
        .join(format!("{kind}.txt"));
    if std::env::var_os("UPDATE_PROMPT_SNAPSHOTS").is_some() {
        assert!(
            preserves_json_order(),
            "write the goldens from a workspace build: cargo test --workspace --lib prompt_snapshot_"
        );
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let mut golden = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing golden {}", path.display()));
    if !preserves_json_order() {
        golden = sort_json_lines(&golden);
    }
    assert!(
        actual == golden,
        "{kind} differs from its golden:\n{actual}"
    );
}

#[cfg(test)]
fn preserves_json_order() -> bool {
    let map = serde_json::json!({"b": 0, "a": 0});
    map.as_object().unwrap().keys().next().unwrap() == "b"
}

/// Each line that is a whole compact JSON object, re-serialized with sorted
/// keys; every other line as it is.
#[cfg(test)]
fn sort_json_lines(text: &str) -> String {
    text.split('\n')
        .map(
            |line| match serde_json::from_str::<serde_json::Value>(line) {
                Ok(value @ serde_json::Value::Object(_))
                    if value.to_string().len() == line.len() =>
                {
                    value.to_string()
                }
                _ => line.to_string(),
            },
        )
        .collect::<Vec<_>>()
        .join("\n")
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
    fn the_attribution_prompt_is_generic_and_ships() {
        let prompt = default("attribution");
        assert!(prompt.contains("sushi-attribution"));
        assert!(!prompt.ends_with('\n'));
    }

    #[test]
    fn every_brief_text_parses_and_ends_without_a_newline() {
        let all = parse(BRIEF).unwrap();
        assert!(all.len() > 50, "{}", all.len());
        for (key, text) in &all {
            assert!(!text.trim().is_empty(), "{key}");
            assert!(!text.ends_with('\n'), "{key}");
            assert_eq!(brief_text(key), text, "{key}");
        }
    }

    #[test]
    fn no_brief_text_shares_a_key_with_the_overridable_prompts() {
        // `get` and its `<data dir>/prompts.yaml` override serve the
        // orchestrator's keys; `brief_text` reads only the compiled-in file.
        let orchestrator = parse(DEFAULTS).unwrap();
        let shared: Vec<String> = parse(BRIEF)
            .unwrap()
            .into_keys()
            .filter(|key| orchestrator.contains_key(key))
            .collect();
        assert!(shared.is_empty(), "{shared:?}");
    }

    #[test]
    fn every_engine_text_parses_and_shares_no_key_with_the_other_files() {
        let all = parse(ENGINE).unwrap();
        let others: Vec<_> = parse(DEFAULTS)
            .unwrap()
            .into_keys()
            .chain(parse(BRIEF).unwrap().into_keys())
            .collect();
        for (key, text) in &all {
            assert!(!text.trim().is_empty(), "{key}");
            assert!(!text.ends_with('\n'), "{key}");
            assert_eq!(engine_text(key), text, "{key}");
            assert!(!others.contains(key), "{key}");
        }
    }

    #[test]
    fn fill_inserts_each_value_verbatim_in_one_pass() {
        let text = "{a} then {b}, {\"json\": 1} and {unknown}";
        assert_eq!(
            fill(text, &[("a", "x {b} {a}"), ("b", "{other}")]),
            "x {b} {a} then {other}, {\"json\": 1} and {unknown}"
        );
        assert_eq!(
            fill("{other}{name}", &[("name", "{other}"), ("other", "Y")]),
            "Y{other}"
        );
        assert_eq!(fill("{a", &[("a", "x")]), "{a");
        assert_eq!(fill("}{}{a}", &[("a", "x")]), "}{}x");
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
