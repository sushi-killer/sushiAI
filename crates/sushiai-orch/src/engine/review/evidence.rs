use crate::policy::paths::glob_match;
use crate::task::Task;
use std::path::Path;
use std::path::PathBuf;

const MAX_REVIEW_SCREENSHOTS: usize = 8;

/// Which images an attempt's evidence may hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvidenceScope {
    /// Every image the attempt wrote: some visual criterion names no path.
    All,
    /// The task has no visual criterion: no image is evidence.
    Nothing,
    /// Only images matching these worktree-relative globs (`artifacts/<file>`,
    /// or `artifacts/<dir>/*` for a directory a check names).
    Named(Vec<String>),
}

/// The scope a task's visual criteria set.
pub fn evidence_scope(task: &Task) -> EvidenceScope {
    let visual = task.visual_criteria_texts();
    if visual.is_empty() {
        return EvidenceScope::Nothing;
    }
    let mut named = Vec::new();
    for criterion in visual {
        let found = named_artifact_paths(criterion);
        if found.is_empty() {
            return EvidenceScope::All;
        }
        for entry in found {
            if !named.contains(&entry) {
                named.push(entry);
            }
        }
    }
    EvidenceScope::Named(named)
}

/// The `artifacts/...` paths a criterion names: an image file as is, anything
/// else as a directory (`artifacts/<dir>/*`). A bare `artifacts/` names none.
fn named_artifact_paths(criterion: &str) -> Vec<String> {
    let mut out = Vec::new();
    for token in criterion.split_whitespace() {
        let token = token.trim_matches(|c: char| {
            !c.is_alphanumeric() && !matches!(c, '/' | '.' | '_' | '-' | '*' | '?')
        });
        let token = token.trim_start_matches("./");
        let Some(rest) = token.strip_prefix("artifacts/") else {
            continue;
        };
        let rest = rest.trim_end_matches('.');
        if rest.is_empty() || rest.trim_matches('/').is_empty() {
            continue;
        }
        let entry = if crate::task::image_mime(Path::new(rest)).is_some() {
            token.trim_end_matches('.').to_string()
        } else {
            format!("artifacts/{}/*", rest.trim_end_matches('/'))
        };
        if !out.contains(&entry) {
            out.push(entry);
        }
    }
    out
}

/// The name `save_evidence` gives a copy of an `artifacts/<rel>` entry.
fn flat_name(entry: &str) -> String {
    let rel = entry.strip_prefix("artifacts/").unwrap_or(entry);
    rel.split('/')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("_")
}

impl EvidenceScope {
    /// Whether the image at `rel` (relative to `artifacts/`) is evidence.
    pub fn allows(&self, rel: &str) -> bool {
        match self {
            EvidenceScope::All => true,
            EvidenceScope::Nothing => false,
            EvidenceScope::Named(entries) => {
                let full = format!("artifacts/{rel}");
                entries.iter().any(|e| glob_match(e, &full))
            }
        }
    }

    /// Whether a saved copy (by its flattened file name) is evidence.
    pub fn allows_saved(&self, saved: &str) -> bool {
        match self {
            EvidenceScope::All => true,
            EvidenceScope::Nothing => false,
            EvidenceScope::Named(entries) => {
                let name = Path::new(saved)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                entries.iter().any(|e| glob_match(&flat_name(e), &name))
            }
        }
    }

    /// The named entries no saved copy in `evidence` matches.
    pub fn missing(&self, evidence: &[String]) -> Vec<String> {
        let EvidenceScope::Named(entries) = self else {
            return Vec::new();
        };
        let names: Vec<String> = evidence
            .iter()
            .filter_map(|p| Path::new(p).file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .collect();
        entries
            .iter()
            .filter(|e| !names.iter().any(|n| glob_match(&flat_name(e), n)))
            .map(|e| {
                e.strip_suffix('*')
                    .filter(|d| d.ends_with('/'))
                    .unwrap_or(e)
                    .to_string()
            })
            .collect()
    }
}

/// Images under the worktree's `artifacts/` (gitignored, so never in the
/// diff) written since `since_ms` and inside `scope`, newest first, at most
/// `MAX_REVIEW_SCREENSHOTS`.
pub(in crate::engine) fn attempt_screenshots(
    worktree: &Path,
    since_ms: i64,
    scope: &EvidenceScope,
) -> Vec<PathBuf> {
    let mut found = attempt_images(worktree, since_ms, scope);
    found.truncate(MAX_REVIEW_SCREENSHOTS);
    found
}

/// Copies the images the attempt wrote under `artifacts/` that `scope` allows
/// into `evidence_dir` (flattened names), so they survive the worktree.
/// Returns the copies.
pub(in crate::engine) fn save_evidence(
    worktree: &Path,
    since_ms: i64,
    evidence_dir: &Path,
    scope: &EvidenceScope,
) -> Vec<String> {
    let images = attempt_images(worktree, since_ms, scope);
    if images.is_empty() || std::fs::create_dir_all(evidence_dir).is_err() {
        return Vec::new();
    }
    let root = worktree.join("artifacts");
    let mut saved = Vec::new();
    for image in images {
        let rel = image.strip_prefix(&root).unwrap_or(&image);
        let name = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("_");
        let target = evidence_dir.join(name);
        if std::fs::copy(&image, &target).is_ok() {
            saved.push(target.display().to_string());
        }
    }
    saved.sort();
    saved
}

fn attempt_images(worktree: &Path, since_ms: i64, scope: &EvidenceScope) -> Vec<PathBuf> {
    let root = worktree.join("artifacts");
    let mut found: Vec<(i64, PathBuf)> = Vec::new();
    let mut dirs = vec![worktree.join("artifacts")];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                dirs.push(path);
                continue;
            }
            let rel = path.strip_prefix(&root).unwrap_or(&path);
            let rel = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            let is_image = crate::task::image_mime(&path).is_some() && scope.allows(&rel);
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_millis() as i64);
            if is_image && modified >= since_ms {
                found.push((modified, path));
            }
        }
    }
    found.sort_by_key(|item| std::cmp::Reverse(item.0));
    found.into_iter().map(|(_, p)| p).collect()
}
