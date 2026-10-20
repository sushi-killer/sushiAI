//! A task whose base is named (`base` on `task.create`, or the project's
//! default branch) branches from that branch as its remote has it now: the
//! branch is fetched when the task is created and again when it starts. A
//! clean local checkout that is behind is fast-forwarded; one that is dirty
//! or has diverged is left as it is, and the task branches from the fetched
//! tip. A fetch that fails or takes too long leaves the task on the local
//! branch with a `Warning:` in its decisions; it never fails the task.

#[cfg(test)]
use serde_json::json;

#[cfg(test)]
use crate::engine::lifecycle::carry::carry_onto_newest_base;
#[cfg(test)]
use crate::engine::lifecycle::carry::BaseCarry;
#[cfg(test)]
use crate::harness::cancel::CancelToken;
#[cfg(test)]
use crate::task::TaskStatus;

use crate::clip::truncate_chars;
use crate::clock::now_ms;
use crate::engine::protected::carry::carry_with_planner_tests;
use crate::engine::secrets;
use crate::engine::state::App;
#[cfg(test)]
use crate::engine::test_support::attempt_with_failure;
#[cfg(test)]
use crate::engine::test_support::git_in;
#[cfg(test)]
use crate::engine::test_support::test_app;
use crate::git::command::{run_bounded_with, Bounded, BoundedError};
use crate::git::paths::short_sha;
use crate::task::FreshBase;
use crate::task::Task;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::policy::delivery::BaseFrom;

#[cfg(test)]
use std::io::Read;

/// How long one base fetch may take before the task goes on without it.
pub(super) const FETCH_TIMEOUT: Duration = Duration::from_secs(20);

impl App {
    pub(super) fn base_fetch_timeout(&self) -> Duration {
        Duration::from_millis(self.base_fetch_timeout_ms.load(Ordering::SeqCst))
    }

    /// The base of a top-level `task.create` that names none: the project's
    /// default branch when it has one, else `HEAD`.
    pub(super) fn default_base(&self, repo: &str) -> (String, BaseFrom) {
        match secrets::branch_for_repo(&self.secrets.read().unwrap(), repo) {
            Some(branch) => (branch, BaseFrom::Project),
            None => ("HEAD".to_string(), BaseFrom::Plain),
        }
    }
}

/// [`prepare`] for a base from `from`; [`Prepared::plain`] when it is not
/// named.
pub(super) fn prepare_new(repo: &Path, base: &str, from: BaseFrom, timeout: Duration) -> Prepared {
    match from {
        BaseFrom::Plain => Prepared::plain(repo, base),
        BaseFrom::Given => prepare(repo, base, false, timeout),
        BaseFrom::Project => prepare(repo, base, true, timeout),
    }
}

fn git_out(repo: &Path, args: &[&str]) -> Option<String> {
    crate::git::command::run(repo, args)
        .ok()
        .map(|out| out.trim().to_string())
}

/// Where a base branch is fetched from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Upstream {
    pub(super) remote: String,
    /// The branch's name on the remote.
    pub(super) branch: String,
}

impl Upstream {
    fn tracking_ref(&self) -> String {
        format!("refs/remotes/{}/{}", self.remote, self.branch)
    }

    fn name(&self) -> String {
        format!("{}/{}", self.remote, self.branch)
    }
}

/// The branch's configured upstream (`branch.<b>.remote`/`.merge`), else
/// the same name on `origin`; `None` when there is no remote to ask.
pub(super) fn upstream_of(repo: &Path, branch: &str) -> Option<Upstream> {
    let remotes = git_out(repo, &["remote"])?;
    let known = |name: &str| remotes.lines().any(|r| r.trim() == name);
    let configured = git_out(
        repo,
        &["config", "--get", &format!("branch.{branch}.remote")],
    )
    .filter(|r| !r.is_empty());
    if let Some(remote) = configured {
        // "." is a local upstream, and a URL has no tracking refs: neither is
        // a remote this fetch can follow.
        if !known(&remote) {
            return None;
        }
        let merge = git_out(
            repo,
            &["config", "--get", &format!("branch.{branch}.merge")],
        );
        let name = merge
            .as_deref()
            .map(|m| m.strip_prefix("refs/heads/").unwrap_or(m))
            .filter(|m| !m.is_empty())
            .unwrap_or(branch);
        return Some(Upstream {
            remote,
            branch: name.to_string(),
        });
    }
    known("origin").then(|| Upstream {
        remote: "origin".into(),
        branch: branch.into(),
    })
}

