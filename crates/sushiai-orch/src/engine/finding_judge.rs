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

/// The repeated findings the implementer disputed, each with its dispute. A
/// pair matches when the dispute names the finding's id, or when there is
/// exactly one repeated finding and exactly one dispute.
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
            let id = id_of(finding)?;
            let dispute = disputes.iter().find(|d| names_id(&d.finding, &id))?;
            Some((finding.clone(), dispute))
        })
        .collect()
}

/// Resolve the finding judge route. A configured named route is accepted
/// unless it is the reviewer or implementer; otherwise use the existing
/// automatic rule and return the reason the name was refused.
pub(super) fn resolve_judge_route<'a>(
    settings: &'a Settings,
    judge_id: Option<&str>,
    reviewer: &Route,
    implementer: &Route,
    tier: Tier,
) -> (Option<&'a Route>, Option<&'static str>) {
    let judge_id = judge_id.unwrap_or("auto");
    if judge_id != "auto" {
        let named = settings.routes.iter().find(|r| r.id == judge_id);
        let refusal = match named {
            None => Some("is not configured"),
            Some(route) if super::review::is_excluded(route, &[reviewer]) => {
                Some("is the reviewer")
            }
            Some(route) if super::review::is_excluded(route, &[implementer]) => {
                Some("is the implementer")
            }
            Some(route) => return (Some(route), None),
        };
        let floor = review_floor(implementer, tier);
        let auto = cheapest_route_at(settings, floor, &[reviewer, implementer], reviewer, false)
            .map(|(route, _)| route);
        return (auto, refusal);
    }
    let floor = review_floor(implementer, tier);
    let auto = cheapest_route_at(settings, floor, &[reviewer, implementer], reviewer, false)
        .map(|(route, _)| route);
    (auto, None)
}

fn refusal_decision(judge_id: &str, reason: &str) -> String {
    format!("Orchestrator: judge route {judge_id} refused ({reason}); using auto")
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
    let criteria = task
        .criteria
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
/// `attempt_n`. `Err` is
/// why there is no ruling.
#[allow(clippy::too_many_arguments)]
pub(super) async fn judge_finding(
    app: &Arc<App>,
    task: &mut Task,
    attempt_n: u32,
    finding: &str,
    dispute: &Dispute,
    route: &Route,
    cancel: &CancelToken,
) -> Result<FindingJudgement, String> {
    let settings = app.settings.read().unwrap().clone();
    let worktree = PathBuf::from(&task.worktree);
    let run_dir = app
        .store
        .claim_helper_run_dir(&task.id, "finding-judge", Some(attempt_n))
        .map_err(|e| e.to_string())?;
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
        resume: None,
        ..review_request(route, &worktree, &mcp_path, &settings_path, &images)
    };
    let limit = app.helper_timeout();
    let result = run_harness(
        app,
        &task.id,
        attempt_n,
        RunTrack::No,
        &worktree,
        &req,
        CostTag::task("finding_judge", &route.id),
        &brief_text,
        &events_path,
        cancel,
        Guard::helper("finding judge", limit),
    )
    .await;
    let _ = std::fs::remove_file(&key_path);
    let outcome = result.map_err(|_| "the judge run was stopped or did not start".to_string())?;
    if outcome.timed_out {
        task.decisions
            .push(helper_timeout_message("finding judge", limit));
    }
    task.cost_usd += outcome.cost_usd.unwrap_or(0.0);
    if outcome.timed_out {
        return Err("the judge run timed out".to_string());
    }
    if let Some(error) = outcome.error {
        return Err(format!("the judge errored: {}", head_chars(&error, 200)));
    }
    parse_finding_judgement(&outcome.final_text.unwrap_or_default())
        .ok_or_else(|| "the judge gave no finding_valid".to_string())
}

