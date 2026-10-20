#[cfg(test)]
use crate::engine::attempts::secrets::run_secrets;
#[cfg(test)]
use crate::engine::test_support::task_with_status;
#[cfg(test)]
use crate::engine::test_support::test_app;
use crate::git::paths::same_repo_path;
#[cfg(test)]
use crate::harness::cancel::CancelToken;
use crate::store;
#[cfg(test)]
use crate::task::TaskStatus;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
#[cfg(test)]
use std::time::Duration;

pub(super) fn project_for_repo(
    secrets: &Secrets,
    repo: &str,
    stored: Option<&str>,
) -> Result<Option<String>, String> {
    let repo_path = std::fs::canonicalize(repo).unwrap_or_else(|_| PathBuf::from(repo));
    let matches = |folder: &str| {
        same_repo_path(folder, repo)
            || repo_path.starts_with(
                std::fs::canonicalize(folder).unwrap_or_else(|_| PathBuf::from(folder)),
            )
    };
    if let Some(id) = stored {
        return if secrets
            .repo_projects
            .iter()
            .any(|(folder, project)| project == id && matches(folder))
        {
            Ok(Some(id.to_string()))
        } else {
            Err(format!("projectId {id:?} does not match repo {repo:?}"))
        };
    }
    let exact = secrets
        .repo_projects
        .iter()
        .find(|(folder, _)| same_repo_path(folder, repo));
    let containing = || {
        secrets
            .repo_projects
            .iter()
            .filter(|(folder, _)| matches(folder))
            .max_by_key(|(folder, _)| (Path::new(folder).components().count(), *folder))
    };
    Ok(exact.or_else(containing).map(|(_, id)| id.clone()))
}

/// The default branch of the project whose folder is `repo` (or holds it,
/// the deepest such folder).
pub(super) fn branch_for_repo(secrets: &Secrets, repo: &str) -> Option<String> {
    let repo_path = std::fs::canonicalize(repo).unwrap_or_else(|_| PathBuf::from(repo));
    let exact = secrets
        .project_branches
        .iter()
        .find(|(folder, _)| same_repo_path(folder, repo));
    let containing = || {
        secrets
            .project_branches
            .iter()
            .filter(|(folder, _)| {
                repo_path.starts_with(
                    std::fs::canonicalize(folder).unwrap_or_else(|_| PathBuf::from(folder)),
                )
            })
            .max_by_key(|(folder, _)| (Path::new(folder).components().count(), *folder))
    };
    exact.or_else(containing).map(|(_, branch)| branch.clone())
}