/// The line of git's stderr that says what went wrong.
fn fetch_error(stderr: &str) -> String {
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let line = lines
        .iter()
        .find(|l| l.starts_with("fatal:") || l.starts_with("error:"))
        .or(lines.last())
        .copied()
        .unwrap_or("git fetch failed");
    let line = line
        .strip_prefix("fatal: ")
        .or_else(|| line.strip_prefix("error: "))
        .unwrap_or(line);
    truncate_chars(line, 300)
}

/// `git fetch --no-tags` of one branch into its remote-tracking ref, with
/// nothing that could wait for a person: no terminal prompt, no askpass, ssh
/// in batch mode (unless the owner set their own ssh command). Killed after
/// `timeout`. Returns the fetched tip.
pub(super) fn fetch(repo: &Path, up: &Upstream, timeout: Duration) -> Result<String, String> {
    let refspec = format!("+refs/heads/{}:{}", up.branch, up.tracking_ref());
    let own_ssh = std::env::var_os("GIT_SSH_COMMAND").is_some()
        || std::env::var_os("GIT_SSH").is_some()
        || git_out(repo, &["config", "--get", "core.sshCommand"]).is_some();
    let mut env = vec![
        ("SSH_ASKPASS_REQUIRE", "never"),
        ("GCM_INTERACTIVE", "never"),
    ];
    if !own_ssh {
        env.push(("GIT_SSH_COMMAND", "ssh -o BatchMode=yes"));
    }
    let opts = Bounded {
        env: &env,
        env_remove: &["GIT_ASKPASS", "SSH_ASKPASS"],
        clean_index_lock: false,
    };
    let args = [
        "-c",
        "core.askPass=",
        "fetch",
        "--quiet",
        "--no-tags",
        "--no-recurse-submodules",
        "--no-write-fetch-head",
        "--",
        &up.remote,
        &refspec,
    ];
    run_bounded_with(repo, &args, timeout, Duration::from_secs(1), &opts).map_err(|e| match e {
        BoundedError::Io(e) => format!("git fetch: {e}"),
        BoundedError::TimedOut => format!("no answer within {timeout:?}"),
        BoundedError::Failed(stderr) => fetch_error(&stderr),
    })?;
    crate::git::history::resolve_commit(repo, &up.tracking_ref()).map_err(|e| e.to_string())
}

/// What a task's work is carried onto: its local base branch, unless that
/// branch does not contain the tip the task last fetched (a dirty or
/// diverged checkout left behind), then that tip.
pub(super) fn carry_rev(repo: &Path, base_ref: &str, tip: Option<&str>) -> String {
    match tip {
        Some(tip) if crate::git::history::is_ancestor(repo, tip, base_ref) == Some(false) => {
            tip.to_string()
        }
        _ => base_ref.to_string(),
    }
}

fn tip_of(task: &Task) -> Option<String> {
    task.fresh_base.as_ref().and_then(|f| f.tip.clone())
}

/// A named base branch after its fetch.
#[derive(Debug)]
pub(super) struct Fresh {
    /// What the task branches from.
    pub(super) rev: String,
    /// The remote tip, the one fetched now or, when the fetch failed, before.
    pub(super) tip: Option<String>,
    pub(super) warnings: Vec<String>,
}

