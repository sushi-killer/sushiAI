use serde::Serialize;
use std::collections::BTreeSet;

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub source_root: String,
    pub source_digest: String,
    pub files: Vec<FileMetric>,
    pub functions: Vec<FunctionMetric>,
    pub edges: BTreeSet<Edge>,
    pub source_macros: Vec<MacroMetric>,
    pub review: Vec<String>,
    pub violations: Vec<String>,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileMetric {
    pub path: String,
    pub runtime_lines: usize,
    pub test_lines: usize,
    pub data_lines: usize,
    pub total_lines: usize,
    pub bytes: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionMetric {
    pub path: String,
    pub symbol: String,
    pub line: usize,
    pub lines: usize,
    pub arguments: usize,
    pub test: bool,
}

#[derive(Clone, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub path: String,
    pub line: usize,
}

#[derive(Serialize)]
pub struct MacroMetric {
    pub kind: &'static str,
    pub path: String,
    pub symbol: String,
    pub line: usize,
    pub lines: usize,
}
