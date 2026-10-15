use crate::policy::paths::is_test_path;
use crate::task::attempt::VerifyOutcome;
use crate::task::review::ReviewResult;
use crate::task::review::Verdict;
use crate::verify::comparison::{compare, Compared};
use std::path::Path;

/// Files (not tests) the task itself changed between an earlier attempt's
/// evidence tree and the worktree now. Files that differ only because the base
/// moved (carried-in commits, a conflict resolved onto a new base) are not the
/// task's doing; without the earlier base every difference counts.
pub(crate) fn own_changes_since(
    wt: &Path,
    old_tree: &str,
    old_base: Option<&str>,
    now_base: &str,
) -> Result<Vec<String>, crate::git::GitError> {
    let now = crate::git::changes::worktree_tree(wt)?;
    let paths = match old_base {
        Some(old_base) => {
            crate::git::changes::own_changes_between(wt, old_tree, old_base, &now, now_base)?
        }
        None => crate::git::changes::diff_trees_name_only(wt, old_tree, &now)?,
    };
    Ok(paths.into_iter().filter(|p| !is_test_path(p)).collect())
}

/// Whether the task's run fails the same named tests as the base's run.
pub(crate) fn same_failure(run: &VerifyOutcome, base: &VerifyOutcome) -> bool {
    if matches!(compare(run, base), Compared::Covered(_)) {
        return true;
    }
    let (now, before): (
        std::collections::BTreeSet<&String>,
        std::collections::BTreeSet<&String>,
    ) = (run.failing.iter().collect(), base.failing.iter().collect());
    !now.is_empty() && now == before
}

/// After the first attempt only a P0/P1 finding about the task's own work
/// blocks: an unmet criterion, a P0, a finding that came back, or a P1 that
/// is not in `old_on_base`. Everything else (P2/P3, a P1 whose scenario also
/// fails on the base) is moved out of `review.findings` and returned as
/// follow-ups; a review left with no blocking finding becomes a PASS.
pub(crate) fn scope_review(review: &mut ReviewResult, old_on_base: &[usize]) -> Vec<String> {
    let own = review.severities.len().min(review.findings.len());
    let mut unmet = review.criteria.iter().filter(|r| r.met != Some(true));
    let mut keep_findings = Vec::new();
    let mut keep_severities = Vec::new();
    let mut keep_details = Vec::new();
    let mut follow_ups = Vec::new();
    for (i, finding) in review.findings.iter().enumerate() {
        let severity = review
            .severities
            .get(i)
            .copied()
            .flatten()
            .or_else(|| crate::task::review::severity_of(finding));
        let detail = review.details.get(i);
        let blocks = if i >= own {
            unmet.next().is_none_or(|r| r.met == Some(false))
        } else if finding.starts_with("Another attempt was asked for:") {
            true
        } else {
            match severity {
                Some(0) => true,
                Some(1) | None => review.repeated.contains(finding) || !old_on_base.contains(&i),
                Some(_) => false,
            }
        };
        if blocks {
            keep_findings.push(finding.clone());
            keep_severities.push(severity);
            keep_details.extend(detail.cloned());
        } else {
            follow_ups.push(finding.clone());
        }
    }
    review.findings = keep_findings;
    review.severities = keep_severities;
    if !review.details.is_empty() {
        review.details = keep_details;
    }
    if review.verdict == Verdict::Fail && review.findings.is_empty() {
        review.verdict = Verdict::Pass;
    }
    follow_ups
}