/// Fetches local branch `branch` and lines the local branch up with it: a
/// clean checkout that is behind is fast-forwarded (`crate::git::branches::move_branch`, never
/// forced); one ahead or equal is kept; one dirty or diverged is left as it
/// is and the task takes the fetched tip, with a warning. `last_tip` is the
/// tip fetched before, kept when this fetch fails.
pub(super) fn refresh(
    repo: &Path,
    branch: &str,
    last_tip: Option<&str>,
    timeout: Duration,
) -> Fresh {
    let mut fresh = Fresh {
        rev: carry_rev(repo, branch, last_tip),
        tip: last_tip.map(str::to_string),
        warnings: vec![],
    };
    let Some(up) = upstream_of(repo, branch) else {
        return fresh;
    };
    let tip = match fetch(repo, &up, timeout) {
        Ok(tip) => tip,
        Err(why) => {
            let at = crate::git::history::resolve_commit(repo, &fresh.rev).unwrap_or_default();
            let from = if fresh.rev == branch {
                format!("the local {branch}")
            } else {
                format!("{} as fetched before", up.name())
            };
            fresh.warnings.push(format!(
                "Warning: could not fetch {branch} from {} ({why}); branching from {from} at {}",
                up.remote,
                short_sha(&at)
            ));
            return fresh;
        }
    };
    fresh.rev = branch.to_string();
    fresh.tip = Some(tip.clone());
    let local = crate::git::history::resolve_commit(repo, &format!("refs/heads/{branch}"))
        .unwrap_or_default();
    // Ahead of the remote, or level with it: the local branch is newest.
    if local == tip || crate::git::history::is_ancestor(repo, &tip, &local) == Some(true) {
        return fresh;
    }
    let mut left_as_is = |why: String| {
        fresh.rev = tip.clone();
        fresh.warnings.push(format!(
            "Warning: {why}, so it was left as it is; branching from the fetched {} at {}",
            up.name(),
            short_sha(&tip)
        ));
    };
    let diverged = format!("the local {branch} has diverged from {}", up.name());
    if crate::git::history::is_ancestor(repo, &local, &tip) != Some(true) {
        left_as_is(diverged);
        return fresh;
    }
    match crate::git::branches::move_branch(repo, branch, &tip) {
        Ok(crate::git::branches::BranchMove::Done { .. }) => {}
        Ok(crate::git::branches::BranchMove::Dirty { checkout }) => left_as_is(format!(
            "the local {branch} is behind {} but its checkout {} has uncommitted changes",
            up.name(),
            checkout.display()
        )),
        Ok(crate::git::branches::BranchMove::Diverged) => left_as_is(diverged),
        Err(e) => left_as_is(format!(
            "the local {branch} could not be fast-forwarded ({e})"
        )),
    }
    fresh
}

/// What `create_task_record` branches a new task from.
#[derive(Debug)]
pub(super) struct Prepared {
    pub(super) rev: String,
    pub(super) base_ref: Option<String>,
    pub(super) fresh: Option<FreshBase>,
    pub(super) warnings: Vec<String>,
}

impl Prepared {
    /// `base` as it is here, with no fetch: `HEAD`, a sha, a subtask's parent.
    pub(super) fn plain(repo: &Path, base: &str) -> Self {
        Prepared {
            rev: base.to_string(),
            base_ref: crate::git::branches::branch_of(repo, base),
            fresh: None,
            warnings: vec![],
        }
    }
}

/// Fetches a named base before a top-level task is created. `HEAD` and
/// anything that is not a branch (a sha, a tag, `origin/main`) are taken as
/// they are. A branch only the remote has gets a local branch tracking it,
/// and the task branches from the fetched tip; when the project's default
/// branch is nowhere, the task takes `HEAD`.
pub(super) fn prepare(
    repo: &Path,
    base: &str,
    project_default: bool,
    timeout: Duration,
) -> Prepared {
    if base == "HEAD" {
        return Prepared::plain(repo, base);
    }
    if crate::git::branches::branch_of(repo, base).is_some() {
        let fresh = refresh(repo, base, None, timeout);
        return Prepared {
            rev: fresh.rev,
            base_ref: Some(base.to_string()),
            fresh: Some(FreshBase { tip: fresh.tip }),
            warnings: fresh.warnings,
        };
    }
    if crate::git::history::resolve_commit(repo, base).is_ok() {
        return Prepared::plain(repo, base);
    }
    let up = upstream_of(repo, base);
    let fetched = up.as_ref().map(|up| fetch(repo, up, timeout));
    match (&up, fetched) {
        (Some(up), Some(Ok(tip))) => {
            // A local branch tracking it, as `git switch` would make: the
            // task lands on it and fetches it again when it starts.
            let tracked = git_out(
                repo,
                &["branch", "--quiet", "--track", base, &up.tracking_ref()],
            )
            .is_some();
            Prepared {
                rev: tip.clone(),
                base_ref: tracked.then(|| base.to_string()),
                fresh: tracked.then_some(FreshBase { tip: Some(tip) }),
                warnings: vec![],
            }
        }
        (_, fetched) if project_default => {
            let why = match (&up, fetched) {
                (Some(up), Some(Err(e))) => format!("not on {} either: {e}", up.remote),
                _ => "and the repository has no remote".to_string(),
            };
            let head = crate::git::history::resolve_commit(repo, "HEAD").unwrap_or_default();
            let mut plain = Prepared::plain(repo, "HEAD");
            plain.warnings.push(format!(
                "Warning: the project's default branch {base} does not exist locally ({why}); branching from HEAD at {}",
                short_sha(&head)
            ));
            plain
        }
        // Left for `create_worktree` to say it does not resolve.
        _ => Prepared::plain(repo, base),
    }
}

