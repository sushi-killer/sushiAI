//! The evolution loop, part two: cluster the recorded signals, start a
//! read-only proposer run for each cluster with enough evidence, gate what
//! comes back and store it as a proposal, and measure the adopted ones.
//! The judgement of what to change is the model's; this module only picks
//! what to ask about, holds the answer to fixed rules and keeps count.

use super::*;
use crate::brief::{ProposalBriefInput, ProposalExcerpt};
use crate::engine::decision::normalize_signature_line;
use std::collections::{HashMap, HashSet};

/// Verb stems of "make a check weaker", matched as word prefixes.
const WEAKENING_VERBS: [&str; 9] = [
    "remov", "delet", "skip", "disabl", "weaken", "relax", "ignor", "bypass", "loosen",
];

/// What a weakening verb must be near to count: a check, gate or protected
/// path, as whole words.
const CHECK_WORDS: [&str; 19] = [
    "check",
    "checks",
    "test",
    "tests",
    "testing",
    "lint",
    "linter",
    "linting",
    "gate",
    "gates",
    "hook",
    "hooks",
    "verify",
    "verifies",
    "verification",
    "verifier",
    "protected",
    "assertion",
    "assertions",
];

/// Text that weakens a check on its face, whatever is near it.
const WEAKENING_LITERALS: [&str; 8] = [
    "--no-verify",
    "eslint-disable",
    "#[ignore",
    "@ts-ignore",
    "@ts-nocheck",
    ".skip(",
    "continue-on-error",
    "|| true",
];

/// Files a change may never target, on either track.
const PROTECTED_TARGETS: [&str; 4] = ["agents.md", "claude.md", "memory.md", "/memory/"];

/// Forms a repo-track proposal may take.
const REPO_FORMS: [ProposalForm; 6] = [
    ProposalForm::Script,
    ProposalForm::Test,
    ProposalForm::Lint,
    ProposalForm::Doc,
    ProposalForm::Command,
    ProposalForm::Skill,
];

/// A signal as stored, with the `ts` a line may carry (0 when it has none).
struct Stored {
    signal: Signal,
    ts: i64,
}

fn read_signals(data_dir: &Path) -> Vec<Stored> {
    let text =
        std::fs::read_to_string(evolution_dir(data_dir).join("signals.jsonl")).unwrap_or_default();
    text.lines()
        .filter_map(|line| {
            let v: serde_json::Value = serde_json::from_str(line).ok()?;
            let ts = v.get("ts").and_then(|t| t.as_i64()).unwrap_or(0);
            Some(Stored {
                signal: serde_json::from_value(v).ok()?,
                ts,
            })
        })
        .collect()
}

/// Signals of tasks that still exist and are not archived: what a proposer
/// may still learn from. Measuring adopted proposals reads every signal.
fn read_live_signals(data_dir: &Path) -> Vec<Stored> {
    let mut live: HashMap<String, bool> = HashMap::new();
    let mut is_live = |task_id: &str| {
        *live.entry(task_id.to_string()).or_insert_with(|| {
            std::fs::read_to_string(data_dir.join("tasks").join(task_id).join("task.json"))
                .ok()
                .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                .is_some_and(|task| task.get("archived").and_then(|a| a.as_bool()) != Some(true))
        })
    };
    read_signals(data_dir)
        .into_iter()
        .filter(|s| is_live(&s.signal.task_id))
        .collect()
}

fn kind_name(kind: SignalKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default()
}

/// The first word of an owner's answer, lower-cased ("continue", "retry",
/// "drop"): the kind of decision, so a pattern of answers can add up where
/// the free text after it never repeats.
fn owner_answer_kind(detail: &str) -> String {
    let answer = detail.strip_prefix("Owner:").unwrap_or(detail);
    answer
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase()
}

fn cluster_key(signal: &Signal) -> String {
    let normalised = match signal.kind {
        SignalKind::OwnerQuestion => owner_answer_kind(&signal.detail),
        _ => normalize_signature_line(&signal.detail),
    };
    format!("{}:{}", kind_name(signal.kind), simple_hash(&normalised))
}

/// Signals of one kind and normalised detail.
struct Cluster<'a> {
    key: String,
    kind: String,
    occurrences: Vec<&'a Stored>,
}

impl Cluster<'_> {
    fn tasks(&self) -> usize {
        self.occurrences
            .iter()
            .map(|o| o.signal.task_id.as_str())
            .collect::<HashSet<_>>()
            .len()
    }

    fn wasted_calls(&self) -> u32 {
        self.occurrences.iter().map(|o| o.signal.wasted_calls).sum()
    }

    fn wasted_usd(&self) -> f64 {
        self.occurrences.iter().map(|o| o.signal.wasted_usd).sum()
    }

    /// The repo with the most occurrences; the smallest path on a tie.
    fn main_repo(&self) -> &str {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for o in &self.occurrences {
            *counts.entry(o.signal.repo.as_str()).or_default() += 1;
        }
        counts
            .into_iter()
            .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(a.0)))
            .map(|(repo, _)| repo)
            .unwrap_or("")
    }

    /// Latest by `ts`, then by position in the file.
    fn latest(&self) -> &Signal {
        let mut latest = &self.occurrences[0];
        for o in &self.occurrences {
            if o.ts >= latest.ts {
                latest = o;
            }
        }
        &latest.signal
    }

    fn qualifies(&self, settings: &EvolutionSettings) -> bool {
        self.tasks() >= settings.min_tasks as usize
            || self.wasted_calls() >= settings.min_wasted_calls
            || self.wasted_usd() >= settings.min_wasted_usd
    }
}

