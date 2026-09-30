use super::*;

const JUDGE_PROMPT: &str = "Rule on one review finding that the implementer disputes. You may read files in the working directory but change nothing. Check the finding, the criteria and the rebuttal against the repository yourself. Answer finding_valid true when the finding is a real defect the implementer still has to fix; false when it is wrong, already fixed, or not something the criteria ask for.";

const JUDGE_REPLY: &str = "Reply with only:\n```sushi-judge\n{\"finding_valid\": true|false, \"why\": \"<one sentence>\"}\n```";

const MAX_WHY_CHARS: usize = 300;

/// The judge's ruling on a disputed finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FindingJudgement {
    pub finding_valid: bool,
    pub why: String,
}

/// How a finding is compared and remembered: lowercase, digits and paths
/// stripped, whitespace collapsed.
pub(super) fn finding_key(text: &str) -> String {
    normalize_signature_line(&text.to_lowercase())
}

/// The repeated findings the implementer disputed, each with its dispute. A
/// pair matches when the keys are equal or one contains the other, or when
/// there is exactly one repeated finding and exactly one dispute.
pub(super) fn pair_disputes<'a>(
    repeated: &[String],
    disputes: &'a [Dispute],
) -> Vec<(String, &'a Dispute)> {
    if let ([finding], [dispute]) = (repeated, disputes) {
        return vec![(finding.clone(), dispute)];
    }
    repeated
        .iter()
        .filter_map(|finding| {
            let key = finding_key(finding);
            let dispute = disputes.iter().find(|d| {
                let other = finding_key(&d.finding);
                !key.is_empty()
                    && !other.is_empty()
                    && (key == other || key.contains(&other) || other.contains(&key))
            })?;
            Some((finding.clone(), dispute))
        })
        .collect()
}

/// The cheapest route no weaker than the implementer's floor (see
/// `review_floor`), never the reviewer's own; on a tie, one on another
/// harness than the reviewer's.
pub(super) fn judge_route<'a>(
    settings: &'a Settings,
    reviewer: &Route,
    implementer: &Route,
    tier: Tier,
) -> Option<&'a Route> {
    let floor = review_floor(implementer, tier);
    cheapest_route_at(settings, floor, Some(&reviewer.id), reviewer, None, false)
        .map(|(route, _)| route)
}

/// The judge's reply: the last ```sushi-judge fence, else the outermost
/// `{...}`. `None` without a `finding_valid`.
pub(super) fn parse_finding_judgement(text: &str) -> Option<FindingJudgement> {
    let v: serde_json::Value =
        serde_json::from_str(&brief::tagged_json(text, "sushi-judge")?).ok()?;
    let finding_valid = match v.get("finding_valid")? {
        serde_json::Value::Bool(b) => *b,
        serde_json::Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "true" => true,
            "false" => false,
            _ => return None,
        },
        _ => return None,
    };
    let why = v
        .get("why")
        .and_then(|w| w.as_str())
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    Some(FindingJudgement {
        finding_valid,
        why: truncate_chars(&why, MAX_WHY_CHARS),
    })
}

/// The criteria whose text the finding quotes, else all of them.
pub(super) fn criterion_for(task: &Task, finding: &str) -> Vec<String> {
    let key = finding_key(finding);
    let quoted: Vec<String> = task
        .criteria
        .iter()
        .filter(|c| {
            let c = finding_key(c);
            !c.is_empty() && key.contains(&c)
        })
        .cloned()
        .collect();
    if quoted.is_empty() {
        task.criteria.clone()
    } else {
        quoted
    }
}

/// The image a piece of dispute evidence names, when it exists under the
/// worktree's `artifacts/`.
fn artifact_image(worktree: &Path, entry: &str) -> Option<PathBuf> {
    let entry = entry.trim().trim_matches('`');
    let path = Path::new(entry);
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        worktree.join(path)
    };
    let real = path.canonicalize().ok()?;
    let root = worktree.join("artifacts").canonicalize().ok()?;
    (real.starts_with(&root) && real.is_file() && crate::model::image_mime(&real).is_some())
        .then_some(real)
}

