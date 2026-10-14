//! JSON-file store: `<data>/settings.json`,
//! `<data>/tasks/<id>/task.json`, `<data>/tasks/<id>/runs/<n>/*`,
//! `<data>/tasks/<id>/helpers/<stage>-<seq>/*`,
//! `<data>/audits/<id>/{audit.json,report.json,brief.md,events.jsonl}`.
//! All writes to `task.json`/`settings.json` are atomic (write tmp file in
//! the same directory, then rename) so a crash never leaves a half-written
//! file for the next read.

use crate::model::{
    is_option, AnsweredBy, AttemptStatus, Audit, AuditReport, AuditStatus, BaseAnswer, CheckSource,
    Proposal, QuestionKind, RepoNote, Settings, SourcedCheck, Stage, Task, TaskStatus,
    UnreviewedPart, PRE_EXISTING_RETRY, STATE_VERSION,
};
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[derive(serde::Serialize, serde::Deserialize)]
struct HelperRun {
    stage: String,
    attempt: Option<u32>,
}

pub struct Store {
    data_dir: PathBuf,
}

/// Reads a task file; a task from before typed loop state is migrated once.
fn parse_task(text: &str) -> Result<Task, serde_json::Error> {
    let mut task: Task = serde_json::from_str(text)?;
    migrate_loop_state(&mut task);
    migrate_base_and_permission_state(&mut task);
    migrate_final_check_failures(&mut task);
    migrate_check_sources(&mut task);
    Ok(task)
}

/// Version 1 of the loop state: derives the typed fields from the decision
/// lines and question texts older versions wrote them into. This is the only
/// reader of those formats; afterwards the fields are the state.
fn migrate_loop_state(task: &mut Task) {
    if task.loop_state.state_version >= 1 {
        return;
    }
    let decisions = &task.decisions;
    let lines = |prefix: &'static str| decisions.iter().filter(move |d| d.starts_with(prefix));
    let unreviewed_attempt = task
        .attempts
        .iter()
        .rfind(|a| a.stage == Stage::Implement)
        .filter(|a| {
            task.unreviewed
                && a.status == AttemptStatus::Blocked
                && task.decisions.last().map(String::as_str)
                    == Some(&format!(
                        "Orchestrator: attempt {} committed unreviewed: its open review question was retired",
                        a.n
                    ))
        })
        .map(|a| a.n);
    let agent_restarted = task
        .decisions
        .iter()
        .any(|d| d == "Orchestrator: restarted a failed task");
    let dependency_restarted = task
        .decisions
        .iter()
        .any(|d| d == "Orchestrator: restarted a dependency that failed on the environment");
    let approach_changed_after = lines("Ladder: changed approach after attempt ")
        .filter_map(|d| {
            let rest = d.strip_prefix("Ladder: changed approach after attempt ")?;
            rest.split(|c: char| !c.is_ascii_digit())
                .next()?
                .parse()
                .ok()
        })
        .next_back();
    let stuck_retries = lines("Retry later: ").count() as u32;
    // The count belonged to the landing in progress; a landed task has none.
    let rereviews = if task.landed_at.is_some() {
        0
    } else {
        lines("Land: the carry").count() as u32
    };
    let exhausted = "Attempts keep failing";
    let orchestrator = lines("Orchestrator: Attempts keep failing").count();
    let like = lines("Overseer: answered like ")
        .filter(|d| d.contains(&format!(" -- {exhausted} -> ")))
        .count();
    let legacy_lead = lines("Lead: Attempts keep failing -> ").count();
    let lead = task
        .question_history
        .iter()
        .filter(|q| {
            q.kind == QuestionKind::AttemptsFailing
                && q.answered_by == AnsweredBy::Lead
                && !q.answer.trim().eq_ignore_ascii_case("stop")
        })
        .count();
    let non_owner_continues = (orchestrator + like + lead.max(legacy_lead)) as u32;
    let part_suffix = ") landed unreviewed: no review route gave a verdict";
    let unreviewed_parts: Vec<UnreviewedPart> = lines("Orchestrator: subtask \"")
        .filter_map(|d| {
            let rest = d
                .strip_prefix("Orchestrator: subtask \"")?
                .strip_suffix(part_suffix)?;
            let (title, id) = rest.rsplit_once("\" (")?;
            Some(UnreviewedPart {
                id: id.to_string(),
                title: title.to_string(),
            })
        })
        .collect();
    let impossible_answers: Vec<usize> = task
        .assumptions
        .iter()
        .filter(|a| {
            (a.by == "policy" || a.by == "judge") && a.kind == Some(QuestionKind::Impossible)
        })
        .filter_map(|a| legacy_criterion(&a.question))
        .collect();
    if let Some(q) = task.question.as_mut() {
        if q.kind == QuestionKind::Impossible {
            q.criterion = legacy_criterion(&q.text);
            q.evidence = q
                .text
                .split_once(". Evidence: ")
                .map(|(_, e)| e.to_string());
        }
        if q.kind == QuestionKind::AgentQuestion && q.text.starts_with("This task has spent") {
            q.kind = QuestionKind::Budget;
        }
        if q.kind == QuestionKind::PreexistingFailure && q.base_tails.is_empty() {
            q.base_tails = q
                .commands
                .iter()
                .map(|command| legacy_base_tail(&q.text, command))
                .collect();
        }
    }
    let state = &mut task.loop_state;
    state.state_version = 1;
    state.unreviewed_attempt = unreviewed_attempt;
    state.agent_restarted = agent_restarted;
    state.dependency_restarted = dependency_restarted;
    state.approach_changed_after = approach_changed_after;
    state.stuck_retries = stuck_retries;
    state.rereviews = rereviews;
    state.non_owner_continues = non_owner_continues;
    state.unreviewed_parts = unreviewed_parts;
    state.impossible_answers = impossible_answers;
}