/// Signals grouped by kind and normalised detail, most wasteful first.
fn clusters(signals: &[Stored]) -> Vec<Cluster<'_>> {
    let mut by_key: HashMap<String, Cluster> = HashMap::new();
    for s in signals {
        by_key
            .entry(cluster_key(&s.signal))
            .or_insert_with(|| Cluster {
                key: cluster_key(&s.signal),
                kind: kind_name(s.signal.kind),
                occurrences: Vec::new(),
            })
            .occurrences
            .push(s);
    }
    let mut out: Vec<Cluster> = by_key.into_values().collect();
    out.sort_by(|a, b| {
        b.wasted_usd()
            .total_cmp(&a.wasted_usd())
            .then_with(|| b.wasted_calls().cmp(&a.wasted_calls()))
            .then_with(|| b.tasks().cmp(&a.tasks()))
            .then_with(|| a.key.cmp(&b.key))
    });
    out
}

/// Up to `MAX_PROPOSAL_EXCERPT_LINES` raw lines around an excerpt ref, or
/// `None` when the ref does not resolve to a readable file under `data_dir`.
fn read_excerpt(data_dir: &Path, signal: &Signal) -> Option<ProposalExcerpt> {
    let r = &signal.excerpt_ref;
    let relative = Path::new(&r.file);
    if !relative
        .components()
        .all(|c| matches!(c, std::path::Component::Normal(_)))
    {
        return None;
    }
    let root = data_dir.canonicalize().ok()?;
    let path = root.join(relative).canonicalize().ok()?;
    if !path.starts_with(&root) {
        return None;
    }
    let text = std::fs::read_to_string(&path).ok()?;
    let all: Vec<&str> = text.lines().collect();
    let max = brief::MAX_PROPOSAL_EXCERPT_LINES;
    let from = r.from_line.max(1);
    let to = r.to_line.max(from);
    let len = to - from + 1;
    let (start, count) = if len >= max {
        (from + (len - max) / 2, max)
    } else {
        (from.saturating_sub((max - len) / 2).max(1), max)
    };
    let lines: Vec<String> = all
        .iter()
        .skip(start - 1)
        .take(count)
        .map(|l| l.to_string())
        .collect();
    if lines.is_empty() {
        return None;
    }
    Some(ProposalExcerpt {
        task_id: signal.task_id.clone(),
        file: r.file.clone(),
        from_line: start,
        lines,
    })
}

/// Excerpts of the cluster's newest occurrences.
fn excerpts(data_dir: &Path, cluster: &Cluster) -> Vec<ProposalExcerpt> {
    let mut newest: Vec<&Stored> = cluster.occurrences.clone();
    newest.reverse();
    newest.sort_by_key(|o| std::cmp::Reverse(o.ts));
    newest
        .into_iter()
        .filter_map(|o| read_excerpt(data_dir, &o.signal))
        .take(brief::MAX_PROPOSAL_OCCURRENCES)
        .collect()
}

/// Whether `text` says to weaken, skip or remove a check, gate or protected
/// path. Deliberately blunt: a false rejection costs one retry of a wording,
/// a false pass lets a proposal loosen the rules that keep the others honest.
fn weakens_a_check(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    if let Some(lit) = WEAKENING_LITERALS.iter().find(|l| lower.contains(*l)) {
        return Some(format!("`{lit}`"));
    }
    for segment in lower.split(['.', ';', '\n', '!', '?']) {
        let words: Vec<&str> = segment
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .collect();
        let is_verb = |i: usize| {
            WEAKENING_VERBS.iter().any(|v| words[i].starts_with(v))
                || (words[i] == "off" && i > 0 && words[i - 1].starts_with("turn"))
        };
        let is_object = |i: usize| CHECK_WORDS.contains(&words[i]);
        for i in 0..words.len() {
            if is_verb(i) {
                let ahead = (i + 1..words.len().min(i + 5)).find(|&j| is_object(j));
                let behind = (i.saturating_sub(3)..i).find(|&j| is_object(j));
                if let Some(j) = ahead.or(behind) {
                    return Some(format!("\"{} ... {}\"", words[i], words[j]));
                }
            }
        }
    }
    None
}

