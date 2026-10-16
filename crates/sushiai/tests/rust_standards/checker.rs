#[path = "boundaries.rs"]
mod boundaries;
#[path = "config.rs"]
pub mod config;
#[path = "imports.rs"]
mod imports;
#[path = "json.rs"]
mod json;
#[path = "limits.rs"]
mod limits;
#[path = "locals.rs"]
mod locals;
#[path = "metrics.rs"]
mod metrics;
#[path = "model.rs"]
pub mod model;
#[path = "references.rs"]
mod references;
#[path = "sources.rs"]
mod sources;
#[path = "syntax.rs"]
mod syntax;
#[path = "targets.rs"]
mod targets;

use config::Config;
use model::{FileMetric, Report};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use syn::visit::Visit;

pub fn analyze(root: &Path, config: Option<&Config>) -> Result<Report, String> {
    if let Some(config) = config {
        config.validate(root)?;
    }
    let sources = sources::Sources::discover(root, config)?;
    let mut report = Report {
        source_root: sources.root.to_string_lossy().into(),
        source_digest: sources.digest(),
        ..Report::default()
    };
    let default = Config::default();
    let rules = config.unwrap_or(&default);
    let (imports, roots) = dependencies(&sources);
    for source in sources.code.values() {
        let context = source
            .contexts
            .iter()
            .find(|c| !c.test)
            .or_else(|| source.contexts.iter().next())
            .unwrap();
        let full_test = source.contexts.iter().all(|c| c.test);
        let mut metric = metrics::Metrics::new(&source.path, &context.module, full_test);
        metric.visit_file(&source.tree);
        let total = source.text.lines().count();
        let tests = if full_test {
            total
        } else {
            metric
                .test_lines
                .iter()
                .filter(|&&l| l > 0 && l <= total)
                .count()
        };
        report.files.push(FileMetric {
            path: source.path.clone(),
            runtime_lines: total - tests,
            test_lines: tests,
            data_lines: 0,
            total_lines: total,
            bytes: source.text.len(),
        });
        report.functions.extend(metric.functions);
        report.source_macros.extend(metric.macros);
        for context in &source.contexts {
            let module = context.module.join("::");
            if !context.test
                && !module.is_empty()
                && !rules.module_edges.is_empty()
                && !rules
                    .module_edges
                    .keys()
                    .any(|r| config::inside(&module, r))
            {
                report.violations.push(format!(
                    "{}: module {module} has no configured owner",
                    source.path
                ));
            }
            boundaries::Boundaries {
                config: rules,
                resolver: imports::Resolver {
                    imports: &imports,
                    roots: &roots,
                },
                report: &mut report,
                path: &source.path,
                module: context.module.clone(),
                test: context.test,
                local: BTreeMap::new(),
                values: locals::Values::default(),
            }
            .visit_file(&source.tree);
        }
    }
    for (path, bytes) in &sources.data {
        if sources.code.contains_key(path) {
            continue;
        }
        let lines = std::str::from_utf8(bytes).map_or(0, |s| s.lines().count());
        report.files.push(FileMetric {
            path: sources::relative(&sources.root, path),
            data_lines: lines,
            total_lines: lines,
            bytes: bytes.len(),
            ..FileMetric::default()
        });
    }
    limits::enforce(&mut report, rules, &sources);
    report.files.sort_by(|a, b| a.path.cmp(&b.path));
    report.violations.sort();
    report.violations.dedup();
    Ok(report)
}

fn dependencies(sources: &sources::Sources<'_>) -> (imports::Imports, BTreeSet<String>) {
    let mut imports = BTreeMap::new();
    let mut roots = BTreeSet::new();
    for source in sources.code.values() {
        for context in source.contexts.iter().filter(|c| !c.test) {
            imports::register(&source.tree, &context.module, &mut imports);
            if let Some(root) = context.module.first() {
                roots.insert(root.clone());
            }
        }
    }
    roots.extend(
        imports
            .keys()
            .filter_map(|p| p.split("::").next())
            .filter(|s| !s.is_empty())
            .map(str::to_string),
    );
    (imports, roots)
}