/// Version 2 of the loop state: the kept checks, base-check answers, waited
/// out bases and answered permissions that version 1 kept in decision lines
/// and assumption texts. Like version 1, the only reader of those formats.
fn migrate_base_and_permission_state(task: &mut Task) {
    if task.loop_state.state_version >= 2 {
        return;
    }
    let mut kept_checks: Vec<String> = Vec::new();
    let mut base_answers: Vec<BaseAnswer> = Vec::new();
    let mut waited_out_bases: Vec<String> = Vec::new();
    let mut answered_permissions: Vec<String> = Vec::new();
    let mut push_answer = |answer: BaseAnswer| {
        if !base_answers.contains(&answer) {
            base_answers.push(answer);
        }
    };
    for d in &task.decisions {
        if let Some(rest) = d.strip_prefix("Orchestrator: kept check ") {
            if let Some((command, _reason)) = rest.rsplit_once(", ") {
                if !kept_checks.iter().any(|c| c == command) {
                    kept_checks.push(command.to_string());
                }
            }
        } else if let Some(rest) = d.strip_prefix("Orchestrator: waited out base ") {
            if let Some(sha) = rest.strip_suffix("; not waiting for it to move again") {
                waited_out_bases.push(sha.to_string());
            }
        } else if let Some(rest) = d.strip_prefix("Orchestrator: retry base check ") {
            if let Some((command, rest)) = rest.rsplit_once(" after ") {
                if let Some(sha) = rest.strip_suffix(" moves") {
                    push_answer(BaseAnswer {
                        command: command.to_string(),
                        sha: sha.to_string(),
                        by: String::new(),
                        retry: true,
                    });
                }
            }
        } else if let Some(name) = [
            "Permission: allowed once: ",
            "Permission: allowed for this repo: ",
            "Permission: denied: ",
        ]
        .iter()
        .find_map(|p| d.strip_prefix(p))
        {
            if !answered_permissions.iter().any(|n| n == name) {
                answered_permissions.push(name.to_string());
            }
        } else if let Some((who, rest)) = d.split_once(": base check ") {
            if !matches!(who, "Owner" | "Orchestrator" | "Lead") {
                continue;
            }
            let Some((head, answer)) = rest.split_once(" -> ") else {
                continue;
            };
            let Some((command, sha)) = head.rsplit_once(" on ") else {
                continue;
            };
            if sha.is_empty() || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
                continue;
            }
            push_answer(BaseAnswer {
                command: command.to_string(),
                sha: sha.to_string(),
                by: who.to_string(),
                retry: is_option(answer, PRE_EXISTING_RETRY),
            });
        }
    }
    for a in &task.assumptions {
        if a.kind != Some(QuestionKind::PreexistingFailure) || a.overturned {
            continue;
        }
        if !matches!(a.by.as_str(), "policy" | "judge") {
            continue;
        }
        let first_line = a.question.lines().next().unwrap_or_default();
        let sha = first_line
            .split_once(" on base ")
            .map(|(_, rest)| rest.split(':').next().unwrap_or_default())
            .unwrap_or_default();
        for command in &a.commands {
            push_answer(BaseAnswer {
                command: command.clone(),
                sha: sha.to_string(),
                by: a.by.clone(),
                retry: is_option(&a.answer, PRE_EXISTING_RETRY),
            });
        }
    }
    let state = &mut task.loop_state;
    state.state_version = 2;
    state.kept_checks = kept_checks;
    state.base_answers = base_answers;
    state.waited_out_bases = waited_out_bases;
    state.answered_permissions = answered_permissions;
}

/// Version 3 of the loop state: the commands a parent's final check failed
/// on, which version 2 kept only in a decision line.
fn migrate_final_check_failures(task: &mut Task) {
    if task.loop_state.state_version >= STATE_VERSION {
        return;
    }
    let mut failures: Vec<String> = Vec::new();
    for d in &task.decisions {
        let command = d
            .strip_prefix("Orchestrator: every subtask landed, but `")
            .and_then(|r| r.split_once("` exited"))
            .map(|(command, _)| command);
        if let Some(command) = command {
            if !failures.iter().any(|c| c == command) {
                failures.push(command.to_string());
            }
        }
    }
    task.loop_state.final_check_failures = failures;
    task.loop_state.state_version = STATE_VERSION;
}

/// Older tasks stored the commands of the plan as bare strings. A command of a
/// criterion, a check, the held-out check or verify came from the plan (`Planner`),
/// one the request contains is the owner's, any other is the repo's, which is
/// the cautious default.
fn migrate_check_sources(task: &mut Task) {
    if task.planner_tests.legacy_commands.is_empty() {
        return;
    }
    let planner = task.planner_check_commands();
    let request = task.request.clone().unwrap_or_default();
    let flat = |c: &str| c.split_whitespace().collect::<Vec<_>>().join(" ");
    let request = flat(&request);
    let legacy = std::mem::take(&mut task.planner_tests.legacy_commands);
    for command in legacy {
        if task
            .planner_tests
            .sources
            .iter()
            .any(|s| s.command == command)
        {
            continue;
        }
        let source = if !command.trim().is_empty() && request.contains(&flat(&command)) {
            CheckSource::Owner
        } else if planner.contains(&command) {
            CheckSource::Planner
        } else {
            CheckSource::Repo
        };
        task.planner_tests
            .sources
            .push(SourcedCheck { command, source });
    }
}

/// The criterion number in the text of an impossible question.
fn legacy_criterion(text: &str) -> Option<usize> {
    text.strip_prefix("Criterion ")?
        .split_once(" cannot be met as written")?
        .0
        .parse()
        .ok()
}

/// What a base-check question text quoted for `command`.
fn legacy_base_tail(text: &str, command: &str) -> String {
    let listed = format!("- {command}: ");
    text.lines()
        .find_map(|line| {
            line.strip_prefix(&listed).map(str::to_string).or_else(|| {
                line.strip_prefix(&format!("{command} already fails on base "))
                    .and_then(|rest| rest.split_once(": ").map(|(_, tail)| tail.to_string()))
            })
        })
        .unwrap_or_default()
}

