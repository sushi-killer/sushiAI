//! The "whose failure" turn: a check that fails on the task's tree and on its
//! base, and that the deterministic comparison cannot settle, is read by one
//! read-only agent turn in a session of its own. Code keeps the guards: no
//! evidence means the failure is the task's, and a verdict that lowers
//! caution holds only when the test names it gives appear in the base output.

use crate::engine::attempts::ledger::CostTag;
use crate::engine::attempts::run::run_harness;
use crate::engine::attempts::tracking::RunTrack;
use crate::engine::review;
use crate::engine::review::review_request;
use crate::engine::review::write_readonly_claude_settings_with_profile;
use crate::engine::state::App;
use crate::git::paths::short_sha;
use crate::harness;
use crate::harness::cancel::CancelToken;
use crate::harness::guard::helper_timeout_message;
use crate::harness::guard::Guard;
use crate::policy::routing::Harness;
use crate::prompts::text::tail_chars;
use crate::task::Blame;
use crate::task::Stage;
use crate::task::Task;
use crate::task::VerifyOutcome;
use serde::Deserialize;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use crate::engine::finding_judge::resolve_judge_route;
use std::hash::{Hash, Hasher};

const RUN_HEAD_CHARS: usize = 1500;
const RUN_TAIL_CHARS: usize = 3000;

/// The turn's typed reply. Unknown fields fail the parse.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Attribution {
    pub cause: Blame,
    /// Tests failing in the task's run and not in the base run.
    #[serde(default)]
    pub new_failures: Vec<String>,
    /// For `inherited`: the failing tests, copied from the base run.
    #[serde(default)]
    pub pre_existing: Vec<String>,
    /// The request, goal or criteria ask for this check to pass.
    #[serde(default)]
    pub target: bool,
    #[serde(default)]
    pub evidence: String,
}

/// Task id, hash of the task's diff, base sha, command, run key and base run
/// key.
pub(super) type AttributionKey = (String, String, String, String, String, String);

/// A hash of the task's changes against its base, so a verdict is not reused
/// for another task or another diff.
fn diff_hash(task: &Task) -> String {
    let worktree = Path::new(&task.worktree);
    let mut h = std::collections::hash_map::DefaultHasher::new();
    crate::git::changes::diff_full(worktree, &task.base_sha, usize::MAX)
        .unwrap_or_default()
        .hash(&mut h);
    crate::git::changes::changed_files(worktree, &task.base_sha)
        .unwrap_or_default()
        .hash(&mut h);
    format!("{:016x}", h.finish())
}

/// The reply's last `sushi-attribution` block, else its outermost `{...}`.
pub(super) fn parse_attribution(text: &str) -> Option<Attribution> {
    serde_json::from_str(&crate::prompts::reply::tagged_json(
        text,
        "sushi-attribution",
    )?)
    .ok()
}

/// Whether `name` is a whole line of `tail`, or the end of one after
/// whitespace (`FAIL suite > alpha` holds `suite > alpha`, not `FAIL`).
fn in_lines(tail: &str, name: &str) -> bool {
    let name = name.trim();
    !name.is_empty()
        && tail.lines().any(|line| {
            let line = line.trim();
            line == name
                || line
                    .strip_suffix(name)
                    .is_some_and(|rest| rest.ends_with(char::is_whitespace))
        })
}

/// Whether the failed run's own output is covered by the base run: the same
/// output fingerprint, no crash and no early stop, every failing name code
/// parsed from it a failing name of the base, and every passing test of the
/// base still passing. A new error next to the base's failure, a crash after
/// it, and a fail-fast run that stopped at it are not covered.
fn failed_run_covered(failed: &VerifyOutcome, base: &VerifyOutcome) -> bool {
    failed.output_key == base.output_key
        && !failed.crashed
        && !failed.stopped_early
        && failed.failing.iter().all(|n| base.failing.contains(n))
        && base.passing.iter().all(|n| failed.passing.contains(n))
}