/// Fetches a top-level task's named base again when it starts, before
/// planning or its first attempt, and moves a worktree nothing has touched
/// onto it. A failed fetch keeps the task where it is, with a warning.
pub(super) async fn refresh_at_start(app: &App, task: &mut Task) {
    if task.parent.is_some() || !task.attempts.is_empty() {
        return;
    }
    let (Some(fresh), Some(base_ref)) = (task.fresh_base.clone(), task.base_ref.clone()) else {
        return;
    };
    let repo = PathBuf::from(&task.repo);
    let wt = PathBuf::from(&task.worktree);
    let (from, r, timeout) = (
        task.base_sha.clone(),
        base_ref.clone(),
        app.base_fetch_timeout(),
    );
    let done = tokio::task::spawn_blocking(move || {
        let fresh = refresh(&repo, &r, fresh.tip.as_deref(), timeout);
        let moved = crate::delivery::git::carry::reset_untouched_to_base(&wt, &fresh.rev, &from);
        (fresh, moved)
    })
    .await;
    let Ok((fresh, moved)) = done else {
        return;
    };
    task.record_fresh_base(FreshBase { tip: fresh.tip });
    task.record_decisions(fresh.warnings);
    if let Ok(Some(new_sha)) = moved {
        task.record_decision(format!(
            "Rebase: starting from {base_ref} at {}",
            short_sha(&new_sha)
        ));
        crate::task::transitions::move_base(task, new_sha);
    }
    task.touch(now_ms());
    app.save_task_logged(task);
    app.broadcast_task(task);
}

/// A task planned late (a backlog request) plans on the base as it is now,
/// when nothing has touched its worktree yet: its local base branch, or the
/// fetched tip that branch was left behind.
pub(super) async fn plan_on_newest_base(task: &mut Task) {
    let Some(base_ref) = task.base_ref.clone() else {
        return;
    };
    let (wt, from, r, tip) = (
        PathBuf::from(&task.worktree),
        task.base_sha.clone(),
        base_ref.clone(),
        tip_of(task),
    );
    let moved = tokio::task::spawn_blocking(move || {
        crate::delivery::git::carry::reset_untouched_to_base(
            &wt,
            &carry_rev(&wt, &r, tip.as_deref()),
            &from,
        )
    })
    .await;
    if let Ok(Ok(Some(new_sha))) = moved {
        task.record_decision(format!(
            "Rebase: planning from {base_ref} at {}",
            short_sha(&new_sha)
        ));
        crate::task::transitions::move_base(task, new_sha);
    }
}

