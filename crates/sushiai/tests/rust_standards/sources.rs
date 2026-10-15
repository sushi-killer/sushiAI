use super::config::Config;
use super::references::{module_directory, References};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use syn::visit::Visit;

#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
pub struct Context {
    pub module: Vec<String>,
    pub test: bool,
}
pub struct Source {
    pub path: String,
    pub text: String,
    pub tree: syn::File,
    pub contexts: BTreeSet<Context>,
}
pub struct Sources<'a> {
    pub root: PathBuf,
    workspace: PathBuf,
    config: Option<&'a Config>,
    edition: String,
    pub code: BTreeMap<PathBuf, Source>,
    pub data: BTreeMap<PathBuf, Vec<u8>>,
    active: BTreeSet<PathBuf>,
    pub shared_used: BTreeSet<PathBuf>,
    pub raw: BTreeMap<PathBuf, Vec<u8>>,
    pub entries: BTreeSet<PathBuf>,
}

pub fn relative(root: &Path, path: &Path) -> String {
    let root: Vec<_> = root.components().collect();
    let path: Vec<_> = path.components().collect();
    let common = root.iter().zip(&path).take_while(|(a, b)| a == b).count();
    let mut out = PathBuf::new();
    for _ in common..root.len() {
        out.push("..");
    }
    for part in &path[common..] {
        out.push(part.as_os_str());
    }
    out.to_string_lossy().replace('\\', "/")
}

fn workspace(root: &Path) -> PathBuf {
    root.ancestors()
        .find(|p| {
            std::fs::read_to_string(p.join("Cargo.toml"))
                .ok()
                .and_then(|s| s.parse::<toml_edit::DocumentMut>().ok())
                .is_some_and(|d| d.contains_key("workspace"))
        })
        .unwrap_or(root)
        .to_path_buf()
}

fn edition(root: &Path, workspace: &Path) -> String {
    for path in [root.join("Cargo.toml"), workspace.join("Cargo.toml")] {
        if let Ok(text) = std::fs::read_to_string(path) {
            if let Ok(doc) = text.parse::<toml_edit::DocumentMut>() {
                for keys in [
                    ["package", "edition", ""],
                    ["workspace", "package", "edition"],
                ] {
                    let mut value = doc.as_item();
                    for key in keys.into_iter().filter(|k| !k.is_empty()) {
                        value = value.get(key).unwrap_or(&toml_edit::Item::None);
                    }
                    if let Some(value) = value.as_str() {
                        return value.to_string();
                    }
                }
            }
        }
    }
    "2021".into()
}

