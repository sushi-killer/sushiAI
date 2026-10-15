use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Config {
    pub version: u32,
    pub limits: Limits,
    pub module_edges: BTreeMap<String, Vec<String>>,
    pub pure_roots: Vec<String>,
    pub forbidden_pure_paths: Vec<String>,
    pub forbidden_pure_symbols: Vec<String>,
    pub no_reexport_roots: Vec<String>,
    pub data_roots: Vec<String>,
    pub shared_sources: Vec<SharedSource>,
    pub exceptions: Vec<Exception>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: 1,
            limits: Limits::default(),
            module_edges: BTreeMap::new(),
            pure_roots: Vec::new(),
            forbidden_pure_paths: [
                "std::fs",
                "std::io",
                "std::net",
                "std::process",
                "std::env",
                "std::thread",
                "tokio",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
            forbidden_pure_symbols: vec!["App".into()],
            no_reexport_roots: Vec::new(),
            data_roots: Vec::new(),
            shared_sources: Vec::new(),
            exceptions: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    pub production: usize,
    pub test: usize,
    pub entry: usize,
    pub function: usize,
    pub arguments: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            production: 600,
            test: 800,
            entry: 150,
            function: 100,
            arguments: 5,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedSource {
    pub path: String,
    pub reason: String,
    pub max: usize,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Exception {
    pub path: String,
    pub category: Category,
    #[serde(default)]
    pub symbol: Option<String>,
    pub reason: String,
    pub max: usize,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Production,
    Test,
    Entry,
    Function,
    Arguments,
    SourceMacro,
}

pub fn module_name(value: &str) -> String {
    value.replace('/', "::")
}
pub fn inside(module: &str, root: &str) -> bool {
    module == root
        || module
            .strip_prefix(root)
            .is_some_and(|s| s.starts_with("::"))
}

fn reason_valid(reason: &str) -> bool {
    reason.trim().len() >= 24 && reason.split_whitespace().count() >= 4
}

fn exact_path(path: &str, shared: bool) -> bool {
    !path.is_empty()
        && !path.contains(['*', '?', '[', ']'])
        && !Path::new(path).is_absolute()
        && Path::new(path).components().all(|c| match c {
            Component::Normal(_) => true,
            Component::ParentDir => shared,
            _ => false,
        })
}

impl Config {
    pub fn parse(text: &str, root: &Path) -> Result<Self, String> {
        super::json::validate(text).map_err(|e| format!("invalid rust-standards.json: {e}"))?;
        let mut config: Self =
            serde_json::from_str(text).map_err(|e| format!("invalid rust-standards.json: {e}"))?;
        config.normalize_modules()?;
        config.validate(root)?;
        Ok(config)
    }

    fn normalize_modules(&mut self) -> Result<(), String> {
        let previous = std::mem::take(&mut self.module_edges);
        for (key, values) in previous {
            if self
                .module_edges
                .insert(
                    module_name(&key),
                    values.iter().map(|s| module_name(s)).collect(),
                )
                .is_some()
            {
                return Err(format!("duplicate normalized module owner: {key}"));
            }
        }
        for roots in [&mut self.pure_roots, &mut self.no_reexport_roots] {
            *roots = roots.iter().map(|s| module_name(s)).collect();
        }
        Ok(())
    }

    pub fn validate(&self, root: &Path) -> Result<(), String> {
        if self.version != 1 {
            return Err("rust standards version must be 1".into());
        }
        let d = Limits::default();
        for (name, limit, ceiling) in [
            ("production", self.limits.production, d.production),
            ("test", self.limits.test, d.test),
            ("entry", self.limits.entry, d.entry),
            ("function", self.limits.function, d.function),
            ("arguments", self.limits.arguments, d.arguments),
        ] {
            if limit == 0 || limit > ceiling {
                return Err(format!(
                    "{name} limit must be 1..={ceiling}; use an exact exception"
                ));
            }
        }
        self.validate_modules()?;
        let mut shared = BTreeSet::new();
        for source in &self.shared_sources {
            if !exact_path(&source.path, true) || !reason_valid(&source.reason) || source.max == 0 {
                return Err(format!("invalid bounded shared source: {}", source.path));
            }
            let path = root
                .join(&source.path)
                .canonicalize()
                .map_err(|e| format!("shared source {}: {e}", source.path))?;
            if !path.is_file() || !shared.insert(path) {
                return Err(format!(
                    "duplicate or non-file shared source: {}",
                    source.path
                ));
            }
        }
        let mut keys = BTreeSet::new();
        for exception in &self.exceptions {
            let is_shared = self.shared_sources.iter().any(|s| s.path == exception.path);
            let needs_symbol = matches!(
                exception.category,
                Category::Function | Category::Arguments | Category::SourceMacro
            );
            if !exact_path(&exception.path, is_shared)
                || !reason_valid(&exception.reason)
                || exception.max == 0
                || (needs_symbol
                    != exception
                        .symbol
                        .as_ref()
                        .is_some_and(|s| !s.trim().is_empty() && !s.contains(['*', '?', '[', ']'])))
            {
                return Err(format!(
                    "invalid exact exception: {} {:?}",
                    exception.path, exception.category
                ));
            }
            let path = root
                .join(&exception.path)
                .canonicalize()
                .map_err(|e| format!("exception {}: {e}", exception.path))?;
            if !path.is_file() || !keys.insert((path, exception.category, exception.symbol.clone()))
            {
                return Err(format!(
                    "duplicate or non-file exception: {}",
                    exception.path
                ));
            }
        }
        for path in &self.data_roots {
            if !exact_path(path, false) || !root.join(path).exists() {
                return Err(format!("invalid data root: {path}"));
            }
        }
        Ok(())
    }

    fn validate_modules(&self) -> Result<(), String> {
        let valid = |s: &str| {
            !s.is_empty()
                && s.split("::")
                    .all(|p| syn::parse_str::<syn::Ident>(p).is_ok())
        };
        for (source, targets) in &self.module_edges {
            if !valid(source)
                || targets.iter().any(|s| !valid(s))
                || targets.iter().collect::<BTreeSet<_>>().len() != targets.len()
            {
                return Err(format!("invalid module edge configuration: {source}"));
            }
        }
        for roots in [&self.pure_roots, &self.no_reexport_roots] {
            if roots.iter().any(|s| !valid(s))
                || roots.iter().collect::<BTreeSet<_>>().len() != roots.len()
            {
                return Err("invalid or duplicate module prefix".into());
            }
        }
        for path in &self.forbidden_pure_paths {
            if syn::parse_str::<syn::Path>(path).is_err() {
                return Err(format!("invalid forbidden path: {path}"));
            }
        }
        for symbol in &self.forbidden_pure_symbols {
            if syn::parse_str::<syn::Ident>(symbol).is_err() {
                return Err(format!("invalid forbidden symbol: {symbol}"));
            }
        }
        Ok(())
    }
}