/// The reason a proposal is rejected outright, if it is.
fn gate(reply: &ProposalReply) -> Option<String> {
    if let Some(what) = weakens_a_check(&reply.change) {
        return Some(format!("the change weakens or removes a check: {what}"));
    }
    if reply.track == Track::Repo && !REPO_FORMS.contains(&reply.form) {
        return Some(format!(
            "a repo-track proposal may only be a script, test, lint, doc, command or skill, not {}",
            serde_json::to_value(reply.form)
                .ok()
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_default()
        ));
    }
    let lower = reply.change.to_lowercase();
    if let Some(target) = PROTECTED_TARGETS.iter().find(|t| lower.contains(*t)) {
        return Some(format!(
            "the change targets `{target}`, which proposals may not edit"
        ));
    }
    None
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// What the cluster's latest task supplies to the eval command.
struct Origin {
    request: String,
    base_sha: String,
}

fn eval_command(app: &App, repo: &str, origin: &Origin, arm: Option<&serde_json::Value>) -> String {
    let arm = arm.cloned().unwrap_or_else(|| json!({"<flag>": true}));
    let mut cmd = format!(
        "SUSHIAI_HOME={} {} orch eval run --repo {} --request {}",
        shell_quote(&app.home.to_string_lossy()),
        shell_quote(&app.exe_path),
        shell_quote(repo),
        shell_quote(&origin.request),
    );
    if !origin.base_sha.is_empty() {
        cmd.push_str(&format!(" --base {}", shell_quote(&origin.base_sha)));
    }
    cmd.push_str(&format!(
        " --arms {}",
        shell_quote(&json!([{}, arm]).to_string())
    ));
    cmd
}

/// A proposer run's job: what it was started for.
struct Job {
    id: String,
    key: String,
    kind: String,
    repo: String,
    toplevel: PathBuf,
    origin: Origin,
}

fn proposal_id_param(app: &App, params: &serde_json::Value) -> Result<String, String> {
    let id = params
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("id is required")?;
    uuid::Uuid::parse_str(id).map_err(|_| "invalid proposal id".to_string())?;
    if !app
        .store
        .proposal_path(id)
        .starts_with(app.store.proposals_dir())
    {
        return Err("invalid proposal id".to_string());
    }
    Ok(id.to_string())
}

/// Adopts an approved repo-track proposal whose task is done, then counts
/// the finished, non-eval tasks of its repo (and the cluster's signals they
/// left) on each side of the adoption, suggesting a revert once enough tasks
/// ran after it without the signals falling. Returns whether anything changed.
fn measure(
    p: &mut Proposal,
    tasks: &[Task],
    signals: &[Stored],
    settings: &EvolutionSettings,
) -> bool {
    let before_state = p.clone();
    if p.status == ProposalStatus::Approved && p.track == Track::Repo {
        let done = p
            .task_id
            .as_ref()
            .and_then(|id| tasks.iter().find(|t| &t.id == id))
            .filter(|t| t.status == TaskStatus::Done);
        if let Some(task) = done {
            p.status = ProposalStatus::Adopted;
            p.adopted_at = Some(task.updated_at);
        }
    }
    if p.status != ProposalStatus::Adopted {
        return *p != before_state;
    }
    let Some(adopted_at) = p.adopted_at else {
        return *p != before_state;
    };
    let finished: Vec<&Task> = tasks
        .iter()
        .filter(|t| {
            t.repo == p.repo
                && t.eval_set.is_none()
                && matches!(t.status, TaskStatus::Done | TaskStatus::Failed)
                && p.task_id.as_deref() != Some(t.id.as_str())
        })
        .collect();
    let count = |after: bool| {
        let ids: HashSet<&str> = finished
            .iter()
            .filter(|t| (t.updated_at >= adopted_at) == after)
            .map(|t| t.id.as_str())
            .collect();
        let hits = signals
            .iter()
            .filter(|s| {
                ids.contains(s.signal.task_id.as_str()) && cluster_key(&s.signal) == p.cluster_key
            })
            .count();
        ProposalCounts {
            tasks: ids.len() as u32,
            signals: hits as u32,
        }
    };
    let (before, after) = (count(false), count(true));
    p.before = Some(before);
    p.after = Some(after);
    // No tasks before it means nothing to compare the rate to.
    let no_drop = before.tasks > 0
        && after.tasks >= settings.revert_after_tasks
        && u64::from(after.signals) * u64::from(before.tasks)
            >= u64::from(before.signals) * u64::from(after.tasks);
    if no_drop {
        p.status = ProposalStatus::RevertSuggested;
        p.reason = Some(format!(
            "{} matching signal(s) in {} task(s) after adoption against {} in {} before: no drop, consider reverting",
            after.signals, after.tasks, before.signals, before.tasks
        ));
    }
    *p != before_state
}

impl App {
    fn broadcast_proposal(&self, p: &Proposal) {
        let _ = self.events_tx.send(Event::Proposal {
            proposal: Box::new(serde_json::to_value(p).unwrap_or_else(|_| json!({}))),
        });
    }

    fn store_proposal(&self, p: &Proposal) -> Result<(), String> {
        self.store.save_proposal(p).map_err(|e| e.to_string())?;
        self.broadcast_proposal(p);
        Ok(())
    }

    /// Adoption and measurement of every stored proposal; the ones that
    /// changed.
    async fn measure_proposals(&self) -> Vec<Proposal> {
        let _guard = self.proposal_lock.lock().await;
        let settings = self.settings.read().unwrap().evolution.clone();
        let proposals = self.store.list_proposals().unwrap_or_default();
        if proposals.is_empty() {
            return Vec::new();
        }
        let tasks = self.store.list_tasks().unwrap_or_default();
        let signals = read_signals(&self.data_dir);
        let mut changed = Vec::new();
        for mut p in proposals {
            if measure(&mut p, &tasks, &signals, &settings) && self.store_proposal(&p).is_ok() {
                changed.push(p);
            }
        }
        changed
    }

    /// The route a proposer run uses: `proposerRoute` when it names a
    /// configured route, else the hard tier's.
    fn proposer_route(settings: &Settings) -> Result<Route, String> {
        let named = |id: &str| settings.routes.iter().find(|r| r.id == id).cloned();
        named(&settings.evolution.proposer_route)
            .or_else(|| settings.tiers.get(&Tier::Hard).and_then(|id| named(id)))
            .ok_or_else(|| "no route is configured for the hard tier".to_string())
    }

    /// `evolution.run`: measures the adopted proposals, then starts a
    /// proposer run for each qualifying cluster that has no proposal, up to
    /// `maxProposals`, and returns at once.
    pub(crate) async fn handle_evolution_run(&self) -> Result<serde_json::Value, String> {
        let updated = self.measure_proposals().await;
        let settings = self.settings.read().unwrap().clone();
        let stored: HashSet<String> = self
            .store
            .list_proposals()
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|p| p.cluster_key)
            .collect();
        let signals = read_live_signals(&self.data_dir);
        let mut started = Vec::new();
        let mut jobs = Vec::new();
        let limit = settings.evolution.max_proposals as usize;
        for cluster in clusters(&signals) {
            if jobs.len() >= limit {
                break;
            }
            if !cluster.qualifies(&settings.evolution) || stored.contains(&cluster.key) {
                continue;
            }
            let repo = cluster.main_repo().to_string();
            let path = PathBuf::from(&repo);
            let Ok(Ok(toplevel)) =
                tokio::task::spawn_blocking(move || git::repo_toplevel(&path)).await
            else {
                continue;
            };
            let latest = cluster.latest();
            let task = self.store.load_task(&latest.task_id).ok().flatten();
            let origin = Origin {
                request: task
                    .as_ref()
                    .map(|t| {
                        t.request
                            .clone()
                            .filter(|r| !r.trim().is_empty())
                            .unwrap_or_else(|| t.goal.clone())
                    })
                    .unwrap_or_else(|| latest.detail.clone()),
                base_sha: task.map(|t| t.base_sha).unwrap_or_default(),
            };
            let tasks = cluster.tasks();
            let (calls, usd) = (cluster.wasted_calls(), cluster.wasted_usd());
            let brief_text = brief::build_proposal_brief(&ProposalBriefInput {
                kind: &cluster.kind,
                detail: &latest.detail,
                repo: &repo,
                occurrences: cluster.occurrences.len(),
                tasks,
                wasted_calls: calls,
                wasted_usd: usd,
                excerpts: &excerpts(&self.data_dir, &cluster),
            });
            let job = Job {
                id: uuid::Uuid::new_v4().to_string(),
                key: cluster.key.clone(),
                kind: cluster.kind.clone(),
                repo: repo.clone(),
                toplevel,
                origin,
            };
            let cancel = CancelToken::new();
            {
                let mut in_flight = self.proposals.lock().unwrap();
                if in_flight.contains_key(&job.key) {
                    continue;
                }
                in_flight.insert(job.key.clone(), cancel.clone());
            }
            started.push(json!({
                "id": job.id, "clusterKey": job.key, "kind": job.kind, "repo": repo,
                "tasks": tasks, "wastedCalls": calls, "wastedUsd": usd,
            }));
            jobs.push((job, brief_text, cancel));
        }
        if !jobs.is_empty() {
            let route = match Self::proposer_route(&settings) {
                Ok(route) => route,
                Err(e) => {
                    let mut in_flight = self.proposals.lock().unwrap();
                    for (job, ..) in &jobs {
                        in_flight.remove(&job.key);
                    }
                    return Err(e);
                }
            };
            for (job, brief_text, cancel) in jobs {
                let dir = self.store.proposal_run_dir(&job.id);
                let prepared = std::fs::create_dir_all(&dir)
                    .and_then(|()| std::fs::write(dir.join("brief.md"), &brief_text));
                if prepared.is_err() {
                    self.proposals.lock().unwrap().remove(&job.key);
                    continue;
                }
                let app = self.arc();
                let route = route.clone();
                tokio::spawn(
                    async move { run_proposer(&app, job, route, brief_text, cancel).await },
                );
            }
        }
        Ok(json!({
            "started": started,
            "updated": updated,
        }))
    }

    /// `evolution.list {repo?}`: every proposal, newest first.
    pub(crate) async fn handle_evolution_list(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let repo = params.get("repo").and_then(|v| v.as_str());
        let list = self.store.list_proposals().map_err(|e| e.to_string())?;
        Ok(serde_json::Value::Array(
            list.iter()
                .filter(|p| repo.is_none_or(|r| p.repo == r))
                .map(|p| serde_json::to_value(p).unwrap_or_default())
                .collect(),
        ))
    }

    fn load_proposal_param(&self, params: &serde_json::Value) -> Result<Proposal, String> {
        let id = proposal_id_param(self, params)?;
        self.store
            .load_proposal(&id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "no such proposal".to_string())
    }

    /// `evolution.approve {id}`: a repo-track proposal becomes an ordinary
    /// task, started at once.
    pub(crate) async fn handle_evolution_approve(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let _guard = self.proposal_lock.lock().await;
        let mut p = self.load_proposal_param(&params)?;
        if p.track != Track::Repo {
            return Err("only a repo-track proposal can be approved; adopt a harness-track one after its eval".to_string());
        }
        if p.status != ProposalStatus::Proposed {
            return Err(format!("proposal is already {:?}", p.status).to_lowercase());
        }
        let request = format!(
            "Evolution proposal: {}\n\nEvidence: {}\nMetric: {}\nTest: {}",
            p.change, p.evidence, p.metric, p.test
        );
        let task = self
            .handle_task_create(json!({"repo": p.repo, "request": request, "start": true}))
            .await?;
        p.task_id = task.get("id").and_then(|v| v.as_str()).map(String::from);
        if matches!(p.form, ProposalForm::Doc | ProposalForm::Command) {
            self.add_repo_note(&p.repo, &p.change, format!("proposal:{}", p.id))
                .await?;
        }
        p.status = ProposalStatus::Approved;
        self.store_proposal(&p)?;
        serde_json::to_value(&p).map_err(|e| e.to_string())
    }

    /// `evolution.reject {id, reason?}`.
    pub(crate) async fn handle_evolution_reject(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let _guard = self.proposal_lock.lock().await;
        let mut p = self.load_proposal_param(&params)?;
        p.status = ProposalStatus::Rejected;
        p.reason = params
            .get("reason")
            .and_then(|v| v.as_str())
            .filter(|r| !r.trim().is_empty())
            .map(String::from)
            .or_else(|| Some("rejected by the owner".to_string()));
        self.store_proposal(&p)?;
        serde_json::to_value(&p).map_err(|e| e.to_string())
    }

    /// `evolution.adopt {id}`: the change is in; measurement starts now.
    pub(crate) async fn handle_evolution_adopt(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let _guard = self.proposal_lock.lock().await;
        let mut p = self.load_proposal_param(&params)?;
        if p.status == ProposalStatus::Rejected {
            return Err("a rejected proposal cannot be adopted".to_string());
        }
        p.status = ProposalStatus::Adopted;
        p.adopted_at = Some(now_ms());
        p.reason = None;
        self.store_proposal(&p)?;
        serde_json::to_value(&p).map_err(|e| e.to_string())
    }
}

