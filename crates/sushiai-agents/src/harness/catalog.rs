//! What each agent CLI offers: models, reasoning levels, the smaller set the
//! routine builder offers, permission modes and defaults. The daemon serves it as
//! `agents.catalog`.
//!
//! The table is the same on every host except Codex's models, which its CLI
//! caches per host. The caller reads those files and passes the text in
//! ([`Facts`]); this module parses and never touches the disk. The shape is the
//! wire shape, so it lives in `sushiai-protocol` and is re-exported here.

use serde::Deserialize;

pub use sushiai_protocol::agents::{AgentCatalog as Catalog, CatalogModel as Model};

/// What the caller read from the disk.
#[derive(Debug, Default, Clone)]
pub struct Facts {
    /// Codex's `models_cache.json`, when the file exists.
    pub codex_cache: Option<String>,
    /// Codex's `config.toml`, when the file exists.
    pub codex_config: Option<String>,
}

/// The levels a Claude chat accepts, in offer order.
pub(super) const CLAUDE_EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max"];

/// The levels a Codex chat accepts: Claude's plus `ultra`.
pub(super) const CODEX_EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max", "ultra"];

/// The levels the routine builder offers a Codex run: the first three, since the
/// headless builder runs unattended. Claude's builder offers its whole scale.
pub(super) const BUILDER_EFFORTS: &[&str] = &["low", "medium", "high"];

/// The permission modes the composer offers. Codex maps a mode onto its sandbox;
/// Claude takes it as it is.
pub(super) const PERMISSIONS: &[&str] = &["default", "acceptEdits", "plan", "bypassPermissions"];

/// The context window to show before the CLI reports its real one.
pub(super) const CLAUDE_CONTEXT: u64 = 200_000;

pub(super) fn words(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

/// Codex's model cache (`models_cache.json`) shaped for a picker. A hidden model
/// stays out; a broken or missing cache is an empty list.
pub(super) fn codex_models(cache: Option<&str>, levels: &[&str]) -> Vec<Model> {
    /// An explicit `null` reads as the field's default, like a missing key.
    fn or_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
    where
        D: serde::Deserializer<'de>,
        T: Deserialize<'de> + Default,
    {
        Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
    }
    /// Rows stay raw so one broken row is skipped alone, not the whole list.
    #[derive(Deserialize)]
    struct Cache {
        #[serde(default, deserialize_with = "or_default")]
        models: Vec<serde_json::Value>,
    }
    #[derive(Deserialize)]
    struct Raw {
        /// A missing `slug` drops the row; an empty one does not, as before.
        slug: Option<String>,
        #[serde(default, deserialize_with = "or_default")]
        display_name: String,
        #[serde(default, deserialize_with = "or_default")]
        description: String,
        #[serde(default, deserialize_with = "or_default")]
        visibility: String,
        #[serde(default)]
        priority: Option<i64>,
        #[serde(default)]
        context_window: Option<u64>,
        #[serde(default, deserialize_with = "or_default")]
        default_reasoning_level: String,
        #[serde(default, deserialize_with = "or_default")]
        supported_reasoning_levels: Vec<Level>,
    }
    #[derive(Deserialize)]
    struct Level {
        #[serde(default, deserialize_with = "or_default")]
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
        .filter_map(|row| serde_json::from_value::<Raw>(row).ok())
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

/// Codex's configured default model as (id, label, window). A model the cache
/// does not list keeps its id as the label and has no window.
pub(super) fn codex_default(config: Option<&str>, models: &[Model]) -> (String, String, u64) {
    let id = toml_root(config, "model");
    let listed = models.iter().find(|m| m.id == id);
    (
        id.clone(),
        listed.map_or_else(|| id.clone(), |m| m.label.clone()),
        listed.map_or(0, |m| m.context),
    )
}

/// Top-level `key = "value"` of a TOML document; empty when absent, not a string
/// or the document does not parse.
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

    /// A listed model, a hidden one, one without a slug, one with no priority,
    /// and a level outside the CLI's own list.
    const CACHE: &str = r#"{
      "models": [
        {"slug":"hidden","visibility":"hide","priority":1},
        {"slug":7,"visibility":"list"},
        {"slug":"nulls","display_name":null,"description":null,"visibility":"list",
         "priority":3,"default_reasoning_level":null,
         "supported_reasoning_levels":[{"effort":null},{"effort":"low"}]},
        {"slug":"null-levels","visibility":"list","priority":4,
         "supported_reasoning_levels":null},
        {"visibility":"list","slug":"no-priority"},
        {"slug":"b","display_name":"B","visibility":"list","priority":2,
         "default_reasoning_level":"bogus",
         "supported_reasoning_levels":[{"effort":"low"},{"effort":"nope"}]},
        {"slug":"a","display_name":"A","visibility":"list","priority":1,
         "context_window":2000,"default_reasoning_level":"low",
         "supported_reasoning_levels":[{"effort":"high"},{"effort":"low"}]}
      ]
    }"#;

    #[test]
    fn the_cache_becomes_rows_in_priority_order() {
        let models = codex_models(Some(CACHE), CODEX_EFFORTS);
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        // The row with no priority sorts last; the hidden and slugless rows are gone.
        // A row with a non-string slug is skipped alone; explicit nulls read as empty.
        assert_eq!(ids, ["a", "b", "nulls", "null-levels", "no-priority"]);
        assert_eq!(
            (models[0].efforts.as_slice(), models[0].context),
            (&["high".to_owned(), "low".to_owned()][..], 2000)
        );
        // A level outside the CLI's list is dropped, and so is a default outside it.
        assert_eq!(models[1].efforts, ["low"]);
        assert_eq!(models[1].default_effort, "");
        // No display name shows the slug.
        assert_eq!(models[4].label, "no-priority");
        assert_eq!(models[2].label, "nulls");
        assert_eq!(models[2].efforts, ["low"]);
        assert!(models[3].efforts.is_empty());
        for broken in [None, Some("not json"), Some("{}")] {
            assert!(codex_models(broken, CODEX_EFFORTS).is_empty());
        }
    }

    #[test]
    fn only_a_root_key_of_a_valid_document_is_read() {
        let text = "model = \"gpt-5\"\n[tui]\nmodel = \"other\"\n";
        assert_eq!(toml_root(Some(text), "model"), "gpt-5");
        assert_eq!(toml_root(Some(text), "absent"), "");
        assert_eq!(toml_root(None, "model"), "");
        assert_eq!(toml_root(Some("[not toml"), "model"), "");
        // A literal string is a value too, which a line regex would have missed.
        assert_eq!(toml_root(Some("model = 'plain'\n"), "model"), "plain");
    }
}
