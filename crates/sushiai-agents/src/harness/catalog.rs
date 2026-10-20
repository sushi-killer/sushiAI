//! What each agent CLI offers: the models a person picks from, the reasoning
//! levels of each, the smaller set the routine builder offers, the permission
//! modes, and the defaults.
//!
//! One table for the whole product. The daemon serves it (`agents.catalog`), the
//! CLI prints it (`sushiai agents catalog`), the desktop, the routines page and
//! the link read it. Nothing else holds a model or an effort list.
//!
//! The table is the same on every host except Codex's models, which the CLI
//! caches per host. The caller reads the files and hands the text in
//! ([`Facts`]); this module parses and never touches the disk.

use serde::{Deserialize, Serialize};

/// One model of a CLI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub id: String,
    pub label: String,
    pub description: String,
    /// The reasoning levels this model accepts, in the CLI's own order.
    pub efforts: Vec<String>,
    /// The level the CLI uses when the caller names none; empty when it has none.
    pub default_effort: String,
    /// The context window in tokens; 0 when the CLI does not say.
    pub context: u64,
}

/// Everything one agent CLI offers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub id: String,
    pub label: String,
    /// The models in offer order. Empty when the CLI lists none.
    pub models: Vec<Model>,
    /// The levels the chat composer offers.
    pub efforts: Vec<String>,
    /// The smaller set the routine builder offers.
    pub builder_efforts: Vec<String>,
    /// The permission modes. Empty for a CLI without interactive approvals.
    pub permissions: Vec<String>,
    /// The model the CLI uses when the caller names none: the id to pass back to
    /// it. Empty when the CLI has no default of its own.
    pub default_model: String,
    /// How that model is written for a person; the id again when the CLI lists
    /// no label for it.
    pub default_model_label: String,
    /// The level the CLI uses when the caller names none.
    pub default_effort: String,
    /// The window of the default model; 0 when it is unknown.
    pub default_context: u64,
}

/// Params of `agents.catalog`: which Codex home to read. The caller's own
/// (`CODEX_HOME`, else `~/.codex`) when it names none.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Query {
    #[serde(default)]
    pub config_home: Option<String>,
}

/// What the caller read from the disk. This crate parses, never reads.
#[derive(Debug, Default, Clone)]
pub struct Facts {
    /// Codex's `models_cache.json`, when the file exists.
    pub codex_cache: Option<String>,
    /// Codex's `config.toml`, when the file exists.
    pub codex_config: Option<String>,
}

/// The levels a Claude chat accepts, in offer order.
pub(super) const CLAUDE_EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max"];

/// The levels a Codex chat accepts. Codex publishes `ultra` on top of Claude's.
pub(super) const CODEX_EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max", "ultra"];

/// The levels the routine builder offers. The headless builder runs unattended,
/// so it stays on the levels both CLIs share.
pub(super) const BUILDER_EFFORTS: &[&str] = &["low", "medium", "high"];

/// The permission modes the composer offers. Codex has no interactive approvals
/// in `exec` and maps a mode onto its sandbox; Claude takes the mode as it is.
pub(super) const PERMISSIONS: &[&str] = &["default", "acceptEdits", "plan", "bypassPermissions"];

/// The context window to show before the CLI reports its real one.
pub(super) const CLAUDE_CONTEXT: u64 = 200_000;

