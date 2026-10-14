//! The overseer's decision memory at work (the store is `precedents.rs`):
//! the hard limits on what it may decide, the owner-precedent auto-answer
//! (`questions.rs` calls it when a question parks), the precedents and
//! proposed rules shown to an overseer turn and the digest, and the
//! `precedents.*` RPCs. The RPCs are not MCP tools: the owner uses them in the
//! Orchestrator panel.

use super::*;
use crate::precedents;

/// What the overseer may never decide, whatever the precedent: the question
/// always goes to the owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HardLimit {
    /// Permission, protected path and budget questions. The standing grant is
    /// the owner's own `Always for this repo` answer.
    OwnerOnlyKind,
    /// Dropping a check or criterion.
    SharedCheckDrop,
    /// Loosening the sandbox, or anything about secrets and security.
    SandboxOrSecurity,
    /// Pushing, a pull request, merging or releasing.
    PushPrMergeRelease,
    /// A paid service or spend.
    Money,
    /// Deleting or overwriting.
    Destructive,
    /// A production deploy.
    ProductionDeploy,
}

/// Phrases (lower-case words) that put a question under a limit. Matching is
/// per whole word, so `pr` never matches `improve`.
const KEYWORDS: [(HardLimit, &[&str]); 5] = [
    (
        HardLimit::SandboxOrSecurity,
        &[
            "security",
            "secret",
            "secrets",
            "credential",
            "credentials",
            "password",
            "passwords",
            "keychain",
            "api key",
            "ssh key",
            "private key",
            "token",
            "tokens",
            "vulnerability",
            "unsandboxed",
            "disable the sandbox",
            "disable sandbox",
            "without the sandbox",
            "without sandbox",
            "sandbox off",
            "bypass",
            "allowlist",
            "permission",
            "permissions",
            "sudo",
            "chmod",
            "network access",
        ],
    ),
    (
        HardLimit::PushPrMergeRelease,
        &[
            "push",
            "pushing",
            "merge",
            "merging",
            "pull request",
            "pr",
            "release",
            "releasing",
            "publish",
            "publishing",
        ],
    ),
    (
        HardLimit::Money,
        &[
            "paid",
            "pay",
            "payment",
            "billing",
            "subscription",
            "invoice",
            "purchase",
            "buy",
            "usd",
            "budget",
            "credits",
            "spend",
            "price",
            "pricing",
        ],
    ),
    (
        HardLimit::Destructive,
        &[
            "delete",
            "deleting",
            "destroy",
            "wipe",
            "purge",
            "rm",
            "reset hard",
            "force",
            "overwrite",
            "drop table",
            "truncate",
            "irreversible",
        ],
    ),
    (
        HardLimit::ProductionDeploy,
        &[
            "deploy",
            "deploying",
            "deployment",
            "production",
            "prod",
            "go live",
            "rollout",
        ],
    ),
];

/// Lower-case words of `text` separated by single spaces, with a space at
/// both ends so a phrase matches only whole words.
fn words(text: &str) -> String {
    let spaced = text.to_lowercase().replace('$', " usd ");
    let words: Vec<&str> = spaced
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    format!(" {} ", words.join(" "))
}

