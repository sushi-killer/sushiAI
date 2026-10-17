//! One cargo target directory per worktree. A cargo config outside the
//! repository (`~/.cargo/config.toml`, a parent folder's `.cargo/config.toml`)
//! or the daemon's own `CARGO_TARGET_DIR` can point every checkout at one
//! shared directory: worktrees then share a build lock and binaries, and a
//! sandboxed check cannot write there at all. orchd runs cargo of a task with
//! `CARGO_TARGET_DIR=<worktree>/target` whenever the configured directory lies
//! outside the worktree, and says so once in the task's decisions.

use std::path::{Path, PathBuf};

/// Where the configured target directory comes from, when it lies outside
/// `worktree`: (the directory, the file or variable that sets it).
pub fn outside_target_dir(worktree: &Path) -> Option<(PathBuf, String)> {
    outside_target_dir_in(worktree, |var| std::env::var_os(var))
}

fn outside_target_dir_in(
    worktree: &Path,
    vars: impl Fn(&str) -> Option<std::ffi::OsString>,
) -> Option<(PathBuf, String)> {
    for var in ["CARGO_TARGET_DIR", "CARGO_BUILD_TARGET_DIR"] {
        if let Some(dir) = vars(var).filter(|v| !v.is_empty()) {
            let dir = PathBuf::from(dir);
            return (!dir.starts_with(worktree)).then(|| (dir, var.to_string()));
        }
    }
    let cargo_home = vars("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| vars("HOME").map(|h| PathBuf::from(h).join(".cargo")));
    // Cargo reads the deepest folder's config first; the first one that sets
    // `build.target-dir` wins, and CARGO_HOME's comes last.
    let configs = worktree
        .ancestors()
        .map(|dir| dir.join(".cargo"))
        .chain(cargo_home);
    for dot_cargo in configs {
        for name in ["config.toml", "config"] {
            let file = dot_cargo.join(name);
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            if let Some(dir) = target_dir_setting(&text) {
                let base = dot_cargo.parent().unwrap_or(&dot_cargo);
                let dir = base.join(dir);
                return (!dir.starts_with(worktree)).then(|| (dir, file.display().to_string()));
            }
        }
    }
    None
}

/// `build.target-dir` of one cargo config file: a `target-dir` key in the
/// `[build]` table or a top-level `build.target-dir` key.
fn target_dir_setting(text: &str) -> Option<String> {
    let mut table = String::new();
    for line in text.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.split(']').next()) {
            table = name.trim().to_string();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().trim_matches('"');
        let wanted = (table == "build" && key == "target-dir")
            || (table.is_empty() && key == "build.target-dir");
        if wanted {
            let value = value.split('#').next().unwrap_or("").trim();
            return Some(value.trim_matches(|c| c == '"' || c == '\'').to_string());
        }
    }
    None
}

/// Points cargo of `command` at `<worktree>/target` when the configured
/// target directory lies outside the worktree. Returns the decision line to
/// record, or None when nothing changed.
pub fn isolate(command: &mut std::process::Command, worktree: &Path) -> Option<String> {
    isolate_in(command, worktree, outside_target_dir(worktree))
}

fn isolate_in(
    command: &mut std::process::Command,
    worktree: &Path,
    outside: Option<(PathBuf, String)>,
) -> Option<String> {
    let (dir, source) = outside?;
    let own = worktree.join("target");
    command.env("CARGO_TARGET_DIR", &own);
    Some(format!(
        "cargo target directory: {source} points at {}, outside the worktree; \
         cargo here builds in {} so worktrees share no build lock or binaries \
         and a sandboxed check can write its build",
        dir.display(),
        own.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_build_table_and_a_dotted_key_both_set_the_target_dir() {
        assert_eq!(
            target_dir_setting(
                "[build]\nrustc-wrapper = \"sccache\"\ntarget-dir = \"/x/t\" # shared\n"
            ),
            Some("/x/t".into())
        );
        assert_eq!(
            target_dir_setting("build.target-dir = 'rel'\n"),
            Some("rel".into())
        );
        assert_eq!(
            target_dir_setting("[target.x]\ntarget-dir = \"/no\"\n"),
            None
        );
        assert_eq!(target_dir_setting("[build]\njobs = 4\n"), None);
    }

    #[test]
    fn a_config_above_the_worktree_that_points_outside_is_overridden() {
        let tmp = tempfile::tempdir().unwrap();
        let worktree = tmp.path().join("repo");
        std::fs::create_dir_all(worktree.join("src")).unwrap();
        std::fs::create_dir_all(tmp.path().join(".cargo")).unwrap();
        std::fs::write(
            tmp.path().join(".cargo/config.toml"),
            "[build]\ntarget-dir = \"shared-target\"\n",
        )
        .unwrap();
        let home = tmp.path().join("home");
        let vars = |var: &str| (var == "HOME").then(|| home.clone().into_os_string());
        let outside = outside_target_dir_in(&worktree, vars);
        let (dir, source) = outside.clone().expect("outside");
        assert_eq!(dir, tmp.path().join("shared-target"));
        assert!(source.ends_with("config.toml"), "{source}");
        // The daemon's own CARGO_TARGET_DIR outside the worktree counts too.
        let shared = |var: &str| (var == "CARGO_TARGET_DIR").then(|| "/shared".into());
        assert_eq!(
            outside_target_dir_in(&worktree, shared).map(|(d, _)| d),
            Some(PathBuf::from("/shared"))
        );
        let mut cmd = std::process::Command::new("true");
        let line = isolate_in(&mut cmd, &worktree, outside).expect("a decision line");
        assert!(line.contains("outside the worktree"), "{line}");
        let set = cmd
            .get_envs()
            .find(|(k, _)| *k == "CARGO_TARGET_DIR")
            .and_then(|(_, v)| v.map(PathBuf::from));
        assert_eq!(set, Some(worktree.join("target")));
    }

    #[test]
    fn the_repository_config_inside_the_worktree_is_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let worktree = tmp.path().join("repo");
        std::fs::create_dir_all(worktree.join(".cargo")).unwrap();
        std::fs::write(
            worktree.join(".cargo/config.toml"),
            "[build]\ntarget-dir = \"target\"\n",
        )
        .unwrap();
        assert_eq!(outside_target_dir_in(&worktree, |_| None), None);
    }
}