/// Judges each repeated finding of a FAIL that the attempt disputed and no
/// judge ruled on before; returns the (finding, why) pairs ruled invalid.
/// Each finding's id is saved in `task.judged_findings` before its judge
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
    fallback_rebuttal: Option<&str>,
    settings: &Settings,
    judge_id: Option<&str>,
    review_route: &Route,
    implementer: &Route,
    tier: Tier,
    cancel: &CancelToken,
) -> Vec<(String, String)> {
    let disputes = task.attempts[idx].disputes.clone();
    let mut pairs: Vec<(String, Dispute)> = pair_disputes(repeated, &disputes)
        .into_iter()
        .map(|(finding, dispute)| (finding, dispute.clone()))
        .collect();
    if let Some(rebuttal) = fallback_rebuttal {
        // A finding the attempt did not dispute is answered by the
        // implementer's own report.
        for finding in repeated {
            if !pairs.iter().any(|(f, _)| f == finding) {
                pairs.push((
                    finding.clone(),
                    Dispute {
                        finding: finding.clone(),
                        rebuttal: rebuttal.to_string(),
                        evidence: Vec::new(),
                    },
                ));
            }
        }
    }
    let mut dropped = Vec::new();
    let mut refusal_logged = false;
    for (finding, dispute) in pairs {
        let dispute = &dispute;
        let key = id_of(&finding).unwrap_or_else(|| finding.clone());
        if task.judged_findings.contains(&key) {
            continue;
        }
        task.judged_findings.push(key);
        task.updated_at = now_ms();
        app.save_task_logged(task);
        let (route, refusal) =
            resolve_judge_route(settings, judge_id, review_route, implementer, tier);
        if let Some(reason) = refusal.filter(|_| !refusal_logged) {
            task.decisions
                .push(refusal_decision(judge_id.unwrap_or("auto"), reason));
            refusal_logged = true;
            task.updated_at = now_ms();
            app.save_task_logged(task);
        }
        let Some(route) = route else {
            task.decisions.push(format!(
                "Orchestrator: disputed review finding could not be judged (no other route at strength {}): {finding}",
                review_floor(implementer, tier)
            ));
            continue;
        };
        let ruling = judge_finding(app, task, attempt_n, &finding, dispute, route, cancel).await;
        match ruling {
            Ok(j) if !j.finding_valid => {
                task.assumptions.push(Assumption {
                    question: format!("Review finding disputed by the implementer: {finding}"),
                    answer: "finding dropped".to_string(),
                    evidence: j.why.clone(),
                    by: "review-judge".to_string(),
                    kind: Some(QuestionKind::ReviewDispute),
                    attempt: Some(attempt_n),
                    commands: Vec::new(),
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
        app.save_task_logged(task);
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
    fn a_dispute_pairs_by_the_id_it_names() {
        let found = pairs(
            &[
                "P1: the loop skips the last item [F-1]",
                "P1: other thing [F-12]",
            ],
            &["F-12: wrong, it is fine", "unrelated words"],
        );
        assert_eq!(
            found,
            [(
                "P1: other thing [F-12]".to_string(),
                "F-12: wrong, it is fine".to_string()
            )]
        );
    }

    #[test]
    fn a_lone_pair_matches_and_unrelated_ones_do_not() {
        assert_eq!(pairs(&["alpha"], &["beta"]).len(), 1);
        assert!(pairs(&["alpha [F-1]", "gamma [F-2]"], &["beta"]).is_empty());
        assert!(pairs(&["alpha [F-1]"], &["beta", "delta"]).is_empty());
    }

    fn route(id: &str, harness: Harness) -> Route {
        Route {
            id: id.into(),
            label: id.into(),
            harness,
            model: None,
            effort: None,
            profile_id: None,
            account_id: None,
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
        // Standard work: the cheapest other route at strength 2 (codex has no
        // model, so its strength is unknown and it never qualifies).
        let judge = |reviewer: &Route, tier| {
            resolve_judge_route(&settings, None, reviewer, &sonnet, tier).0
        };
        assert_eq!(judge(&sonnet, Tier::Standard).unwrap().id, "claude-opus");
        // Hard work: only opus reaches 3, and it is not the judge when it reviews.
        assert_eq!(judge(&sonnet, Tier::Hard).unwrap().id, "claude-opus");
        assert!(judge(&opus, Tier::Hard).is_none());
        // A strong implementer raises the floor above the tier's.
        assert!(
            resolve_judge_route(&settings, None, &opus, &opus, Tier::Standard)
                .0
                .is_none()
        );
        assert_ne!(judge(&codex, Tier::Standard).unwrap().id, "codex");
    }

    #[test]
    fn finding_judge_never_returns_the_implementer_or_the_reviewer() {
        let with_model = |id: &str, harness, model: &str| Route {
            model: Some(model.into()),
            strength: Some(2),
            ..route(id, harness)
        };
        let implementer = with_model("impl", Harness::Claude, "m-impl");
        let twin = with_model("impl-copy", Harness::Claude, "m-impl");
        let reviewer = with_model("rev", Harness::Codex, "m-rev");
        let other = with_model("other", Harness::Claude, "m-other");
        let settings = Settings {
            routes: vec![
                implementer.clone(),
                twin.clone(),
                reviewer.clone(),
                other.clone(),
            ],
            ..Default::default()
        };
        let judge = resolve_judge_route(&settings, None, &reviewer, &implementer, Tier::Standard).0;
        assert_eq!(judge.unwrap().id, "other");
        let settings = Settings {
            routes: vec![implementer.clone(), twin, reviewer.clone()],
            ..Default::default()
        };
        assert!(
            resolve_judge_route(&settings, None, &reviewer, &implementer, Tier::Standard)
                .0
                .is_none()
        );
    }

    #[test]
    fn preset_judge_is_used_and_reviewer_conflicts_fall_back_to_auto() {
        let settings = Settings::default();
        let reviewer = settings
            .routes
            .iter()
            .find(|r| r.id == "claude-sonnet")
            .unwrap();
        let implementer = settings.routes.iter().find(|r| r.id == "codex").unwrap();
        let preset = PresetValues {
            judge: Some("claude-opus".into()),
            ..PresetValues::default()
        };
        let mut variant = Variant::default();
        preset.apply(&mut variant);
        let (route, refused) = resolve_judge_route(
            &settings,
            variant.judge_route.as_deref(),
            reviewer,
            implementer,
            Tier::Standard,
        );
        assert_eq!(route.unwrap().id, "claude-opus");
        assert_eq!(refused, None);

        let (route, refused) = resolve_judge_route(
            &settings,
            Some("claude-sonnet"),
            reviewer,
            implementer,
            Tier::Standard,
        );
        assert_eq!(refused, Some("is the reviewer"));
        assert_eq!(route.unwrap().id, "claude-opus");
        assert_eq!(
            refusal_decision("claude-sonnet", "is the reviewer"),
            "Orchestrator: judge route claude-sonnet refused (is the reviewer); using auto"
        );

        let (route, refused) = resolve_judge_route(
            &settings,
            Some("removed-route"),
            reviewer,
            implementer,
            Tier::Standard,
        );
        assert_eq!(refused, Some("is not configured"));
        assert_eq!(route.unwrap().id, "claude-opus");
    }

    #[test]
    fn judge_matching_implementer_by_harness_and_model_falls_back_to_auto() {
        let named = Route {
            model: Some("m-impl".into()),
            strength: Some(2),
            ..route("named-copy", Harness::Claude)
        };
        let implementer = Route {
            id: "impl".into(),
            model: Some("m-impl".into()),
            ..named.clone()
        };
        let reviewer = Route {
            model: Some("m-review".into()),
            ..route("reviewer", Harness::Codex)
        };
        let auto = strong(route("auto", Harness::Claude), 2);
        let settings = Settings {
            routes: vec![named, implementer.clone(), reviewer.clone(), auto.clone()],
            ..Default::default()
        };
        let (route, refused) = resolve_judge_route(
            &settings,
            Some("named-copy"),
            &reviewer,
            &implementer,
            Tier::Standard,
        );
        assert_eq!(refused, Some("is the implementer"));
        assert_eq!(route.unwrap().id, auto.id);
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
            resolve_judge_route(&settings, None, &reviewer, &reviewer, Tier::Standard)
                .0
                .unwrap()
                .id,
            "x1"
        );
        let reviewer = route("x1", Harness::Codex);
        assert_eq!(
            resolve_judge_route(&settings, None, &reviewer, &reviewer, Tier::Standard)
                .0
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

#[cfg(test)]
mod timeout_tests {
    use super::*;

    #[tokio::test]
    async fn finding_judge_timed_out_keeps_the_finding() {
        let (app, _dir, mut task, route) = slow_helper_task();
        let finding = "P1: README.md:1 - missing guard".to_string();
        task.attempts[0].disputes = vec![Dispute {
            finding: finding.clone(),
            rebuttal: "Already handled".into(),
            evidence: vec![],
        }];
        let settings = app.settings.read().unwrap().clone();
        let mut implementer = route.clone();
        implementer.id = "implementer".into();
        implementer.model = Some("other".into());
        let dropped = judge_disputed_findings(
            &app,
            &mut task,
            0,
            1,
            &[finding],
            None,
            &settings,
            None,
            &route,
            &implementer,
            Tier::Mechanical,
            &CancelToken::new(),
        )
        .await;
        assert!(dropped.is_empty());
        assert!(task.assumptions.is_empty());
        assert_helper_timeout(&task, "finding judge");
    }
}
