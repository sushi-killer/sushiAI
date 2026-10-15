//! The overseer's decision memory at work (the store is `precedents.rs`):
//! the hard limits on what it may decide, the owner-precedent auto-answer
//! (`questions.rs` calls it when a question parks), the precedents and
//! proposed rules shown to an overseer turn and the digest, and the
//! `precedents.*` RPCs. The RPCs are not MCP tools: the owner uses them in the
//! Orchestrator panel.

#[cfg(test)]
use crate::clock::now_ms;
#[cfg(test)]
use crate::engine::answer_policy::{record_policy_answer, AutomaticAnswer};
#[cfg(test)]
use crate::engine::lines::LIKE_PREFIX;
#[cfg(test)]
use crate::engine::questions::answers::record_answer_for;
#[cfg(test)]
use crate::engine::questions::parking::QuestionSlot;
#[cfg(test)]
use crate::engine::run_control::TaskControl;
#[cfg(test)]
use crate::engine::run_control::TaskSignals;
#[cfg(test)]
use crate::harness::cancel::CancelToken;
#[cfg(test)]
use crate::task::AnsweredBy;
#[cfg(test)]
use crate::task::AnsweredQuestion;
#[cfg(test)]
use crate::task::AskedBy;
#[cfg(test)]
use crate::task::TaskStatus;
#[cfg(test)]
use crate::task::VerifyOutcome;
#[cfg(test)]
use crate::task::PRE_EXISTING_RETRY;
#[cfg(test)]
use std::path::Path;
#[cfg(test)]
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex as StdMutex;
#[cfg(test)]
use std::time::Duration;

#[cfg(test)]
use crate::engine::decision::EXHAUSTED_ANSWER_PREFIX;

use crate::engine::digest;
use crate::engine::state::App;
#[cfg(test)]
use crate::engine::test_support::attempt_with_failure;
#[cfg(test)]
use crate::engine::test_support::init_git_repo;
#[cfg(test)]
use crate::engine::test_support::task_with_status;
#[cfg(test)]
use crate::engine::test_support::test_app;
#[cfg(test)]
use crate::engine::verification::base::answers::PRE_EXISTING_DROP;
use crate::policy::questions::limits::hard_limit;
#[cfg(test)]
use crate::policy::questions::limits::HardLimit;
use crate::policy::questions::owner_only;
use crate::prompts::engine_text;
use crate::store::precedents;
use crate::task::is_option;
use crate::task::Question;
#[cfg(test)]
use crate::task::QuestionKind;
#[cfg(test)]
use crate::task::Risk;
use crate::task::Task;
use serde::Deserialize;
use serde_json::json;

/// The `like` of a `task.answer` call: only the overseer sends one, never for
/// a question under a hard limit; one line, at most 120 characters.
pub(super) fn checked_like(
    like: Option<&str>,
    source: Option<&str>,
    task: &Task,
    answer: &str,
) -> Result<Option<String>, String> {
    let Some(like) = like.map(str::trim).filter(|l| !l.is_empty()) else {
        return Ok(None);
    };
    if source != Some("overseer") {
        return Err("`like` is for the overseer's answers only".to_string());
    }
    if let Some(limit) = task.question.as_ref().and_then(|q| hard_limit(q, answer)) {
        return Err(format!(
            "this question is under a hard limit ({limit:?}): the owner decides, not a precedent"
        ));
    }
    Ok(Some(
        like.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(120)
            .collect(),
    ))
}

fn day(ms: i64) -> String {
    digest::local_day_and_time(ms / 1000)
        .map(|(day, _, _)| day)
        .unwrap_or_default()
}

/// Whether `like` claims an owner precedent.
fn claims_owner_precedent(like: &str) -> bool {
    like.trim_start()
        .get(..15)
        .is_some_and(|head| head.eq_ignore_ascii_case("owner precedent"))
}

/// `prompt` followed by the owner precedents `notes` and the standing
/// instruction to answer with `like`.
pub(super) fn precedents_prompt(prompt: &str, notes: &str) -> String {
    format!(
        "{prompt}\n\n{}\n{notes}",
        engine_text("decision_memory_precedents")
    )
}

/// The digest prompt `base` followed by the open proposed `rules`.
pub(super) fn rules_prompt(base: &str, rules: &str) -> String {
    format!(
        "{base}\n\n{}\n{rules}",
        engine_text("decision_memory_rules")
    )
}

impl App {
    /// Refuses a `like` that names an owner precedent no owner record backs: a
    /// decision line must not claim a precedent that does not exist.
    pub(super) fn verify_like(
        &self,
        like: Option<&str>,
        task: &Task,
        answer: &str,
    ) -> Result<(), String> {
        if !like.is_some_and(claims_owner_precedent) {
            return Ok(());
        }
        let backed = task.question.as_ref().is_some_and(|q| {
            let key = precedents::question_key(q, task);
            precedents::owner_matches(&self.data_dir, &task.repo, &key)
                .iter()
                .any(|p| p.answer == answer.trim() || is_option(answer, &p.answer))
        });
        if backed {
            Ok(())
        } else {
            Err(format!(
                "`like` names an owner precedent, but the owner has no matching answer to \"{}\"",
                answer.trim()
            ))
        }
    }