async fn run_proposer(
    app: &Arc<App>,
    job: Job,
    route: Route,
    brief_text: String,
    cancel: CancelToken,
) {
    let dir = app.store.proposal_run_dir(&job.id);
    // A parallel slot, like every other harness run.
    let permit = tokio::select! {
        _ = cancel.cancelled() => None,
        p = app.slots.clone().acquire_owned() => p.ok(),
    };
    if let Some(_permit) = permit {
        let settings = app.settings.read().unwrap().clone();
        let (mcp_path, settings_path, key_path) =
            super::audit::prepare_run(app, &route, &dir, &settings);
        let req = super::audit::audit_request(&route, &job.toplevel, &mcp_path, &settings_path);
        let result = run_harness(
            app,
            &job.id,
            0,
            RunTrack::No,
            &job.toplevel,
            &req,
            CostTag::repo("evolution", &route.id, &job.repo),
            &brief_text,
            &dir.join("events.jsonl"),
            &cancel,
            Guard::off(),
        )
        .await;
        let _ = std::fs::remove_file(&key_path);
        if let Ok(outcome) = result {
            let text = outcome.final_text.clone().unwrap_or_default();
            // No parsable block stores nothing, so the next run retries.
            if let Ok(reply) = brief::parse_proposal(&text) {
                store_reply(app, &job, reply, &outcome).await;
            }
        }
    }
    app.proposals.lock().unwrap().remove(&job.key);
}

