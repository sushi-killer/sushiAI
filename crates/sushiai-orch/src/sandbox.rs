//! Filesystem and socket policy shared by task implement runs and checks.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub struct Inputs<'a> {
    pub repo_root: &'a Path,
    pub worktree: &'a Path,
    pub run_dir: &'a Path,
    pub data_dir: &'a Path,
    pub sushiai_home: &'a Path,
    pub worktree_root: &'a str,
    pub home: &'a Path,
    pub daemon_socket: &'a Path,
    pub cargo_home: Option<&'a Path>,
    pub rustup_home: Option<&'a Path>,
    pub tmp_dir: Option<&'a Path>,
    pub npm_cache: Option<&'a Path>,
    pub known_project_folders: &'a [PathBuf],
    pub known_task_worktrees: &'a [PathBuf],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskSandbox {
    pub write_roots: Vec<PathBuf>,
    pub denied_read_roots: Vec<PathBuf>,
    pub read_tool_denied_roots: Vec<PathBuf>,
    pub read_exceptions: Vec<PathBuf>,
    pub read_metadata_paths: Vec<PathBuf>,
    pub read_metadata_subpaths: Vec<PathBuf>,
    pub daemon_socket: PathBuf,
    pub check_env: BTreeMap<String, String>,
    pub check_env_remove_prefixes: Vec<String>,
}

// Resolve existing ancestors too: run directories and caches may not exist yet.
fn resolved(path: &Path) -> PathBuf {
    if let Ok(path) = path.canonicalize() {
        return path;
    }
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) if parent != path => resolved(parent).join(name),
        _ => path.to_path_buf(),
    }
}

fn git_dir(root: &Path) -> PathBuf {
    let dot_git = root.join(".git");
    if let Ok(text) = std::fs::read_to_string(&dot_git) {
        if let Some(dir) = text.trim().strip_prefix("gitdir: ") {
            return resolved(&root.join(dir));
        }
    }
    resolved(&dot_git)
}

// Claude tool denies cannot be lifted by allowRead. Enumerate siblings along
// each kept path instead of denying a parent that also contains that path.
fn deny_except(root: &Path, keep: &[&Path], denied: &mut Vec<PathBuf>) {
    if keep.iter().any(|path| root.starts_with(path)) {
        return;
    }
    if !keep.iter().any(|path| path.starts_with(root)) {
        denied.push(root.to_path_buf());
        return;
    }
    if let Ok(children) = std::fs::read_dir(root) {
        for child in children.flatten() {
            let child = resolved(&child.path());
            // A symlink back to an ancestor contributes no new subtree.
            if root.starts_with(&child) {
                continue;
            }
            deny_except(&child, keep, denied);
        }
    }
}

fn denied_roots(
    inputs: &Inputs<'_>,
    repo: &Path,
    worktree: &Path,
    data_dir: &Path,
    home: &Path,
) -> Vec<PathBuf> {
    let mut denied_read_roots = vec![
        data_dir.to_path_buf(),
        resolved(inputs.sushiai_home).join("worktrees.noindex"),
        home.join(".claude/projects"),
        home.join(".codex/sessions"),
        home.join(".codex/archived_sessions"),
    ];
    for project in std::iter::once(inputs.repo_root)
        .chain(inputs.known_project_folders.iter().map(PathBuf::as_path))
    {
        denied_read_roots.push(crate::git::worktree::worktree_path(
            project,
            inputs.sushiai_home,
            inputs.worktree_root,
            "",
        ));
    }
    // Existing tasks retain their paths when worktreeRoot changes.
    denied_read_roots.extend(
        inputs
            .known_task_worktrees
            .iter()
            .map(|path| resolved(path))
            .filter(|path| *path != worktree),
    );
    for project in inputs.known_project_folders {
        let project = resolved(project);
        if project == repo || project == worktree {
            continue;
        }
        if repo.starts_with(&project) {
            deny_except(&project, &[repo, worktree], &mut denied_read_roots);
        } else {
            denied_read_roots.push(project);
        }
    }
    denied_read_roots
}

