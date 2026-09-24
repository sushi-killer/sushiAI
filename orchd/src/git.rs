//! Worktree lifecycle and plain `git` plumbing. All functions here are
//! synchronous (they shell out); the engine runs them via
//! `tokio::task::spawn_blocking`.

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug)]
pub struct GitError(pub String);

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for GitError {}

fn run(cwd: &Path, args: &[&str]) -> Result<String, GitError> {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|e| GitError(format!("git {}: {e}", args.join(" "))))?;
    if !out.status.success() {
        return Err(GitError(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// lowercase, [a-z0-9-], collapsed, max 40 chars, no leading/trailing '-'.
pub fn slugify(title: &str, max_len: usize) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in title.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            out.push(lower);
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.len() > max_len {
        let mut end = max_len;
        while end > 0 && !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
        while out.ends_with('-') {
            out.pop();
        }
    }
    if out.is_empty() {
        out.push_str("task");
    }
    out
}

fn branch_exists(repo_root: &Path, branch: &str) -> bool {
    Command::new("git")
        .args([
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .current_dir(repo_root)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Default branch name `task/<slug>`, disambiguated with `-2`, `-3`, ... if
/// it already exists.
pub fn unique_branch_name(repo_root: &Path, title: &str) -> String {
    let slug = slugify(title, 40);
    let base = format!("task/{slug}");
    if !branch_exists(repo_root, &base) {
        return base;
    }
    let mut n = 2;
    loop {
        let candidate = format!("{base}-{n}");
        if !branch_exists(repo_root, &candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// Sibling worktree path: `<parent>/<root-basename>-<branch with / -> ->`.
pub fn worktree_path(repo_root: &Path, branch: &str) -> PathBuf {
    let parent = repo_root.parent().unwrap_or_else(|| Path::new("."));
    let root_basename = repo_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repo");
    let branch_part = branch.replace('/', "-");
    parent.join(format!("{root_basename}-{branch_part}"))
}

/// `git rev-parse --show-toplevel` -- `task.create`'s `repo` param can be
/// any path inside the checkout (or a symlinked/relative one); this
/// resolves it to the actual repo root before anything else touches it.
pub fn repo_toplevel(path: &Path) -> Result<PathBuf, GitError> {
    let out = run(path, &["rev-parse", "--show-toplevel"])?;
    Ok(PathBuf::from(out.trim()))
}

pub struct CreatedWorktree {
    pub path: PathBuf,
    pub base_sha: String,
}

/// `git worktree add -b <branch> <path> HEAD`, then records `baseSha`.
/// `base` is any commit-ish (`HEAD`, a branch, a sha): the new branch
/// starts there, and `base_sha` is that exact commit, resolved once up
/// front so a checkout moving meanwhile can't skew the diff base.
pub fn create_worktree(
    repo_root: &Path,
    branch: &str,
    path: &Path,
    base: &str,
) -> Result<CreatedWorktree, GitError> {
    let base_sha = run(
        repo_root,
        &["rev-parse", "--verify", &format!("{base}^{{commit}}")],
    )?
    .trim()
    .to_string();
    run(
        repo_root,
        &[
            "worktree",
            "add",
            "-b",
            branch,
            path.to_str()
                .ok_or_else(|| GitError("non-utf8 path".into()))?,
            &base_sha,
        ],
    )?;
    Ok(CreatedWorktree {
        path: path.to_path_buf(),
        base_sha,
    })
}

/// `git check-ignore` reports true only when the main checkout actually
/// ignores the path (its own `.gitignore`, global excludes, ...) -- being
/// merely untracked isn't enough, or an ordinary WIP file someone forgot to
/// `git add` would get bootstrap-copied into every worktree too.
fn is_git_ignored(repo_root: &Path, rel_path: &str) -> bool {
    Command::new("git")
        .args(["check-ignore", "--quiet", rel_path])
        .current_dir(repo_root)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Append `rel_paths` (repo-root-relative) to the worktree's own
/// `info/exclude` (resolved via `git rev-parse --git-path`, since a
/// worktree's git-dir lives under the main checkout's `.git/worktrees/<name>`,
/// not `<worktree>/.git`). Best-effort: bootstrapping still succeeds even if
/// this fails, it just means those paths could show up as "changed" until
/// fixed by hand.
fn append_to_info_exclude(worktree: &Path, rel_paths: &[String]) -> std::io::Result<()> {
    let out = match run(worktree, &["rev-parse", "--git-path", "info/exclude"]) {
        Ok(out) => out,
        Err(_) => return Ok(()),
    };
    let exclude_path = worktree.join(out.trim());
    if let Some(parent) = exclude_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&exclude_path)?;
    use std::io::Write;
    for p in rel_paths {
        writeln!(f, "/{p}")?;
    }
    Ok(())
}

/// Copy `.env`, `.env.local`, `.claude/settings.local.json` from the main
/// checkout when present and gitignored there; symlink `node_modules` when
/// the main checkout has it and the worktree doesn't. Every bootstrapped
/// path is also recorded in the worktree's `info/exclude`, so it never shows
/// up as a "changed file" in the gate or gets swept into `git add -A`.
pub fn bootstrap_worktree(main_root: &Path, worktree: &Path) -> std::io::Result<()> {
    let mut bootstrapped = Vec::new();
    for rel in [".env", ".env.local", ".claude/settings.local.json"] {
        let src = main_root.join(rel);
        if !src.is_file() {
            continue;
        }
        if !is_git_ignored(main_root, rel) {
            continue; // only ever copy files the main checkout itself ignores
        }
        let dst = worktree.join(rel);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(&src, &dst)?;
        bootstrapped.push(rel.to_string());
    }

    let main_modules = main_root.join("node_modules");
    let worktree_modules = worktree.join("node_modules");
    if main_modules.is_dir() && !worktree_modules.exists() {
        #[cfg(unix)]
        std::os::unix::fs::symlink(&main_modules, &worktree_modules)?;
        bootstrapped.push("node_modules".to_string());
    }

    if !bootstrapped.is_empty() {
        append_to_info_exclude(worktree, &bootstrapped)?;
    }
    Ok(())
}

/// `git status --porcelain -z --untracked-files=all`, one path per returned
/// entry. `-z` (NUL-terminated, no quoting/escaping) is safer to parse than
/// the default newline format; `--untracked-files=all` lists every file
/// inside an untracked directory instead of just the directory name, so an
/// agent-created directory's contents are all individually visible to the
/// protected-path check.
pub fn status_porcelain(cwd: &Path) -> Result<Vec<String>, GitError> {
    let out = run(
        cwd,
        &["status", "--porcelain", "-z", "--untracked-files=all"],
    )?;
    let mut paths = Vec::new();
    let mut fields = out.split('\0').filter(|s| !s.is_empty());
    while let Some(entry) = fields.next() {
        if entry.len() < 3 {
            continue;
        }
        let status_code = &entry[0..2];
        paths.push(entry[3..].to_string());
        if status_code.starts_with('R') || status_code.starts_with('C') {
            // Rename/copy entries are followed by the origin path as a
            // second NUL-terminated field; consume and discard it.
            fields.next();
        }
    }
    Ok(paths)
}

pub fn status_short(cwd: &Path) -> Result<String, GitError> {
    run(cwd, &["status", "--short"])
}

/// `git diff --name-only <base>`.
pub fn diff_name_only(cwd: &Path, base: &str) -> Result<Vec<String>, GitError> {
    let out = run(cwd, &["diff", "--name-only", base])?;
    Ok(out
        .lines()
        .map(|l| l.to_string())
        .filter(|s| !s.is_empty())
        .collect())
}

pub fn diff_stat(cwd: &Path, base: &str) -> Result<String, GitError> {
    run(cwd, &["diff", "--stat", base])
}

/// `git diff <base>`, truncated to `max_chars` with a note (spec: review
/// diff capped at 60000 chars).
pub fn diff_full(cwd: &Path, base: &str, max_chars: usize) -> Result<String, GitError> {
    let out = run(cwd, &["diff", base])?;
    if out.len() <= max_chars {
        return Ok(out);
    }
    let mut end = max_chars;
    while end > 0 && !out.is_char_boundary(end) {
        end -= 1;
    }
    Ok(format!(
        "{}\n... [truncated, {} bytes total]",
        &out[..end],
        out.len()
    ))
}

/// Changed files, combining working-tree changes and committed changes
/// since `base` (spec step 5: `git status --porcelain` + `git diff
/// --name-only <base>`), de-duplicated.
pub fn changed_files(cwd: &Path, base: &str) -> Result<Vec<String>, GitError> {
    let mut files = status_porcelain(cwd)?;
    files.extend(diff_name_only(cwd, base)?);
    files.sort();
    files.dedup();
    Ok(files)
}

/// `git add -A && git commit -m "<title>" --trailer "Task-Id: <id>"
/// --trailer "Attempt: <n>"`.
/// `-c core.hooksPath=/dev/null` on both calls: this runs on the *host*, on
/// a commit an agent's own worktree may have staged, and an agent-written
/// `pre-commit`/`commit-msg` hook must never get to execute arbitrary code
/// there.
pub fn commit(cwd: &Path, title: &str, task_id: &str, attempt_n: u32) -> Result<(), GitError> {
    run(cwd, &["-c", "core.hooksPath=/dev/null", "add", "-A"])?;
    run(
        cwd,
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "commit",
            "-m",
            title,
            "--trailer",
            &format!("Task-Id: {task_id}"),
            "--trailer",
            &format!("Attempt: {attempt_n}"),
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as StdCommand;

    fn init_repo(dir: &Path) {
        let run = |args: &[&str]| {
            let status = StdCommand::new("git")
                .args(args)
                .current_dir(dir)
                .status()
                .unwrap();
            assert!(status.success(), "git {:?} failed", args);
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "Test"]);
        std::fs::write(dir.join("README.md"), "hello\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "init"]);
    }

    #[test]
    fn slugify_lowercases_and_strips_punctuation() {
        assert_eq!(slugify("Add a Save Button!", 40), "add-a-save-button");
        assert_eq!(slugify("  weird---title__here ", 40), "weird-title-here");
        let long = "a".repeat(80);
        assert_eq!(slugify(&long, 40).len(), 40);
        assert_eq!(slugify("", 40), "task");
    }

    #[test]
    fn worktree_path_is_sibling_with_dashed_branch() {
        let repo = Path::new("/Users/me/Desktop/Test");
        let path = worktree_path(repo, "task/add-a-button");
        assert_eq!(path, Path::new("/Users/me/Desktop/Test-task-add-a-button"));
    }

    #[test]
    fn create_worktree_starts_from_the_given_base_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_root = tmp.path().join("repo");
        std::fs::create_dir_all(&repo_root).unwrap();
        init_repo(&repo_root);
        let git = |args: &[&str]| {
            let out = StdCommand::new("git")
                .args(args)
                .current_dir(&repo_root)
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        git(&["checkout", "-q", "-b", "feature"]);
        std::fs::write(repo_root.join("feature.txt"), "x\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "feature"]);
        let feature_sha = git(&["rev-parse", "HEAD"]);
        git(&["checkout", "-q", "-"]);

        let wt_path = worktree_path(&repo_root, "task/on-feature");
        let created = create_worktree(&repo_root, "task/on-feature", &wt_path, "feature").unwrap();
        assert_eq!(created.base_sha, feature_sha);
        assert!(wt_path.join("feature.txt").exists());
    }

    #[test]
    fn create_worktree_and_status_and_commit_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_root = tmp.path().join("repo");
        std::fs::create_dir_all(&repo_root).unwrap();
        init_repo(&repo_root);

        let branch = unique_branch_name(&repo_root, "Add a button");
        assert_eq!(branch, "task/add-a-button");
        let wt_path = worktree_path(&repo_root, &branch);
        let created = create_worktree(&repo_root, &branch, &wt_path, "HEAD").unwrap();
        assert!(created.path.exists());
        assert!(!created.base_sha.is_empty());

        // A second call with the same title must disambiguate.
        let branch2 = unique_branch_name(&repo_root, "Add a button");
        assert_eq!(branch2, "task/add-a-button-2");

        std::fs::write(wt_path.join("new_file.txt"), "content\n").unwrap();
        let changed = changed_files(&wt_path, &created.base_sha).unwrap();
        assert!(changed.contains(&"new_file.txt".to_string()));

        commit(&wt_path, "Add a button", "t1", 1).unwrap();

        // After commit, working tree is clean relative to itself but the
        // file shows up against base.
        let against_base = diff_name_only(&wt_path, &created.base_sha).unwrap();
        assert!(against_base.contains(&"new_file.txt".to_string()));
        assert!(status_porcelain(&wt_path).unwrap().is_empty());
    }

    #[test]
    fn commit_never_runs_the_worktrees_own_hooks() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_root = tmp.path().join("repo");
        std::fs::create_dir_all(&repo_root).unwrap();
        init_repo(&repo_root);

        let branch = unique_branch_name(&repo_root, "Hook test");
        let wt_path = worktree_path(&repo_root, &branch);
        let created = create_worktree(&repo_root, &branch, &wt_path, "HEAD").unwrap();

        // An agent-writable pre-commit hook that would prove it ran by
        // creating a marker file. `commit()` must never let it run.
        let hooks_dir = String::from_utf8(
            Command::new("git")
                .args(["rev-parse", "--git-path", "hooks"])
                .current_dir(&wt_path)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        let hooks_dir = wt_path.join(hooks_dir.trim());
        std::fs::create_dir_all(&hooks_dir).unwrap();
        let hook_path = hooks_dir.join("pre-commit");
        std::fs::write(&hook_path, "#!/bin/sh\ntouch hook-ran.txt\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        std::fs::write(wt_path.join("new_file.txt"), "content\n").unwrap();
        commit(&wt_path, "Add a file", "t1", 1).unwrap();

        assert!(
            !wt_path.join("hook-ran.txt").exists(),
            "the worktree's pre-commit hook must not have run"
        );
        let _ = created;
    }

    #[test]
    fn bootstrap_copies_gitignored_env_and_symlinks_node_modules_and_excludes_both() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_root = tmp.path().join("repo");
        std::fs::create_dir_all(&repo_root).unwrap();
        init_repo(&repo_root);
        std::fs::write(repo_root.join(".gitignore"), ".env\nnode_modules/\n").unwrap();
        Command::new("git")
            .args(["add", ".gitignore"])
            .current_dir(&repo_root)
            .status()
            .unwrap();
        Command::new("git")
            .args(["commit", "-q", "-m", "add gitignore"])
            .current_dir(&repo_root)
            .status()
            .unwrap();
        std::fs::write(repo_root.join(".env"), "SECRET=1\n").unwrap();
        std::fs::create_dir_all(repo_root.join("node_modules/pkg")).unwrap();
        std::fs::write(repo_root.join("node_modules/pkg/index.js"), "").unwrap();

        let branch = unique_branch_name(&repo_root, "Bootstrap test");
        let wt_path = worktree_path(&repo_root, &branch);
        let created = create_worktree(&repo_root, &branch, &wt_path, "HEAD").unwrap();

        bootstrap_worktree(&repo_root, &wt_path).unwrap();

        assert!(wt_path.join(".env").exists());
        assert_eq!(
            std::fs::read_to_string(wt_path.join(".env")).unwrap(),
            "SECRET=1\n"
        );
        assert!(wt_path.join("node_modules/pkg/index.js").exists());

        // Neither bootstrapped path is ever a "changed file".
        let changed = changed_files(&wt_path, &created.base_sha).unwrap();
        assert!(!changed
            .iter()
            .any(|f| f == ".env" || f.starts_with("node_modules")));
    }

    #[test]
    fn bootstrap_does_not_copy_an_ordinary_untracked_file() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_root = tmp.path().join("repo");
        std::fs::create_dir_all(&repo_root).unwrap();
        init_repo(&repo_root);
        // Untracked, but NOT gitignored -- e.g. just forgotten `git add`.
        std::fs::write(repo_root.join(".env"), "SECRET=1\n").unwrap();

        let branch = unique_branch_name(&repo_root, "No bootstrap test");
        let wt_path = worktree_path(&repo_root, &branch);
        create_worktree(&repo_root, &branch, &wt_path, "HEAD").unwrap();

        bootstrap_worktree(&repo_root, &wt_path).unwrap();

        assert!(!wt_path.join(".env").exists());
    }

    #[test]
    fn repo_toplevel_resolves_a_subdirectory_to_the_repo_root() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_root = tmp.path().join("repo");
        std::fs::create_dir_all(repo_root.join("src/nested")).unwrap();
        init_repo(&repo_root);

        let resolved = repo_toplevel(&repo_root.join("src/nested")).unwrap();
        // macOS temp dirs are often themselves a symlink (`/tmp` ->
        // `/private/tmp`); canonicalize both sides before comparing.
        assert_eq!(
            resolved.canonicalize().unwrap(),
            repo_root.canonicalize().unwrap()
        );
    }
}