/// Splits a dispute's evidence into the text shown to the judge and the
/// images attached to it. Image paths are never shown as text.
fn split_evidence(worktree: &Path, evidence: &[String]) -> (Vec<String>, Vec<PathBuf>) {
    let mut text = Vec::new();
    let mut images = Vec::new();
    for entry in evidence {
        if let Some(image) = artifact_image(worktree, entry) {
            images.push(image);
        } else if crate::model::image_mime(Path::new(entry.trim().trim_matches('`'))).is_none() {
            text.push(entry.clone());
        }
    }
    (text, images)
}

fn judge_brief(task: &Task, finding: &str, dispute: &Dispute, evidence: &[String]) -> String {
    let criteria = criterion_for(task, finding)
        .iter()
        .map(|c| format!("- {c}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut rebuttal = dispute.rebuttal.clone();
    if !evidence.is_empty() {
        rebuttal.push_str("\n\nEvidence:\n");
        rebuttal.push_str(&evidence.join("\n"));
    }
    format!(
        "{JUDGE_PROMPT}\n\n{}\n{}\n{}\n{JUDGE_REPLY}\n",
        brief::untrusted_block("The review finding", finding),
        brief::untrusted_block("The acceptance criteria it concerns", &criteria),
        brief::untrusted_block("The implementer's rebuttal", &rebuttal),
    )
}

/// Runs a read-only judge on `route` over one disputed finding of attempt
/// `attempt_n` (`seq` = 1 for the first judge run of the attempt). `Err` is
/// why there is no ruling.
#[allow(clippy::too_many_arguments)]
pub(super) async fn judge_finding(
    app: &Arc<App>,
    task: &mut Task,
    attempt_n: u32,
    seq: u32,
    finding: &str,
    dispute: &Dispute,
    route: &Route,
    cancel: &CancelToken,
) -> Result<FindingJudgement, String> {
    let settings = app.settings.read().unwrap().clone();
    let worktree = PathBuf::from(&task.worktree);
    let run_dir = app.store.run_dir(&task.id, attempt_n).join(if seq <= 1 {
        "finding-judge".to_string()
    } else {
        format!("finding-judge-{seq}")
    });
    let _ = std::fs::create_dir_all(&run_dir);
    let mcp_path = run_dir.join("mcp.json");
    let _ = std::fs::write(&mcp_path, br#"{"mcpServers":{}}"#);
    let settings_path = run_dir.join("settings.json");
    let key_path = run_dir.join("key");
    let deny_read = vec![app.data_dir.to_string_lossy().to_string()];
    if matches!(route.harness, Harness::Claude) {
        write_readonly_claude_settings_with_profile(
            route,
            app,
            &key_path,
            &settings,
            &deny_read,
            &settings_path,
        );
    }
    let (evidence_text, dispute_images) = split_evidence(&worktree, &dispute.evidence);
    let mut images: Vec<PathBuf> = task
        .attempts
        .iter()
        .find(|a| a.n == attempt_n && a.stage == Stage::Implement)
        .map(|a| a.evidence.iter().map(PathBuf::from).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .filter(|p| p.is_file())
        .collect();
    for image in dispute_images {
        if !images.contains(&image) {
            images.push(image);
        }
    }
    let brief_text = judge_brief(task, finding, dispute, &evidence_text);
    let _ = std::fs::write(run_dir.join("brief.md"), &brief_text);
    let events_path = run_dir.join("events.jsonl");
    let req = harness::RunRequest {
        repo_settings: false,
        ..review_request(route, &worktree, &mcp_path, &settings_path, &images)
    };
    let result = run_harness(
        app,
        &task.id,
        attempt_n,
        false,
        &worktree,
        &req,
        CostTag::task("finding_judge", &route.id),
        &brief_text,
        &events_path,
        cancel,
        None,
        None,
    )
    .await;
    let _ = std::fs::remove_file(&key_path);
    let outcome = result.map_err(|_| "the judge run was stopped or did not start".to_string())?;
    task.cost_usd += outcome.cost_usd.unwrap_or(0.0);
    if let Some(error) = outcome.error {
        return Err(format!("the judge errored: {}", head_chars(&error, 200)));
    }
    parse_finding_judgement(&outcome.final_text.unwrap_or_default())
        .ok_or_else(|| "the judge gave no finding_valid".to_string())
}

/// Judges each repeated finding of a FAIL that the attempt disputed and no
/// judge ruled on before; returns the (finding, why) pairs ruled invalid.
/// Each finding's key is saved in `task.judged_findings` before its judge
/// runs, so it is judged at most once, even across a restart. Every outcome
/// leaves a decision line; a finding not dropped goes on to the existing
/// escalation. None of this reads `settings.answerPolicy`.
#[allow(clippy::too_many_arguments)]
pub(super) async fn judge_disputed_findings(
    app: &Arc<App>,
    task: &mut Task,
    idx: usize,
    attempt_n: u32,
    repeated: &[String],
    settings: &Settings,
    review_route: &Route,
    implementer: &Route,
    tier: Tier,
    judge_runs: &mut u32,
    cancel: &CancelToken,
) -> Vec<(String, String)> {
    let disputes = task.attempts[idx].disputes.clone();
    let mut dropped = Vec::new();
    for (finding, dispute) in pair_disputes(repeated, &disputes) {
        let key = finding_key(&finding);
        if task.judged_findings.contains(&key) {
            continue;
        }
        task.judged_findings.push(key);
        task.updated_at = now_ms();
        let _ = app.store.save_task(task);
        let Some(route) = judge_route(settings, review_route, implementer, tier) else {
            task.decisions.push(format!(
                "Orchestrator: disputed review finding could not be judged (no other route at strength {}): {finding}",
                review_floor(implementer, tier)
            ));
            continue;
        };
        *judge_runs += 1;
        let ruling = judge_finding(
            app,
            task,
            attempt_n,
            *judge_runs,
            &finding,
            dispute,
            route,
            cancel,
        )
        .await;
        match ruling {
            Ok(j) if !j.finding_valid => {
                task.assumptions.push(Assumption {
                    question: format!("Review finding disputed by the implementer: {finding}"),
                    answer: "finding dropped".to_string(),
                    evidence: j.why.clone(),
                    by: "review-judge".to_string(),
                    kind: Some(QuestionKind::ReviewDispute),
                    attempt: Some(attempt_n),
                    overturned: false,
                    owner_answer: None,
                });
                task.decisions.push(format!(
                    "Orchestrator: disputed review finding dropped by the judge ({}) for attempt {attempt_n}: {finding} -- {}",
                    route.id, j.why
                ));
                dropped.push((finding, j.why));
            }
            Ok(j) => task.decisions.push(format!(
                "Orchestrator: disputed review finding kept by the judge ({}): {finding} -- {}",
                route.id, j.why
            )),
            Err(reason) => task.decisions.push(format!(
                "Orchestrator: disputed review finding could not be judged ({reason}): {finding}"
            )),
        }
        task.updated_at = now_ms();
        let _ = app.store.save_task(task);
    }
    dropped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dispute(finding: &str) -> Dispute {
        Dispute {
            finding: finding.into(),
            rebuttal: "wrong".into(),
            evidence: vec![],
        }
    }

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|i| i.to_string()).collect()
    }

    /// The (repeated finding, dispute finding) pairs.
    fn pairs(repeated: &[&str], disputes: &[&str]) -> Vec<(String, String)> {
        let disputes: Vec<Dispute> = disputes.iter().map(|d| dispute(d)).collect();
        pair_disputes(&s(repeated), &disputes)
            .into_iter()
            .map(|(f, d)| (f, d.finding.clone()))
            .collect()
    }

    #[test]
    fn equal_or_containing_texts_pair() {
        let found = pairs(
            &[
                "P1 Src/A.rs:12: the loop skips the last item",
                "other thing",
            ],
            &["the loop skips the last item", "unrelated words"],
        );
        assert_eq!(
            found,
            [(
                "P1 Src/A.rs:12: the loop skips the last item".to_string(),
                "the loop skips the last item".to_string()
            )]
        );
        assert_eq!(
            pairs(&["short", "z"], &["A SHORT finding, quoted", "y"]).len(),
            1
        );
    }

    #[test]
    fn a_lone_pair_matches_and_unrelated_ones_do_not() {
        assert_eq!(pairs(&["alpha"], &["beta"]).len(), 1);
        assert!(pairs(&["alpha", "gamma"], &["beta"]).is_empty());
        assert!(pairs(&["alpha"], &["beta", "delta"]).is_empty());
    }

    fn route(id: &str, harness: Harness) -> Route {
        Route {
            id: id.into(),
            label: id.into(),
            harness,
            model: None,
            effort: None,
            profile_id: None,
            strength: None,
        }
    }

    fn strong(mut r: Route, strength: u32) -> Route {
        r.strength = Some(strength);
        r
    }

    #[test]
    fn the_judge_is_never_weaker_than_the_floor_nor_the_reviewer() {
        let settings = Settings::default();
        let by_id = |id: &str| settings.routes.iter().find(|r| r.id == id).unwrap().clone();
        let (sonnet, opus, codex) = (by_id("claude-sonnet"), by_id("claude-opus"), by_id("codex"));
        // Standard work: the cheapest other route at strength 2 (codex is
        // unpriced, so it sorts after priced opus).
        let judge = |reviewer: &Route, tier| judge_route(&settings, reviewer, &sonnet, tier);
        assert_eq!(judge(&sonnet, Tier::Standard).unwrap().id, "claude-opus");
        // Hard work: only opus reaches 3, and it is not the judge when it reviews.
        assert_eq!(judge(&sonnet, Tier::Hard).unwrap().id, "claude-opus");
        assert!(judge(&opus, Tier::Hard).is_none());
        // A strong implementer raises the floor above the tier's.
        assert!(judge_route(&settings, &opus, &opus, Tier::Standard).is_none());
        assert_ne!(judge(&codex, Tier::Standard).unwrap().id, "codex");
    }

    #[test]
    fn a_judge_tie_prefers_another_harness_than_the_reviewer() {
        let settings = Settings {
            routes: vec![
                strong(route("c1", Harness::Claude), 2),
                strong(route("c2", Harness::Claude), 2),
                strong(route("x1", Harness::Codex), 2),
            ],
            ..Default::default()
        };
        let reviewer = route("c1", Harness::Claude);
        assert_eq!(
            judge_route(&settings, &reviewer, &reviewer, Tier::Standard)
                .unwrap()
                .id,
            "x1"
        );
        let reviewer = route("x1", Harness::Codex);
        assert_eq!(
            judge_route(&settings, &reviewer, &reviewer, Tier::Standard)
                .unwrap()
                .id,
            "c1"
        );
    }

    #[test]
    fn a_judgement_reads_fenced_or_bare_json() {
        let fenced = "text\n```sushi-judge\n{\"finding_valid\": false, \"why\": \"a\\n  b\"}\n```";
        assert_eq!(
            parse_finding_judgement(fenced),
            Some(FindingJudgement {
                finding_valid: false,
                why: "a b".into()
            })
        );
        let bare = "{\"finding_valid\": \"TRUE\", \"why\": \"real\"}";
        assert!(parse_finding_judgement(bare).unwrap().finding_valid);
        let last = "```sushi-judge\n{\"finding_valid\": true}\n```\n```sushi-judge\n{\"finding_valid\": \"false\"}\n```";
        assert!(!parse_finding_judgement(last).unwrap().finding_valid);
        assert!(parse_finding_judgement("{\"why\": \"x\"}").is_none());
        assert!(parse_finding_judgement("{\"finding_valid\": \"maybe\"}").is_none());
        assert!(parse_finding_judgement("no idea").is_none());
        let long = format!(
            "{{\"finding_valid\": true, \"why\": \"{}\"}}",
            "w ".repeat(400)
        );
        assert!(parse_finding_judgement(&long).unwrap().why.chars().count() <= MAX_WHY_CHARS + 1);
    }

    #[test]
    fn a_finding_quoting_a_criterion_selects_it() {
        let mut task = crate::engine::test_support::task_with_status(TaskStatus::Running);
        task.criteria = s(&["Docs are updated", "The parser rejects empty input"]);
        assert_eq!(
            criterion_for(&task, "criterion fails: the parser rejects empty input"),
            s(&["The parser rejects empty input"])
        );
        assert_eq!(criterion_for(&task, "something else"), task.criteria);
    }

    #[test]
    fn only_artifact_images_are_attached() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path().join("wt");
        std::fs::create_dir_all(wt.join("artifacts")).unwrap();
        std::fs::write(wt.join("artifacts/a.png"), b"x").unwrap();
        std::fs::write(wt.join("outside.png"), b"x").unwrap();
        let evidence = s(&[
            "artifacts/a.png",
            "outside.png",
            "../secret.png",
            "src/a.rs:4",
            "cargo test: ok",
        ]);
        let (text, images) = split_evidence(&wt, &evidence);
        assert_eq!(images.len(), 1);
        assert!(images[0].ends_with("artifacts/a.png"));
        assert_eq!(text, s(&["src/a.rs:4", "cargo test: ok"]));
    }
}
