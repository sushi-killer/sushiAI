use super::checker::{analyze, config::Config, model::Report};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

struct Tree {
    _temporary: tempfile::TempDir,
    root: PathBuf,
}
impl Tree {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::write(
            temporary.path().join("Cargo.toml"),
            "[workspace]\nmembers=[]\n",
        )
        .unwrap();
        let root = temporary.path().join("crate");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname='synthetic'\nversion='0.1.0'\nedition='2021'\n",
        )
        .unwrap();
        std::fs::write(root.join("src/lib.rs"), "").unwrap();
        Self {
            _temporary: temporary,
            root,
        }
    }
    fn write(&self, path: &str, text: &str) {
        let path = self.root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    fn config(&self, value: Value) -> Config {
        Config::parse(&value.to_string(), &self.root).unwrap()
    }
    fn report(&self, value: Value) -> Report {
        analyze(&self.root, Some(&self.config(value))).unwrap()
    }
}
fn declarations(n: usize) -> String {
    (0..n)
        .map(|n| format!("const VALUE_{n}:usize={n};\n"))
        .collect()
}
fn body(n: usize) -> String {
    format!(
        "fn exercise(){{\n{}}}\n",
        (0..n)
            .map(|n| format!("let value_{n}={n};\n"))
            .collect::<String>()
    )
}
fn has(report: &Report, text: &str) -> bool {
    report.violations.iter().any(|v| v.contains(text))
}
fn exception(path: &str, category: &str, max: usize) -> Value {
    json!({"path":path,"category":category,"max":max,"reason":"Reviewed cohesive operation with a bounded algorithm and preserved behavior"})
}
fn module_tree() -> Tree {
    let tree = Tree::new();
    tree.write(
        "src/lib.rs",
        "pub mod task; pub mod engine; pub mod policy;",
    );
    tree.write("src/task.rs", "pub struct Value;");
    tree.write("src/engine.rs", "pub struct App;");
    tree.write("src/policy.rs", "pub struct Rule;");
    tree
}
fn graph() -> Value {
    json!({"moduleEdges":{"task":[],"policy":["task"],"engine":["task","policy"]},"pureRoots":["task"]})
}

#[test]
fn production_threshold_and_nonexceptable_godfile_are_exact() {
    let tree = Tree::new();
    tree.write("src/rules.rs", &declarations(600));
    assert!(!has(&tree.report(json!({})), "Production"));
    tree.write("src/rules.rs", &declarations(601));
    assert!(has(&tree.report(json!({})), "Production"));
    tree.write("src/rules.rs", &declarations(1000));
    assert!(has(
        &tree.report(json!({"exceptions":[exception("src/rules.rs","production",1001)]})),
        "cannot be excepted"
    ));
}

#[test]
fn test_and_entry_thresholds_are_independent() {
    let tree = Tree::new();
    tree.write("tests/large.rs", &declarations(800));
    assert!(!has(&tree.report(json!({})), "Test"));
    tree.write("tests/large.rs", &declarations(801));
    assert!(has(&tree.report(json!({})), "Test"));
    tree.write("src/lib.rs", &declarations(150));
    assert!(!has(&tree.report(json!({})), "Entry"));
    tree.write("src/lib.rs", &declarations(151));
    assert!(has(&tree.report(json!({})), "Entry"));
}

#[test]
fn nested_interleaved_cfg_tests_do_not_hide_runtime() {
    let tree = Tree::new();
    tree.write(
        "src/rules.rs",
        r#"
const BEFORE: usize = 0;
#[cfg(all(test, unix))]
mod checks { const INSIDE: usize = 0; mod nested { fn probe() {} } }
const AFTER: usize = 1;
#[cfg(any(test, feature="runtime"))]
fn possible_runtime() {}
#[cfg(not(test))]
fn production() {}
"#,
    );
    let report = tree.report(json!({}));
    let file = report
        .files
        .iter()
        .find(|f| f.path == "src/rules.rs")
        .unwrap();
    assert!(file.test_lines > 0 && file.runtime_lines > 0);
    assert_eq!(file.total_lines, file.runtime_lines + file.test_lines);
    assert!(
        report
            .functions
            .iter()
            .find(|f| f.symbol.ends_with("probe"))
            .unwrap()
            .test
    );
    assert!(
        !report
            .functions
            .iter()
            .find(|f| f.symbol.ends_with("possible_runtime"))
            .unwrap()
            .test
    );
    assert!(
        !report
            .functions
            .iter()
            .find(|f| f.symbol.ends_with("production"))
            .unwrap()
            .test
    );
}

