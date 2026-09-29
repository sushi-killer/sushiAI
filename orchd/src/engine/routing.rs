use super::*;

/// `overrides` is a partial `Variant` object; its keys replace the defaults'.
pub(super) fn resolve_variant(
    defaults: &Variant,
    overrides: Option<&serde_json::Value>,
) -> Result<Variant, String> {
    let Some(overrides) = overrides else {
        return Ok(defaults.clone());
    };
    let Some(fields) = overrides.as_object() else {
        return Err("variant must be an object".to_string());
    };
    let mut merged = serde_json::to_value(defaults).map_err(|e| e.to_string())?;
    for (k, v) in fields {
        if Variant::RETIRED_KEYS.contains(&k.as_str()) {
            return Err(format!(
                "variant flag {k} was retired: its winning behaviour is now the default (or it was removed), so drop it from the override"
            ));
        }
        // Checked here, not with deny_unknown_fields: a task.json keeps
        // loading after a flag it names is retired.
        if merged.get(k).is_none() && !Variant::OPTIONAL_KEYS.contains(&k.as_str()) {
            return Err(format!("unknown variant flag: {k}"));
        }
        merged[k] = v.clone();
    }
    let variant: Variant =
        serde_json::from_value(merged).map_err(|e| format!("invalid variant: {e}"))?;
    variant.check()?;
    Ok(variant)
}

/// Either a classified tier (with the raw `choice`/`p` for the caller to
/// turn into a `task.decisions` line once it knows the resolved route), or
/// a fallback to `Tier::Standard` with a reason from the fixed set `pick_tier`
/// documents -- this type doesn't persist anything itself so the caller can
/// push a line onto the same in-memory `Task` it's about to save (see
/// `append_jev_decision` for why a call site with no live `Task` in scope
/// has to do it differently).
pub(super) enum TierPick {
    Classified { tier: Tier, choice: String, p: f64 },
    Fallback { reason: String },
}

/// Turns a classifier call outcome into a tier decision. Pure and
/// unit-testable: no network, no store, no app state. `reason` is always
/// one of `classifier off`, `no classifier key`, `classifier call failed`,
/// `no tier answer`, or `unsure: <choice> p <p:.2>` -- never the raw
/// classifier error text, a key, a URL, or a response body.
pub(super) fn pick_tier(
    result: &Result<classify::Answers, classify::ClassifyError>,
    backend_off: bool,
) -> TierPick {
    if backend_off {
        return TierPick::Fallback {
            reason: "classifier off".to_string(),
        };
    }
    let answers = match result {
        Err(e) if e.0 == classify::NO_KEY_REASON => {
            return TierPick::Fallback {
                reason: "no classifier key".to_string(),
            };
        }
        Err(_) => {
            return TierPick::Fallback {
                reason: "classifier call failed".to_string(),
            };
        }
        Ok(answers) => answers,
    };
    let Some(a) = answers.get("tier") else {
        return TierPick::Fallback {
            reason: "no tier answer".to_string(),
        };
    };
    let Some(choice) = &a.choice else {
        return TierPick::Fallback {
            reason: "no tier answer".to_string(),
        };
    };
    let p = a
        .probabilities
        .as_ref()
        .and_then(|p| p.get(choice))
        .copied()
        .unwrap_or(1.0);
    if p < 0.5 {
        return TierPick::Fallback {
            reason: format!("unsure: {choice} p {p:.2}"),
        };
    }
    let tier = match choice.as_str() {
        "mechanical" => Tier::Mechanical,
        "hard" => Tier::Hard,
        _ => Tier::Standard,
    };
    TierPick::Classified {
        tier,
        choice: choice.clone(),
        p,
    }
}

pub(super) async fn classify_tier(app: &Arc<App>, task: &Task) -> TierPick {
    let settings = app.settings.read().unwrap().classifier.clone();
    let key = app.secrets.read().unwrap().classifier_key.clone();
    let base_url = app.secrets.read().unwrap().classifier_base_url.clone();
    let backend_off = settings.backend == ClassifierBackend::None;
    let state = json!({"goal": task.goal, "criteria": task.criteria});
    let questions = vec![classify::QuestionSpec::Choice {
        name: "tier".to_string(),
        prompt: "How hard is this task: mechanical, standard, or hard?".to_string(),
        options: vec!["mechanical".into(), "standard".into(), "hard".into()],
    }];
    let start = std::time::Instant::now();
    let s2 = settings.clone();
    let k2 = key.clone();
    let b2 = base_url.clone();
    let q2 = questions.clone();
    let state2 = state.clone();
    let result = tokio::task::spawn_blocking(move || {
        classify::decide(&s2, k2.as_deref(), b2.as_deref(), &state2, &q2)
    })
    .await
    .unwrap_or_else(|e| Err(classify::ClassifyError(e.to_string())));
    app.journal(&task.id, "tier", &result, start.elapsed());
    pick_tier(&result, backend_off)
}
