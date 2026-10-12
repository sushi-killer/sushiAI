use super::*;

const UNSAFE_FILES: &[&str] = &[
    "Cargo.toml",
    "Cargo.lock",
    "package.json",
    "package-lock.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    ".sushiai/setup.json",
    "rust-toolchain*",
    ".cargo/config*",
    "tsconfig*.json",
    "vite.config.*",
    "eslint.config.*",
    ".prettierrc*",
    "Makefile",
    ".github/**",
];

pub(super) struct ScopedRun {
    pub commands: Vec<String>,
    pub decisions: Vec<String>,
    pub deferred: Vec<String>,
}

fn package_name(text: &str) -> Option<String> {
    let mut in_package = false;
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if !in_package {
            continue;
        }
        let Some(("name", value)) = line.split_once('=').map(|(k, v)| (k.trim(), v.trim())) else {
            continue;
        };
        let name = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))?;
        if !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        {
            return Some(name.to_string());
        }
        return None;
    }
    None
}

fn has_section(text: &str, section: &str) -> bool {
    text.lines()
        .any(|line| line.split('#').next().unwrap_or("").trim() == section)
}

fn unsafe_file(root: &Path, file: &str) -> bool {
    if file == "Cargo.toml" {
        return true;
    }
    let name = Path::new(file)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(file);
    let matched = UNSAFE_FILES.iter().any(|pattern| {
        glob_match(pattern, file)
            || glob_match(pattern, name)
            || file
                .match_indices('/')
                .any(|(at, _)| glob_match(pattern, &file[at + 1..]))
    });
    if !matched || name != "Cargo.toml" {
        return matched;
    }
    std::fs::read_to_string(root.join(file)).map_or(true, |text| {
        has_section(&text, "[workspace]") || !has_section(&text, "[package]")
    })
}

fn nearest_package(root: &Path, file: &str) -> Option<String> {
    let path = Path::new(file);
    if path
        .components()
        .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return None;
    }
    let mut dir = root.join(path).parent()?.to_path_buf();
    loop {
        if let Ok(text) = std::fs::read_to_string(dir.join("Cargo.toml")) {
            if has_section(&text, "[package]") {
                return package_name(&text);
            }
        }
        if dir == root || !dir.pop() {
            return None;
        }
    }
}

fn test_stem(path: &Path) -> Option<&str> {
    let name = path.file_name()?.to_str()?;
    [".test.cjs", ".test.mjs", ".test.js"]
        .iter()
        .find_map(|suffix| name.strip_suffix(suffix))
}

fn test_files(dir: &Path, recursive: bool, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_file() && test_stem(&path).is_some() {
            files.push(path);
        } else if recursive && kind.is_dir() {
            test_files(&path, true, files);
        }
    }
}