/// A list of static words as the catalog carries them.
pub(super) fn words(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

/// Codex's own model cache (`$CODEX_HOME/models_cache.json`), shaped for a
/// picker. A hidden model stays out; a broken or missing cache is an empty list.
pub(super) fn codex_models(cache: Option<&str>, levels: &[&str]) -> Vec<Model> {
    #[derive(Deserialize)]
    struct Cache {
        #[serde(default)]
        models: Vec<Raw>,
    }
    #[derive(Deserialize)]
    struct Raw {
        /// A missing `slug` drops the row; an empty one does not, as before.
        slug: Option<String>,
        #[serde(default)]
        display_name: String,
        #[serde(default)]
        description: String,
        #[serde(default)]
        visibility: String,
        #[serde(default)]
        priority: Option<i64>,
        #[serde(default)]
        context_window: Option<u64>,
        #[serde(default)]
        default_reasoning_level: String,
        #[serde(default)]
        supported_reasoning_levels: Vec<Level>,
    }
    #[derive(Deserialize)]
    struct Level {
        #[serde(default)]
        effort: String,
    }

    let Some(text) = cache else {
        return Vec::new();
    };
    let Ok(cache) = serde_json::from_str::<Cache>(text) else {
        return Vec::new();
    };
    let mut rows: Vec<(i64, Model)> = cache
        .models
        .into_iter()
        .filter(|m| m.visibility == "list")
        .filter_map(|m| {
            let slug = m.slug?;
            let model = Model {
                label: match m.display_name.is_empty() {
                    true => slug.clone(),
                    false => m.display_name,
                },
                id: slug,
                description: m.description,
                efforts: m
                    .supported_reasoning_levels
                    .into_iter()
                    .map(|l| l.effort)
                    .filter(|e| levels.contains(&e.as_str()))
                    .collect(),
                default_effort: if levels.contains(&m.default_reasoning_level.as_str()) {
                    m.default_reasoning_level
                } else {
                    String::new()
                },
                context: m.context_window.unwrap_or(0),
            };
            Some((m.priority.unwrap_or(999), model))
        })
        .collect();
    rows.sort_by_key(|(priority, _)| *priority);
    rows.into_iter().map(|(_, model)| model).collect()
}

/// Codex's configured default model, resolved against the models its cache
/// lists: the id to pass back, the label to show and the window. A model the
/// cache does not list keeps its id as the label and no window.
pub(super) fn codex_default(config: Option<&str>, models: &[Model]) -> (String, String, u64) {
    let id = toml_root(config, "model");
    let listed = models.iter().find(|m| m.id == id);
    (
        id.clone(),
        listed.map_or_else(|| id.clone(), |m| m.label.clone()),
        listed.map_or(0, |m| m.context),
    )
}

/// Top-level `key = "value"` of a TOML document; empty when it is absent, of
/// another type, or the document does not parse.
pub(super) fn toml_root(text: Option<&str>, key: &str) -> String {
    let Some(text) = text else {
        return String::new();
    };
    let Ok(doc) = text.parse::<toml_edit::DocumentMut>() else {
        return String::new();
    };
    doc.get(key)
        .and_then(toml_edit::Item::as_str)
        .unwrap_or_default()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape of the real cache: a listed model, a hidden one, one without a
    /// slug, one with no priority, and a level outside the CLI's own list.
    const CACHE: &str = r#"{
      "models": [
        {"slug":"hidden","visibility":"hide","priority":1},
        {"visibility":"list","slug":"no-priority"},
        {"slug":"b","display_name":"B","description":"second","visibility":"list",
         "priority":2,"context_window":1000,"default_reasoning_level":"bogus",
         "supported_reasoning_levels":[{"effort":"low"},{"effort":"nope"}]},
        {"slug":"a","display_name":"A","description":"first","visibility":"list",
         "priority":1,"context_window":2000,"default_reasoning_level":"low",
         "supported_reasoning_levels":[{"effort":"high"},{"effort":"low"}]}
      ]
    }"#;

    #[test]
    fn the_cache_becomes_rows_in_priority_order() {
        let models = codex_models(Some(CACHE), CODEX_EFFORTS);
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        // `a` (1) and `b` (2) first, then the row with no priority (999). The
        // hidden row and the row without a slug are gone.
        assert_eq!(ids, ["a", "b", "no-priority"]);
        assert_eq!(models[0].label, "A");
        assert_eq!(models[0].efforts, ["high", "low"]);
        assert_eq!(models[0].default_effort, "low");
        assert_eq!(models[0].context, 2000);
        // A level the CLI does not list is dropped, and so is a default.
        assert_eq!(models[1].efforts, ["low"]);
        assert_eq!(models[1].default_effort, "");
        // A model with no display name shows its slug, and no window is 0.
        assert_eq!(models[2].label, "no-priority");
        assert_eq!(models[2].context, 0);
    }

    #[test]
    fn a_broken_or_missing_cache_is_an_empty_list() {
        assert!(codex_models(None, CODEX_EFFORTS).is_empty());
        assert!(codex_models(Some("not json"), CODEX_EFFORTS).is_empty());
        assert!(codex_models(Some("{}"), CODEX_EFFORTS).is_empty());
    }

    #[test]
    fn only_a_root_key_of_a_valid_document_is_read() {
        let text =
            "model = \"gpt-5\"\nmodel_reasoning_effort = \"high\"\n[tui]\nmodel = \"other\"\n";
        assert_eq!(toml_root(Some(text), "model"), "gpt-5");
        assert_eq!(toml_root(Some(text), "model_reasoning_effort"), "high");
        assert_eq!(toml_root(Some(text), "absent"), "");
        assert_eq!(toml_root(None, "model"), "");
        assert_eq!(toml_root(Some("[not toml"), "model"), "");
        // A literal string is a value too, which a line regex would have missed.
        assert_eq!(toml_root(Some("model = 'plain'\n"), "model"), "plain");
    }
}
