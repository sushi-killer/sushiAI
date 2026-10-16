use super::config::{Category, Config};
use super::model::Report;
use super::sources::Sources;
use std::collections::BTreeSet;

struct Exceptions<'a> {
    config: &'a Config,
    used: BTreeSet<usize>,
}

impl Exceptions<'_> {
    fn check(
        &mut self,
        path: &str,
        category: Category,
        symbol: Option<&str>,
        (value, limit): (usize, usize),
    ) -> Option<String> {
        if value <= limit {
            return None;
        }
        let exception = self.config.exceptions.iter().enumerate().find(|(_, e)| {
            e.path == path && e.category == category && e.symbol.as_deref() == symbol
        });
        if let Some((index, exception)) = exception {
            self.used.insert(index);
            if value <= exception.max {
                return None;
            }
            return Some(format!(
                "{path} {:?} {} grew to {value}, above audited maximum {}",
                category,
                symbol.unwrap_or(""),
                exception.max
            ));
        }
        Some(format!(
            "{path} {:?} {} is {value}, limit {limit}; requires an exact audited exception",
            category,
            symbol.unwrap_or("")
        ))
    }
}

pub fn enforce(report: &mut Report, config: &Config, sources: &Sources<'_>) {
    let mut exceptions = Exceptions {
        config,
        used: BTreeSet::new(),
    };
    for file in &report.files {
        if file.runtime_lines > 0 && file.total_lines >= 1000 {
            report.violations.push(format!(
                "{}: mixed/runtime file has {} lines; 1000+ runtime files cannot be excepted",
                file.path, file.total_lines
            ));
        }
        let entry = sources.entries.contains(&sources.root.join(&file.path))
            || matches!(
                std::path::Path::new(&file.path)
                    .file_name()
                    .and_then(|s| s.to_str()),
                Some("lib.rs" | "mod.rs")
            );
        let rules = if entry && file.runtime_lines + file.test_lines > 0 {
            vec![(
                Category::Entry,
                file.runtime_lines + file.test_lines,
                config.limits.entry,
            )]
        } else {
            vec![
                (
                    Category::Production,
                    file.runtime_lines,
                    config.limits.production,
                ),
                (Category::Test, file.test_lines, config.limits.test),
            ]
        };
        for (category, value, limit) in rules {
            if let Some(error) = exceptions.check(&file.path, category, None, (value, limit)) {
                report.violations.push(error);
            }
        }
    }
    for function in &report.functions {
        for (category, value, limit) in [
            (Category::Function, function.lines, config.limits.function),
            (
                Category::Arguments,
                function.arguments,
                config.limits.arguments,
            ),
        ] {
            if let Some(error) = exceptions.check(
                &function.path,
                category,
                Some(&function.symbol),
                (value, limit),
            ) {
                report.violations.push(error);
            }
        }
        if function.lines > 60 || function.arguments > 5 {
            report.review.push(format!(
                "{}:{} {}: {} lines, {} arguments",
                function.path, function.line, function.symbol, function.lines, function.arguments
            ));
        }
    }
    for mac in report.source_macros.iter().filter(|m| m.kind == "item") {
        if let Some(error) = exceptions.check(
            &mac.path,
            Category::SourceMacro,
            Some(&mac.symbol),
            (mac.lines, 0),
        ) {
            report.violations.push(error);
        }
    }
    for (index, exception) in config.exceptions.iter().enumerate() {
        if !exceptions.used.contains(&index) {
            report.violations.push(format!(
                "stale/unnecessary exception: {} {:?} {}",
                exception.path,
                exception.category,
                exception.symbol.as_deref().unwrap_or("")
            ));
        }
    }
    shared_sources(report, config, sources);
}

fn shared_sources(report: &mut Report, config: &Config, sources: &Sources<'_>) {
    for shared in &config.shared_sources {
        let path = sources.root.join(&shared.path).canonicalize().unwrap();
        if !sources.shared_used.contains(&path) {
            report
                .violations
                .push(format!("unused shared source: {}", shared.path));
            continue;
        }
        let lines = sources
            .code
            .get(&path)
            .map(|s| s.text.lines().count())
            .or_else(|| {
                sources
                    .data
                    .get(&path)
                    .map(|b| String::from_utf8_lossy(b).lines().count())
            })
            .unwrap_or(0);
        if lines > shared.max {
            report.violations.push(format!(
                "shared source {} grew to {lines}, maximum {}",
                shared.path, shared.max
            ));
        }
    }
}