impl TaskSandbox {
    /// Builds from explicit inputs and a read-only snapshot of their paths;
    /// no app state, process environment or files are changed.
    pub fn build(inputs: Inputs<'_>) -> Self {
        let repo = resolved(inputs.repo_root);
        let worktree = resolved(inputs.worktree);
        let run_dir = resolved(inputs.run_dir);
        let data_dir = resolved(inputs.data_dir);
        let home = resolved(inputs.home);
        let git_dir = git_dir(if worktree.join(".git").exists() {
            &worktree
        } else {
            &repo
        });
        let common = std::fs::read_to_string(git_dir.join("commondir"))
            .map(|dir| resolved(&git_dir.join(dir.trim())))
            .unwrap_or_else(|_| git_dir.clone());
        let mut write_roots = vec![
            worktree.clone(),
            common,
            git_dir,
            run_dir.clone(),
            PathBuf::from("/dev"),
            resolved(inputs.cargo_home.unwrap_or(&home.join(".cargo"))),
            resolved(inputs.rustup_home.unwrap_or(&home.join(".rustup"))),
            resolved(inputs.npm_cache.unwrap_or(&home.join(".npm"))),
            home.join(".cache"),
            // Codex writes its own state; its session directories stay unreadable.
            home.join(".codex"),
        ];
        #[cfg(target_os = "macos")]
        write_roots.extend([
            PathBuf::from("/private/var/folders"),
            PathBuf::from("/private/tmp"),
            home.join("Library/Caches"),
        ]);
        #[cfg(not(target_os = "macos"))]
        write_roots.extend([PathBuf::from("/tmp"), PathBuf::from("/var/tmp")]);
        if let Some(tmp) = inputs.tmp_dir {
            write_roots.push(resolved(tmp));
        }
        let modules = worktree.join("node_modules");
        if modules.is_symlink() && resolved(&modules) == resolved(&repo.join("node_modules")) {
            write_roots.push(resolved(&modules));
        }

        let mut denied_read_roots = denied_roots(&inputs, &repo, &worktree, &data_dir, &home);
        write_roots = write_roots.iter().map(|path| resolved(path)).collect();
        denied_read_roots = denied_read_roots
            .iter()
            .map(|path| resolved(path))
            .collect();
        write_roots.sort();
        write_roots.dedup();
        denied_read_roots.sort();
        denied_read_roots.dedup();
        let mut read_tool_denied_roots = Vec::new();
        for root in &denied_read_roots {
            // Keep per-run credentials denied to Claude's unsandboxed Read
            // tool, even though the task's shell can read its own run.
            let keep: &[&Path] = if *root == data_dir {
                &[&worktree]
            } else {
                &[&worktree, &run_dir]
            };
            deny_except(root, keep, &mut read_tool_denied_roots);
        }
        read_tool_denied_roots.sort();
        read_tool_denied_roots.dedup();
        let read_exceptions = vec![run_dir, worktree];
        // Codex create_dir_all checks existing date directories with stat
        // before creating a session file. Metadata access does not expose
        // session contents, which remain denied by denied_read_roots.
        let read_metadata_subpaths = vec![home.join(".codex/sessions")];
        // Cargo resolves the allowed paths through their otherwise hidden parents.
        let mut read_metadata_paths: Vec<PathBuf> = read_exceptions
            .iter()
            .flat_map(|path| path.ancestors().skip(1))
            .filter(|path| denied_read_roots.iter().any(|root| path.starts_with(root)))
            .map(Path::to_path_buf)
            .collect();
        read_metadata_paths.sort();
        read_metadata_paths.dedup();
        Self {
            write_roots,
            denied_read_roots,
            read_tool_denied_roots,
            read_exceptions,
            read_metadata_paths,
            read_metadata_subpaths,
            daemon_socket: resolved(inputs.daemon_socket),
            check_env: [
                ("ELECTRON_DISABLE_SANDBOX".into(), "1".into()),
                ("RUSTC_WRAPPER".into(), String::new()),
            ]
            .into(),
            check_env_remove_prefixes: vec!["SCCACHE_".into()],
        }
    }

    /// Apply the check environment owned by this policy, including removing
    /// inherited compiler-cache configuration that cannot run inside Seatbelt.
    pub fn apply_check_env(&self, command: &mut std::process::Command) {
        command.envs(&self.check_env);
        for (key, _) in std::env::vars_os().filter(|(key, _)| {
            let key = key.to_string_lossy();
            self.check_env_remove_prefixes
                .iter()
                .any(|prefix| key.starts_with(prefix))
        }) {
            command.env_remove(key);
        }
    }
}

