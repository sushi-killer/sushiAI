use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use toml_edit::{DocumentMut, Table};

#[derive(Eq, Ord, PartialEq, PartialOrd)]
pub struct Target {
    pub path: PathBuf,
    pub test: bool,
    pub library: bool,
}

fn text<'a>(table: &'a Table, key: &str) -> Result<Option<&'a str>, String> {
    table
        .get(key)
        .map(|value| {
            value
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| format!("Cargo target {key} must be a nonempty string"))
        })
        .transpose()
}

fn conventional(directory: &Path, test: bool) -> Result<Vec<Target>, String> {
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut targets = Vec::new();
    for entry in std::fs::read_dir(directory).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.is_file() && path.extension().is_some_and(|ext| ext == "rs") {
            targets.push(Target {
                path,
                test,
                library: false,
            });
        } else if path.is_dir() && path.join("main.rs").is_file() {
            targets.push(Target {
                path: path.join("main.rs"),
                test,
                library: false,
            });
        }
    }
    Ok(targets)
}

fn named_path(root: &Path, kind: &str, name: &str, package: &str) -> PathBuf {
    if kind == "bin" && name == package && root.join("src/main.rs").is_file() {
        return root.join("src/main.rs");
    }
    let directory = match kind {
        "bin" => "src/bin",
        "test" => "tests",
        "bench" => "benches",
        "example" => "examples",
        _ => unreachable!(),
    };
    let direct = root.join(directory).join(format!("{name}.rs"));
    if direct.is_file() {
        direct
    } else {
        root.join(directory).join(name).join("main.rs")
    }
}

fn explicit(root: &Path, doc: &DocumentMut, kind: &str) -> Result<Vec<Target>, String> {
    let Some(item) = doc.get(kind) else {
        return Ok(Vec::new());
    };
    let tables = item
        .as_array_of_tables()
        .ok_or_else(|| format!("Cargo {kind} targets must be an array of tables"))?;
    let package = doc["package"]["name"].as_str().unwrap_or("");
    let mut targets = Vec::new();
    for table in tables {
        let name = text(table, "name")?;
        let path = match text(table, "path")? {
            Some(path) => root.join(path),
            None => named_path(
                root,
                kind,
                name.ok_or_else(|| format!("Cargo {kind} target needs a name or path"))?,
                package,
            ),
        };
        targets.push(Target {
            path,
            test: matches!(kind, "test" | "bench"),
            library: false,
        });
    }
    Ok(targets)
}

pub fn discover(root: &Path, manifest: &str) -> Result<BTreeSet<Target>, String> {
    let doc = manifest
        .parse::<DocumentMut>()
        .map_err(|e| format!("invalid Cargo.toml: {e}"))?;
    let mut targets = BTreeSet::new();
    for path in ["src/lib.rs", "src/main.rs"] {
        if root.join(path).is_file() {
            targets.insert(Target {
                path: root.join(path),
                test: false,
                library: path == "src/lib.rs",
            });
        }
    }
    if let Some(item) = doc.get("lib") {
        let table = item.as_table().ok_or("Cargo lib must be a table")?;
        targets.insert(Target {
            path: root.join(text(table, "path")?.unwrap_or("src/lib.rs")),
            test: false,
            library: true,
        });
    }
    let build = doc.get("package").and_then(|p| p.get("build"));
    let build_path = match build {
        Some(value) if value.as_bool() == Some(false) => None,
        Some(value) if value.as_bool() == Some(true) => Some("build.rs"),
        Some(value) => Some(
            value
                .as_str()
                .filter(|p| !p.is_empty())
                .ok_or("Cargo package.build must be a boolean or nonempty source path")?,
        ),
        None if root.join("build.rs").is_file() => Some("build.rs"),
        None => None,
    };
    if let Some(path) = build_path {
        targets.insert(Target {
            path: root.join(path),
            test: false,
            library: false,
        });
    }
    for kind in ["bin", "test", "bench", "example"] {
        targets.extend(explicit(root, &doc, kind)?);
    }
    for (directory, test) in [
        ("src/bin", false),
        ("tests", true),
        ("benches", true),
        ("examples", false),
    ] {
        targets.extend(conventional(&root.join(directory), test)?);
    }
    Ok(targets)
}