async fn store_reply(app: &App, job: &Job, reply: ProposalReply, outcome: &harness::RunOutcome) {
    let reason = gate(&reply);
    let eval_command = (reply.track == Track::Harness)
        .then(|| eval_command(app, &job.repo, &job.origin, reply.arm.as_ref()));
    let proposal = Proposal {
        id: job.id.clone(),
        cluster_key: job.key.clone(),
        kind: job.kind.clone(),
        repo: job.repo.clone(),
        track: reply.track,
        form: reply.form,
        change: reply.change,
        evidence: reply.evidence,
        metric: reply.metric,
        test: reply.test,
        status: if reason.is_some() {
            ProposalStatus::Rejected
        } else {
            ProposalStatus::Proposed
        },
        created_at: now_ms(),
        cost_usd: outcome.cost_usd.unwrap_or(0.0),
        fingerprint: outcome.fingerprint.clone(),
        eval_command,
        task_id: None,
        adopted_at: None,
        before: None,
        after: None,
        reason,
    };
    let _guard = app.proposal_lock.lock().await;
    let _ = app.store_proposal(&proposal);
}

#[cfg(test)]
mod tests {
    use super::super::super::test_support::{task_with_status, test_app};
    use super::*;

    fn signal(
        kind: SignalKind,
        task: &str,
        repo: &str,
        detail: &str,
        calls: u32,
        usd: f64,
    ) -> Stored {
        Stored {
            signal: Signal {
                kind,
                task_id: task.into(),
                repo: repo.into(),
                attempt: 1,
                detail: detail.into(),
                wasted_calls: calls,
                wasted_usd: usd,
                excerpt_ref: ExcerptRef {
                    file: "tasks/x/runs/1/events.jsonl".into(),
                    from_line: 1,
                    to_line: 1,
                },
            },
            ts: 0,
        }
    }