pub(super) fn redact_old_run_mcp(data_dir: &Path, secrets: &Secrets) {
    let marker = data_dir.join("run-mcp-redacted");
    if marker.exists() {
        return;
    }
    let mut values: Vec<_> = secrets
        .projects
        .values()
        .chain(secrets.project_mcp.values())
        .flat_map(|env| env.iter())
        .filter(|(_, value)| !value.is_empty())
        .collect();
    values.sort_by_key(|(name, value)| (std::cmp::Reverse(value.len()), *name));
    fn redact(value: &mut serde_json::Value, values: &[(&String, &String)]) {
        match value {
            serde_json::Value::String(text) => {
                for (name, secret) in values {
                    *text = text.replace(secret.as_str(), &format!("${{{name}}}"));
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    redact(item, values);
                }
            }
            serde_json::Value::Object(items) => {
                for (key, item) in items {
                    if (key == "env" || key == "headers") && item.is_object() {
                        for (name, value) in item.as_object_mut().unwrap() {
                            if value.is_string() {
                                *value = json!(format!("${{{name}}}"));
                            } else {
                                redact(value, values);
                            }
                        }
                    } else {
                        redact(item, values);
                    }
                }
            }
            _ => {}
        }
    }
    fn walk(dir: &Path, values: &[(&String, &String)]) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() {
                walk(&path, values);
            } else if kind.is_file() && entry.file_name() == "mcp.json" {
                if let Ok(mut config) = std::fs::read(&path).and_then(|bytes| {
                    serde_json::from_slice::<serde_json::Value>(&bytes)
                        .map_err(std::io::Error::other)
                }) {
                    redact(&mut config, values);
                    let _ = store::files::write_json_private(&path, &config);
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
                }
            }
        }
    }
    if let Ok(tasks) = std::fs::read_dir(data_dir.join("tasks")) {
        for task in tasks.flatten() {
            if task.file_type().is_ok_and(|kind| kind.is_dir()) {
                walk(&task.path().join("runs"), &values);
            }
        }
    }
    let _ = store::files::write_text_private(&marker, "");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stored_project_mismatch_gets_no_agent_or_mcp_values() {
        let (app, dir) = test_app();
        let mut task = task_with_status(TaskStatus::Stopped);
        task.fixture_data().repo = dir.path().to_string_lossy().into_owned();
        task.fixture_data().project_id = Some("old-project".into());
        app.store.save_task(&task).unwrap();
        let params = |project| {
            json!({
                "projects": {"old-project": {"PROJECT_TOKEN": "invented-agent-secret"}},
                "projectMcp": {"old-project": {"MCP_TOKEN": "invented-mcp-secret"}},
                "projectRepos": {&task.repo: project}
            })
        };
        app.handle_secrets_set(params("old-project")).await.unwrap();
        let values = run_secrets(&app, &task.id).unwrap();
        assert_eq!(
            values.env.get("PROJECT_TOKEN").unwrap(),
            "invented-agent-secret"
        );
        assert!(!values.env.contains_key("MCP_TOKEN"));
        assert_eq!(
            values.redaction.get("MCP_TOKEN").unwrap(),
            "invented-mcp-secret"
        );
        let settings = app.settings.read().unwrap().clone();
        assert_eq!(
            crate::engine::permissions::tools::task_tools(&app, &settings, &task, dir.path())
                .env
                .len(),
            2
        );
        app.handle_secrets_set(params("new-project")).await.unwrap();
        let error = run_secrets(&app, &task.id).err().unwrap();
        assert!(
            error.contains("old-project") && error.contains(&task.repo),
            "{error}"
        );
        assert!(
            crate::engine::permissions::tools::task_tools(&app, &settings, &task, dir.path())
                .env
                .is_empty()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_run_after_a_restart_waits_for_the_app_values_instead_of_failing() {
        let (app, dir) = test_app();
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().repo = dir.path().to_string_lossy().into_owned();
        task.fixture_data().project_id = Some("mine".into());
        app.store.save_task(&task).unwrap();
        // A fresh daemon: nothing sent yet, so the project check would fail.
        assert!(run_secrets(&app, &task.id).is_err());

        let sender = app.clone();
        let repo = task.repo.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await; // timing: the owner's values arrive late, after the run began to wait; the wait has no observable "started" signal
            sender
                .handle_secrets_set(json!({
                    "projects": {"mine": {"KEY": "invented-value"}},
                    "projectRepos": {repo: "mine"}
                }))
                .await
                .unwrap();
        });
        // The values must end the wait, not its 120 s limit: half of it is virtual time here.
        tokio::time::timeout(
            Duration::from_secs(60),
            crate::engine::attempts::secrets::wait_for_app_secrets(
                &app,
                &task.id,
                &CancelToken::new(),
            ),
        )
        .await
        .expect("the wait ended on the values, not on its limit");
        assert_eq!(
            run_secrets(&app, &task.id).unwrap().env.get("KEY").unwrap(),
            "invented-value"
        );
    }

    #[test]
    fn old_run_mcp_redaction_is_private_and_runs_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tasks/task/runs/1/b/mcp.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = json!({"args": ["invented-secret-long", "invented-secret", "invented-mcp-secret"],
            "env": {"TOKEN": "unknown-value"}, "headers": {"Authorization": "Bearer unknown-value"}});
        std::fs::write(&path, original.to_string()).unwrap();
        let secrets = Secrets {
            projects: [(
                "project".into(),
                [
                    ("LONG".into(), "invented-secret-long".into()),
                    ("SHORT".into(), "invented-secret".into()),
                ]
                .into(),
            )]
            .into(),
            project_mcp: [(
                "project".into(),
                [("MCP".into(), "invented-mcp-secret".into())].into(),
            )]
            .into(),
            ..Default::default()
        };
        redact_old_run_mcp(dir.path(), &secrets);
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            value,
            json!({"args": ["${LONG}", "${SHORT}", "${MCP}"], "env": {"TOKEN": "${TOKEN}"}, "headers": {"Authorization": "${Authorization}"}})
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert!(dir.path().join("run-mcp-redacted").exists());
        std::fs::write(&path, original.to_string()).unwrap();
        redact_old_run_mcp(dir.path(), &secrets);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            original.to_string()
        );
    }

    #[test]
    fn project_for_repo_resolves_exact_containing_and_canonical_paths() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("project/repo");
        let sibling = dir.path().join("project-other/repo");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        let repo = repo.to_str().unwrap();
        let mut secrets = Secrets::default();
        assert_eq!(project_for_repo(&secrets, repo, None).unwrap(), None);
        assert!(project_for_repo(&secrets, repo, Some("foreign")).is_err());
        let parent = dir.path().join("project").to_string_lossy().into_owned();
        secrets
            .repo_projects
            .insert(parent.clone(), "parent".into());
        assert_eq!(
            project_for_repo(&secrets, repo, None).unwrap(),
            Some("parent".into())
        );
        assert_eq!(
            project_for_repo(&secrets, repo, Some("parent")).unwrap(),
            Some("parent".into())
        );
        assert!(project_for_repo(&secrets, sibling.to_str().unwrap(), Some("parent")).is_err());
        secrets.repo_projects.insert(repo.into(), "exact".into());
        assert_eq!(
            project_for_repo(&secrets, repo, None).unwrap(),
            Some("exact".into())
        );
        #[cfg(unix)]
        {
            let alias = dir.path().join("alias");
            std::os::unix::fs::symlink(&parent, &alias).unwrap();
            let alias_repo = alias.join("repo");
            assert_eq!(
                project_for_repo(&secrets, alias_repo.to_str().unwrap(), Some("exact")).unwrap(),
                Some("exact".into())
            );
            secrets.repo_projects.clear();
            secrets
                .repo_projects
                .insert(alias.to_str().unwrap().into(), "alias-parent".into());
            assert_eq!(
                project_for_repo(&secrets, repo, Some("alias-parent")).unwrap(),
                Some("alias-parent".into())
            );
        }
    }
}