#[test]
fn raw_strings_nested_comments_and_nested_type_commas_are_ast_parsed() {
    let tree = Tree::new();
    tree.write(
        "src/rules.rs",
        r####"
/* outer { /* nested fn fake(a,b,c,d,e,f) {} */ } */
fn actual(a: (u8,u8), b: [u8;2], c: fn(u8,u8), d: Option<(u8,u8)>, e: u8) {
    let text=r###" } fn invented(a,b,c,d,e,f) { /* "###;
}
"####,
    );
    let report = tree.report(json!({}));
    assert_eq!(report.functions.len(), 1);
    assert_eq!(report.functions[0].arguments, 5);
    assert!(report.violations.is_empty(), "{:?}", report.violations);
}

#[test]
fn function_threshold_review_band_and_receiver_arguments_are_measured() {
    let tree = Tree::new();
    tree.write("src/rules.rs", &body(98));
    let report = tree.report(json!({}));
    assert_eq!(report.functions[0].lines, 100);
    assert!(!has(&report, "Function"));
    assert_eq!(report.review.len(), 1);
    tree.write("src/rules.rs", &body(99));
    assert!(has(&tree.report(json!({})), "Function"));
    tree.write(
        "src/rules.rs",
        "struct Thing; impl Thing { fn act(&self,a:u8,b:u8,c:u8,d:u8,e:u8) {} }",
    );
    let report = tree.report(json!({}));
    assert_eq!(report.functions[0].arguments, 6);
    assert!(has(&report, "Arguments"));
}

#[test]
fn exact_exceptions_reject_growth_staleness_duplicates_and_wrong_symbols() {
    let tree = Tree::new();
    tree.write("src/rules.rs", &declarations(601));
    let entry = exception("src/rules.rs", "production", 601);
    assert!(tree
        .report(json!({"exceptions":[entry.clone()]}))
        .violations
        .is_empty());
    tree.write("src/rules.rs", &declarations(602));
    assert!(has(
        &tree.report(json!({"exceptions":[entry.clone()]})),
        "above audited maximum"
    ));
    tree.write("src/rules.rs", &declarations(600));
    assert!(has(
        &tree.report(json!({"exceptions":[entry.clone()]})),
        "stale/unnecessary"
    ));
    assert!(Config::parse(
        &json!({"exceptions":[entry.clone(),entry]}).to_string(),
        &tree.root
    )
    .is_err());
    tree.write("src/rules.rs", &body(99));
    let mut entry = exception("src/rules.rs", "function", 101);
    entry["symbol"] = json!("rules::missing");
    assert!(has(
        &tree.report(json!({"exceptions":[entry]})),
        "stale/unnecessary"
    ));
}

#[test]
fn malformed_or_broad_configuration_fails_closed() {
    let tree = Tree::new();
    for value in [
        json!({"version":2}),
        json!({"unknown":true}),
        json!({"limits":{"production":1000}}),
        json!({"exceptions":[exception("src/*.rs","production",900)]}),
        json!({"exceptions":[exception("src/missing.rs","production",900)]}),
        json!({"exceptions":[{"path":"src/lib.rs","category":"entry","max":151,"reason":"legacy"}]}),
        json!({"moduleEdges":{"bad/root!":[]}}),
    ] {
        assert!(
            Config::parse(&value.to_string(), &tree.root).is_err(),
            "{value}"
        );
    }
    tree.write("src/broken.rs", "fn missing( {");
    assert!(analyze(&tree.root, Some(&Config::default())).is_err());
}