    /// The answer an owner precedent gives to `question` (asked on `task`) and
    /// the `like` text for its decision line. None unless the owner's newest
    /// precedents for the question agree on one answer that is still an option,
    /// the answer is not `stop` and no hard limit applies.
    pub(super) fn precedent_answer(
        &self,
        task: &Task,
        question: &Question,
    ) -> Option<(String, String)> {
        if owner_only(question.kind) {
            return None;
        }
        let key = precedents::question_key(question, task);
        let (answer, last_at) = precedents::owner_answer(&self.data_dir, &task.repo, &key)?;
        let option = question
            .options
            .iter()
            .find(|o| o.as_str() == answer || is_option(o, &answer))?;
        if option.trim().eq_ignore_ascii_case("stop") || hard_limit(question, option).is_some() {
            return None;
        }
        Some((
            option.clone(),
            format!("owner precedent: {option}, {}", day(last_at)),
        ))
    }

    /// `prompt` followed by the owner precedents that match the open questions
    /// of `tasks` (at most [`precedents::MAX_PROMPT`] each), and the standing
    /// instruction to answer with `like`.
    pub(super) fn with_precedents(&self, prompt: String, tasks: &[&Task]) -> String {
        let mut notes = String::new();
        for task in tasks {
            let Some(question) = task.question.as_ref().filter(|q| !owner_only(q.kind)) else {
                continue;
            };
            let key = precedents::question_key(question, task);
            let found = precedents::owner_matches(&self.data_dir, &task.repo, &key);
            let found: Vec<_> = found
                .into_iter()
                .filter(|p| hard_limit(question, &p.answer).is_none())
                .collect();
            if found.is_empty() {
                continue;
            }
            notes.push_str(&format!("\nTask \"{}\":\n", task.title));
            for p in found {
                let why = if p.why.is_empty() {
                    String::new()
                } else {
                    format!(": {}", p.why)
                };
                notes.push_str(&format!(
                    "- the owner answered \"{}\" on {} (x{}){why}\n",
                    p.answer,
                    day(p.last_at),
                    p.count
                ));
            }
        }
        if notes.is_empty() {
            return prompt;
        }
        precedents_prompt(&prompt, &notes)
    }

    /// The digest prompt with the repo's open proposed rules.
    pub(super) fn digest_prompt(&self, repo: &str) -> String {
        let base = crate::prompts::get("digest");
        let (_, open) = precedents::list(&self.data_dir, repo);
        if open.is_empty() {
            return base;
        }
        let rules: String = open.iter().map(|p| format!("- {}\n", p.rule)).collect();
        rules_prompt(&base, &rules)
    }

    /// `precedents.list {repo}`: the repo's precedents (newest first) and open
    /// proposals.
    pub(super) async fn handle_precedents_list(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let (repo, _) = self.precedent_params(params).await?;
        let (list, proposals) = precedents::list(&self.data_dir, &repo);
        Ok(json!({"repo": repo, "precedents": list, "proposals": proposals}))
    }

    /// `precedents.remove {repo, id}`: an unknown id is an error.
    pub(super) async fn handle_precedents_remove(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let (repo, id) = self.precedent_params(params).await?;
        precedents::remove(&self.data_dir, &repo, &id)?;
        Ok(json!({"removed": id}))
    }

    /// `precedents.rule.accept {repo, id}`: the proposed rule becomes an
    /// owner repo note and the proposal closes.
    pub(super) async fn handle_precedents_rule_accept(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let (repo, id) = self.precedent_params(params).await?;
        let (_, open) = precedents::list(&self.data_dir, &repo);
        let rule = open
            .into_iter()
            .find(|p| p.id == id)
            .ok_or_else(|| format!("unknown proposal: {id}"))?
            .rule;
        let note = self
            .add_repo_note(&repo, &rule, "owner".to_string())
            .await?;
        let proposal = precedents::close_proposal(&self.data_dir, &repo, &id, true)?;
        Ok(json!({"note": note, "proposal": proposal}))
    }

    /// `precedents.rule.dismiss {repo, id}`: the proposal closes and its key is
    /// never proposed again.
    pub(super) async fn handle_precedents_rule_dismiss(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let (repo, id) = self.precedent_params(params).await?;
        let proposal = precedents::close_proposal(&self.data_dir, &repo, &id, false)?;
        Ok(json!({"proposal": proposal}))
    }

    /// The resolved repo root and the `id` (empty for a list) of a call.
    async fn precedent_params(
        &self,
        params: serde_json::Value,
    ) -> Result<(String, String), String> {
        #[derive(Deserialize)]
        struct P {
            repo: String,
            #[serde(default)]
            id: String,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        Ok((self.resolve_repo(&p.repo).await?, p.id))
    }
}

#[cfg(test)]
#[path = "decision_memory/tests.rs"]
mod tests;