/// [`carry_with_planner_tests`] onto the task's newest base (see
/// [`carry_rev`]).
pub(super) fn carry_work(
    task: &mut Task,
    worktree: &Path,
    base_ref: &str,
) -> Result<crate::delivery::git::carry::Rebase, crate::git::GitError> {
    let rev = carry_rev(worktree, base_ref, tip_of(task).as_deref());
    carry_with_planner_tests(task, worktree, &rev)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn configure(dir: &Path) {
        git_in(dir, &["config", "user.email", "test@example.com"]);
        git_in(dir, &["config", "user.name", "Test"]);
        git_in(dir, &["config", "commit.gpgsign", "false"]);
    }

    fn commit(dir: &Path, file: &str, text: &str) -> String {
        std::fs::write(dir.join(file), text).unwrap();
        git_in(dir, &["add", "."]);
        git_in(dir, &["commit", "-q", "-m", file]);
        git_in(dir, &["rev-parse", "HEAD"])
    }

    /// A bare remote with `main`, the repo that pushes to it, and a clone of
    /// it whose `main` tracks `origin/main`.
    struct Fixture {
        seed: PathBuf,
        clone: PathBuf,
    }

    impl Fixture {
        fn new(dir: &Path) -> Self {
            let bare = dir.join("remote.git");
            let seed = dir.join("seed");
            let clone = dir.join("clone");
            std::fs::create_dir_all(&seed).unwrap();
            git_in(
                dir,
                &["init", "-q", "--bare", "-b", "main", bare.to_str().unwrap()],
            );
            git_in(&seed, &["init", "-q", "-b", "main"]);
            configure(&seed);
            commit(&seed, "README.md", "hello\n");
            git_in(&seed, &["remote", "add", "origin", bare.to_str().unwrap()]);
            git_in(&seed, &["push", "-q", "origin", "main"]);
            git_in(
                dir,
                &[
                    "clone",
                    "-q",
                    bare.to_str().unwrap(),
                    clone.to_str().unwrap(),
                ],
            );
            configure(&clone);
            Fixture { seed, clone }
        }

        /// A new commit on the remote's `branch`.
        fn push(&self, branch: &str, file: &str) -> String {
            git_in(&self.seed, &["checkout", "-q", "-B", branch]);
            let sha = commit(&self.seed, file, &format!("{file}\n"));
            git_in(&self.seed, &["push", "-q", "origin", branch]);
            sha
        }

        fn local(&self, rev: &str) -> String {
            git_in(&self.clone, &["rev-parse", rev])
        }

        fn offline(&self) {
            let missing = self.clone.parent().unwrap().join("missing.git");
            git_in(
                &self.clone,
                &["remote", "set-url", "origin", missing.to_str().unwrap()],
            );
        }
    }

    async fn create(app: &App, repo: &Path, extra: serde_json::Value) -> Task {
        let mut params = json!({
            "repo": repo,
            "title": "Fresh base",
            "goal": "Branch from the newest base",
            "start": false,
        });
        for (key, value) in extra.as_object().unwrap() {
            params[key] = value.clone();
        }
        let made = app.handle_task_create(params).await.unwrap();
        app.store
            .load_task(made["id"].as_str().unwrap())
            .unwrap()
            .unwrap()
    }

    fn warnings(task: &Task) -> Vec<&String> {
        task.decisions
            .iter()
            .filter(|d| d.starts_with("Warning:"))
            .collect()
    }

    fn worktree_head(task: &Task) -> String {
        git_in(Path::new(&task.worktree), &["rev-parse", "HEAD"])
    }

    /// An executable shell script at `path`.
    fn script(path: &Path, body: &str) -> String {
        std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path.to_string_lossy().into_owned()
    }

    #[tokio::test]
    async fn create_fetches_and_fast_forwards_clean_local() {
        let (app, dir) = test_app();
        let fx = Fixture::new(dir.path());
        let tip = fx.push("main", "new.txt");
        assert_ne!(fx.local("main"), tip);
        let task = create(&app, &fx.clone, json!({"base": "main"})).await;
        assert_eq!(task.base_sha, tip);
        assert_eq!(worktree_head(&task), tip);
        assert_eq!(task.base_ref.as_deref(), Some("main"));
        assert_eq!(fx.local("main"), tip);
        // The checkout's files moved with the branch.
        assert!(fx.clone.join("new.txt").exists());
        assert!(warnings(&task).is_empty(), "{:?}", task.decisions);
        assert_eq!(
            task.fresh_base.as_ref().and_then(|f| f.tip.as_deref()),
            Some(tip.as_str())
        );
    }

    #[tokio::test]
    async fn create_without_upstream_uses_origin_branch() {
        let (app, dir) = test_app();
        let fx = Fixture::new(dir.path());
        git_in(&fx.clone, &["branch", "--unset-upstream", "main"]);
        let tip = fx.push("main", "new.txt");
        assert_eq!(
            upstream_of(&fx.clone, "main"),
            Some(Upstream {
                remote: "origin".into(),
                branch: "main".into()
            })
        );
        let task = create(&app, &fx.clone, json!({"base": "main"})).await;
        assert_eq!(task.base_sha, tip);
        assert_eq!(fx.local("refs/remotes/origin/main"), tip);
        assert_eq!(fx.local("main"), tip);
        assert!(warnings(&task).is_empty(), "{:?}", task.decisions);
    }

    #[tokio::test]
    async fn create_uses_project_default_branch() {
        let (app, dir) = test_app();
        let fx = Fixture::new(dir.path());
        fx.push("develop", "develop.txt");
        fx.push("release", "release.txt");
        git_in(&fx.clone, &["fetch", "-q", "origin"]);
        git_in(&fx.clone, &["branch", "-q", "develop", "origin/develop"]);
        let tip = fx.push("develop", "newer.txt");
        let head = fx.local("HEAD");
        let repo = fx.clone.to_string_lossy().into_owned();
        app.handle_secrets_set(json!({"projectBranches": {&repo: "develop"}}))
            .await
            .unwrap();
        let task = create(&app, &fx.clone, json!({"base": ""})).await;
        assert_eq!(task.base_sha, tip);
        assert_ne!(task.base_sha, head);
        assert_eq!(task.base_ref.as_deref(), Some("develop"));
        assert_eq!(fx.local("develop"), tip);
        assert!(warnings(&task).is_empty(), "{:?}", task.decisions);
        // An explicit base still wins over the project's.
        let task = create(&app, &fx.clone, json!({"base": "main"})).await;
        assert_eq!(task.base_ref.as_deref(), Some("main"));
        // A default branch only the remote has is branched from as fetched,
        // through a local branch that tracks it.
        app.handle_secrets_set(json!({"projectBranches": {&repo: "release"}}))
            .await
            .unwrap();
        let task = create(&app, &fx.clone, json!({})).await;
        let release = fx.local("refs/remotes/origin/release");
        assert_eq!(task.base_sha, release);
        assert_eq!(task.base_ref.as_deref(), Some("release"));
        assert_eq!(fx.local("release"), release);
        assert_eq!(
            git_in(
                &fx.clone,
                &["rev-parse", "--abbrev-ref", "release@{upstream}"]
            ),
            "origin/release"
        );
        assert!(warnings(&task).is_empty(), "{:?}", task.decisions);
    }

    #[tokio::test]
    async fn unknown_project_branch_falls_back_with_warning() {
        let (app, dir) = test_app();
        let fx = Fixture::new(dir.path());
        let head = fx.local("HEAD");
        let repo = fx.clone.to_string_lossy().into_owned();
        app.handle_secrets_set(json!({"projectBranches": {&repo: "nope"}}))
            .await
            .unwrap();
        let task = create(&app, &fx.clone, json!({})).await;
        assert_eq!(task.base_sha, head);
        assert_eq!(task.base_ref.as_deref(), Some("main"));
        let warned = warnings(&task);
        assert_eq!(warned.len(), 1, "{:?}", task.decisions);
        assert!(
            warned[0].contains("nope") && warned[0].contains(short_sha(&head)),
            "{}",
            warned[0]
        );
    }

    #[tokio::test]
    async fn create_offline_falls_back_and_warns() {
        let (app, dir) = test_app();
        let fx = Fixture::new(dir.path());
        fx.offline();
        let local = fx.local("main");
        let task = create(&app, &fx.clone, json!({"base": "main"})).await;
        assert_eq!(task.base_sha, local);
        assert_eq!(task.status, TaskStatus::Queued);
        let warned = warnings(&task);
        assert_eq!(warned.len(), 1, "{:?}", task.decisions);
        assert!(
            warned[0].contains("main") && warned[0].contains(short_sha(&local)),
            "{}",
            warned[0]
        );
    }

    #[tokio::test]
    async fn start_offline_after_online_create_warns() {
        let (app, dir) = test_app();
        let fx = Fixture::new(dir.path());
        let tip = fx.push("main", "new.txt");
        let mut task = create(&app, &fx.clone, json!({"base": "main"})).await;
        assert_eq!(task.base_sha, tip);
        assert!(warnings(&task).is_empty(), "{:?}", task.decisions);
        fx.offline();
        refresh_at_start(&app, &mut task).await;
        assert_eq!(task.base_sha, tip);
        assert_eq!(worktree_head(&task), tip);
        assert_ne!(task.status, TaskStatus::Failed);
        let warned = warnings(&task);
        assert_eq!(warned.len(), 1, "{:?}", task.decisions);
        assert!(warned[0].contains(short_sha(&tip)), "{}", warned[0]);
        // Saved, not only changed in memory.
        let stored = app.store.load_task(&task.id).unwrap().unwrap();
        assert_eq!(warnings(&stored).len(), 1);
    }

    #[tokio::test]
    async fn start_fetches_again_and_moves_untouched_worktree() {
        let (app, dir) = test_app();
        let fx = Fixture::new(dir.path());
        let mut task = create(&app, &fx.clone, json!({"base": "main"})).await;
        let newer = fx.push("main", "later.txt");
        refresh_at_start(&app, &mut task).await;
        assert_eq!(task.base_sha, newer);
        assert_eq!(worktree_head(&task), newer);
        assert_eq!(fx.local("main"), newer);
        assert!(warnings(&task).is_empty(), "{:?}", task.decisions);
        // A task that already ran an attempt is not moved again.
        let after = fx.push("main", "after.txt");
        task.fixture_data()
            .attempts
            .push(attempt_with_failure(1, "s"));
        refresh_at_start(&app, &mut task).await;
        assert_eq!(task.base_sha, newer);
        assert_ne!(fx.local("main"), after);
    }

    #[tokio::test]
    async fn dirty_local_checkout_untouched() {
        let (app, dir) = test_app();
        let fx = Fixture::new(dir.path());
        let before = fx.local("main");
        let tip = fx.push("main", "new.txt");
        std::fs::write(fx.clone.join("README.md"), "owner's edit\n").unwrap();
        let task = create(&app, &fx.clone, json!({"base": "main"})).await;
        assert_eq!(task.base_sha, tip);
        assert_eq!(worktree_head(&task), tip);
        assert_eq!(fx.local("main"), before);
        assert_eq!(
            std::fs::read_to_string(fx.clone.join("README.md")).unwrap(),
            "owner's edit\n"
        );
        assert!(!fx.clone.join("new.txt").exists());
        let warned = warnings(&task);
        assert_eq!(warned.len(), 1, "{:?}", task.decisions);
        assert!(warned[0].contains("uncommitted"), "{}", warned[0]);
    }

    #[tokio::test]
    async fn diverged_local_untouched_branches_from_fetched_tip() {
        let (app, dir) = test_app();
        let fx = Fixture::new(dir.path());
        let own = commit(&fx.clone, "own.txt", "mine\n");
        let tip = fx.push("main", "new.txt");
        let task = create(&app, &fx.clone, json!({"base": "main"})).await;
        assert_eq!(task.base_sha, tip);
        assert_eq!(worktree_head(&task), tip);
        assert_eq!(fx.local("main"), own);
        assert_eq!(git_in(&fx.clone, &["status", "--porcelain"]), "");
        let warned = warnings(&task);
        assert_eq!(warned.len(), 1, "{:?}", task.decisions);
        assert!(warned[0].contains("diverged"), "{}", warned[0]);
    }

    #[tokio::test]
    async fn local_ahead_keeps_local() {
        let (app, dir) = test_app();
        let fx = Fixture::new(dir.path());
        let own = commit(&fx.clone, "own.txt", "mine\n");
        let task = create(&app, &fx.clone, json!({"base": "main"})).await;
        assert_eq!(task.base_sha, own);
        assert_eq!(fx.local("main"), own);
        assert!(warnings(&task).is_empty(), "{:?}", task.decisions);
    }

    /// A task created while the local `main` has diverged: its base is the
    /// fetched tip, which the local branch does not contain.
    async fn left_behind(app: &App, fx: &Fixture) -> (Task, String, String) {
        let own = commit(&fx.clone, "own.txt", "mine\n");
        let tip = fx.push("main", "new.txt");
        let task = create(app, &fx.clone, json!({"base": "main"})).await;
        assert_eq!(task.base_sha, tip);
        (task, own, tip)
    }

    #[tokio::test]
    async fn planning_reset_stays_on_fetched_tip() {
        let (app, dir) = test_app();
        let fx = Fixture::new(dir.path());
        let (mut task, own, tip) = left_behind(&app, &fx).await;
        plan_on_newest_base(&mut task).await;
        assert_eq!(task.base_sha, tip);
        assert_eq!(worktree_head(&task), tip);
        assert_eq!(fx.local("main"), own);
        // Once the owner brings main past the tip, planning follows it again.
        git_in(&fx.clone, &["reset", "-q", "--hard", &tip]);
        let newer = commit(&fx.clone, "fixed.txt", "fixed\n");
        plan_on_newest_base(&mut task).await;
        assert_eq!(task.base_sha, newer);
        assert_eq!(worktree_head(&task), newer);
    }

    #[tokio::test]
    async fn carry_after_attempt_stays_on_fetched_tip() {
        let (app, dir) = test_app();
        let fx = Fixture::new(dir.path());
        let (mut task, own, tip) = left_behind(&app, &fx).await;
        let wt = PathBuf::from(&task.worktree);
        // A prior attempt's uncommitted work.
        task.fixture_data()
            .attempts
            .push(attempt_with_failure(1, "s"));
        std::fs::write(wt.join("work.txt"), "work\n").unwrap();
        std::fs::write(wt.join("README.md"), "changed\n").unwrap();
        let carried = carry_work(&mut task, &wt, "main").unwrap();
        assert_eq!(carried, crate::delivery::git::carry::Rebase::Unchanged);
        assert_eq!(worktree_head(&task), tip);
        assert!(!wt.join("own.txt").exists());
        assert_eq!(
            std::fs::read_to_string(wt.join("work.txt")).unwrap(),
            "work\n"
        );
        // The retry's carry onto the newest base stays there too.
        assert!(matches!(
            carry_onto_newest_base(&app, &mut task, &wt, &CancelToken::new()).await,
            BaseCarry::Ready
        ));
        assert_eq!(task.base_sha, tip);
        assert_eq!(worktree_head(&task), tip);
        assert_eq!(
            std::fs::read_to_string(wt.join("README.md")).unwrap(),
            "changed\n"
        );
        assert_eq!(fx.local("main"), own);
        // A local main that moves past the tip is carried onto again.
        git_in(&fx.clone, &["reset", "-q", "--hard", &tip]);
        let newer = commit(&fx.clone, "fixed.txt", "fixed\n");
        let carried = carry_work(&mut task, &wt, "main").unwrap();
        assert!(
            matches!(&carried, crate::delivery::git::carry::Rebase::Moved { new_sha, .. } if *new_sha == newer),
            "{carried:?}"
        );
        assert_eq!(worktree_head(&task), newer);
        assert_eq!(
            std::fs::read_to_string(wt.join("work.txt")).unwrap(),
            "work\n"
        );
    }

    #[tokio::test]
    async fn stalled_fetch_times_out_and_falls_back() {
        let (app, dir) = test_app();
        let fx = Fixture::new(dir.path());
        let stall = script(&dir.path().join("stall.sh"), "sleep 30");
        git_in(&fx.clone, &["config", "remote.origin.uploadpack", &stall]);
        app.base_fetch_timeout_ms.store(500, Ordering::SeqCst);
        let local = fx.local("main");
        let task = create(&app, &fx.clone, json!({"base": "main"})).await;
        assert_eq!(task.base_sha, local);
        let warned = warnings(&task);
        assert_eq!(warned.len(), 1, "{:?}", task.decisions);
        assert!(warned[0].contains("no answer within"), "{}", warned[0]);
    }

    /// An HTTP remote that answers every request with 401.
    fn auth_remote() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut seen = Vec::new();
                let mut buf = [0u8; 4096];
                while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => seen.extend_from_slice(&buf[..n]),
                    }
                }
                let _ = stream.write_all(
                    b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"orchd\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
        });
        format!("http://127.0.0.1:{port}/repo.git")
    }

    #[tokio::test]
    async fn credential_gated_fetch_does_not_prompt() {
        let (app, dir) = test_app();
        let fx = Fixture::new(dir.path());
        git_in(&fx.clone, &["remote", "set-url", "origin", &auth_remote()]);
        // An askpass program the repo names is never run.
        let asked = dir.path().join("asked");
        let askpass = script(
            &dir.path().join("askpass.sh"),
            &format!("touch '{}'\necho secret", asked.display()),
        );
        git_in(&fx.clone, &["config", "core.askPass", &askpass]);
        app.base_fetch_timeout_ms.store(15_000, Ordering::SeqCst);
        let local = fx.local("main");
        // It fails on the missing credentials, not on the timeout (checked on the warning below).
        let task = create(&app, &fx.clone, json!({"base": "main"})).await;
        assert_eq!(task.base_sha, local);
        assert!(!asked.exists());
        let warned = warnings(&task);
        assert_eq!(warned.len(), 1, "{:?}", task.decisions);
        assert!(!warned[0].contains("no answer within"), "{}", warned[0]);
    }

    #[tokio::test]
    async fn unnamed_base_does_not_fetch() {
        let (app, dir) = test_app();
        let fx = Fixture::new(dir.path());
        let fetched = dir.path().join("fetched");
        let mark = script(
            &dir.path().join("mark.sh"),
            &format!("touch '{}'\nexit 1", fetched.display()),
        );
        git_in(&fx.clone, &["config", "remote.origin.uploadpack", &mark]);
        let head = fx.local("HEAD");
        let mut task = create(&app, &fx.clone, json!({})).await;
        assert_eq!(task.base_sha, head);
        assert_eq!(task.base_ref.as_deref(), Some("main"));
        assert_eq!(task.fresh_base, None);
        refresh_at_start(&app, &mut task).await;
        assert_eq!(task.base_sha, head);
        assert!(task.decisions.is_empty(), "{:?}", task.decisions);
        assert!(!fetched.exists());
        // The same remote is fetched for a named base: the probe works.
        create(&app, &fx.clone, json!({"base": "main"})).await;
        assert!(fetched.exists());
    }
}