fn formatted(path: &Path, text: &str, edition: &str) -> Result<String, String> {
    let mut child = Command::new("rustfmt")
        .args([
            "--emit",
            "stdout",
            "--edition",
            edition,
            "--config",
            "skip_children=true",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("rustfmt {}: {e}", path.display()))?;
    child
        .stdin
        .take()
        .ok_or("rustfmt stdin missing")?
        .write_all(text.as_bytes())
        .map_err(|e| e.to_string())?;
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "rustfmt {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    String::from_utf8(output.stdout).map_err(|e| e.to_string())
}

fn walk(path: &Path, root: &Path, files: &mut BTreeSet<PathBuf>) -> Result<(), String> {
    if path.is_symlink() {
        return Err(format!(
            "source tree symlinks must not conceal files: {}",
            relative(root, path)
        ));
    }
    if path.is_file() {
        files.insert(path.to_path_buf());
        return Ok(());
    }
    for entry in std::fs::read_dir(path).map_err(|e| format!("{}: {e}", path.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        if ["target", ".git", "node_modules"]
            .iter()
            .any(|n| entry.file_name() == *n)
        {
            continue;
        }
        walk(&entry.path(), root, files)?;
    }
    Ok(())
}

impl<'a> Sources<'a> {
    pub fn discover(root: &Path, config: Option<&'a Config>) -> Result<Self, String> {
        let root = root.canonicalize().map_err(|e| e.to_string())?;
        let workspace = workspace(&root);
        let mut found = Self {
            edition: edition(&root, &workspace),
            root,
            workspace,
            config,
            code: BTreeMap::new(),
            data: BTreeMap::new(),
            active: BTreeSet::new(),
            shared_used: BTreeSet::new(),
            raw: BTreeMap::new(),
            entries: BTreeSet::new(),
        };
        let mut files = BTreeSet::new();
        walk(&found.root, &found.root, &mut files)?;
        let manifest_path = found.root.join("Cargo.toml");
        let manifest = std::fs::read_to_string(&manifest_path)
            .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
        for target in super::targets::discover(&found.root, &manifest)? {
            if target.library {
                let path = found.checked_path(&target.path)?;
                found.entries.insert(path);
            }
            found.load(
                &target.path,
                Context {
                    module: Vec::new(),
                    test: target.test,
                },
                true,
            )?;
        }
        found.raw.insert(manifest_path, manifest.into_bytes());
        let rust: Vec<_> = files
            .iter()
            .filter(|p| p.extension().is_some_and(|e| e == "rs"))
            .cloned()
            .collect();
        for path in rust {
            if found.code.contains_key(&path) || found.data.contains_key(&path) {
                continue;
            }
            let rel = relative(&found.root, &path);
            if found.is_data(&path) {
                found.add_data(&path)?;
                continue;
            }
            let module: Vec<_> = rel
                .strip_prefix("src/")
                .unwrap_or(&rel)
                .trim_end_matches(".rs")
                .split('/')
                .filter(|s| !matches!(*s, "mod" | "lib" | "main"))
                .map(str::to_string)
                .collect();
            found.load(
                &path,
                Context {
                    module,
                    test: rel.starts_with("tests/") || rel.starts_with("benches/"),
                },
                false,
            )?;
        }
        for path in files {
            if found.is_data(&path) && !found.code.contains_key(&path) {
                found.add_data(&path)?;
            }
        }
        Ok(found)
    }

    fn is_data(&self, path: &Path) -> bool {
        let defaults = vec!["prompts".to_string(), "tests/fixtures".to_string()];
        let roots = self.config.map_or(&defaults, |c| &c.data_roots);
        roots.iter().any(|p| path.starts_with(self.root.join(p)))
    }

    fn checked_path(&mut self, path: &Path) -> Result<PathBuf, String> {
        let path = path
            .canonicalize()
            .map_err(|e| format!("source {}: {e}", path.display()))?;
        if !path.starts_with(&self.workspace) {
            return Err(format!("source path escapes workspace: {}", path.display()));
        }
        if !path.starts_with(&self.root) {
            if let Some(config) = self.config {
                let source = config
                    .shared_sources
                    .iter()
                    .find(|s| self.root.join(&s.path).canonicalize().ok().as_ref() == Some(&path));
                if source.is_none() {
                    return Err(format!(
                        "source path escapes crate without exact sharedSources entry: {}",
                        path.display()
                    ));
                }
            }
            self.shared_used.insert(path.clone());
        }
        if !path.is_file() {
            return Err(format!("source is not a file: {}", path.display()));
        }
        Ok(path)
    }

    fn add_data(&mut self, path: &Path) -> Result<(), String> {
        let path = self.checked_path(path)?;
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        self.raw.insert(path.clone(), bytes.clone());
        self.data.insert(path, bytes);
        Ok(())
    }

    fn load(&mut self, path: &Path, context: Context, crate_root: bool) -> Result<(), String> {
        let path = self.checked_path(path)?;
        if self.active.contains(&path) {
            return Err(format!("source inclusion cycle: {}", path.display()));
        }
        if self
            .code
            .get(&path)
            .is_some_and(|s| s.contexts.contains(&context))
        {
            return Ok(());
        }
        self.active.insert(path.clone());
        if !self.code.contains_key(&path) {
            let raw = std::fs::read(&path).map_err(|e| e.to_string())?;
            let text = std::str::from_utf8(&raw).map_err(|e| e.to_string())?;
            let text = formatted(&path, text, &self.edition)?;
            let tree =
                syn::parse_file(&text).map_err(|e| format!("AST {}: {e}", path.display()))?;
            self.raw.insert(path.clone(), raw);
            self.data.remove(&path);
            self.code.insert(
                path.clone(),
                Source {
                    path: relative(&self.root, &path),
                    text,
                    tree,
                    contexts: BTreeSet::new(),
                },
            );
        }
        let source = self.code.get_mut(&path).unwrap();
        source.contexts.insert(context.clone());
        let tree = source.tree.clone();
        let mut refs = References {
            directory: if crate_root {
                path.parent().unwrap().into()
            } else {
                module_directory(&path)
            },
            attribute_base: path.parent().unwrap().into(),
            file_directory: path.parent().unwrap().into(),
            module: context.module,
            test: context.test,
            found: Vec::new(),
            errors: Vec::new(),
        };
        refs.visit_file(&tree);
        if !refs.errors.is_empty() {
            return Err(format!("{}: {}", path.display(), refs.errors.join("; ")));
        }
        for reference in refs.found {
            if reference.data {
                self.add_data(&reference.path)?;
            } else {
                self.load(
                    &reference.path,
                    Context {
                        module: reference.module,
                        test: reference.test,
                    },
                    false,
                )?;
            }
        }
        self.active.remove(&path);
        Ok(())
    }

    pub fn digest(&self) -> String {
        let mut digest = Sha256::new();
        for (path, bytes) in &self.raw {
            digest.update(relative(&self.root, path));
            digest.update([0]);
            digest.update(bytes);
            digest.update([0]);
        }
        format!("{:x}", digest.finalize())
    }
}
