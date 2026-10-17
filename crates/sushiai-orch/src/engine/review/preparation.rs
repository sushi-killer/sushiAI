use super::evidence::{attempt_screenshots, evidence_scope};
use super::narrow::{carry_settled, narrow_basis, narrow_inputs, NarrowInputs};
use super::native::head_chars;
use super::run::ReviewSession;
use super::types::{ReviewFailure, ReviewSaw};
use crate::engine::lifecycle::report::ImplementerReport;
use crate::engine::protected::restore::grown_paths;
use crate::harness::RunOutcome;
use crate::task::{Decline, ReviewResult, ReviewScope, Stage, Task, VerifyOutcome};
use crate::verify::review::brief::{
    build_narrow_review_brief, build_review_brief, NarrowReview, ReviewEvidence,
};
use crate::verify::review::routing::Lenses;
use std::path::PathBuf;

/// The exact tree, diff and recorded images supplied to this review pass.
pub(super) struct PreparedReview {
    tree: Option<String>,
    pub(super) narrow: Option<NarrowInputs>,
    diff: String,
    pub(super) images: Vec<PathBuf>,
    shown: Vec<(String, Option<u32>)>,
    evidence_from: Option<u32>,
    declines: Vec<Decline>,
    grown: String,
}

impl PreparedReview {
    pub(super) async fn gather(session: &ReviewSession, task: &Task) -> Self {
        let wt = session.worktree.clone();
        let tree =
            tokio::task::spawn_blocking(move || crate::git::changes::worktree_tree(&wt).ok())
                .await
                .unwrap_or(None);
        let narrow = match (
            session.mode.force_full,
            &tree,
            narrow_basis(task, session.attempt_n),
        ) {
            (false, Some(now_tree), Some(basis)) => {
                narrow_inputs(task, basis, now_tree, &session.worktree, &session.base_sha).await
            }
            _ => None,
        };
        let diff = if narrow.is_some() {
            String::new()
        } else {
            let (wt, base) = (session.worktree.clone(), session.base_sha.clone());
            tokio::task::spawn_blocking(move || {
                crate::git::changes::diff_full(&wt, &base, 60_000).unwrap_or_default()
            })
            .await
            .unwrap_or_default()
        };
        let grown = {
            let (wt, base, task) = (
                session.worktree.clone(),
                session.base_sha.clone(),
                task.clone(),
            );
            tokio::task::spawn_blocking(move || {
                let paths = grown_paths(&task, &wt);
                crate::git::changes::diff_paths(&wt, &base, &paths, 20_000).unwrap_or_default()
            })
            .await
            .unwrap_or_default()
        };
        let this_attempt = task
            .attempts
            .iter()
            .find(|a| a.n == session.attempt_n && a.stage == Stage::Implement);
        let evidence_from = this_attempt.and_then(|a| a.evidence_from);
        let images = if !task.variant().review_evidence {
            Vec::new()
        } else if evidence_from.is_some() {
            this_attempt
                .map(|a| a.evidence.iter().map(PathBuf::from).collect())
                .unwrap_or_default()
        } else {
            attempt_screenshots(
                &session.worktree,
                this_attempt.map_or(0, |a| a.started_at),
                &evidence_scope(task),
            )
        };
        let saved = this_attempt.map(|a| a.evidence.clone()).unwrap_or_default();
        let source_attempt = evidence_from.unwrap_or(session.attempt_n);
        let shown = if saved.is_empty() {
            images
                .iter()
                .map(|image| {
                    let rel = image.strip_prefix(&session.worktree).unwrap_or(image);
                    (rel.display().to_string(), Some(session.attempt_n))
                })
                .collect()
        } else {
            saved
                .into_iter()
                .map(|s| (s, Some(source_attempt)))
                .collect()
        };
        Self {
            tree,
            narrow,
            diff,
            images,
            shown,
            evidence_from,
            declines: this_attempt.map(|a| a.declines.clone()).unwrap_or_default(),
            grown,
        }
    }

    pub(super) fn brief(
        &self,
        task: &Task,
        report: &ImplementerReport,
        verified: &[VerifyOutcome],
        lenses: &Lenses,
    ) -> String {
        let evidence = ReviewEvidence {
            implementer_note: &report.note,
            agent_decisions: &report.decisions,
            verify_results: verified,
            screenshots: &self.shown,
            evidence_from: self.evidence_from,
            declines: &self.declines,
            grown_protected: &self.grown,
        };
        match &self.narrow {
            Some(narrow) => build_narrow_review_brief(
                task,
                &evidence,
                &NarrowReview {
                    open_criteria: &narrow.open_criteria,
                    settled_criteria: &narrow.settled,
                    delta: &narrow.delta,
                },
            ),
            None => build_review_brief(task, &evidence, &self.diff, lenses),
        }
    }

    pub(super) fn resolve(
        self,
        task: &Task,
        outcome: RunOutcome,
    ) -> Result<(ReviewResult, bool, ReviewSaw), ReviewFailure> {
        let text = outcome.final_text.unwrap_or_default();
        if let Some((mut result, recorded_as_pass)) =
            crate::verify::review::reply::parse_review_with_rule(&text, &task.criteria)
        {
            let scope = match &self.narrow {
                Some(narrow) => {
                    carry_settled(narrow, &mut result);
                    ReviewScope::Narrow
                }
                None => ReviewScope::Full,
            };
            let rulings = crate::verify::review::reply::parse_decline_rulings(&text);
            return Ok((
                result,
                recorded_as_pass,
                ReviewSaw {
                    scope,
                    tree: self.tree,
                    rulings,
                },
            ));
        }
        Err(match outcome.error {
            Some(error) => ReviewFailure::Harness(format!("review did not run ({error})")),
            None => ReviewFailure::NoVerdict(format!(
                "review reply had no sushi-review verdict: {}",
                head_chars(text.trim(), 300)
            )),
        })
    }
}