#[test]
fn grouped_aliases_and_direct_paths_enforce_edges_and_purity() {
    let tree = module_tree();
    tree.write("src/task.rs","use crate::{engine::{App as State},policy::Rule}; fn f(value:State) { crate::engine::App; }");
    let report = tree.report(graph());
    assert!(has(&report, "task -> engine"));
    assert!(has(
        &report,
        "pure module task references crate::engine::App"
    ));
    tree.write(
        "src/task.rs",
        "use std::{fs as disk,io}; fn f() { disk::read(\"file\"); }",
    );
    let report = tree.report(graph());
    assert!(has(&report, "std::fs"));
    assert!(has(&report, "std::io"));
}

#[test]
fn nested_pure_roots_local_aliases_and_macro_tokens_are_checked() {
    let tree = module_tree();
    tree.write("src/policy.rs","pub mod rules { fn f() { use std::fs as disk; disk::read(\"file\"); opaque!(crate::engine::App); } }");
    let mut config = graph();
    config["pureRoots"] = json!(["policy/rules"]);
    let report = tree.report(config);
    assert!(has(&report, "pure module policy::rules references std::fs"));
    assert!(has(&report, "crate::engine::App"));
}

#[test]
fn named_canonical_exports_are_allowed_and_globs_are_not() {
    let tree = module_tree();
    tree.write(
        "src/lib.rs",
        "pub mod task; pub mod engine; pub mod policy; pub use task::Value;",
    );
    assert!(tree.report(graph()).violations.is_empty());
    tree.write("src/engine.rs", "pub use crate::task::Value;");
    let mut config = graph();
    config["noReexportRoots"] = json!(["engine"]);
    assert!(has(&tree.report(config), "reexports are prohibited"));
    tree.write(
        "src/task.rs",
        "use std::collections::*; pub use crate::engine::*;",
    );
    assert!(has(&tree.report(graph()), "glob import/reexport"));
}

#[test]
fn cfg_test_imports_and_helpers_remain_test_code() {
    let tree = module_tree();
    tree.write(
        "src/task.rs",
        "pub struct Value; #[cfg(test)] mod checks { use std::fs::*; #[test] fn checks() {} }",
    );
    assert!(tree.report(graph()).violations.is_empty());
    tree.write(
        "tests/runner.rs",
        "mod common; #[test] fn works() { common::helper(); }",
    );
    tree.write("tests/common.rs", "pub fn helper() {}");
    let report = tree.report(graph());
    let helper = report
        .files
        .iter()
        .find(|f| f.path == "tests/common.rs")
        .unwrap();
    assert_eq!(helper.runtime_lines, 0);
}

#[test]
fn literal_paths_and_includes_are_scanned_in_the_consuming_module() {
    let tree = module_tree();
    tree.write(
        "src/task.rs",
        "#[path=\"special.rs\"] mod hidden; include!(\"included.rs\");",
    );
    tree.write("src/special.rs", "use std::fs; fn read() {} ");
    tree.write(
        "src/included.rs",
        "fn hidden() { std::fs::read(\"file\"); }",
    );
    let report = tree.report(graph());
    assert!(has(&report, "pure module task::hidden references std::fs"));
    assert!(has(&report, "pure module task references std::fs"));
    assert!(report
        .functions
        .iter()
        .any(|f| f.path == "src/included.rs" && f.symbol == "task::hidden"));
}

#[test]
fn path_escapes_cycles_and_nonliteral_includes_fail_closed() {
    let tree = Tree::new();
    tree.write("../outside.rs", "fn outside() {}");
    tree.write("src/lib.rs", "#[path=\"../../outside.rs\"] mod outside;");
    assert!(analyze(&tree.root, Some(&Config::default())).is_err());
    tree.write("src/lib.rs", "include!(\"lib.rs\");");
    assert!(analyze(&tree.root, Some(&Config::default())).is_err());
    tree.write(
        "src/lib.rs",
        "include!(concat!(env!(\"OUT_DIR\"),\"/generated.rs\"));",
    );
    assert!(analyze(&tree.root, Some(&Config::default())).is_err());
}