/// The limit that puts `answer` to `question` with the owner, if any. A
/// base-check question is read by its header, not the check output it quotes.
pub(crate) fn hard_limit(question: &Question, answer: &str) -> Option<HardLimit> {
    if owner_only(question.kind) {
        return Some(HardLimit::OwnerOnlyKind);
    }
    let drops = match question.kind {
        QuestionKind::PreexistingFailure => is_option(answer, PRE_EXISTING_DROP),
        QuestionKind::Impossible => answer.trim().eq_ignore_ascii_case("drop criterion"),
        _ => false,
    };
    if drops {
        return Some(HardLimit::SharedCheckDrop);
    }
    let prose = if question.kind == QuestionKind::PreexistingFailure {
        overseer::question_prose(question)
    } else {
        question.text.clone()
    };
    let text = words(&format!("{prose} {} {answer}", question.options.join(" ")));
    KEYWORDS
        .iter()
        .find(|(_, phrases)| {
            phrases
                .iter()
                .any(|phrase| text.contains(&format!(" {phrase} ")))
        })
        .map(|(limit, _)| *limit)
}

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
        format!(
            "{prompt}\n\nOwner precedents for matching questions. When one covers the question and no hard limit applies, answer with task_answer {{like}} naming it. Never write a repo note yourself.\n{notes}"
        )
    }

    /// The digest prompt with the repo's open proposed rules.
    pub(super) fn digest_prompt(&self, repo: &str) -> String {
        let base = crate::prompts::get("digest");
        let (_, open) = precedents::list(&self.data_dir, repo);
        if open.is_empty() {
            return base;
        }
        let rules: String = open.iter().map(|p| format!("- {}\n", p.rule)).collect();
        format!(
            "{base}\n\nProposed rules learned from the owner's repeated answers. They are proposals only: the owner accepts or dismisses each in the Orchestrator panel. Mention them briefly; never add them as repo notes yourself.\n{rules}"
        )
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
mod tests {
    use super::*;

    const RETRY: &str = PRE_EXISTING_RETRY;

    fn base_q() -> Question {
        let mut q = Question::new(
            "cargo test fails on base deadbee: 3 failed",
            vec![RETRY.into(), PRE_EXISTING_DROP.into(), "stop".into()],
            QuestionKind::PreexistingFailure,
            AskedBy::Implement,
        );
        q.commands = vec!["cargo test".into()];
        q
    }

    fn free_q(text: &str, options: &[&str]) -> Question {
        Question::new(
            text,
            options.iter().map(|o| o.to_string()).collect(),
            QuestionKind::AgentQuestion,
            AskedBy::Implement,
        )
    }

    fn exhausted_q() -> Question {
        Question::new(
            "Attempts keep failing with a review failure: continue, change approach, or stop?",
            vec!["continue".into(), "stop".into()],
            QuestionKind::AttemptsFailing,
            AskedBy::Implement,
        )
    }

    /// A task of `repo` that was asked `q` and answered it.
    fn answered_task(repo: &str, q: &Question, answer: &str, by: AnsweredBy) -> Task {
        let mut task = task_with_status(TaskStatus::Queued);
        task.repo = repo.to_string();
        record_answer_for(&mut task, Some(q), answer, by, None, None);
        task
    }

    fn waiting_task(repo: &str, q: Question) -> Task {
        let mut task = task_with_status(TaskStatus::Waiting);
        task.repo = repo.to_string();
        task.question = Some(q);
        task
    }

    /// A git repo under `dir` and its root as orchd resolves it.
    async fn repo_root(app: &App, dir: &Path) -> String {
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_git_repo(&repo);
        app.resolve_repo(repo.to_str().unwrap()).await.unwrap()
    }

    fn remember(app: &App, repo: &str, q: &Question, answer: &str, by: AnsweredBy) {
        app.store
            .save_task(&answered_task(repo, q, answer, by))
            .unwrap();
    }

    /// Parks `task` the way its loop does and returns the answer it got and its
    /// final copy. A question nobody answers within a moment is cancelled.
    async fn park(app: &Arc<App>, task: Task) -> (Option<String>, Task) {
        app.store.save_task(&task).unwrap();
        let id = task.id.clone();
        let pending = Arc::new(StdMutex::new(None));
        let cancel = CancelToken::new();
        app.controls.lock().unwrap().insert(
            id.clone(),
            TaskControl {
                cancel: cancel.clone(),
                pending_answer: pending.clone(),
                pending_amend: Arc::new(StdMutex::new(None)),
                handle: tokio::spawn(async {}),
            },
        );
        let mut parked = tokio::spawn({
            let (app, pending, cancel) = (app.clone(), pending.clone(), cancel.clone());
            async move {
                let mut task = task;
                let answer =
                    park_for_answer(&app, &id, &mut task, &pending, &cancel, &mut None).await;
                (answer, task)
            }
        });
        match tokio::time::timeout(Duration::from_millis(1500), &mut parked).await {
            Ok(done) => done.unwrap(),
            Err(_) => {
                cancel.cancel();
                parked.await.unwrap()
            }
        }
    }

    fn log_kinds(app: &App) -> Vec<String> {
        overseer::read(&app.data_dir, None, None)
            .into_iter()
            .map(|e| e.kind)
            .collect()
    }

    fn overseer_app() -> (Arc<App>, tempfile::TempDir) {
        let _ = chat::fake_overseer_harness();
        let (app, dir) = test_app();
        app.settings.write().unwrap().auto_answer = true;
        (app, dir)
    }

    #[tokio::test]
    async fn decision_memory_records_every_answer_source() {
        let (app, _dir) = test_app();
        let q = free_q("Which way?", &["A", "B"]);
        for (by, name) in [
            (AnsweredBy::Owner, "owner"),
            (AnsweredBy::Lead, "lead"),
            (AnsweredBy::Overseer, "overseer"),
            (AnsweredBy::Orchestrator, "chat"),
        ] {
            remember(&app, "/r1", &q, "A", by);
            let (list, _) = precedents::list(&app.data_dir, "/r1");
            assert!(
                list.iter().any(|p| p.by == name && p.answer == "A"),
                "{name}"
            );
        }
        // A waiting-SLA default and a rule are policy; a judge is its own source.
        let mut policy = task_with_status(TaskStatus::Queued);
        policy.repo = "/r1".into();
        record_answer_for(
            &mut policy,
            Some(&q),
            "B",
            AnsweredBy::Owner,
            Some("nobody answered"),
            None,
        );
        app.store.save_task(&policy).unwrap();
        let mut judge = task_with_status(TaskStatus::Queued);
        judge.repo = "/r1".into();
        record_policy_answer(&mut judge, &q, "B", "judge", "verified the evidence", 1);
        app.store.save_task(&judge).unwrap();
        let (list, _) = precedents::list(&app.data_dir, "/r1");
        let find = |by: &str| list.iter().find(|p| p.by == by && p.answer == "B");
        assert_eq!(
            find("policy").map(|p| p.why.as_str()),
            Some("nobody answered")
        );
        assert!(find("judge").is_some_and(|p| p.why == "verified the evidence"));
        assert!(list.iter().all(|p| p.kind == QuestionKind::AgentQuestion));
        let file = precedents::path(&app.data_dir);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[tokio::test]
    async fn decision_memory_keeps_facts_but_never_a_check_output_tail() {
        let (app, _dir) = test_app();
        let mut q = base_q();
        q.text = "cargo test fails on base deadbee: token sk-live-SECRET99 leaked".into();
        let mut task = task_with_status(TaskStatus::Queued);
        task.repo = "/r1".into();
        let mut attempt = attempt_with_failure(1, "verify:x");
        attempt.changed_files = vec!["src/a.rs".into()];
        attempt.verify = vec![VerifyOutcome {
            command: "cargo test".into(),
            code: Some(1),
            tail: "setup failed: EEXIST sk-live-SECRET99".into(),
            ms: 1,
            load: 0.0,
            cores: 1,
            load_timeout: false,
            decisions: vec![],
            ..Default::default()
        }];
        task.attempts.push(attempt);
        record_answer_for(&mut task, Some(&q), RETRY, AnsweredBy::Owner, None, None);
        app.store.save_task(&task).unwrap();
        let text = std::fs::read_to_string(precedents::path(&app.data_dir)).unwrap();
        assert!(!text.contains("SECRET99"), "{text}");
        let (list, _) = precedents::list(&app.data_dir, "/r1");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].commands, ["cargo test"]);
        assert_eq!(list[0].signals, ["install"]);
        assert_eq!(list[0].files, ["src/a.rs"]);
        assert_eq!(list[0].goal, "Do the thing");
        assert_eq!(list[0].question, None);
    }

    #[test]
    fn decision_memory_signals_read_the_check_results() {
        let outcome = |code: Option<i32>, tail: &str| VerifyOutcome {
            command: "npm test".into(),
            code,
            tail: tail.into(),
            ms: 1,
            load: 0.0,
            cores: 1,
            load_timeout: false,
            decisions: vec![],
            ..Default::default()
        };
        let attempt = |n, outcome| {
            let mut a = attempt_with_failure(n, "s");
            a.verify = vec![outcome];
            a
        };
        let mut task = task_with_status(TaskStatus::Queued);
        task.attempts = vec![
            attempt(1, outcome(Some(1), "not ok 1 - alpha")),
            attempt(2, outcome(Some(1), "not ok 2 - beta")),
        ];
        assert_eq!(precedents::signals_of(&task), ["flaky"]);
        task.attempts = vec![attempt(1, outcome(None, "killed after 600s"))];
        assert_eq!(precedents::signals_of(&task), ["timeout"]);
        task.attempts = vec![attempt(1, outcome(Some(1), "npm ERR! failed to install"))];
        assert_eq!(precedents::signals_of(&task), ["install"]);
        let mut other = attempt(1, outcome(Some(1), "FAIL tests/other.test.js"));
        other.changed_files = vec!["src/mine.js".into()];
        task.attempts = vec![other];
        assert_eq!(precedents::signals_of(&task), ["outside_diff"]);
        let mut mine = attempt(1, outcome(Some(1), "FAIL tests/mine.test.js"));
        mine.changed_files = vec!["tests/mine.test.js".into()];
        task.attempts = vec![mine];
        assert!(precedents::signals_of(&task).is_empty());
    }

    #[tokio::test]
    async fn decision_memory_merges_the_same_key_and_caps_a_repo() {
        let (app, _dir) = test_app();
        let q = base_q();
        for _ in 0..3 {
            remember(&app, "/r1", &q, RETRY, AnsweredBy::Owner);
        }
        remember(&app, "/r1", &q, RETRY, AnsweredBy::Lead);
        remember(&app, "/r1", &q, "stop", AnsweredBy::Owner);
        let (list, _) = precedents::list(&app.data_dir, "/r1");
        assert_eq!(list.len(), 3);
        let owner = list
            .iter()
            .find(|p| p.by == "owner" && p.answer == RETRY)
            .unwrap();
        assert_eq!(owner.count, 3);
        assert!(owner.last_at >= owner.first_at);
        // A resave of one task adds nothing.
        let task = answered_task("/r2", &q, RETRY, AnsweredBy::Owner);
        app.store.save_task(&task).unwrap();
        app.store.save_task(&task).unwrap();
        assert_eq!(precedents::list(&app.data_dir, "/r2").0[0].count, 1);

        let mut task = task_with_status(TaskStatus::Queued);
        task.repo = "/r3".into();
        let base = now_ms();
        for i in 0..(precedents::MAX_PER_REPO + 5) {
            task.question_history.push(AnsweredQuestion {
                question: "q".into(),
                options: vec![],
                kind: QuestionKind::PreexistingFailure,
                asked_by: None,
                asked_at: None,
                answer: RETRY.into(),
                answered_at: base + i as i64,
                answered_by: AnsweredBy::Owner,
                failure_signature: None,
                commands: vec![format!("cmd-{i}")],
            });
        }
        app.store.save_task(&task).unwrap();
        let (list, _) = precedents::list(&app.data_dir, "/r3");
        assert_eq!(list.len(), precedents::MAX_PER_REPO);
        assert!(list.iter().all(|p| p.commands[0] != "cmd-4"));
        assert!(list.iter().any(|p| p.commands[0] == "cmd-204"));
    }

    #[tokio::test]
    async fn decision_memory_owner_precedent_answers_without_asking_the_owner() {
        let (app, dir) = overseer_app();
        let repo = repo_root(&app, dir.path()).await;
        remember(&app, &repo, &base_q(), RETRY, AnsweredBy::Owner);
        remember(&app, &repo, &base_q(), RETRY, AnsweredBy::Owner);
        let (answer, task) = park(&app, waiting_task(&repo, base_q())).await;
        assert_eq!(answer.as_deref(), Some(RETRY));
        let line = task
            .decisions
            .iter()
            .find(|d| d.starts_with(LIKE_PREFIX))
            .cloned()
            .unwrap_or_default();
        let prefix = "Overseer: answered like owner precedent: retry after the base is fixed, ";
        assert!(line.starts_with(prefix), "{line}");
        assert!(
            line.ends_with(&format!(" -- {} -> {RETRY}", base_q().text)),
            "{line}"
        );
        let answered = task.question_history.last().unwrap();
        assert_eq!(answered.answered_by, AnsweredBy::Overseer);
        assert!(task.question.is_none());
        let kinds = log_kinds(&app);
        assert!(kinds.contains(&"answered".to_string()), "{kinds:?}");
        assert!(!kinds.contains(&"asked_owner".to_string()), "{kinds:?}");
        chat::wait_idle(&app, &repo).await;
        assert_eq!(chat::fake_overseer_calls(Path::new(&repo)), 0);
    }

    #[tokio::test]
    async fn decision_memory_other_sources_and_disagreeing_owners_answer_nothing() {
        for by in [
            AnsweredBy::Lead,
            AnsweredBy::Policy,
            AnsweredBy::Overseer,
            AnsweredBy::Orchestrator,
            AnsweredBy::Judge,
        ] {
            let (app, dir) = overseer_app();
            let repo = repo_root(&app, dir.path()).await;
            remember(&app, &repo, &base_q(), RETRY, by);
            let (answer, _) = park(&app, waiting_task(&repo, base_q())).await;
            assert_eq!(answer, None, "{by:?}");
            chat::wait_idle(&app, &repo).await;
            assert_eq!(chat::fake_overseer_calls(Path::new(&repo)), 1, "{by:?}");
        }
        let (app, dir) = overseer_app();
        let repo = repo_root(&app, dir.path()).await;
        remember(&app, &repo, &base_q(), RETRY, AnsweredBy::Owner);
        remember(&app, &repo, &base_q(), RETRY, AnsweredBy::Owner);
        remember(&app, &repo, &base_q(), "stop", AnsweredBy::Owner);
        let (answer, _) = park(&app, waiting_task(&repo, base_q())).await;
        assert_eq!(answer, None);
        chat::wait_idle(&app, &repo).await;
        assert_eq!(chat::fake_overseer_calls(Path::new(&repo)), 1);
    }

    #[tokio::test]
    async fn decision_memory_a_different_failure_or_a_stop_is_not_the_same_question() {
        let (app, dir) = overseer_app();
        let repo = repo_root(&app, dir.path()).await;
        remember(&app, &repo, &base_q(), RETRY, AnsweredBy::Owner);
        remember(&app, &repo, &base_q(), RETRY, AnsweredBy::Owner);
        let mut other = base_q();
        other.commands = vec!["npm test".into()];
        assert!(app
            .precedent_answer(&waiting_task(&repo, other.clone()), &other)
            .is_none());
        let q = base_q();
        assert!(app
            .precedent_answer(&waiting_task(&repo, q.clone()), &q)
            .is_some());
        // The owner's stop is never replayed.
        let stop = free_q("Keep going?", &["go on", "stop"]);
        remember(&app, &repo, &stop, "stop", AnsweredBy::Owner);
        assert!(app
            .precedent_answer(&waiting_task(&repo, stop.clone()), &stop)
            .is_none());
    }

    #[tokio::test]
    async fn decision_memory_every_hard_limit_reaches_the_owner() {
        let (app, dir) = overseer_app();
        let repo = repo_root(&app, dir.path()).await;
        let permission = Question::new(
            "Allow Bash for this task?",
            vec!["Allow once".into(), "Deny".into()],
            QuestionKind::Permission,
            AskedBy::Implement,
        );
        let cases: Vec<(HardLimit, Question, &str)> = vec![
            (HardLimit::OwnerOnlyKind, permission, "Allow once"),
            (HardLimit::SharedCheckDrop, base_q(), PRE_EXISTING_DROP),
            (
                HardLimit::SandboxOrSecurity,
                free_q("Disable the sandbox for this check?", &["disable", "keep"]),
                "disable",
            ),
            (
                HardLimit::PushPrMergeRelease,
                free_q("Push the branch to origin?", &["push", "hold"]),
                "push",
            ),
            (
                HardLimit::PushPrMergeRelease,
                free_q("Merge it now?", &["merge", "hold"]),
                "merge",
            ),
            (
                HardLimit::Money,
                free_q("Use the paid plan for the load test?", &["use it", "skip"]),
                "use it",
            ),
            (
                HardLimit::Destructive,
                free_q("Delete the old fixtures?", &["delete", "keep"]),
                "delete",
            ),
            (
                HardLimit::ProductionDeploy,
                free_q("Deploy to production?", &["deploy", "wait"]),
                "deploy",
            ),
        ];
        for (limit, q, answer) in cases {
            assert_eq!(hard_limit(&q, answer), Some(limit), "{}", q.text);
            remember(&app, &repo, &q, answer, AnsweredBy::Owner);
            remember(&app, &repo, &q, answer, AnsweredBy::Owner);
            let task = waiting_task(&repo, q.clone());
            assert!(app.precedent_answer(&task, &q).is_none(), "{}", q.text);
            assert!(
                checked_like(Some("owner precedent"), Some("overseer"), &task, answer).is_err(),
                "{}",
                q.text
            );
            // A plain overseer answer, no `like`, is refused as well; the
            // owner's own answer is not.
            app.store.save_task(&task).unwrap();
            let err = app
                .handle_task_answer(json!({"id": task.id, "answer": answer, "source": "overseer"}))
                .await;
            assert!(err.is_err(), "{}", q.text);
            let still = app.store.load_task(&task.id).unwrap().unwrap();
            assert_eq!(still.status, TaskStatus::Waiting, "{}", q.text);
        }
        // Whole words only: `improve` is not a PR, a routine question is free.
        let routine = free_q("Improve the naming of the helper?", &["yes", "no"]);
        assert_eq!(hard_limit(&routine, "yes"), None);
        // End to end: the question parks and the overseer is woken.
        let push = free_q("Push the branch to origin?", &["push", "hold"]);
        let (answer, _) = park(&app, waiting_task(&repo, push)).await;
        assert_eq!(answer, None);
        chat::wait_idle(&app, &repo).await;
        assert_eq!(chat::fake_overseer_calls(Path::new(&repo)), 1);
    }

    #[tokio::test]
    async fn decision_memory_respects_the_continue_cap_and_the_base_retry_guard() {
        let (app, dir) = overseer_app();
        let repo = repo_root(&app, dir.path()).await;
        remember(&app, &repo, &exhausted_q(), "continue", AnsweredBy::Owner);
        remember(&app, &repo, &exhausted_q(), "continue", AnsweredBy::Owner);
        // One continue left: the precedent answers and its line counts.
        let mut task = waiting_task(&repo, exhausted_q());
        task.decisions
            .push(format!("{EXHAUSTED_ANSWER_PREFIX} -> continue (r)"));
        task.loop_state.non_owner_continues = 1;
        let (answer, task) = park(&app, task).await;
        assert_eq!(answer.as_deref(), Some("continue"));
        assert_eq!(task.loop_state.non_owner_continues, 2);
        assert!(task
            .decisions
            .iter()
            .any(|d| is_orchestrator_answer_decision(d)));
        // One non-owner retry per base.
        remember(&app, &repo, &base_q(), RETRY, AnsweredBy::Owner);
        remember(&app, &repo, &base_q(), RETRY, AnsweredBy::Owner);
        let mut retried = waiting_task(&repo, base_q());
        retried.decisions.push(format!(
            "Lead: base check cargo test on {} -> {RETRY}",
            &retried.base_sha[..7]
        ));
        let (answer, _) = park(&app, retried).await;
        assert_eq!(answer, None);
        chat::wait_idle(&app, &repo).await;
        assert_eq!(chat::fake_overseer_calls(Path::new(&repo)), 1);
    }

    #[tokio::test]
    async fn decision_memory_two_owner_answers_propose_a_rule_the_owner_accepts() {
        let (app, dir) = test_app();
        let repo = repo_root(&app, dir.path()).await;
        let q = base_q();
        remember(&app, &repo, &q, RETRY, AnsweredBy::Owner);
        assert!(precedents::list(&app.data_dir, &repo).1.is_empty());
        remember(&app, &repo, &q, RETRY, AnsweredBy::Owner);
        remember(&app, &repo, &q, RETRY, AnsweredBy::Owner);
        let listed = app
            .handle_precedents_list(json!({"repo": repo}))
            .await
            .unwrap();
        let proposals = listed["proposals"].as_array().unwrap();
        assert_eq!(proposals.len(), 1, "{listed}");
        let rule = proposals[0]["rule"].as_str().unwrap().to_string();
        assert!(rule.chars().count() <= 160 && !rule.contains('\n'));
        assert_eq!(listed["precedents"].as_array().unwrap().len(), 1);
        assert!(app.digest_prompt(&repo).contains(&rule));
        assert!(!app.store.repo_notes_path().exists());
        // Another repo's digest does not carry it.
        assert!(!app.digest_prompt("/elsewhere").contains(&rule));

        let id = proposals[0]["id"].as_str().unwrap();
        let err = app
            .handle_precedents_rule_accept(json!({"repo": repo, "id": "nope"}))
            .await
            .unwrap_err();
        assert!(err.contains("unknown proposal"), "{err}");
        app.handle_precedents_rule_accept(json!({"repo": repo, "id": id}))
            .await
            .unwrap();
        let notes = app
            .handle_repo_notes_list(json!({"repo": repo}))
            .await
            .unwrap();
        assert_eq!(notes["notes"][0]["text"], rule.as_str());
        assert_eq!(notes["notes"][0]["source"], "owner");
        assert!(precedents::list(&app.data_dir, &repo).1.is_empty());
        remember(&app, &repo, &q, RETRY, AnsweredBy::Owner);
        assert!(precedents::list(&app.data_dir, &repo).1.is_empty());

        // A dismissed key is not proposed again.
        let other = free_q("Rename the helper?", &["yes", "no"]);
        remember(&app, &repo, &other, "yes", AnsweredBy::Owner);
        remember(&app, &repo, &other, "yes", AnsweredBy::Owner);
        let open = precedents::list(&app.data_dir, &repo).1;
        assert_eq!(open.len(), 1);
        app.handle_precedents_rule_dismiss(json!({"repo": repo, "id": open[0].id}))
            .await
            .unwrap();
        remember(&app, &repo, &other, "yes", AnsweredBy::Owner);
        assert!(precedents::list(&app.data_dir, &repo).1.is_empty());
        assert_eq!(
            app.handle_repo_notes_list(json!({"repo": repo}))
                .await
                .unwrap()["notes"]
                .as_array()
                .unwrap()
                .len(),
            1
        );

        // Remove: an unknown id is an error.
        let list = precedents::list(&app.data_dir, &repo).0;
        let err = app
            .handle_precedents_remove(json!({"repo": repo, "id": "nope"}))
            .await
            .unwrap_err();
        assert!(err.contains("unknown precedent"), "{err}");
        app.handle_precedents_remove(json!({"repo": repo, "id": list[0].id}))
            .await
            .unwrap();
        assert_eq!(
            precedents::list(&app.data_dir, &repo).0.len(),
            list.len() - 1
        );
    }

    #[tokio::test]
    async fn decision_memory_the_overseer_cannot_write_repo_notes() {
        let (app, dir) = test_app();
        let repo = repo_root(&app, dir.path()).await;
        let note = app
            .handle_repo_notes_add(json!({"repo": repo, "text": "Use tabs"}))
            .await
            .unwrap();
        for result in [
            app.handle_repo_notes_add(json!({"repo": repo, "text": "x", "source": "overseer"}))
                .await,
            app.handle_repo_notes_remove(
                json!({"repo": repo, "id": note["id"], "source": "overseer"}),
            )
            .await,
        ] {
            assert!(result.unwrap_err().contains("owner's"));
        }
        let notes = app
            .handle_repo_notes_list(json!({"repo": repo}))
            .await
            .unwrap();
        assert_eq!(notes["notes"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn decision_memory_like_is_the_overseers_and_cut_to_one_line() {
        let (app, _dir) = test_app();
        let task = waiting_task("/r1", free_q("Which way?", &["A", "B"]));
        app.store.save_task(&task).unwrap();
        let err = app
            .handle_task_answer(
                json!({"id": task.id, "answer": "A", "source": "lead", "like": "x"}),
            )
            .await
            .unwrap_err();
        assert!(err.contains("overseer"), "{err}");
        let long = format!("repo note\n{}", "y".repeat(300));
        app.handle_task_answer(
            json!({"id": task.id, "answer": "A", "source": "overseer", "like": long}),
        )
        .await
        .unwrap();
        let saved = app.store.load_task(&task.id).unwrap().unwrap();
        let line = saved.decisions.last().unwrap();
        let like = line
            .strip_prefix("Overseer: answered like ")
            .and_then(|l| l.split_once(" -- "))
            .unwrap()
            .0;
        assert_eq!(like.chars().count(), 120);
        assert!(!like.contains('\n'));
        assert_eq!(saved.question_history[0].answered_by, AnsweredBy::Overseer);
    }

    #[tokio::test]
    async fn decision_memory_the_overseer_prompt_carries_the_matching_owner_precedents() {
        let (app, _dir) = test_app();
        let q = base_q();
        let task = waiting_task("/r1", q.clone());
        assert_eq!(app.with_precedents("Prompt.".into(), &[&task]), "Prompt.");
        remember(&app, "/r1", &q, RETRY, AnsweredBy::Owner);
        remember(&app, "/r1", &q, RETRY, AnsweredBy::Lead);
        let text = app.with_precedents("Prompt.".into(), &[&task]);
        assert!(text.starts_with("Prompt."));
        assert!(
            text.contains("task_answer {like}") && text.contains(RETRY),
            "{text}"
        );
        assert!(!text.contains("lead"), "{text}");
        assert_eq!(text.matches("the owner answered").count(), 1);
    }

    #[tokio::test]
    async fn decision_memory_the_prompt_hides_precedents_that_hit_a_hard_limit() {
        let (app, _dir) = test_app();
        let q = free_q("Push the branch to origin?", &["push", "hold"]);
        remember(&app, "/r1", &q, "push", AnsweredBy::Owner);
        let task = waiting_task("/r1", q);
        assert_eq!(app.with_precedents("Prompt.".into(), &[&task]), "Prompt.");
    }

    #[tokio::test]
    async fn decision_memory_a_like_must_name_a_precedent_that_exists() {
        let (app, dir) = overseer_app();
        let repo = repo_root(&app, dir.path()).await;
        let q = free_q("Which naming style?", &["snake", "camel"]);
        let task = waiting_task(&repo, q.clone());
        app.store.save_task(&task).unwrap();
        let err = app
            .handle_task_answer(json!({
                "id": task.id, "answer": "snake", "source": "overseer",
                "like": "owner precedent: snake, 2026-10-01"
            }))
            .await
            .unwrap_err();
        assert!(err.contains("no matching answer"), "{err}");
        remember(&app, &repo, &q, "snake", AnsweredBy::Owner);
        // A precedent for another answer does not back this one.
        let err = app
            .handle_task_answer(json!({
                "id": task.id, "answer": "camel", "source": "overseer",
                "like": "Owner precedent: camel"
            }))
            .await;
        assert!(err.is_err());
        app.handle_task_answer(json!({
            "id": task.id, "answer": "snake", "source": "overseer",
            "like": "owner precedent: snake"
        }))
        .await
        .unwrap();
    }

    /// An owner answer to `q` given `ago` ms before now, on its own task.
    fn remember_at(app: &App, repo: &str, q: &Question, answer: &str, ago: i64) {
        let mut task = answered_task(repo, q, answer, AnsweredBy::Owner);
        task.question_history[0].answered_at = crate::model::now_ms() - ago;
        app.store.save_task(&task).unwrap();
    }

    #[tokio::test]
    async fn decision_memory_the_newest_owner_answers_must_agree() {
        let (app, dir) = overseer_app();
        let repo = repo_root(&app, dir.path()).await;
        let q = free_q("Which naming style?", &["snake", "camel"]);
        let key = precedents::question_key(&q, &waiting_task(&repo, q.clone()));
        let agreed = |app: &App| precedents::owner_answer(&app.data_dir, &repo, &key);
        // One answer is not enough.
        remember_at(&app, &repo, &q, "camel", 9000);
        assert_eq!(agreed(&app), None);
        // One old different answer, then two newer ones: the window holds
        // camel, snake, snake and blocks.
        remember_at(&app, &repo, &q, "snake", 5000);
        remember_at(&app, &repo, &q, "snake", 4000);
        assert_eq!(agreed(&app), None);
        // A third newer answer fills the window: the old one stops blocking.
        remember_at(&app, &repo, &q, "snake", 3000);
        assert_eq!(agreed(&app).map(|a| a.0), Some("snake".to_string()));
        // A newer different answer blocks again.
        remember_at(&app, &repo, &q, "camel", 1000);
        assert_eq!(agreed(&app), None);
    }

    #[tokio::test]
    async fn decision_memory_a_resave_after_eviction_counts_nothing_twice() {
        let dir = tempfile::tempdir().unwrap();
        let q = free_q("Which naming style?", &["snake", "camel"]);
        let now = crate::model::now_ms();
        let mut first = answered_task("/r1", &q, "snake", AnsweredBy::Owner);
        first.question_history[0].answered_at = now - 30_000;
        precedents::sync(dir.path(), &first);
        for i in 0..520 {
            let mut other = answered_task("/r2", &q, "camel", AnsweredBy::Owner);
            other.question_history[0].answered_at = now - 20_000 + i;
            precedents::sync(dir.path(), &other);
        }
        precedents::forget_synced();
        precedents::sync(dir.path(), &first);
        let (list, _) = precedents::list(dir.path(), "/r1");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].count, 1);
    }
}
