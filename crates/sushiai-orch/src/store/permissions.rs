use crate::store;
use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex as StdMutex;

/// Rules the owner granted: per repo (`Always for this repo`, inherited by
/// every later task) and per task (`enable:<tool id>` from the brief check, an
/// answer to a denial after the run). A rule is `tool:<claude tool name>`,
/// `path:<repo-relative path>` (a trailing `/**` covers a directory) or
/// `enable:<connected tool id>`.
#[derive(Default, Serialize, Deserialize)]
struct Grants {
    #[serde(default)]
    repos: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    tasks: BTreeMap<String, Vec<String>>,
}

static GRANTS_LOCK: StdMutex<()> = StdMutex::new(());

fn grants_path(data_dir: &Path) -> PathBuf {
    data_dir.join("permissions.json")
}

fn load_grants(data_dir: &Path) -> Grants {
    std::fs::read_to_string(grants_path(data_dir))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub(crate) enum Scope<'a> {
    Repo(&'a str),
    Task(&'a str),
}

pub(crate) fn add_rule(data_dir: &Path, scope: Scope, rule: &str) {
    let _guard = GRANTS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut grants = load_grants(data_dir);
    let list = match scope {
        Scope::Repo(repo) => grants.repos.entry(repo.to_string()).or_default(),
        Scope::Task(id) => grants.tasks.entry(id.to_string()).or_default(),
    };
    if !list.iter().any(|r| r == rule) {
        list.push(rule.to_string());
        let _ = store::files::write_json_atomic(&grants_path(data_dir), &grants);
    }
}

/// Every rule that applies to `task`: its repo's, then its own.
pub(crate) fn rules_for(data_dir: &Path, repo: &str, task_id: &str) -> Vec<String> {
    let grants = load_grants(data_dir);
    grants
        .repos
        .get(repo)
        .into_iter()
        .chain(grants.tasks.get(task_id))
        .flatten()
        .cloned()
        .collect()
}