    fn thresholds(tasks: u32, calls: u32, usd: f64) -> EvolutionSettings {
        EvolutionSettings {
            min_tasks: tasks,
            min_wasted_calls: calls,
            min_wasted_usd: usd,
            ..Default::default()
        }
    }

    /// Two occurrences in two tasks, 10 calls and $0.50 in all.
    fn small_cluster() -> Vec<Stored> {
        vec![
            signal(SignalKind::Loop, "a", "/r", "reads foo 3 times", 4, 0.2),
            signal(SignalKind::Loop, "b", "/r", "reads foo 7 times", 6, 0.3),
        ]
    }

    #[test]
    fn evolution_owner_answers_cluster_by_their_first_word() {
        let answers = [
            (
                "a",
                "Owner: continue: attempt 7 failed only because of a restart",
            ),
            ("b", "Owner: Continue once more, with a new brief"),
            ("c", "Owner: continue - change no code"),
            ("d", "Owner: drop this check (the parent runs it)"),
        ];
        let signals: Vec<Stored> = answers
            .iter()
            .map(|(t, d)| signal(SignalKind::OwnerQuestion, t, "/r", d, 0, 0.0))
            .collect();
        let all = clusters(&signals);
        assert_eq!(all.len(), 2);
        let cont = all.iter().find(|c| c.occurrences.len() == 3).unwrap();
        assert!(cont.qualifies(&thresholds(3, 999, 99.0)));
        assert!(!all
            .iter()
            .find(|c| c.occurrences.len() == 1)
            .unwrap()
            .qualifies(&thresholds(3, 999, 99.0)));
    }

    #[test]
    fn evolution_live_signals_drop_deleted_and_archived_tasks() {
        let (app, _dir) = test_app();
        let live = task_with_status(TaskStatus::Done);
        let mut archived = task_with_status(TaskStatus::Done);
        archived.archived = true;
        app.store.save_task(&live).unwrap();
        app.store.save_task(&archived).unwrap();
        let dir = evolution_dir(&app.data_dir);
        for id in [live.id.as_str(), archived.id.as_str(), "deleted-task"] {
            let s = signal(SignalKind::Loop, id, "/r", "x", 1, 0.0).signal;
            append_line(
                &dir.join("signals.jsonl"),
                &serde_json::to_string(&s).unwrap(),
            )
            .unwrap();
        }
        let ids: Vec<String> = read_live_signals(&app.data_dir)
            .into_iter()
            .map(|s| s.signal.task_id)
            .collect();
        assert_eq!(ids, vec![live.id.clone()]);
        assert_eq!(read_signals(&app.data_dir).len(), 3);
    }

    #[test]
    fn evolution_cluster_qualifies_on_the_task_bound_alone() {
        let signals = small_cluster();
        let c = &clusters(&signals)[0];
        assert!(c.qualifies(&thresholds(2, 999, 99.0)));
        assert!(!c.qualifies(&thresholds(3, 999, 99.0)));
    }

    #[test]
    fn evolution_cluster_qualifies_on_the_wasted_calls_bound_alone() {
        let signals = small_cluster();
        let c = &clusters(&signals)[0];
        assert!(c.qualifies(&thresholds(99, 10, 99.0)));
        assert!(!c.qualifies(&thresholds(99, 11, 99.0)));
    }

    #[test]
    fn evolution_cluster_qualifies_on_the_wasted_usd_bound_alone() {
        let signals = small_cluster();
        let c = &clusters(&signals)[0];
        assert!(c.qualifies(&thresholds(99, 999, 0.5)));
        assert!(!c.qualifies(&thresholds(99, 999, 0.6)));
    }