/// The verdict code accepts. A cause other than `introduced` needs evidence;
/// `inherited` also needs a non-empty `pre_existing`, no `new_failures`, the
/// failed run covered by the base run (see `failed_run_covered`), and every
/// name in `pre_existing` a failing name or a line of the base run, and for
/// a run with no parsed names a line of the failed run too. `new_failures`
/// can only withhold a waiver, never grant one. Anything else is
/// `introduced`.
pub(super) fn checked(
    mut att: Attribution,
    failed: &VerifyOutcome,
    base: &VerifyOutcome,
) -> Attribution {
    let evidenced = !att.evidence.trim().is_empty();
    let inherited_holds = !att.pre_existing.is_empty()
        && att.new_failures.is_empty()
        && failed_run_covered(failed, base)
        && att.pre_existing.iter().all(|n| {
            let n = n.trim();
            (base.failing.iter().any(|f| f == n) || in_lines(&base.tail, n))
                && (!failed.failing.is_empty() || in_lines(&failed.tail, n))
        });
    let holds = match att.cause {
        Blame::Introduced => true,
        Blame::Inherited => evidenced && inherited_holds,
        Blame::Environment | Blame::Incomplete => evidenced,
    };
    if !holds {
        att.cause = Blame::Introduced;
    }
    att
}

/// What identifies one run's output: its fingerprint and test names, or its
/// tail when it has no fingerprint.
fn run_key(run: &VerifyOutcome) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    run.output_key.hash(&mut h);
    run.failing.hash(&mut h);
    // An empty fingerprint says nothing about the output: use the tail.
    if run.output_key.is_empty() {
        run.tail.hash(&mut h);
    }
    format!("{:016x}", h.finish())
}

fn head_tail(text: &str) -> String {
    let count = text.chars().count();
    if count <= RUN_HEAD_CHARS + RUN_TAIL_CHARS {
        return text.to_string();
    }
    format!(
        "{}\n[...]\n{}",
        review::head_chars(text, RUN_HEAD_CHARS),
        tail_chars(text, RUN_TAIL_CHARS)
    )
}