impl Store {
    pub fn new(data_dir: impl Into<PathBuf>) -> io::Result<Self> {
        let data_dir = data_dir.into();
        fs::create_dir_all(&data_dir)?;
        fs::create_dir_all(data_dir.join("tasks"))?;
        Ok(Store { data_dir })
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn settings_path(&self) -> PathBuf {
        self.data_dir.join("settings.json")
    }

    pub fn task_dir(&self, id: &str) -> PathBuf {
        self.data_dir.join("tasks").join(id)
    }

    pub fn task_path(&self, id: &str) -> PathBuf {
        self.task_dir(id).join("task.json")
    }

    pub fn run_dir(&self, id: &str, n: u32) -> PathBuf {
        self.task_dir(id).join("runs").join(n.to_string())
    }

    pub fn helper_run_dir(&self, task_id: &str, stage: &str, seq: u32) -> PathBuf {
        self.task_dir(task_id)
            .join("helpers")
            .join(format!("{stage}-{seq}"))
    }

    pub fn claim_helper_run_dir(
        &self,
        task_id: &str,
        stage: &str,
        attempt: Option<u32>,
    ) -> io::Result<PathBuf> {
        fs::create_dir_all(self.task_dir(task_id).join("helpers"))?;
        for seq in 1..=u32::MAX {
            let dir = self.helper_run_dir(task_id, stage, seq);
            match fs::create_dir(&dir) {
                Ok(()) => {
                    write_json_atomic(
                        &dir.join("helper.json"),
                        &HelperRun {
                            stage: stage.to_string(),
                            attempt,
                        },
                    )?;
                    return Ok(dir);
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::other("helper sequence exhausted"))
    }

    pub fn helper_run_dirs(
        &self,
        task_id: &str,
        stage: &str,
        attempt: Option<u32>,
    ) -> Vec<PathBuf> {
        let Ok(entries) = fs::read_dir(self.task_dir(task_id).join("helpers")) else {
            return Vec::new();
        };
        let prefix = format!("{stage}-");
        let mut dirs: Vec<(u32, PathBuf)> = entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name();
                let seq = name.to_str()?.strip_prefix(&prefix)?.parse::<u32>().ok()?;
                let dir = entry.path();
                let text = fs::read_to_string(dir.join("helper.json")).ok()?;
                let helper: HelperRun = serde_json::from_str(&text).ok()?;
                (helper.stage == stage && helper.attempt == attempt).then_some((seq, dir))
            })
            .collect();
        dirs.sort_by_key(|(seq, _)| *seq);
        dirs.into_iter().map(|(_, dir)| dir).collect()
    }

    // -- settings --------------------------------------------------------

    pub fn load_settings(&self) -> io::Result<Settings> {
        let path = self.settings_path();
        if !path.exists() {
            return Ok(Settings::default());
        }
        let text = fs::read_to_string(&path)?;
        let settings: Settings = serde_json::from_str(&text)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok(settings)
    }

    pub fn save_settings(&self, settings: &Settings) -> io::Result<()> {
        write_json_atomic(&self.settings_path(), settings)
    }

    // -- tasks -------------------------------------------------------------

    pub fn save_task(&self, task: &Task) -> io::Result<()> {
        let dir = self.task_dir(&task.id);
        fs::create_dir_all(&dir)?;
        write_json_atomic(&self.task_path(&task.id), task)?;
        // Every answered question lands in the decision memory (`precedents.rs`).
        crate::precedents::sync(&self.data_dir, task);
        Ok(())
    }

    pub fn load_task(&self, id: &str) -> io::Result<Option<Task>> {
        let path = self.task_path(id);
        if !path.exists() {
            return Ok(None);
        }
        let text = fs::read_to_string(&path)?;
        let task = parse_task(&text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok(Some(task))
    }

    pub fn list_tasks(&self) -> io::Result<Vec<Task>> {
        let tasks_dir = self.data_dir.join("tasks");
        let mut out = Vec::new();
        if !tasks_dir.exists() {
            return Ok(out);
        }
        for entry in fs::read_dir(&tasks_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let task_json = entry.path().join("task.json");
            if !task_json.exists() {
                continue;
            }
            let text = match fs::read_to_string(&task_json) {
                Ok(text) => text,
                Err(e) => {
                    eprintln!("orch: reading task {} failed: {e}", task_json.display());
                    continue;
                }
            };
            match parse_task(&text) {
                Ok(task) => out.push(task),
                Err(e) => {
                    eprintln!("orch: parsing task {} failed: {e}", task_json.display());
                }
            }
        }
        out.sort_by_key(|task| std::cmp::Reverse(task.created_at));
        Ok(out)
    }

    /// On daemon start: any attempt left `running` from a previous process
    /// means that process died mid-attempt. `killpg` its stored pgid first
    /// (if still alive -- the child was `setsid()`ed, so it survives the
    /// daemon's own death and would otherwise run forever unsupervised),
    /// delete its leftover per-run API key file if any, then mark the
    /// attempt `interrupted` and requeue its task unless already `stopped`.
    /// `settle(task, idx)` runs on each such attempt once its process is
    /// gone (the engine counts the spend it left in `events.jsonl`).
    /// Returns the tasks that were mutated.
    pub fn recover_interrupted(
        &self,
        settle: impl FnMut(&mut Task, usize),
    ) -> io::Result<Vec<Task>> {
        self.recover_interrupted_with(|_, _| RunFate::Interrupted, settle)
    }

    /// [`Self::recover_interrupted`], asking `fate` first about each attempt
    /// left `running`. A file-backed run can outlive the daemon: `Resume`
    /// leaves the attempt and its task alone (the task's loop follows the
    /// run). Only `Interrupted` kills the group.
    pub fn recover_interrupted_with(
        &self,
        mut fate: impl FnMut(&Task, usize) -> RunFate,
        mut settle: impl FnMut(&mut Task, usize),
    ) -> io::Result<Vec<Task>> {
        let mut recovered = Vec::new();
        for mut task in self.list_tasks()? {
            let mut interrupted = false;
            for idx in 0..task.attempts.len() {
                if task.attempts[idx].status != AttemptStatus::Running {
                    continue;
                }
                match fate(&task, idx) {
                    RunFate::Resume => {}
                    RunFate::Interrupted => {
                        let attempt = &mut task.attempts[idx];
                        let run_dir = self.run_dir(&task.id, attempt.n);
                        if let Some(pgid) = attempt.pgid {
                            kill_stale_process_group(pgid, &run_dir);
                        }
                        let key_file = run_dir.join("key");
                        let _ = fs::remove_file(key_file);
                        let _ = fs::remove_file(run_dir.join("mcp.json"));
                        let _ = fs::remove_file(run_dir.join("b/mcp.json"));
                        attempt.status = AttemptStatus::Interrupted;
                        attempt.ended_at = Some(crate::model::now_ms());
                        settle(&mut task, idx);
                        interrupted = true;
                    }
                }
            }
            // Interrupted work resumes unless a saved stop or park says otherwise.
            if interrupted {
                if task.queue.stopped_explicitly {
                    task.status = TaskStatus::Stopped;
                    task.question = None;
                } else if task.queue.park.is_some() {
                    task.status = TaskStatus::Queued;
                    task.question = None;
                } else if !matches!(
                    task.status,
                    TaskStatus::Stopped | TaskStatus::Failed | TaskStatus::Done
                ) {
                    task.status = TaskStatus::Queued;
                }
                task.updated_at = crate::model::now_ms();
                if let Err(e) = self.save_task(&task) {
                    eprintln!("orch: saving task {} failed: {e}", task.id);
                    continue;
                }
                recovered.push(task);
            }
        }
        Ok(recovered)
    }

    // -- audits ------------------------------------------------------------

    pub fn audit_dir(&self, id: &str) -> PathBuf {
        self.data_dir.join("audits").join(id)
    }

    pub fn save_audit(&self, audit: &Audit) -> io::Result<()> {
        write_json_atomic(&self.audit_dir(&audit.id).join("audit.json"), audit)
    }

    pub fn load_audit(&self, id: &str) -> io::Result<Option<Audit>> {
        read_json(&self.audit_dir(id).join("audit.json"))
    }

    pub fn save_audit_report(&self, id: &str, report: &AuditReport) -> io::Result<()> {
        write_json_atomic(&self.audit_dir(id).join("report.json"), report)
    }

    pub fn load_audit_report(&self, id: &str) -> io::Result<Option<AuditReport>> {
        read_json(&self.audit_dir(id).join("report.json"))
    }

    /// Every audit of `repo`, newest first; corrupt entries are skipped.
    pub fn list_audits(&self, repo: &str) -> io::Result<Vec<Audit>> {
        let dir = self.data_dir.join("audits");
        let mut out = Vec::new();
        if !dir.exists() {
            return Ok(out);
        }
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            if let Ok(Some(audit)) = read_json::<Audit>(&entry.path().join("audit.json")) {
                if audit.repo == repo {
                    out.push(audit);
                }
            }
        }
        out.sort_by_key(|audit| std::cmp::Reverse(audit.started_at));
        Ok(out)
    }

    /// On daemon start: an audit left `running` lost its run with the
    /// previous process, so it becomes `stopped`, and its per-run key file
    /// goes. Returns the audits that changed.
    pub fn recover_interrupted_audits(&self) -> io::Result<Vec<Audit>> {
        let dir = self.data_dir.join("audits");
        let mut recovered = Vec::new();
        if !dir.exists() {
            return Ok(recovered);
        }
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let Ok(Some(mut audit)) = read_json::<Audit>(&entry.path().join("audit.json")) else {
                continue;
            };
            if audit.status == AuditStatus::Running {
                let _ = fs::remove_file(entry.path().join("key"));
                audit.status = AuditStatus::Stopped;
                audit.error = Some("Interrupted by a daemon restart.".to_string());
                audit.ended_at = Some(crate::model::now_ms());
                self.save_audit(&audit)?;
                recovered.push(audit);
            }
        }
        Ok(recovered)
    }