#[test]
fn shared_sources_are_exact_bounded_scanned_and_required_to_be_used() {
    let tree = Tree::new();
    tree.write(
        "../shared.rs",
        "pub fn external() { std::fs::read(\"file\"); }",
    );
    tree.write("src/lib.rs", "#[path=\"../../shared.rs\"] pub mod task;");
    let config = json!({"pureRoots":["task"],"sharedSources":[{"path":"../shared.rs","reason":"Shared process adapter has independent consumers in multiple packages","max":10}]});
    let report = tree.report(config.clone());
    assert!(has(&report, "pure module task references std::fs"));
    let mut too_small = config.clone();
    too_small["sharedSources"][0]["max"] = json!(1);
    assert!(has(
        &tree.report(too_small),
        "shared source ../shared.rs grew"
    ));
    tree.write("src/lib.rs", "");
    assert!(has(&tree.report(config), "unused shared source"));
}

#[test]
fn source_generating_macros_are_reported_and_need_exact_audits() {
    let tree = Tree::new();
    tree.write(
        "src/rules.rs",
        "macro_rules! generate { () => { fn hidden(a:u8,b:u8,c:u8,d:u8,e:u8,f:u8) {} }; }",
    );
    let report = tree.report(json!({}));
    assert_eq!(report.source_macros.len(), 1);
    assert!(has(&report, "SourceMacro"));
    let mut entry = exception(
        "src/rules.rs",
        "source_macro",
        report.source_macros[0].lines,
    );
    entry["symbol"] = json!("rules::generate!");
    assert!(!has(
        &tree.report(json!({"exceptions":[entry]})),
        "SourceMacro"
    ));
}

#[test]
fn report_digests_and_data_counts_are_reproducible_without_config() {
    let tree = Tree::new();
    tree.write(
        "src/lib.rs",
        "pub const PROMPT:&str=include_str!(\"../prompts/text.txt\");",
    );
    tree.write("prompts/text.txt", "one\ntwo\n");
    let first = analyze(&tree.root, None).unwrap();
    let second = analyze(&tree.root, None).unwrap();
    assert_eq!(first.source_digest, second.source_digest);
    assert_eq!(
        first.source_root,
        tree.root.canonicalize().unwrap().to_string_lossy()
    );
    let data = first
        .files
        .iter()
        .find(|f| f.path == "prompts/text.txt")
        .unwrap();
    assert_eq!(data.data_lines, 2);
    assert_eq!(data.runtime_lines, 0);
    tree.write("prompts/text.txt", "changed\n");
    assert_ne!(
        first.source_digest,
        analyze(&tree.root, None).unwrap().source_digest
    );
}

#[test]
fn unowned_production_modules_cannot_bypass_the_configured_graph() {
    let tree = module_tree();
    tree.write("src/unowned.rs", "pub fn unrelated() {} ");
    assert!(has(&tree.report(graph()), "has no configured owner"));
}

#[test]
fn report_mode_still_reports_default_size_failures() {
    let tree = Tree::new();
    tree.write("src/rules.rs", &declarations(601));
    assert!(has(&analyze(&tree.root, None).unwrap(), "Production"));
}

#[test]
fn measured_files_use_rustfmt_output_not_minified_input() {
    let tree = Tree::new();
    tree.write("src/rules.rs", "fn tiny(){let a=1;let b=2;let c=3;}");
    let report = tree.report(json!({}));
    let file = report
        .files
        .iter()
        .find(|f| f.path == "src/rules.rs")
        .unwrap();
    assert!(file.total_lines > 1);
    assert_eq!(report.functions[0].lines, 5);
}

#[test]
fn checker_helpers_remain_bounded() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/rust_standards");
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        let lines = std::fs::read_to_string(&path).unwrap().lines().count();
        let max = if path.file_name().is_some_and(|n| n == "tests.rs") {
            800
        } else if path.file_name().is_some_and(|n| n == "checker.rs") {
            150
        } else {
            600
        };
        assert!(
            lines <= max,
            "{} has {lines} lines, maximum {max}",
            path.display()
        );
    }
}