    #[test]
    fn evolution_clusters_group_by_kind_and_normalised_detail_and_pick_the_main_repo() {
        let signals = vec![
            signal(
                SignalKind::Loop,
                "a",
                "/r1",
                "reads /tmp/x1/foo 3 times",
                1,
                0.0,
            ),
            signal(
                SignalKind::Loop,
                "b",
                "/r2",
                "reads /tmp/y2/foo 9 times",
                1,
                0.0,
            ),
            signal(
                SignalKind::Loop,
                "c",
                "/r2",
                "reads /tmp/z/foo 5 times",
                1,
                0.0,
            ),
            signal(SignalKind::Discovery, "a", "/r1", "reads foo times", 1, 0.0),
        ];
        let found = clusters(&signals);
        assert_eq!(found.len(), 2);
        let looped = found.iter().find(|c| c.kind == "loop").unwrap();
        assert_eq!(looped.occurrences.len(), 3);
        assert_eq!(looped.tasks(), 3);
        assert_eq!(looped.main_repo(), "/r2");
        assert!(looped.key.starts_with("loop:"));
        let other = found.iter().find(|c| c.kind == "discovery").unwrap();
        assert_ne!(other.key, looped.key);
    }

    #[test]
    fn evolution_gate_rejects_weakening_and_protected_targets() {
        let reply = |track, form, change: &str| ProposalReply {
            track,
            form,
            change: change.into(),
            evidence: "e".into(),
            metric: "m".into(),
            test: "t".into(),
            arm: None,
        };
        let weakening = [
            "commit with --no-verify to save time",
            "skip the flaky test in CI",
            "Disable the lint step for generated files",
            "turn off the verify gate on docs-only changes",
            "the protected path check is relaxed",
            "add `#[ignore]` to the slow case",
            "set continue-on-error on the job",
            "run the linter || true",
        ];
        for change in weakening {
            let r = gate(&reply(Track::Harness, ProposalForm::Prompt, change));
            assert!(r.is_some(), "not rejected: {change}");
        }
        let fine = [
            "add scripts/find-symbol.sh so agents stop grepping repeatedly",
            "remove the throwaway script; add a test for the parser",
            "document the checkout flow in docs/INSTALL.md",
        ];
        for change in fine {
            assert_eq!(
                gate(&reply(Track::Repo, ProposalForm::Script, change)),
                None,
                "{change}"
            );
        }
        assert!(
            gate(&reply(Track::Repo, ProposalForm::Doc, "extend AGENTS.md"))
                .unwrap()
                .contains("agents.md")
        );
        assert!(gate(&reply(
            Track::Harness,
            ProposalForm::Doc,
            "edit /memory/notes.md"
        ))
        .is_some());
        assert!(gate(&reply(
            Track::Repo,
            ProposalForm::Prompt,
            "add a prompt line"
        ))
        .unwrap()
        .contains("repo-track"));
        assert_eq!(
            gate(&reply(
                Track::Harness,
                ProposalForm::Prompt,
                "add a prompt line"
            )),
            None
        );
    }