    // -- repo notes ----------------------------------------------------------

    pub fn repo_notes_path(&self) -> PathBuf {
        self.data_dir.join("repo-notes.json")
    }

    /// Every repo's notes, keyed by repo root.
    pub fn load_repo_notes(&self) -> io::Result<BTreeMap<String, Vec<RepoNote>>> {
        Ok(read_json(&self.repo_notes_path())?.unwrap_or_default())
    }

    pub fn save_repo_notes(&self, notes: &BTreeMap<String, Vec<RepoNote>>) -> io::Result<()> {
        write_json_atomic(&self.repo_notes_path(), notes)
    }

    // -- evolution proposals -----------------------------------------------

    pub fn proposals_dir(&self) -> PathBuf {
        self.data_dir.join("evolution").join("proposals")
    }

    /// The directory of a proposer run: its brief and events.
    pub fn proposal_run_dir(&self, id: &str) -> PathBuf {
        self.proposals_dir().join(id)
    }

    pub fn proposal_path(&self, id: &str) -> PathBuf {
        self.proposals_dir().join(format!("{id}.json"))
    }

    pub fn save_proposal(&self, proposal: &Proposal) -> io::Result<()> {
        write_json_atomic(&self.proposal_path(&proposal.id), proposal)
    }

    pub fn load_proposal(&self, id: &str) -> io::Result<Option<Proposal>> {
        read_json(&self.proposal_path(id))
    }

    /// Every stored proposal, newest first; corrupt entries are skipped.
    pub fn list_proposals(&self) -> io::Result<Vec<Proposal>> {
        let dir = self.proposals_dir();
        let mut out = Vec::new();
        if !dir.exists() {
            return Ok(out);
        }
        for entry in fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Ok(Some(proposal)) = read_json::<Proposal>(&path) {
                out.push(proposal);
            }
        }
        out.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(out)
    }
}