#[test]
fn production_aliases_are_not_overridden_by_test_imports_or_targets() {
    let tree = module_tree();
    tree.write("src/task.rs", "use std::fs as disk; #[cfg(test)] use std::collections as disk; fn f() { disk::read(\"file\"); }");
    tree.write(
        "tests/runner.rs",
        "use std::collections as disk; #[test] fn test_only() {} ",
    );
    assert!(has(
        &tree.report(graph()),
        "pure module task references std::fs"
    ));
    tree.write(
        "src/task.rs",
        "extern crate tokio as runtime; fn f() { runtime::spawn(async {}); }",
    );
    assert!(has(
        &tree.report(graph()),
        "pure module task references tokio"
    ));
}

#[test]
fn conditional_source_paths_and_normalized_duplicate_owners_fail_closed() {
    let tree = Tree::new();
    tree.write(
        "src/lib.rs",
        "#[cfg_attr(unix, path=\"alternate.rs\")] mod rules;",
    );
    tree.write("src/rules.rs", "");
    tree.write("src/alternate.rs", "");
    assert!(analyze(&tree.root, Some(&Config::default())).is_err());
    assert!(Config::parse(
        r#"{"moduleEdges":{"task/rules":[],"task::rules":[]}}"#,
        &tree.root
    )
    .is_err());
}

#[test]
fn deriving_and_attribute_source_generators_are_visible_in_reports() {
    let tree = Tree::new();
    tree.write(
        "src/rules.rs",
        "#[derive(Clone, Serialize)] struct Value; #[custom::generate] fn generated() {}",
    );
    let report = tree.report(json!({}));
    assert_eq!(report.source_macros.len(), 2);
    assert!(report.source_macros.iter().all(|m| m.kind == "attribute"));
    assert!(report
        .source_macros
        .iter()
        .any(|m| m.symbol.ends_with("@derive")));
    assert!(report
        .source_macros
        .iter()
        .any(|m| m.symbol.ends_with("@custom::generate")));
}

#[test]
fn duplicate_json_keys_fail_at_every_object_level_after_unescaping() {
    let tree = Tree::new();
    for text in [
        r#"{"version":1,"version":1}"#,
        r#"{"moduleEdges":{"task":[],"task":["engine"]}}"#,
        r#"{"moduleEdges":{"task":[],"\u0074ask":[]}}"#,
        r#"{"limits":{"function":90,"function":100}}"#,
        r#"{"exceptions":[{"path":"src/lib.rs","path":"src/other.rs"}]}"#,
    ] {
        let error = Config::parse(text, &tree.root).unwrap_err();
        assert!(error.contains("duplicate JSON key"), "{text}: {error}");
    }
    assert!(Config::parse(
        r#"{"moduleEdges":{"task":[],"engine":["task"]},"pureRoots":["task"]}"#,
        &tree.root
    )
    .is_ok());
}

#[test]
fn custom_cargo_entry_paths_are_ast_scanned_without_rust_extensions() {
    let tree = Tree::new();
    tree.write(
        "Cargo.toml",
        "[package]\nname='synthetic'\nversion='0.1.0'\nbuild='scripts/setup.rust'\n\
         [lib]\npath='custom/library.rust'\n\
         [[bin]]\nname='worker'\npath='custom/worker.rust'\n\
         [[test]]\nname='spec'\npath='checks/spec.rust'\n\
         [[bench]]\nname='speed'\npath='checks/speed.rust'\n\
         [[example]]\nname='demo'\npath='custom/demo.rust'\n",
    );
    tree.write("custom/library.rust", "pub mod task;");
    tree.write("custom/task.rs", "fn read() { std::fs::read(\"file\"); }");
    tree.write("custom/worker.rust", &declarations(1000));
    tree.write("checks/spec.rust", &declarations(801));
    tree.write("checks/speed.rust", "fn benchmark() {}");
    tree.write("custom/demo.rust", "fn main() {}");
    tree.write("scripts/setup.rust", "fn main() {}");
    let report = tree.report(json!({"pureRoots":["task"]}));
    assert!(has(&report, "pure module task references std::fs"));
    assert!(has(&report, "custom/worker.rust: mixed/runtime file"));
    assert!(has(&report, "cannot be excepted"));
    assert!(has(&report, "checks/spec.rust Test"));
    for path in ["checks/spec.rust", "checks/speed.rust"] {
        let file = report.files.iter().find(|f| f.path == path).unwrap();
        assert_eq!(file.runtime_lines, 0, "{path}");
        assert!(file.test_lines > 0, "{path}");
    }
    for path in ["custom/demo.rust", "scripts/setup.rust"] {
        let file = report.files.iter().find(|f| f.path == path).unwrap();
        assert!(file.runtime_lines > 0, "{path}");
        assert_eq!(file.test_lines, 0, "{path}");
    }
}