fn shell_arg(value: &str) -> String {
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-'))
    {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

fn derive(root: &Path, changed: &[String]) -> Result<Vec<String>, String> {
    if let Some(file) = changed.iter().find(|file| unsafe_file(root, file)) {
        return Err(file.clone());
    }
    let mut root_tests = Vec::new();
    test_files(&root.join("tests"), true, &mut root_tests);
    let mut commands = Vec::new();
    for file in changed {
        let mut mapped = Vec::new();
        if let Some(name) = nearest_package(root, file) {
            mapped.push(format!("cargo test -p {name}"));
        }
        let path = root.join(file);
        if test_stem(&path).is_some() && path.is_file() {
            mapped.push(format!("node --test {}", shell_arg(file)));
        } else if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            let mut candidates = root_tests.clone();
            if let Some(dir) = path.parent() {
                test_files(dir, false, &mut candidates);
            }
            candidates.sort();
            candidates.dedup();
            for test in candidates
                .iter()
                .filter(|test| test_stem(test) == Some(stem))
            {
                if let Ok(relative) = test.strip_prefix(root) {
                    mapped.push(format!(
                        "node --test {}",
                        shell_arg(&relative.to_string_lossy())
                    ));
                }
            }
        }
        if mapped.is_empty() {
            return Err(file.clone());
        }
        for command in mapped {
            if !commands.contains(&command) {
                commands.push(command);
            }
        }
    }
    Ok(commands)
}

fn names_target(root: &Path, command: &str) -> bool {
    command.split_whitespace().any(|token| {
        let token = token.trim_matches(['\'', '"', ';', '(', ')']);
        ["-p", "--package", "--test", "--manifest-path"].contains(&token)
            || ["--package=", "--test=", "--manifest-path="]
                .iter()
                .any(|flag| token.starts_with(flag))
            || (token.starts_with("-p") && token.len() > 2 && !token.starts_with("--"))
            || (!token.starts_with('-')
                && !is_assignment(token)
                && (root.join(token).is_file() || Path::new(token).extension().is_some()))
    })
}

pub(super) fn scope(
    root: &Path,
    changed: &[String],
    commands: &[String],
    enabled: bool,
) -> ScopedRun {
    let mut run = ScopedRun {
        commands: Vec::new(),
        decisions: Vec::new(),
        deferred: Vec::new(),
    };
    let derived = enabled.then(|| derive(root, changed));
    for command in commands {
        let replacements = if enabled && runs_ci_entry(command) && !names_target(root, command) {
            match derived.as_ref().unwrap() {
                Ok(derived) => {
                    let label = if derived.is_empty() {
                        "nothing".into()
                    } else {
                        derived.join("; ")
                    };
                    run.decisions.push(format!("Orchestrator: subtask ran {label} instead of {command}; the parent runs {command} on the combined tree"));
                    run.deferred.push(command.clone());
                    derived.clone()
                }
                Err(file) => {
                    run.decisions.push(format!("Orchestrator: subtask runs {command} in full: {file} maps to no package or test"));
                    vec![command.clone()]
                }
            }
        } else {
            vec![command.clone()]
        };
        for replacement in replacements {
            if !run.commands.contains(&replacement) {
                run.commands.push(replacement);
            }
        }
    }
    run
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, file: &str, text: &str) {
        let path = root.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn fixture() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        write(
            root.path(),
            "Cargo.toml",
            "[workspace]\nmembers = ['a', 'b']\n",
        );
        write(
            root.path(),
            "a/Cargo.toml",
            "[package]\nname = 'package-a' # comment\nversion = '0.1.0'\n",
        );
        write(
            root.path(),
            "b/Cargo.toml",
            "[package]\nname = \"package-b\"\n",
        );
        write(root.path(), "a/src/lib.rs", "");
        root
    }

    #[test]
    fn nearest_package_and_duplicate_ci_entries() {
        let root = fixture();
        let changed = vec!["a/src/lib.rs".into(), "a/Cargo.toml".into()];
        let commands = vec!["cargo test --workspace".into(), "npm run ci".into()];
        let result = scope(root.path(), &changed, &commands, true);
        assert_eq!(result.commands, ["cargo test -p package-a"]);
        assert_eq!(result.deferred, commands);
        assert_eq!(result.decisions[0], "Orchestrator: subtask ran cargo test -p package-a instead of cargo test --workspace; the parent runs cargo test --workspace on the combined tree");
        assert_eq!(
            derive(root.path(), &["a/src/lib.rs".into(), "b/src/lib.rs".into()]).unwrap(),
            ["cargo test -p package-a", "cargo test -p package-b"]
        );
        write(
            root.path(),
            "a/nested/Cargo.toml",
            "[package]\nname = \"nested\"\n",
        );
        assert_eq!(
            derive(root.path(), &["a/nested/src/lib.rs".into()]).unwrap(),
            ["cargo test -p nested"]
        );
    }

    #[test]
    fn source_and_test_pairs_in_both_locations_are_quoted_and_deduplicated() {
        let root = tempfile::tempdir().unwrap();
        for file in [
            "src/foo.js",
            "src/foo.test.cjs",
            "tests/nested/foo.test.mjs",
            "tests/unrelated.test.js",
            "src/a b.js",
            "tests/a b.test.js",
        ] {
            write(root.path(), file, "");
        }
        assert_eq!(
            derive(
                root.path(),
                &["src/foo.js".into(), "src/foo.test.cjs".into()]
            )
            .unwrap(),
            [
                "node --test src/foo.test.cjs",
                "node --test tests/nested/foo.test.mjs"
            ]
        );
        assert_eq!(
            derive(root.path(), &["src/a b.js".into()]).unwrap(),
            ["node --test 'tests/a b.test.js'"]
        );
    }

    #[test]
    fn unsafe_and_unmapped_files_keep_the_full_command() {
        let root = fixture();
        for file in UNSAFE_FILES.iter().filter(|file| **file != "Cargo.toml") {
            let file = file.replace('*', "x");
            let run = scope(
                root.path(),
                std::slice::from_ref(&file),
                &["npm run ci".into()],
                true,
            );
            assert_eq!(run.commands, ["npm run ci"]);
            assert_eq!(run.decisions, [format!("Orchestrator: subtask runs npm run ci in full: {file} maps to no package or test")]);
            assert!(run.deferred.is_empty());
        }
        for file in [
            "Cargo.toml",
            "unknown.txt",
            "gone/Cargo.toml",
            "a/.cargo/config.toml",
            "b/package.json",
            "a/.github/workflows/check.yml",
        ] {
            assert_eq!(derive(root.path(), &[file.into()]), Err(file.into()));
        }
        write(
            root.path(),
            "a/Cargo.toml",
            "[package]\nname = 'a'\n[workspace]\n",
        );
        assert!(derive(root.path(), &["a/Cargo.toml".into()]).is_err());
        write(root.path(), "Cargo.toml", "[package]\nname = 'root'\n");
        assert!(derive(root.path(), &["Cargo.toml".into()]).is_err());
    }

    #[test]
    fn explicit_targets_non_ci_and_every_gate_run_unchanged() {
        let root = fixture();
        let commands: Vec<String> = [
            "cargo test -p x --test y",
            "cargo test --package=x",
            "cargo test -px",
            "cargo test --test x",
            "cargo test --manifest-path a/Cargo.toml",
            "npm test tests/foo.test.cjs",
            "npm run ci -- src/foo.js",
            "node --test",
            "true",
        ]
        .iter()
        .map(|c| (*c).into())
        .collect();
        let changed = vec!["a/src/lib.rs".into()];
        let result = scope(root.path(), &changed, &commands, true);
        assert_eq!(result.commands, commands);
        assert!(result.decisions.is_empty() && result.deferred.is_empty());
        let commands = vec!["cargo test --workspace".into()];
        let result = scope(root.path(), &changed, &commands, false);
        assert_eq!(result.commands, commands);
        assert!(result.decisions.is_empty() && result.deferred.is_empty());
    }

    #[test]
    fn empty_diff_runs_nothing_and_defers_the_entry() {
        let root = fixture();
        let result = scope(root.path(), &[], &["cargo test --workspace".into()], true);
        assert!(result.commands.is_empty());
        assert_eq!(result.deferred, ["cargo test --workspace"]);
        assert_eq!(result.decisions, ["Orchestrator: subtask ran nothing instead of cargo test --workspace; the parent runs cargo test --workspace on the combined tree"]);
    }
}