    #[test]
    fn evolution_excerpt_must_resolve_under_the_data_dir() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        std::fs::create_dir_all(data.join("runs")).unwrap();
        let body: String = (1..=100).map(|n| format!("line {n}\n")).collect();
        std::fs::write(data.join("runs/events.jsonl"), body).unwrap();
        std::fs::write(dir.path().join("secret.txt"), "secret\n").unwrap();
        let mut s = signal(SignalKind::Loop, "a", "/r", "d", 1, 0.0);
        s.signal.excerpt_ref = ExcerptRef {
            file: "runs/events.jsonl".into(),
            from_line: 50,
            to_line: 52,
        };
        let e = read_excerpt(&data, &s.signal).unwrap();
        assert_eq!(e.lines.len(), 40);
        assert!(e.lines.contains(&"line 50".to_string()));
        assert!(e.lines.contains(&"line 52".to_string()));
        s.signal.excerpt_ref.file = "../secret.txt".into();
        assert!(read_excerpt(&data, &s.signal).is_none());
        s.signal.excerpt_ref.file = dir.path().join("secret.txt").to_string_lossy().to_string();
        assert!(read_excerpt(&data, &s.signal).is_none());
    }

    fn proposal(status: ProposalStatus, cluster_key: &str) -> Proposal {
        Proposal {
            id: uuid::Uuid::new_v4().to_string(),
            cluster_key: cluster_key.into(),
            kind: "loop".into(),
            repo: "/r".into(),
            track: Track::Repo,
            form: ProposalForm::Script,
            change: "c".into(),
            evidence: "e".into(),
            metric: "m".into(),
            test: "t".into(),
            status,
            created_at: 1,
            cost_usd: 0.0,
            fingerprint: None,
            eval_command: None,
            task_id: None,
            adopted_at: None,
            before: None,
            after: None,
            reason: None,
        }
    }

    fn finished(id: &str, updated_at: i64) -> Task {
        let mut t = task_with_status(TaskStatus::Done);
        t.id = id.into();
        t.repo = "/r".into();
        t.updated_at = updated_at;
        t
    }

    /// 4 tasks before adoption with 4 signals; `after` tasks afterwards,
    /// `hits` of them with a signal.
    fn measured(after: u32, hits: u32) -> (Proposal, Vec<Task>, Vec<Stored>) {
        let key = cluster_key(&signal(SignalKind::Loop, "", "/r", "d", 0, 0.0).signal);
        let mut p = proposal(ProposalStatus::Adopted, &key);
        p.adopted_at = Some(1000);
        p.task_id = Some("own".into());
        let mut tasks = vec![finished("own", 1000)];
        let mut signals = Vec::new();
        for i in 0..4 {
            tasks.push(finished(&format!("b{i}"), 100 + i));
            signals.push(signal(
                SignalKind::Loop,
                &format!("b{i}"),
                "/r",
                "d",
                1,
                0.0,
            ));
        }
        for i in 0..after {
            tasks.push(finished(&format!("a{i}"), 2000 + i64::from(i)));
            if i < hits {
                signals.push(signal(
                    SignalKind::Loop,
                    &format!("a{i}"),
                    "/r",
                    "d",
                    1,
                    0.0,
                ));
            }
        }
        (p, tasks, signals)
    }

    #[test]
    fn evolution_adopted_proposal_with_no_drop_after_ten_tasks_suggests_a_revert() {
        let (mut p, tasks, signals) = measured(10, 10);
        assert!(measure(
            &mut p,
            &tasks,
            &signals,
            &EvolutionSettings::default()
        ));
        assert_eq!(p.status, ProposalStatus::RevertSuggested);
        assert_eq!(
            p.before,
            Some(ProposalCounts {
                tasks: 4,
                signals: 4
            })
        );
        assert_eq!(
            p.after,
            Some(ProposalCounts {
                tasks: 10,
                signals: 10
            })
        );
        assert!(p.reason.as_deref().unwrap().contains("no drop"));
    }

    #[test]
    fn evolution_adopted_proposal_with_a_drop_or_too_few_tasks_stays_adopted() {
        let (mut dropped, tasks, signals) = measured(10, 2);
        measure(
            &mut dropped,
            &tasks,
            &signals,
            &EvolutionSettings::default(),
        );
        assert_eq!(dropped.status, ProposalStatus::Adopted);
        assert_eq!(
            dropped.after,
            Some(ProposalCounts {
                tasks: 10,
                signals: 2
            })
        );
        let (mut young, tasks, signals) = measured(9, 9);
        measure(&mut young, &tasks, &signals, &EvolutionSettings::default());
        assert_eq!(young.status, ProposalStatus::Adopted);
        assert_eq!(young.after.unwrap().tasks, 9);
    }

    #[test]
    fn evolution_approved_repo_proposal_is_adopted_once_its_task_is_done() {
        let mut p = proposal(ProposalStatus::Approved, "loop:x");
        p.task_id = Some("own".into());
        let mut running = finished("own", 500);
        running.status = TaskStatus::Running;
        assert!(!measure(
            &mut p,
            &[running],
            &[],
            &EvolutionSettings::default()
        ));
        assert_eq!(p.status, ProposalStatus::Approved);
        assert!(measure(
            &mut p,
            &[finished("own", 500)],
            &[],
            &EvolutionSettings::default()
        ));
        assert_eq!(p.status, ProposalStatus::Adopted);
        assert_eq!(p.adopted_at, Some(500));
    }

    #[tokio::test]
    async fn evolution_list_filters_by_repo_and_reject_and_adopt_change_status() {
        let (app, _dir) = test_app();
        let mut a = proposal(ProposalStatus::Proposed, "loop:a");
        a.created_at = 1;
        let mut b = proposal(ProposalStatus::Proposed, "loop:b");
        b.created_at = 2;
        b.repo = "/other".into();
        app.store.save_proposal(&a).unwrap();
        app.store.save_proposal(&b).unwrap();
        let all = app.dispatch("evolution.list", json!({})).await.unwrap();
        assert_eq!(all[0]["id"], b.id.as_str());
        assert_eq!(all[1]["id"], a.id.as_str());
        let one = app
            .dispatch("evolution.list", json!({"repo": "/r"}))
            .await
            .unwrap();
        assert_eq!(one.as_array().unwrap().len(), 1);
        let mut events = app.subscribe();
        let rejected = app
            .dispatch("evolution.reject", json!({"id": a.id, "reason": "no"}))
            .await
            .unwrap();
        assert_eq!(rejected["status"], "rejected");
        assert_eq!(rejected["reason"], "no");
        assert!(matches!(events.try_recv(), Ok(Event::Proposal { .. })));
        assert!(app
            .dispatch("evolution.adopt", json!({"id": a.id}))
            .await
            .is_err());
        let adopted = app
            .dispatch("evolution.adopt", json!({"id": b.id}))
            .await
            .unwrap();
        assert_eq!(adopted["status"], "adopted");
        assert!(adopted["adoptedAt"].as_i64().unwrap() > 0);
        let err = app
            .dispatch("evolution.adopt", json!({"id": "../x"}))
            .await
            .unwrap_err();
        assert_eq!(err, "invalid proposal id");
    }

    #[tokio::test]
    async fn evolution_approve_refuses_a_harness_track_proposal() {
        let (app, _dir) = test_app();
        let mut p = proposal(ProposalStatus::Proposed, "loop:a");
        p.track = Track::Harness;
        app.store.save_proposal(&p).unwrap();
        let err = app
            .dispatch("evolution.approve", json!({"id": p.id}))
            .await
            .unwrap_err();
        assert!(err.contains("repo-track"), "{err}");
    }
}