#[test]
fn invalid_or_escaping_cargo_target_paths_fail_closed() {
    let tree = Tree::new();
    tree.write("../external.rust", "fn outside() {}");
    tree.write("custom/broken.rust", "fn broken( {");
    for suffix in [
        "[lib]\npath='../external.rust'\n",
        "[lib]\npath='missing.rust'\n",
        "[lib]\npath='custom/broken.rust'\n",
        "[[bin]]\nname='broken'\npath=7\n",
        "[[test]]\nname='absent'\n",
    ] {
        tree.write(
            "Cargo.toml",
            &format!("[package]\nname='synthetic'\nversion='0.1.0'\n{suffix}"),
        );
        assert!(
            analyze(&tree.root, Some(&Config::default())).is_err(),
            "{suffix}"
        );
    }
}

#[test]
fn a_custom_library_path_keeps_the_module_entry_limit() {
    let tree = Tree::new();
    tree.write(
        "Cargo.toml",
        "[package]\nname='synthetic'\nversion='0.1.0'\n[lib]\npath='library.rust'\n",
    );
    tree.write("library.rust", &declarations(150));
    assert!(!has(&tree.report(json!({})), "library.rust Entry"));
    tree.write("library.rust", &declarations(151));
    assert!(has(&tree.report(json!({})), "library.rust Entry"));
}

#[test]
fn named_directory_targets_resolve_modules_from_the_entry_parent() {
    let tree = Tree::new();
    tree.write("src/bin/worker/main.rs", "mod child; fn main() {}");
    tree.write("src/bin/worker/child.rs", "pub fn work() {}");
    tree.write("tests/spec/main.rs", "mod child; #[test] fn works() {}");
    tree.write("tests/spec/child.rs", "pub fn check() {}");
    let first = tree.report(json!({}));
    let child = first
        .files
        .iter()
        .find(|f| f.path == "tests/spec/child.rs")
        .unwrap();
    assert_eq!(child.runtime_lines, 0);
    assert!(child.test_lines > 0);
    tree.write(
        "Cargo.toml",
        "[package]\nname='synthetic'\nversion='0.1.0'\n[[bin]]\nname='worker'\n",
    );
    let second = tree.report(json!({}));
    assert_ne!(first.source_digest, second.source_digest);
}

