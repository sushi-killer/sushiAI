use super::*;

fn jev_belay_questions() -> Vec<classify::QuestionSpec> {
    vec![
        classify::QuestionSpec::Noul {
            name: "claims_done".to_string(),
            prompt: "Does the assistant's last message claim the work is done?".to_string(),
        },
        classify::QuestionSpec::Noul {
            name: "claims_verified".to_string(),
            prompt:
                "Does the message show the work was actually verified (tests run, checks passed)?"
                    .to_string(),
        },
        classify::QuestionSpec::Noul {
            name: "verification_applies".to_string(),
            prompt: "Does a verification step meaningfully apply to this task?".to_string(),
        },
        classify::QuestionSpec::Choice {
            name: "outcome".to_string(),
            prompt: "What outcome does the message report?".to_string(),
            options: vec!["complete", "partial", "blocked", "other"]
                .into_iter()
                .map(String::from)
                .collect(),
        },
    ]
}

fn to_jev_belay(answers: &classify::Answers) -> Option<hook::JevBelayAnswers> {
    let claims_done = answers.get("claims_done")?.noul?;
    let claims_verified = answers.get("claims_verified")?.noul?;
    let verification_applies = answers.get("verification_applies")?.noul?;
    let outcome = match answers.get("outcome").and_then(|a| a.choice.as_deref()) {
        Some("complete") => hook::ClassifiedOutcome::Complete,
        Some("partial") => hook::ClassifiedOutcome::Partial,
        Some("blocked") => hook::ClassifiedOutcome::Blocked,
        _ => hook::ClassifiedOutcome::Other,
    };
    Some(hook::JevBelayAnswers {
        claims_done,
        claims_verified,
        verification_applies,
        outcome,
    })
}

fn extract_last_assistant_message(payload: &serde_json::Value) -> Option<String> {
    if let Some(s) = payload
        .get("last_assistant_message")
        .and_then(|v| v.as_str())
    {
        return Some(s.to_string());
    }
    let transcript_path = payload.get("transcript_path").and_then(|v| v.as_str())?;
    let text = std::fs::read_to_string(transcript_path).ok()?;
    for line in text.lines().rev() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let role = v
            .get("role")
            .or_else(|| v.get("message").and_then(|m| m.get("role")))
            .and_then(|r| r.as_str());
        if role != Some("assistant") {
            continue;
        }
        let content = v
            .get("message")
            .and_then(|m| m.get("content"))
            .or_else(|| v.get("content"));
        if let Some(content) = content {
            if let Some(text) = content.as_str() {
                return Some(text.to_string());
            }
            if let Some(arr) = content.as_array() {
                let joined: String = arr
                    .iter()
                    .filter_map(|c| c.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n");
                if !joined.is_empty() {
                    return Some(joined);
                }
            }
        }
    }
    None
}

impl App {
    pub(super) async fn handle_hook_stop(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        struct P {
            token: String,
            #[serde(default)]
            payload: serde_json::Value,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        let ctx = { self.hook_tokens.read().unwrap().get(&p.token).cloned() };
        let Some(ctx) = ctx else {
            return Ok(json!({}));
        };

        ctx.hook_running.store(true, Ordering::SeqCst);
        let _hook_done = ClearOnDrop(&ctx.hook_running);
        let worktree = ctx.worktree.clone();
        let base_sha = ctx.base_sha.clone();
        let wt = worktree.clone();
        let base = base_sha.clone();
        let changed =
            tokio::task::spawn_blocking(move || git::changed_files(&wt, &base).unwrap_or_default())
                .await
                .unwrap_or_default();
        let has_changed = !changed.is_empty();
        let verify_configured = !ctx.verify.is_empty();

        let verify_results = if verify_configured && has_changed {
            let app = self.arc();
            let run_dir = self.store.run_dir(&ctx.task_id, ctx.attempt_n);

            // Budget well under Claude's own 600s hook timeout; on timeout,
            // fail open rather than block the agent forever.
            match tokio::time::timeout(
                Duration::from_secs(540),
                run_verify_cached(
                    &app,
                    &ctx.task_id,
                    &worktree,
                    &run_dir,
                    &base_sha,
                    &ctx.verify,
                    &ctx.cancel,
                ),
            )
            .await
            {
                Ok(results) => results,
                Err(_) => return Ok(json!({})),
            }
        } else {
            vec![]
        };

        let classifier_answers = if !verify_configured && has_changed {
            let last_msg = extract_last_assistant_message(&p.payload).unwrap_or_default();
            let wt2 = worktree.clone();
            let base2 = base_sha.clone();
            let diff_stat = tokio::task::spawn_blocking(move || {
                git::diff_stat(&wt2, &base2).unwrap_or_default()
            })
            .await
            .unwrap_or_default();
            let state = json!({"last_assistant_message": last_msg, "diff_stat": diff_stat});
            let questions = jev_belay_questions();
            let settings = self.settings.read().unwrap().classifier.clone();
            let key = self.secrets.read().unwrap().classifier_key.clone();
            let base_url = self.secrets.read().unwrap().classifier_base_url.clone();
            let start = std::time::Instant::now();
            let result = tokio::task::spawn_blocking(move || {
                classify::decide(
                    &settings,
                    key.as_deref(),
                    base_url.as_deref(),
                    &state,
                    &questions,
                )
            })
            .await
            .unwrap_or_else(|e| Err(classify::ClassifyError(e.to_string())));
            self.journal(&ctx.task_id, "stop_gate", &result, start.elapsed());
            result.ok().as_ref().and_then(to_jev_belay)
        } else {
            None
        };

        let facts = hook::StopFacts {
            blocks_so_far: ctx.blocks.load(Ordering::SeqCst),
            has_changed_files: has_changed,
            verify_configured,
            verify_results: &verify_results,
        };
        let decision = hook::decide_stop(&facts, classifier_answers.as_ref());
        if let Some(a) = classifier_answers.as_ref() {
            let blocked = matches!(decision, hook::StopDecision::Block { .. });
            let line = jev_stop_gate_line(blocked, a.claims_done, a.claims_verified);
            self.append_jev_decision(&ctx.task_id, line);
        }
        match decision {
            hook::StopDecision::Allow => Ok(json!({})),
            hook::StopDecision::Block { reason } => {
                ctx.blocks.fetch_add(1, Ordering::SeqCst);
                Ok(json!({"decision": "block", "reason": reason}))
            }
        }
    }
}