/// Seatbelt's later rules reopen this run and worktree under denied roots.
pub fn to_sbpl(sandbox: &TaskSandbox) -> String {
    let quoted = |path: &Path| serde_json::to_string(&path.to_string_lossy()).unwrap();
    let mut profile = String::from("(version 1)\n(allow default)\n");
    for path in &sandbox.denied_read_roots {
        profile.push_str(&format!("(deny file-read* (subpath {}))\n", quoted(path)));
    }
    for path in &sandbox.read_exceptions {
        profile.push_str(&format!("(allow file-read* (subpath {}))\n", quoted(path)));
    }
    for path in &sandbox.read_metadata_paths {
        profile.push_str(&format!(
            "(allow file-read-metadata (literal {}))\n",
            quoted(path)
        ));
    }
    for path in &sandbox.read_metadata_subpaths {
        profile.push_str(&format!(
            "(allow file-read-metadata (subpath {}))\n",
            quoted(path)
        ));
    }
    profile.push_str("(deny file-write* (subpath \"/\"))\n");
    for path in &sandbox.write_roots {
        profile.push_str(&format!("(allow file-write* (subpath {}))\n", quoted(path)));
    }
    profile.push_str(&format!(
        "(deny network-outbound (remote unix-socket (path-literal {})))\n",
        quoted(&sandbox.daemon_socket),
    ));
    profile
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        harness,
        policy::settings::{SandboxMode, Settings},
        sandbox::to_sbpl as task_sandbox_profile,
    };

    fn policy(root: &Path, projects: &[PathBuf]) -> TaskSandbox {
        TaskSandbox::build(Inputs {
            repo_root: &root.join("projects/repo"),
            worktree: &root.join("data/worktree"),
            run_dir: &root.join("data/runs/own"),
            data_dir: &root.join("data"),
            sushiai_home: &root.join("sushiai"),
            worktree_root: "",
            home: &root.join("home"),
            daemon_socket: &root.join("daemon.sock"),
            cargo_home: None,
            rustup_home: None,
            tmp_dir: Some(&root.join("tmp")),
            npm_cache: Some(&root.join("npm-cache")),
            known_project_folders: projects,
            known_task_worktrees: &[],
        })
    }

    #[test]
    fn ancestor_projects_deny_other_children_and_resolve_aliases() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        for name in ["projects/repo", "projects/sibling", "foreign"] {
            std::fs::create_dir_all(root.join(name)).unwrap();
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join("foreign"), root.join("alias")).unwrap();
            std::os::unix::fs::symlink(
                root.join("projects/repo"),
                root.join("projects/repo-alias"),
            )
            .unwrap();
            std::os::unix::fs::symlink(root.join("projects"), root.join("projects/parent-alias"))
                .unwrap();
        }
        let built = policy(
            &root,
            &[
                root.join("projects"),
                root.join("projects/repo"),
                root.join("foreign"),
            ],
        );
        assert!(built
            .denied_read_roots
            .contains(&root.join("projects/sibling")));
        assert!(built.denied_read_roots.contains(&root.join("foreign")));
        assert!(!built
            .denied_read_roots
            .iter()
            .any(|dir| root.join("projects/repo").starts_with(dir)));
        #[cfg(unix)]
        assert!(policy(&root, &[root.join("alias")])
            .denied_read_roots
            .contains(&root.join("foreign")));
    }

    #[test]
    fn default_configured_and_retained_worktree_roots_are_private() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let repo = root.join("projects/repo");
        let sibling = root.join("projects/sibling");
        let sushiai_home = root.join("sushiai");
        let absolute = root.join("custom-worktrees");
        let retained = root.join("old-worktrees/task-other");
        let data = sushiai_home.join("orchestrator");
        let run = data.join("tasks/own/runs/1");
        for setting in [
            "",
            ".sushiai/worktrees",
            ".tasks",
            absolute.to_str().unwrap(),
        ] {
            let worktree =
                crate::git::worktree::worktree_path(&repo, &sushiai_home, setting, "task/own");
            let built = TaskSandbox::build(Inputs {
                repo_root: &repo,
                worktree: &worktree,
                run_dir: &run,
                data_dir: &data,
                sushiai_home: &sushiai_home,
                worktree_root: setting,
                home: &root.join("user"),
                daemon_socket: &sushiai_home.join("daemon.sock"),
                cargo_home: None,
                rustup_home: None,
                tmp_dir: None,
                npm_cache: None,
                known_project_folders: std::slice::from_ref(&sibling),
                known_task_worktrees: &[retained.clone(), worktree.clone()],
            });
            for denied in [
                sushiai_home.join("worktrees.noindex"),
                crate::git::worktree::worktree_path(&repo, &sushiai_home, setting, ""),
                crate::git::worktree::worktree_path(&sibling, &sushiai_home, setting, ""),
                retained.clone(),
            ] {
                assert!(
                    built.denied_read_roots.contains(&denied),
                    "{setting}: {denied:?}"
                );
            }
            assert!(!built.denied_read_roots.contains(&worktree));
            assert!(built.read_exceptions.contains(&worktree));
            let sbpl = task_sandbox_profile(&built);
            let allowed = format!("(allow file-read* (subpath \"{}\"))", worktree.display());
            for denied in &built.denied_read_roots {
                let deny = format!("(deny file-read* (subpath \"{}\"))", denied.display());
                assert!(sbpl.find(&deny).unwrap() < sbpl.find(&allowed).unwrap());
            }
            for parent in &built.read_metadata_paths {
                assert!(worktree.starts_with(parent) || run.starts_with(parent));
                assert!(sbpl.contains(&format!(
                    "(allow file-read-metadata (literal \"{}\"))",
                    parent.display()
                )));
            }
            assert!(!built.read_metadata_paths.contains(&retained));
            let settings = harness::build_claude_task_settings(None, Some(&built), &[], None);
            assert_eq!(
                settings["sandbox"]["filesystem"]["denyRead"],
                serde_json::json!(built.denied_read_roots)
            );
            assert_eq!(
                settings["sandbox"]["filesystem"]["allowRead"],
                serde_json::json!(built.read_exceptions)
            );
        }
    }

    #[test]
    fn claude_read_tool_denies_siblings_without_matching_own_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let repo = root.join("repo");
        let sushiai_home = root.join("sushiai");
        let data = sushiai_home.join("orchestrator");
        let run = data.join("tasks/own/runs/1");
        let absolute = root.join("custom-worktrees");
        for setting in [
            "",
            ".sushiai/worktrees",
            ".tasks",
            absolute.to_str().unwrap(),
            "orchestrator/worktrees",
        ] {
            let worktree = if setting == "orchestrator/worktrees" {
                data.join("worktrees/own")
            } else {
                crate::git::worktree::worktree_path(&repo, &sushiai_home, setting, "task/own")
            };
            let sibling = worktree.with_file_name("task-other");
            let foreign = sushiai_home.join("worktrees.noindex/other-repo/task-other");
            for path in [&worktree, &sibling, &foreign, &run] {
                std::fs::create_dir_all(path).unwrap();
            }
            let built = TaskSandbox::build(Inputs {
                repo_root: &repo,
                worktree: &worktree,
                run_dir: &run,
                data_dir: &data,
                sushiai_home: &sushiai_home,
                worktree_root: setting,
                home: &root.join("home"),
                daemon_socket: &sushiai_home.join("daemon.sock"),
                cargo_home: None,
                rustup_home: None,
                tmp_dir: None,
                npm_cache: None,
                known_project_folders: &[],
                known_task_worktrees: &[],
            });
            let settings = harness::build_claude_task_settings(None, Some(&built), &[], None);
            let denied: Vec<PathBuf> = settings["permissions"]["deny"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|rule| rule.as_str()?.strip_prefix("Read(/")?.strip_suffix("/**)"))
                .map(PathBuf::from)
                .collect();
            for own_file in [worktree.join("src/main.rs"), worktree.join(".env")] {
                assert!(
                    !denied.iter().any(|path| own_file.starts_with(path)),
                    "{setting}: Claude cannot read {own_file:?}: {denied:?}"
                );
            }
            for secret in [
                sibling.join(".env"),
                foreign.join(".env"),
                run.join("secret"),
            ] {
                assert!(
                    denied.iter().any(|path| secret.starts_with(path)),
                    "{setting}: Claude can read {secret:?}: {denied:?}"
                );
            }
        }
    }

    #[test]
    fn data_dir_exceptions_and_both_renderers_keep_other_runs_private() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let policy = policy(&root, &[]);
        let sbpl = task_sandbox_profile(&policy);
        let denied = format!(
            "(deny file-read* (subpath \"{}\"))",
            root.join("data").display()
        );
        assert!(sbpl.contains("(allow default)"));
        for exception in &policy.read_exceptions {
            let allowed = format!("(allow file-read* (subpath \"{}\"))", exception.display());
            assert!(sbpl.find(&denied).unwrap() < sbpl.find(&allowed).unwrap());
        }
        assert_eq!(
            policy.read_exceptions,
            vec![root.join("data/runs/own"), root.join("data/worktree")]
        );
        assert_eq!(
            policy.read_metadata_subpaths,
            vec![root.join("home/.codex/sessions")]
        );
        assert!(sbpl.contains(&format!(
            "(allow file-read-metadata (subpath \"{}\"))",
            root.join("home/.codex/sessions").display()
        )));
        assert!(sbpl.contains(&format!(
            "(deny network-outbound (remote unix-socket (path-literal \"{}\")))",
            policy.daemon_socket.display()
        )));
        let settings =
            harness::build_claude_task_settings(None, Some(&policy), &["*".into()], None);
        assert_eq!(
            settings["sandbox"]["filesystem"]["denyRead"],
            serde_json::json!(policy.denied_read_roots)
        );
        assert_eq!(
            settings["sandbox"]["filesystem"]["allowWrite"],
            serde_json::json!(policy.write_roots)
        );
        for denied in &policy.read_tool_denied_roots {
            assert!(settings["permissions"]["deny"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!(format!(
                    "Read(/{}/**)",
                    denied.display()
                ))));
        }
        assert_eq!(
            settings["sandbox"]["filesystem"]["allowRead"],
            serde_json::json!(policy.read_exceptions)
        );
        let host = harness::build_claude_task_settings(None, None, &[], None);
        assert!(host.get("sandbox").is_none());
        assert_eq!(host["permissions"]["deny"].as_array().unwrap().len(), 2);
        // Review/plan settings retain their original data-dir deny only.
        let review =
            harness::build_claude_settings(None, SandboxMode::Native, &[], &["/data".into()], None);
        assert_eq!(
            review["sandbox"]["filesystem"],
            serde_json::json!({"denyRead": ["/data"]})
        );
        assert_eq!(review["permissions"]["deny"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn check_env_is_applied_only_to_native_checks() {
        let tmp = tempfile::tempdir().unwrap();
        let policy = policy(tmp.path(), &[]);
        assert_eq!(
            policy.check_env,
            [
                ("ELECTRON_DISABLE_SANDBOX".into(), "1".into()),
                ("RUSTC_WRAPPER".into(), String::new()),
            ]
            .into()
        );
    }

    #[test]
    fn sandbox_for_keeps_saved_values_and_matches_symlinked_repos() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let mut settings = Settings::default();
        settings
            .sandbox_repos
            .insert(repo.display().to_string(), SandboxMode::Host);
        assert_eq!(
            settings.sandbox_for(repo.to_str().unwrap()),
            SandboxMode::Host
        );
        assert_eq!(settings.sandbox_for("/another-repo"), SandboxMode::Native);
        #[cfg(unix)]
        {
            let alias = tmp.path().join("alias");
            std::os::unix::fs::symlink(&repo, &alias).unwrap();
            assert_eq!(
                settings.sandbox_for(alias.to_str().unwrap()),
                SandboxMode::Host
            );
        }
        assert_eq!(settings.sandbox, SandboxMode::Native);
        let mut saved = serde_json::to_value(&settings).unwrap();
        saved["sandbox"] = serde_json::json!("host");
        saved.as_object_mut().unwrap().remove("sandboxRepos");
        let loaded: Settings = serde_json::from_value(saved).unwrap();
        assert!(loaded.sandbox_repos.is_empty());
        assert_eq!(
            loaded.sandbox_for(repo.to_str().unwrap()),
            SandboxMode::Host
        );
        assert_eq!(loaded.sandbox, SandboxMode::Host);
    }

    #[test]
    fn linked_modules_are_writable_only_when_they_belong_to_this_checkout() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("projects/repo/node_modules")).unwrap();
        std::fs::create_dir_all(root.join("data/worktree")).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(
                root.join("projects/repo/node_modules"),
                root.join("data/worktree/node_modules"),
            )
            .unwrap();
            assert!(policy(&root, &[])
                .write_roots
                .contains(&root.join("projects/repo/node_modules")));
            std::fs::remove_file(root.join("data/worktree/node_modules")).unwrap();
            std::fs::create_dir(root.join("foreign_modules")).unwrap();
            std::os::unix::fs::symlink(
                root.join("foreign_modules"),
                root.join("data/worktree/node_modules"),
            )
            .unwrap();
            assert!(!policy(&root, &[])
                .write_roots
                .contains(&root.join("foreign_modules")));
        }
    }
}