/// A Claude model profile's secrets (spec `secrets.set`): env vars merged
/// into the run's `settings.json`, and an API key written to a per-run key
/// file the daemon points `apiKeyHelper` at (never inlined into JSON on
/// disk).
#[derive(Debug, Clone, Deserialize, Default)]
pub(super) struct ProfileSecret {
    #[serde(default)]
    pub(super) env: HashMap<String, String>,
    #[serde(default)]
    pub(super) key: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(super) struct AccountSecret {
    pub(super) kind: String,
    pub(super) value: String,
}

/// In-memory only (spec: "travel from Electron to the daemon in memory
/// only"); `secrets.set` is a full replace, never a merge.
#[derive(Default)]
pub(super) struct Secrets {
    pub(super) profiles: HashMap<String, ProfileSecret>,
    pub(super) accounts: HashMap<String, AccountSecret>,
    pub(super) projects: HashMap<String, HashMap<String, String>>,
    pub(super) project_mcp: HashMap<String, HashMap<String, String>>,
    /// Repo path -> project id, for a task made without one (the agent's own
    /// `task.create`): its project's values still reach it.
    pub(super) repo_projects: HashMap<String, String>,
    /// The app has sent the values since this daemon started; until then
    /// every map above is empty, not "no values".
    pub(super) received: bool,
    /// Repo path -> its project's default branch: what a task made with no
    /// `base` branches from.
    pub(super) project_branches: HashMap<String, String>,
}
