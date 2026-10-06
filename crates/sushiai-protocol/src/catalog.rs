//! Catalog wire types: projects, groups, session bindings and their sync
//! params. Additive, camelCase, no logic. The session title stays on
//! `SessionInfo`; a binding carries only project and group.

use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectFolder {
    pub host: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub folders: Vec<ProjectFolder>,
    pub rev: u64,
    /// Milliseconds since the epoch, set by the writer.
    pub updated_at: u64,
    #[serde(default)]
    pub deleted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub order: i64,
    pub rev: u64,
    pub updated_at: u64,
    #[serde(default)]
    pub deleted: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionBinding {
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub group: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectsSync {
    pub host: String,
    pub projects: Vec<Project>,
    /// `true`: the payload is the complete set; absent records are deleted.
    #[serde(default)]
    pub full: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupsSync {
    pub groups: Vec<Group>,
    #[serde(default)]
    pub full: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncResult {
    pub applied: u32,
    pub ignored: u32,
    /// Highest `rev` held in the catalog after the call.
    pub rev: u64,
}

/// Result of `catalog.get`: the live records as stored from the syncs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogSnapshot {
    pub host: String,
    pub projects: Vec<Project>,
    pub groups: Vec<Group>,
}

/// Distinguishes a missing field (leave as is) from `null` (clear).
fn double_option<'de, D, T>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(de).map(Some)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionUpdate {
    pub id: String,
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub project: Option<Option<String>>,
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub group: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}