pub(crate) fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<Option<T>> {
    if !path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(path)?;
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// What becomes of a run that was `running` when the daemon stopped.
pub enum RunFate {
    /// It ended or still runs: the attempt stays `running` and the task's
    /// loop follows the run (review and verify come after it as usual).
    Resume,
    /// Its result is lost: the attempt is interrupted.
    Interrupted,
}

/// Best-effort: SIGTERM then (briefly later) SIGKILL a process group left
/// over from an unclean daemon shutdown, if it's still alive. Never panics
/// or blocks the caller for long -- this runs once at startup, before the
/// tokio runtime is doing anything else that matters.
#[cfg(unix)]
fn kill_stale_process_group(pgid: i32, run_dir: &Path) {
    unsafe {
        if libc::kill(pgid, 0) != 0 || !group_started_with(pgid, run_dir) {
            return; // gone, or a stranger's process that reused the pgid
        }
        libc::killpg(pgid, libc::SIGTERM);
    }
    std::thread::sleep(std::time::Duration::from_millis(200));
    unsafe {
        if libc::kill(pgid, 0) == 0 {
            libc::killpg(pgid, libc::SIGKILL);
        }
    }
}

#[cfg(not(unix))]
fn kill_stale_process_group(_pgid: i32, _run_dir: &Path) {}

/// `true` when process `pgid` was started with `path` on its command line.
/// A run's wrapper shell carries its exit file as `$0`, so this tells the
/// run's group from an unrelated process that reuses a saved pgid.
pub fn group_started_with(pgid: i32, path: &Path) -> bool {
    crate::process::argv(pgid).is_some_and(|args| {
        args.iter()
            .any(|arg| arg.contains(&*path.to_string_lossy()))
    })
}

pub fn write_json_atomic<T: serde::Serialize>(path: &Path, value: &T) -> io::Result<()> {
    write_atomic(path, &json_bytes(value)?, None)
}

/// Atomically replaces a plain-text file the owner alone reads (the overseer
/// log), mode 0600.
pub fn write_text_private(path: &Path, text: &str) -> io::Result<()> {
    write_atomic(path, text.as_bytes(), Some(0o600))
}

/// As `write_json_atomic`, for JSON that carries a secret: mode 0600 from the
/// first byte (a run's settings embed its hook token).
pub fn write_json_private<T: serde::Serialize>(path: &Path, value: &T) -> io::Result<()> {
    write_atomic(path, &json_bytes(value)?, Some(0o600))
}

fn json_bytes<T: serde::Serialize>(value: &T) -> io::Result<Vec<u8>> {
    serde_json::to_vec_pretty(value).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Atomically write a secret-bearing file (a per-run API key) restricted to
/// owner read/write (mode 0600): per-run key files are deleted once their run
/// ends, so this is always a small, short-lived plain-text file, never JSON.
pub fn write_secret_file(path: &Path, contents: &str) -> io::Result<()> {
    write_atomic(path, contents.as_bytes(), Some(0o600))
}

/// Writes `bytes` to a uniquely named temp file next to `path` and renames it
/// over `path`. `mode` is the file's mode from creation on (`None`: the
/// umask's default); the temp name is unique per call, so overlapping writes
/// of one path never collide.
fn write_atomic(path: &Path, bytes: &[u8], mode: Option<u32>) -> io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(dir)?;
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("data");
    static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let tmp_path = dir.join(format!(
        ".{}.tmp-{}-{}",
        file_name,
        std::process::id(),
        WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        if let Some(mode) = mode {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(mode);
        }
        let mut f = options.open(&tmp_path)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp_path, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Attempt, AttemptStatus, Harness, Stage, Tier};

    #[test]
    fn an_older_task_gets_the_source_of_each_planner_command() {
        let mut task = sample_task("old", TaskStatus::Waiting, AttemptStatus::Passed);
        task.criteria = vec!["a -- check: grep -q x f".into(), "b".into()];
        task.request = Some("Please run `make owner` too".into());
        let mut value = serde_json::to_value(&task).unwrap();
        value["plannerTests"] = serde_json::json!({
            "commands": ["true", "grep -q x f", "make owner", "make other"]
        });
        let task = parse_task(&value.to_string()).unwrap();
        let source = |c: &str| {
            task.planner_tests
                .sources
                .iter()
                .find(|s| s.command == c)
                .map(|s| s.source)
        };
        assert_eq!(source("true"), Some(CheckSource::Planner));
        assert_eq!(source("make other"), Some(CheckSource::Repo));
        assert_eq!(source("grep -q x f"), Some(CheckSource::Planner));
        assert_eq!(source("make owner"), Some(CheckSource::Owner));
        assert!(task.planner_tests.legacy_commands.is_empty());
    }

    #[test]
    fn a_plan_records_who_wrote_each_command() {
        let mut task = sample_task("new", TaskStatus::Waiting, AttemptStatus::Passed);
        task.criteria = vec!["a -- check: grep -q x f".into()];
        task.final_verify = vec!["make owner".into()];
        task.record_check_sources(|c| c == "make owner");
        let got: Vec<_> = task
            .planner_tests
            .sources
            .iter()
            .map(|s| (s.command.as_str(), s.source))
            .collect();
        assert_eq!(
            got,
            vec![
                ("true", CheckSource::Planner),
                ("make owner", CheckSource::Owner),
                ("grep -q x f", CheckSource::Planner),
            ]
        );
    }

    fn sample_task(id: &str, status: TaskStatus, attempt_status: AttemptStatus) -> Task {
        Task {
            id: id.to_string(),
            title: "Do thing".into(),
            goal: "Do the thing".into(),
            criteria: vec!["works".into()],
            verify: vec!["true".into()],
            final_verify: vec![],
            checks: vec![],
            held_out: None,
            planner_tests: Default::default(),
            request: None,
            repo: "/repo".into(),
            project_id: None,
            worktree: "/repo-task".into(),
            worktree_removed: false,
            deliverables: vec![],
            visual_criteria: vec![],
            landed_sha: None,
            landed_at: None,
            unlanded: None,
            diff_stat: None,
            report: None,
            report_at: None,
            lead_touch: None,
            follow_up_of: None,
            follow_ups: vec![],
            branch: "task/do-thing".into(),
            base_sha: "deadbeef".into(),
            base_ref: None,
            fresh_base: None,
            depends_on: vec![],
            paths: vec![],
            parent: None,
            status,
            tier: Tier::Standard,
            question: None,
            decisions: vec![],
            attempts: vec![Attempt {
                n: 1,
                stage: Stage::Implement,
                route_id: "claude-sonnet".into(),
                harness: Harness::Claude,
                model: "sonnet".into(),
                reason: "tier default".into(),
                session_id: Some("sess-1".into()),
                pgid: None,
                started_at: 1,
                ended_at: None,
                status: attempt_status,
                summary: None,
                handoff: None,
                declines: vec![],
                changed_files: vec![],
                verify: vec![],
                gate_blocks: 0,
                prefix_tokens: None,
                first_edit_at: None,
                review: None,
                failure: None,
                usage: None,
                cost_usd: None,
                cost_estimated: false,
                review_cost_usd: None,
                evidence: vec![],
                evidence_tree: None,
                review_tree: None,
                review_base: None,
                review_scope: None,
                evidence_base: None,
                evidence_from: None,
                advice: None,
                advisor_cost_usd: None,
                fingerprint: None,
                review_fingerprint: None,
                advisor_fingerprint: None,
                candidates: vec![],
                criteria_results: vec![],
                cause: None,
                diff_stat: None,
            }],
            cost_usd: 0.0,
            budget_raises: 0,
            daily_budget_ok_day: None,
            assumptions: vec![],
            question_history: vec![],
            findings: vec![],
            archived: false,
            pr_url: None,
            pr_branch: None,
            pr_pending: false,
            pr_plan: None,
            overseer_woken: false,
            unreviewed: false,
            failed_count: 0,
            loop_state: Default::default(),
            planned_tier: None,
            planned_by_parent: false,
            tier_fallback: None,
            variant: Default::default(),
            eval_set: None,
            eval_name: None,
            eval_check_cmd: None,
            source: None,
            eval_check: None,
            brief_check: Default::default(),
            plan_gate: Default::default(),
            metrics: None,
            checklist: vec![],
            queue: Default::default(),
            created_at: 1,
            updated_at: 1,
        }
    }

    #[test]
    fn task_round_trips_through_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        let task = sample_task("t1", TaskStatus::Running, AttemptStatus::Running);
        store.save_task(&task).unwrap();
        let loaded = store.load_task("t1").unwrap().unwrap();
        assert_eq!(loaded.id, "t1");
        assert_eq!(loaded.branch, "task/do-thing");
        assert_eq!(loaded.attempts.len(), 1);
        assert!(store.load_task("missing").unwrap().is_none());
    }

    #[test]
    fn archived_flag_round_trips_and_defaults_false_for_a_pre_existing_task_json() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();

        let mut task = sample_task("t1", TaskStatus::Done, AttemptStatus::Passed);
        task.archived = true;
        store.save_task(&task).unwrap();
        let loaded = store.load_task("t1").unwrap().unwrap();
        assert!(loaded.archived);

        // A task.json written before `archived` existed has no such key at
        // all -- simulate that directly rather than via `Task`, which would
        // always serialize the field.
        let old_dir = store.task_dir("old");
        fs::create_dir_all(&old_dir).unwrap();
        let mut value =
            serde_json::to_value(sample_task("old", TaskStatus::Done, AttemptStatus::Passed))
                .unwrap();
        value.as_object_mut().unwrap().remove("archived");
        fs::write(
            old_dir.join("task.json"),
            serde_json::to_vec_pretty(&value).unwrap(),
        )
        .unwrap();
        let old_loaded = store.load_task("old").unwrap().unwrap();
        assert!(!old_loaded.archived);
    }

    /// A task.json written with fields since removed from `Variant` and
    /// `Attempt` must still load: serde ignores unknown fields by default,
    /// so dropping them from the structs is not a breaking change for a
    /// task already on disk.
    #[test]
    fn a_task_json_with_fields_since_removed_from_variant_and_attempt_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        let task_dir = store.task_dir("old");
        fs::create_dir_all(&task_dir).unwrap();
        let mut value =
            serde_json::to_value(sample_task("old", TaskStatus::Done, AttemptStatus::Passed))
                .unwrap();
        // A flag removed from `Variant` (2026-09-28) must not break loading.
        value["variant"]["leanContext"] = serde_json::json!(true);
        value["attempts"][0]["skills"] = serde_json::json!(["deslop"]);
        fs::write(
            task_dir.join("task.json"),
            serde_json::to_vec_pretty(&value).unwrap(),
        )
        .unwrap();
        let loaded = store.load_task("old").unwrap().unwrap();
        assert_eq!(loaded.id, "old");
        assert_eq!(loaded.attempts.len(), 1);
    }

    #[test]
    fn settings_round_trips_and_defaults_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        let defaulted = store.load_settings().unwrap();
        assert_eq!(defaulted.max_attempts, 4);

        let custom = Settings {
            max_attempts: 7,
            ..Settings::default()
        };
        store.save_settings(&custom).unwrap();
        let loaded = store.load_settings().unwrap();
        assert_eq!(loaded.max_attempts, 7);
    }

    #[test]
    fn settings_saved_before_presets_load_with_none() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        let mut value = serde_json::to_value(Settings::default()).unwrap();
        let o = value.as_object_mut().unwrap();
        assert!(o.remove("presets").is_some());
        assert!(o.remove("repoPresets").is_some());
        std::fs::write(store.settings_path(), value.to_string()).unwrap();
        let loaded = store.load_settings().unwrap();
        assert!(loaded.presets.is_empty());
        assert!(loaded.repo_presets.is_empty());
        assert!(loaded.check_presets().is_ok());
    }

    #[test]
    fn old_settings_naming_the_retired_haiku_route_load_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        let mut value = serde_json::to_value(Settings::default()).unwrap();
        value["briefCheckRoute"] = serde_json::json!("claude-haiku");
        std::fs::write(store.settings_path(), value.to_string()).unwrap();
        let loaded = store.load_settings().unwrap();
        assert_eq!(loaded.brief_check_route, "claude-haiku");
        assert!(!loaded.routes.iter().any(|r| r.id == "claude-haiku"));
    }

    #[test]
    fn a_task_with_legacy_state_lines_loads_with_typed_fields() {
        use crate::model::{Question, QuestionKind};
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        let mut task = sample_task("legacy", TaskStatus::Queued, AttemptStatus::Blocked);
        task.unreviewed = true;
        task.decisions = vec![
            "Orchestrator: restarted a failed task".into(),
            "Orchestrator: restarted a dependency that failed on the environment".into(),
            "Ladder: changed approach after attempt 1 (the same tier; the planner re-plans the rest)".into(),
            "Retry later: x; trying again in 30 s (retry 1 of 2)".into(),
            "Retry later: x; trying again in 30 s (retry 2 of 2)".into(),
            "Land: the carry onto b changed f which the review saw".into(),
            "Orchestrator: Attempts keep failing -> continue (r)".into(),
            "Overseer: answered like owner precedent -- Attempts keep failing -> continue".into(),
            "Lead: Attempts keep failing -> continue".into(),
            "Orchestrator: subtask \"Part (a)\" (child-1) landed unreviewed: no review route gave a verdict".into(),
            "Orchestrator: attempt 1 committed unreviewed: its open review question was retired".into(),
        ];
        let mut question = Question::new(
            "Criterion 2 cannot be met as written: x. Evidence: a. Evidence: b",
            vec![],
            QuestionKind::Impossible,
            crate::model::AskedBy::Implement,
        );
        question.commands = vec!["npm test".into(), "npm run lint".into()];
        task.question = Some(question);
        store.save_task(&task).unwrap();
        // Written by an older version: no state version, no typed fields.
        let path = store.task_path("legacy");
        let mut value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        value.as_object_mut().unwrap().remove("stateVersion");
        fs::write(&path, value.to_string()).unwrap();

        let loaded = store.load_task("legacy").unwrap().unwrap();
        let state = &loaded.loop_state;
        assert_eq!(state.state_version, STATE_VERSION);
        assert_eq!(state.unreviewed_attempt, Some(1));
        assert!(state.agent_restarted && state.dependency_restarted);
        assert_eq!(state.approach_changed_after, Some(1));
        assert_eq!((state.stuck_retries, state.rereviews), (2, 1));
        assert_eq!(state.non_owner_continues, 3);
        assert_eq!(state.unreviewed_parts.len(), 1);
        assert_eq!(state.unreviewed_parts[0].id, "child-1");
        assert_eq!(state.unreviewed_parts[0].title, "Part (a)");
        let q = loaded.question.unwrap();
        assert_eq!(q.criterion, Some(2));
        assert_eq!(q.evidence.as_deref(), Some("a. Evidence: b"));
        // Loaded and saved again, the state stays as it is: the lines are not read twice.
        let mut again = store.load_task("legacy").unwrap().unwrap();
        again.loop_state.stuck_retries = 0;
        store.save_task(&again).unwrap();
        assert_eq!(
            store
                .load_task("legacy")
                .unwrap()
                .unwrap()
                .loop_state
                .stuck_retries,
            0
        );
    }

    #[test]
    fn a_version_one_task_gets_base_check_and_permission_state_from_its_lines() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        let mut task = sample_task("v1", TaskStatus::Waiting, AttemptStatus::Passed);
        task.decisions = vec![
            "Orchestrator: kept check npm test, the task is meant to make it pass".into(),
            "Orchestrator: kept check cargo test -- a, b, targets absent on base; the plan creates them".into(),
            "Orchestrator: waited out base abcdef0123; not waiting for it to move again".into(),
            "Lead: base check npm test on 0123456 -> retry after the base is fixed".into(),
            "Owner: base check npm run lint on 0123456 -> drop all".into(),
            "Orchestrator: retry base check npm test after 0123456789abcdef moves".into(),
            "Permission: allowed once: Write(src/a.rs)".into(),
            "Permission: denied: mcp__x__y".into(),
        ];
        let mut policy = crate::model::Assumption {
            question: "npm ci already fails on base 89abcde: boom".into(),
            answer: "retry after the base is fixed".into(),
            evidence: String::new(),
            by: "policy".into(),
            kind: Some(QuestionKind::PreexistingFailure),
            attempt: Some(1),
            commands: vec!["npm ci".into()],
            overturned: false,
            owner_answer: None,
        };
        task.assumptions.push(policy.clone());
        policy.by = "Owner".into();
        task.assumptions.push(policy);
        store.save_task(&task).unwrap();
        let path = store.task_path("v1");
        let mut value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        value["stateVersion"] = 1.into();
        fs::write(&path, value.to_string()).unwrap();

        let state = store.load_task("v1").unwrap().unwrap().loop_state;
        assert_eq!(state.state_version, STATE_VERSION);
        assert_eq!(state.kept_checks, ["npm test", "cargo test -- a, b"]);
        assert_eq!(state.waited_out_bases, ["abcdef0123"]);
        assert_eq!(state.answered_permissions, ["Write(src/a.rs)", "mcp__x__y"]);
        let answer = |command: &str, sha: &str, by: &str, retry: bool| BaseAnswer {
            command: command.into(),
            sha: sha.into(),
            by: by.into(),
            retry,
        };
        assert_eq!(
            state.base_answers,
            [
                answer("npm test", "0123456", "Lead", true),
                answer("npm run lint", "0123456", "Owner", false),
                answer("npm test", "0123456789abcdef", "", true),
                answer("npm ci", "89abcde", "policy", true),
            ]
        );
    }

    #[test]
    fn a_version_two_task_gets_its_final_check_failures_from_the_line() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        let mut task = sample_task("v2", TaskStatus::Failed, AttemptStatus::Passed);
        task.decisions = vec![
            "Orchestrator: every subtask landed, but `npm run ci` exited 2 on b: x".into(),
            "Orchestrator: every subtask landed, but `npm run ci` exited 2 on b: y".into(),
        ];
        store.save_task(&task).unwrap();
        let path = store.task_path("v2");
        let mut value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        value["stateVersion"] = 2.into();
        fs::write(&path, value.to_string()).unwrap();
        let state = store.load_task("v2").unwrap().unwrap().loop_state;
        assert_eq!(state.state_version, STATE_VERSION);
        assert_eq!(state.final_check_failures, ["npm run ci"]);
    }

    #[test]
    fn list_tasks_sorts_newest_first_and_skips_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        let mut older = sample_task("older", TaskStatus::Done, AttemptStatus::Passed);
        older.created_at = 1;
        let mut newer = sample_task("newer", TaskStatus::Done, AttemptStatus::Passed);
        newer.created_at = 2;
        store.save_task(&older).unwrap();
        store.save_task(&newer).unwrap();

        // A corrupt task directory should be skipped, not fail the listing.
        let corrupt_dir = store.task_dir("corrupt");
        fs::create_dir_all(&corrupt_dir).unwrap();
        fs::write(corrupt_dir.join("task.json"), b"not json").unwrap();

        let listed = store.list_tasks().unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].id, "newer");
        assert_eq!(listed[1].id, "older");
    }

    #[test]
    fn restart_recovery_marks_running_attempts_interrupted() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        let running_task = sample_task("running", TaskStatus::Running, AttemptStatus::Running);
        let done_task = sample_task("done", TaskStatus::Done, AttemptStatus::Passed);
        store.save_task(&running_task).unwrap();
        store.save_task(&done_task).unwrap();
        let run_dir = store.run_dir(&running_task.id, running_task.attempts[0].n);
        fs::create_dir_all(run_dir.join("b")).unwrap();
        for file in ["key", "mcp.json", "b/mcp.json"] {
            fs::write(run_dir.join(file), "synthetic secret").unwrap();
        }
        assert!(store
            .recover_interrupted_with(|_, _| RunFate::Resume, |_, _| {})
            .unwrap()
            .is_empty());
        for file in ["key", "mcp.json", "b/mcp.json"] {
            assert!(run_dir.join(file).exists());
        }

        let recovered = store.recover_interrupted(|_, _| {}).unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].id, "running");

        let reloaded = store.load_task("running").unwrap().unwrap();
        assert_eq!(reloaded.status, TaskStatus::Queued);
        assert_eq!(reloaded.attempts[0].status, AttemptStatus::Interrupted);
        for file in ["key", "mcp.json", "b/mcp.json"] {
            assert!(!run_dir.join(file).exists());
        }

        // Untouched task stays as-is.
        let untouched = store.load_task("done").unwrap().unwrap();
        assert_eq!(untouched.status, TaskStatus::Done);
    }

    /// A detached group like the engine's run wrapper: `$0` is `marker`.
    fn spawn_group(marker: &std::path::Path) -> std::process::Child {
        let mut cmd = std::process::Command::new("/bin/sh");
        cmd.arg("-c").arg("sleep 30; true").arg(marker);
        #[cfg(unix)]
        unsafe {
            use std::os::unix::process::CommandExt;
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        cmd.spawn().unwrap()
    }

    /// Waits for `child` to end (reaping it: a zombie's pid still answers
    /// `kill(pid, 0)`). `true` when it ended within `secs`.
    fn ends_within(child: &mut std::process::Child, secs: u64) -> bool {
        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_secs(secs) {
            if child.try_wait().unwrap().is_some() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn recover_interrupted_kills_a_stale_process_group_if_still_alive() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        let mut task = sample_task("running", TaskStatus::Running, AttemptStatus::Running);
        // Proves recovery kills the run's group rather than just marking
        // the attempt interrupted and leaving an orphan running forever.
        let run_dir = store.run_dir("running", task.attempts[0].n);
        let mut child = spawn_group(&run_dir.join("events.exit"));
        task.attempts[0].pgid = Some(child.id() as i32);
        store.save_task(&task).unwrap();

        store.recover_interrupted(|_, _| {}).unwrap();

        assert!(
            ends_within(&mut child, 2),
            "stale process group should have been killed on recovery"
        );
    }

    #[test]
    fn recover_interrupted_leaves_a_reused_pgid_alone() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        let mut task = sample_task("running", TaskStatus::Running, AttemptStatus::Running);
        // The saved pgid belongs to a stranger's process now.
        let mut stranger = spawn_group(std::path::Path::new("/not-a-run"));
        task.attempts[0].pgid = Some(stranger.id() as i32);
        store.save_task(&task).unwrap();

        store.recover_interrupted(|_, _| {}).unwrap();

        assert!(
            !ends_within(&mut stranger, 1),
            "a process that is not the run must not be killed"
        );
        let _ = stranger.kill();
        let _ = stranger.wait();
    }

    #[cfg(unix)]
    #[test]
    fn write_json_private_is_mode_0600_and_leaves_no_temp_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        write_json_private(&path, &serde_json::json!({"token": "t"})).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn concurrent_secret_writes_of_one_path_do_not_collide() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("key");
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    (0..50).all(|_| write_secret_file(&path, &format!("k{i}")).is_ok())
                })
            })
            .collect();
        for t in threads {
            assert!(t.join().unwrap(), "a write failed on a shared temp name");
        }
    }

    #[test]
    fn write_secret_file_is_mode_0600() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("key");
        write_secret_file(&path, "abc123\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "abc123\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }
}