#[test]
fn local_values_do_not_become_module_edges_and_do_not_hide_qualified_paths() {
    let tree = module_tree();
    tree.write(
        "src/lib.rs",
        "pub mod task; pub mod engine; pub mod policy; pub mod report;",
    );
    tree.write("src/report.rs", "pub struct Value;");
    tree.write(
        "src/task.rs",
        r#"
fn observe(report: usize, pair: (usize,usize)) {
    let report = report + 1;
    let (engine, _) = pair;
    let policy = |report: usize| report + engine;
    let _ = policy(report);
    let _ = format!("{}", report);
    match Some(report) { Some(report) => { let _ = report; }, None => {} }
    for report in [report] { let _ = report; }
    if let Some(report) = Some(report) { let _ = report; }
    while let Some(report) = None::<usize> { let _ = report; }
    { let report = 1; let _ = report; }
}
"#,
    );
    let mut rules = graph();
    rules["moduleEdges"]["report"] = json!([]);
    let report = tree.report(rules.clone());
    assert!(report.violations.is_empty(), "{:?}", report.violations);
    tree.write("src/task.rs", r#"fn observe(report: usize) { let _ = report; let _ = crate::report::Value; let _ = std::fs::read("x"); }"#);
    let report = tree.report(rules);
    assert!(has(&report, "task -> report"));
    assert!(has(&report, "pure module task references std::fs::read"));
}

#[test]
fn local_modules_and_canonical_reexports_take_precedence_over_crate_roots() {
    let tree = Tree::new();
    tree.write(
        "src/lib.rs",
        "pub mod task; pub mod delivery; pub mod consumer;",
    );
    tree.write(
        "src/task/mod.rs",
        "pub mod delivery; pub use delivery::{Value as Canonical}; pub(crate) fn container() {} ",
    );
    tree.write("src/task/delivery.rs", "pub struct Value;");
    tree.write("src/delivery.rs", "pub struct IoValue;");
    tree.write(
        "src/consumer.rs",
        "pub fn consume(_:crate::task::Canonical) {} ",
    );
    let output = std::process::Command::new("rustc")
        .args(["--edition=2021", "--crate-type=lib", "--emit=metadata"])
        .arg(tree.root.join("src/lib.rs"))
        .arg("-o")
        .arg(tree.root.join("compiled.rmeta"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rules =
        json!({"moduleEdges":{"task":[],"delivery":[],"consumer":["task"]},"pureRoots":["task"]});
    let report = tree.report(rules.clone());
    assert!(report.violations.is_empty(), "{:?}", report.violations);
    assert!(report
        .edges
        .iter()
        .any(|edge| edge.from == "consumer" && edge.to == "task"));
    assert!(report.edges.iter().all(|edge| !edge.to.is_empty()));
    tree.write("src/task/mod.rs", "pub mod delivery; pub use delivery::{Value as Canonical}; pub fn wrong(_:crate::delivery::IoValue) {} ");
    assert!(has(&tree.report(rules), "task -> delivery"));
}

#[test]
fn macro_fields_and_members_do_not_hide_real_nested_paths() {
    let tree = module_tree();
    tree.write(
        "src/lib.rs",
        "pub mod task; pub mod engine; pub mod policy; pub mod harness; pub mod store;",
    );
    tree.write("src/harness.rs", "pub struct Native;");
    tree.write("src/store.rs", "pub struct Store;");
    tree.write(
        "src/engine.rs",
        r#"
fn inspect(route: Route, app: Context) {
    let _ = vec![Route { harness: Harness::Claude }];
    tokio::select! { _ = async {} => { let _ = app.store; let _ = route.harness; } }
}
"#,
    );
    let mut rules = graph();
    rules["moduleEdges"]["harness"] = json!([]);
    rules["moduleEdges"]["store"] = json!([]);
    let report = tree.report(rules.clone());
    assert!(report.violations.is_empty(), "{:?}", report.violations);
    tree.write(
        "src/task.rs",
        r#"fn wrong() { inspect!(nested!(std::fs::read("x"))); }"#,
    );
    tree.write(
        "src/engine.rs",
        "fn wrong() { inspect!(nested!(crate::store::Store)); }",
    );
    let report = tree.report(rules);
    assert!(has(&report, "pure module task references std::fs::read"));
    assert!(has(&report, "engine -> store"));
}

#[test]
fn absolute_external_paths_cannot_be_shadowed_by_local_modules() {
    let tree = module_tree();
    tree.write(
        "src/task.rs",
        r#"
mod std { pub struct Local; }
use ::std::fs as disk;
fn wrong() {
    let _ = ::std::fs::read("x");
    let _ = disk::read("x");
    inspect!(nested!(::std::fs::read("x")));
}
"#,
    );
    let report = tree.report(graph());
    assert!(
        report
            .violations
            .iter()
            .filter(|v| v.contains("pure module task references std::fs::read"))
            .count()
            >= 3,
        "{:?}",
        report.violations
    );
    assert!(!report
        .edges
        .iter()
        .any(|edge| edge.to.starts_with("task::std::fs")));
}
