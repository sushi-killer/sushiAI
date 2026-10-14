//! The "whose failure" turn: a check that fails on the task's tree and on its
//! base, and that the deterministic comparison cannot settle, is read by one
//! read-only agent turn in a session of its own. Code keeps the guards: no
//! evidence means the failure is the task's, and a verdict that lowers
//! caution holds only when the test names it gives appear in the base output.

use super::*;
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

/// Base sha, command, run key and base run key.
pub(super) type AttributionKey = (String, String, String, String);

/// The reply's last `sushi-attribution` block, else its outermost `{...}`.
pub(super) fn parse_attribution(text: &str) -> Option<Attribution> {
    serde_json::from_str(&brief::tagged_json(text, "sushi-attribution")?).ok()
}

/// Whether `name` is in the base run's output, literally.
fn in_base_output(base: &VerifyOutcome, name: &str) -> bool {
    let name = name.trim();
    !name.is_empty() && (base.tail.contains(name) || base.failing.iter().any(|f| f == name))
}

/// The verdict code accepts. A cause other than `introduced` needs evidence;
/// `inherited` also needs a non-empty `pre_existing`, no `new_failures`, and
/// every name in `pre_existing` literally in the base run. Anything else is
/// `introduced`.
pub(super) fn checked(mut att: Attribution, base: &VerifyOutcome) -> Attribution {
    let evidenced = !att.evidence.trim().is_empty();
    let inherited_holds = !att.pre_existing.is_empty()
        && att.new_failures.is_empty()
        && att.pre_existing.iter().all(|n| in_base_output(base, n));
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
/// tail when it has neither.
fn run_key(run: &VerifyOutcome) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    run.output_key.hash(&mut h);
    run.failing.hash(&mut h);
    if run.output_key.is_empty() && run.failing.is_empty() {
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
    let changed = git::changed_files(worktree, &task.base_sha)
        .unwrap_or_default()
        .join("\n");
    let stat = git::diff_stat(worktree, &task.base_sha).unwrap_or_default();
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
        brief::untrusted_block("The task's request and goal", &ask),
        brief::untrusted_block("The acceptance criteria", &criteria),
        brief::untrusted_block("Changed files", &changed),
        brief::untrusted_block("Diffstat", &stat),
        brief::untrusted_block("The check command", &failed.command),
        brief::untrusted_block("The task's run (head and tail)", &head_tail(&failed.tail)),
        brief::untrusted_block(
            &format!("The base run at {}", short_sha(&task.base_sha)),
            &head_tail(&base.tail)
        ),
        task.attempts
            .iter()
            .rev()
            .filter(|a| a.stage == Stage::Implement)
            .nth(1)
            .and_then(|a| a.failure.as_ref())
            .map(|f| brief::untrusted_block(
                "The previous attempt's failure",
                &tail_chars(&f.detail, RUN_TAIL_CHARS)
            ))
            .unwrap_or_default(),
    );
    out.push('\n');
    out
}

/// The verdict of one attribution turn for `failed` against `base`, checked by
/// code. One turn per (base, command, run, base run): a second call on the
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

/// Whether the verdict of an attribution turn for `command` on this base says
/// the task is meant to make the check pass.
pub(super) fn attributed_target(app: &App, base_sha: &str, command: &str) -> bool {
    app.attributions
        .lock()
        .unwrap()
        .iter()
        .any(|((sha, c, _, _), v)| {
            sha == base_sha && c == command && v.as_ref().is_some_and(|a| a.target)
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
        .0
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
        task.decisions
            .push(helper_timeout_message("attribution", limit));
    }
    task.cost_usd += outcome.cost_usd.unwrap_or(0.0);
    if outcome.timed_out || outcome.error.is_some() {
        return None;
    }
    parse_attribution(&outcome.final_text.unwrap_or_default()).map(|a| checked(a, base))
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
            let got = checked(reply(cause, &["suite > alpha"], "  "), &base());
            assert_eq!(got.cause, Blame::Introduced, "{cause:?}");
        }
    }

    #[test]
    fn inherited_needs_every_name_in_the_base_output() {
        let ok = checked(
            reply(Blame::Inherited, &["suite > alpha", "suite > beta"], "same"),
            &base(),
        );
        assert_eq!(ok.cause, Blame::Inherited);
        let invented = checked(
            reply(
                Blame::Inherited,
                &["suite > alpha", "suite > gamma"],
                "same",
            ),
            &base(),
        );
        assert_eq!(invented.cause, Blame::Introduced);
        let none = checked(reply(Blame::Inherited, &[], "same"), &base());
        assert_eq!(none.cause, Blame::Introduced);
        let mut with_new = reply(Blame::Inherited, &["suite > alpha"], "same");
        with_new.new_failures = vec!["suite > gamma".into()];
        assert_eq!(checked(with_new, &base()).cause, Blame::Introduced);
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