#[cfg(test)]
mod helper_tests {
    use super::*;

    #[test]
    fn helpers_folder_claims_are_unique_and_read_in_numeric_sequence_order() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path()).unwrap();
        let first = store
            .claim_helper_run_dir("task", "review", Some(1))
            .unwrap();
        std::fs::write(first.join("events.jsonl"), "first").unwrap();
        let mut claimed = vec![first.clone()];
        for _ in 0..10 {
            claimed.push(
                store
                    .claim_helper_run_dir("task", "review", Some(1))
                    .unwrap(),
            );
        }
        store
            .claim_helper_run_dir("task", "review", Some(2))
            .unwrap();
        let detached = store
            .claim_helper_run_dir("task", "brief-check", None)
            .unwrap();
        assert_eq!(store.helper_run_dirs("task", "review", Some(1)), claimed);
        assert_eq!(first, tmp.path().join("tasks/task/helpers/review-1"));
        assert_eq!(
            std::fs::read_to_string(first.join("events.jsonl")).unwrap(),
            "first"
        );
        assert_eq!(
            store
                .helper_run_dirs("task", "brief-check", None)
                .as_slice(),
            std::slice::from_ref(&detached)
        );
        let meta: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(detached.join("helper.json")).unwrap())
                .unwrap();
        assert_eq!(
            meta,
            serde_json::json!({"stage":"brief-check", "attempt":null})
        );
        std::thread::scope(|scope| {
            for _ in 0..4 {
                let store = &store;
                scope.spawn(move || {
                    store
                        .claim_helper_run_dir("task", "advisor", Some(1))
                        .unwrap();
                });
            }
        });
        assert_eq!(store.helper_run_dirs("task", "advisor", Some(1)).len(), 4);
    }
}