fn attribution_brief(task: &Task, failed: &VerifyOutcome, base: &VerifyOutcome) -> String {
    let worktree = Path::new(&task.worktree);
    let changed = crate::git::changes::changed_files(worktree, &task.base_sha)
        .unwrap_or_default()
        .join("\n");
    let stat = crate::git::changes::diff_stat(worktree, &task.base_sha).unwrap_or_default();
    let ask = task
        .request
        .iter()
        .chain(std::iter::once(&task.goal))
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join("\n\n");
    let criteria = task
        .criteria
        .iter()
        .map(|c| format!("- {c}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut out = format!(
        "{}\n\n{}{}{}{}{}{}{}{}",
        crate::prompts::get("attribution"),
        crate::prompts::text::untrusted_block("The task's request and goal", &ask),
        crate::prompts::text::untrusted_block("The acceptance criteria", &criteria),
        crate::prompts::text::untrusted_block("Changed files", &changed),
        crate::prompts::text::untrusted_block("Diffstat", &stat),
        crate::prompts::text::untrusted_block("The check command", &failed.command),
        crate::prompts::text::untrusted_block(
            "The task's run (head and tail)",
            &head_tail(&failed.tail)
        ),
        crate::prompts::text::untrusted_block(
            &format!("The base run at {}", short_sha(&task.base_sha)),
            &head_tail(&base.tail)
        ),
        task.attempts
            .iter()
            .rev()
            .filter(|a| a.stage == Stage::Implement)
            .nth(1)
            .and_then(|a| a.failure.as_ref())
            .map(|f| crate::prompts::text::untrusted_block(
                "The previous attempt's failure",
                &tail_chars(&f.detail, RUN_TAIL_CHARS)
            ))
            .unwrap_or_default(),
    );
    out.push('\n');
    out
}

/// The verdict of one attribution turn for `failed` against `base`, checked by
/// code. One turn per (task, diff, base, command, run, base run): a second call on the
/// same evidence gets the first verdict. `None` when no turn gave a valid
/// reply.
pub(super) async fn attribute(
    app: &Arc<App>,
    task: &mut Task,
    failed: &VerifyOutcome,
    base: &VerifyOutcome,
    cancel: &CancelToken,
) -> Option<Attribution> {
    let key = (
        task.id.clone(),
        diff_hash(task),
        task.base_sha.clone(),
        failed.command.clone(),
        run_key(failed),
        run_key(base),
    );
    if let Some(known) = app.attributions.lock().unwrap().get(&key) {
        return known.clone();
    }
    let verdict = run_turn(app, task, failed, base, cancel).await;
    if !cancel.is_cancelled() {
        app.attributions
            .lock()
            .unwrap()
            .insert(key, verdict.clone());
    }
    verdict
}

/// Whether the verdict of an attribution turn of this task, on its current
/// diff, for `command` says the task is meant to make the check pass.
pub(super) fn attributed_target(app: &App, task: &Task, command: &str) -> bool {
    let diff = diff_hash(task);
    app.attributions
        .lock()
        .unwrap()
        .iter()
        .any(|((id, d, sha, c, _, _), v)| {
            *id == task.id
                && *d == diff
                && *sha == task.base_sha
                && c == command
                && v.as_ref().is_some_and(|a| a.target)
        })
}

async fn run_turn(
    app: &Arc<App>,
    task: &mut Task,
    failed: &VerifyOutcome,
    base: &VerifyOutcome,
    cancel: &CancelToken,
) -> Option<Attribution> {
    let settings = app.settings.read().unwrap().clone();
    let attempt = task
        .attempts
        .iter()
        .rev()
        .find(|a| a.stage == Stage::Implement)?;
    let attempt_n = attempt.n;
    let implementer = settings
        .routes
        .iter()
        .find(|r| r.id == attempt.route_id)?
        .clone();
    // A route other than the implementer's when there is one; the session is
    // new either way.
    let route = resolve_judge_route(&settings, None, &implementer, &implementer, task.tier)
        .unwrap_or(&implementer)
        .clone();
    let worktree = PathBuf::from(&task.worktree);
    let run_dir = app
        .store
        .claim_helper_run_dir(&task.id, "attribution", Some(attempt_n))
        .ok()?;
    let mcp_path = run_dir.join("mcp.json");
    let _ = std::fs::write(&mcp_path, br#"{"mcpServers":{}}"#);
    let settings_path = run_dir.join("settings.json");
    let key_path = run_dir.join("key");
    let deny_read = vec![app.data_dir.to_string_lossy().to_string()];
    if matches!(route.harness, Harness::Claude) {
        write_readonly_claude_settings_with_profile(
            &route,
            app,
            &key_path,
            &settings,
            &deny_read,
            &settings_path,
        );
    }
    let brief_text = attribution_brief(task, failed, base);
    let _ = std::fs::write(run_dir.join("brief.md"), &brief_text);
    let req = harness::RunRequest {
        repo_settings: false,
        resume: None,
        ..review_request(&route, &worktree, &mcp_path, &settings_path, &[])
    };
    let limit = app.helper_timeout();
    let result = run_harness(
        app,
        &task.id,
        attempt_n,
        RunTrack::No,
        &worktree,
        &req,
        CostTag::task("attribution", &route.id),
        &brief_text,
        &run_dir.join("events.jsonl"),
        cancel,
        Guard::helper("attribution", limit),
    )
    .await;
    let _ = std::fs::remove_file(&key_path);
    let outcome = result.ok()?;
    if outcome.timed_out {
        task.record_decision(helper_timeout_message("attribution", limit));
    }
    task.charge(outcome.cost_usd.unwrap_or(0.0));
    if outcome.timed_out || outcome.error.is_some() {
        return None;
    }
    parse_attribution(&outcome.final_text.unwrap_or_default()).map(|a| checked(a, failed, base))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> VerifyOutcome {
        VerifyOutcome {
            command: "check".into(),
            code: Some(1),
            tail: "FAIL suite > alpha\nFAIL suite > beta\n".into(),
            ..Default::default()
        }
    }

    /// The task's run: the base's failures and nothing else.
    fn failed() -> VerifyOutcome {
        VerifyOutcome {
            command: "check".into(),
            code: Some(1),
            tail: "FAIL suite > alpha\nFAIL suite > beta\n".into(),
            ..Default::default()
        }
    }

    fn reply(cause: Blame, pre_existing: &[&str], evidence: &str) -> Attribution {
        Attribution {
            cause,
            new_failures: vec![],
            pre_existing: pre_existing.iter().map(|s| s.to_string()).collect(),
            target: false,
            evidence: evidence.into(),
        }
    }

    #[test]
    fn a_verdict_without_evidence_is_the_tasks() {
        for cause in [Blame::Inherited, Blame::Environment, Blame::Incomplete] {
            let got = checked(reply(cause, &["suite > alpha"], "  "), &failed(), &base());
            assert_eq!(got.cause, Blame::Introduced, "{cause:?}");
        }
    }

    #[test]
    fn inherited_needs_every_name_in_the_base_output() {
        let ok = checked(
            reply(Blame::Inherited, &["suite > alpha", "suite > beta"], "same"),
            &failed(),
            &base(),
        );
        assert_eq!(ok.cause, Blame::Inherited);
        let invented = checked(
            reply(
                Blame::Inherited,
                &["suite > alpha", "suite > gamma"],
                "same",
            ),
            &failed(),
            &base(),
        );
        assert_eq!(invented.cause, Blame::Introduced);
        let none = checked(reply(Blame::Inherited, &[], "same"), &failed(), &base());
        assert_eq!(none.cause, Blame::Introduced);
        let mut with_new = reply(Blame::Inherited, &["suite > alpha"], "same");
        with_new.new_failures = vec!["suite > gamma".into()];
        assert_eq!(
            checked(with_new, &failed(), &base()).cause,
            Blame::Introduced
        );
    }

    #[test]
    fn a_bare_marker_is_not_a_test_name() {
        let got = checked(
            reply(Blame::Inherited, &["FAIL"], "same"),
            &failed(),
            &base(),
        );
        assert_eq!(got.cause, Blame::Introduced);
    }

    #[test]
    fn a_new_error_next_to_a_base_failure_is_not_inherited() {
        let mut run = failed();
        run.tail = "error TS2322: type mismatch\nFAIL suite > alpha\n".into();
        run.output_key = "ts".into();
        let mut base = base();
        base.output_key = "plain".into();
        for name in ["suite > alpha", "FAIL"] {
            let got = checked(reply(Blame::Inherited, &[name], "same"), &run, &base);
            assert_eq!(got.cause, Blame::Introduced, "{name}");
        }
        // Even with the same names listed, a failing name the base lacks
        // outweighs the agent's list.
        run.failing = vec!["suite > alpha".into(), "suite > gamma".into()];
        base.failing = vec!["suite > alpha".into()];
        let got = checked(
            reply(Blame::Inherited, &["suite > alpha"], "same"),
            &run,
            &base,
        );
        assert_eq!(got.cause, Blame::Introduced);
        // The same names with a different output fingerprint: a new error.
        run.failing = vec!["suite > alpha".into()];
        let got = checked(
            reply(Blame::Inherited, &["suite > alpha"], "same"),
            &run,
            &base,
        );
        assert_eq!(got.cause, Blame::Introduced);
        run.output_key = base.output_key.clone();
        let got = checked(
            reply(Blame::Inherited, &["suite > alpha"], "same"),
            &run,
            &base,
        );
        assert_eq!(got.cause, Blame::Inherited);
    }

    #[test]
    fn a_crash_a_fail_fast_stop_or_a_lost_pass_is_not_inherited() {
        let mut base = base();
        base.failing = vec!["suite > alpha".into()];
        base.passing = vec!["suite > ok".into()];
        let mut run = failed();
        run.failing = vec!["suite > alpha".into()];
        run.passing = vec!["suite > ok".into()];
        let ask = |run: &VerifyOutcome| {
            checked(
                reply(Blame::Inherited, &["suite > alpha"], "same"),
                run,
                &base,
            )
            .cause
        };
        assert_eq!(ask(&run), Blame::Inherited);
        let mut crashed = run.clone();
        crashed.crashed = true;
        assert_eq!(ask(&crashed), Blame::Introduced);
        let mut early = run.clone();
        early.stopped_early = true;
        assert_eq!(ask(&early), Blame::Introduced);
        let mut lost = run.clone();
        lost.passing.clear();
        assert_eq!(ask(&lost), Blame::Introduced);
    }

    #[test]
    fn new_failures_never_grant_inherited() {
        let mut run = failed();
        run.tail = "error TS2322\n".into();
        run.output_key = "ts".into();
        let mut att = reply(Blame::Inherited, &["suite > alpha"], "same");
        att.new_failures = vec![];
        assert_eq!(checked(att, &run, &base()).cause, Blame::Introduced);
    }

    #[test]
    fn a_run_without_a_fingerprint_is_keyed_by_its_tail() {
        let mut a = failed();
        let mut b = failed();
        a.failing = vec!["x".into()];
        b.failing = vec!["x".into()];
        b.tail = "other".into();
        assert_ne!(run_key(&a), run_key(&b));
    }

    #[test]
    fn the_reply_is_strict() {
        let good =
            "```sushi-attribution\n{\"cause\":\"environment\",\"evidence\":\"timeout\"}\n```";
        assert_eq!(parse_attribution(good).unwrap().cause, Blame::Environment);
        for bad in [
            "{\"cause\":\"mine\",\"evidence\":\"x\"}",
            "{\"cause\":\"inherited\",\"waive\":true}",
            "{\"evidence\":\"x\"}",
            "no json",
        ] {
            assert!(parse_attribution(bad).is_none(), "{bad}");
        }
    }
}
