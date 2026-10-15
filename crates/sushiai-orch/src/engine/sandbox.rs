use crate::engine::state::App;
use crate::policy::settings::SandboxMode;
use crate::sandbox::Inputs as SandboxInputs;
use crate::sandbox::TaskSandbox;
use std::path::Path;
use std::path::PathBuf;
impl App {
    pub(super) fn task_sandbox(
        &self,
        repo: &str,
        worktree: &Path,
        run_dir: &Path,
    ) -> Option<TaskSandbox> {
        let worktree_root = {
            let settings = self.settings.read().unwrap();
            if settings.sandbox_for(repo) == SandboxMode::Host {
                return None;
            }
            settings.worktree_root.clone()
        };
        let mut projects: Vec<PathBuf> = self
            .secrets
            .read()
            .unwrap()
            .repo_projects
            .keys()
            .map(PathBuf::from)
            .collect();
        let tasks = self.store.list_tasks().unwrap_or_default();
        projects.extend(tasks.iter().map(|task| PathBuf::from(&task.repo)));
        let worktrees: Vec<PathBuf> = tasks
            .iter()
            .filter(|task| !task.worktree.is_empty())
            .map(|task| PathBuf::from(&task.worktree))
            .collect();
        let home = PathBuf::from(std::env::var_os("HOME").expect("HOME for task sandbox"));
        let cargo_home = std::env::var_os("CARGO_HOME").map(PathBuf::from);
        let rustup_home = std::env::var_os("RUSTUP_HOME").map(PathBuf::from);
        let tmp_dir = std::env::var_os("TMPDIR").map(PathBuf::from);
        let npm_cache = std::env::var_os("npm_config_cache")
            .or_else(|| std::env::var_os("NPM_CONFIG_CACHE"))
            .map(PathBuf::from);
        Some(TaskSandbox::build(SandboxInputs {
            repo_root: Path::new(repo),
            worktree,
            run_dir,
            data_dir: &self.data_dir,
            sushiai_home: &self.home,
            worktree_root: &worktree_root,
            home: &home,
            daemon_socket: &self.home.join("daemon.sock"),
            cargo_home: cargo_home.as_deref(),
            rustup_home: rustup_home.as_deref(),
            tmp_dir: tmp_dir.as_deref(),
            npm_cache: npm_cache.as_deref(),
            known_project_folders: &projects,
            known_task_worktrees: &worktrees,
        }))
    }
}
