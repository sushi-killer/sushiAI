use super::*;

/// The route of the judge stage: the configured judge when it is usable, else
/// the automatic one. A configured route is refused when it is the reviewer
/// or the implementer. Review findings no longer have a judge turn: the next
/// review rules on each decline (`review_findings::apply_declines`).
pub(super) fn resolve_judge_route<'a>(
    settings: &'a Settings,
    judge_id: Option<&str>,
    reviewer: &Route,
    implementer: &Route,
    tier: Tier,
) -> Option<&'a Route> {
    let judge_id = judge_id.unwrap_or("auto");
    if judge_id != "auto" {
        let named = settings.routes.iter().find(|r| r.id == judge_id);
        if let Some(route) = named.filter(|route| {
            !super::review::is_excluded(route, &[reviewer])
                && !super::review::is_excluded(route, &[implementer])
        }) {
            return Some(route);
        }
    }
    let floor = review_floor(implementer, tier);
    cheapest_route_at(settings, floor, &[reviewer, implementer], reviewer, false)
        .map(|(route, _)| route)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let judge =
            |reviewer: &Route, tier| resolve_judge_route(&settings, None, reviewer, &sonnet, tier);
        assert_eq!(judge(&sonnet, Tier::Standard).unwrap().id, "claude-opus");
        // Hard work: only opus reaches 3, and it is not the judge when it reviews.
        assert_eq!(judge(&sonnet, Tier::Hard).unwrap().id, "claude-opus");
        assert!(judge(&opus, Tier::Hard).is_none());
        // A strong implementer raises the floor above the tier's.
        assert!(resolve_judge_route(&settings, None, &opus, &opus, Tier::Standard).is_none());
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
        let judge = resolve_judge_route(&settings, None, &reviewer, &implementer, Tier::Standard);
        assert_eq!(judge.unwrap().id, "other");
        let settings = Settings {
            routes: vec![implementer.clone(), twin, reviewer.clone()],
            ..Default::default()
        };
        assert!(
            resolve_judge_route(&settings, None, &reviewer, &implementer, Tier::Standard).is_none()
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
        let route = resolve_judge_route(
            &settings,
            variant.judge_route.as_deref(),
            reviewer,
            implementer,
            Tier::Standard,
        );
        assert_eq!(route.unwrap().id, "claude-opus");

        let route = resolve_judge_route(
            &settings,
            Some("claude-sonnet"),
            reviewer,
            implementer,
            Tier::Standard,
        );
        assert_eq!(route.unwrap().id, "claude-opus");

        let route = resolve_judge_route(
            &settings,
            Some("removed-route"),
            reviewer,
            implementer,
            Tier::Standard,
        );
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
        let route = resolve_judge_route(
            &settings,
            Some("named-copy"),
            &reviewer,
            &implementer,
            Tier::Standard,
        );
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
                .unwrap()
                .id,
            "x1"
        );
        let reviewer = route("x1", Harness::Codex);
        assert_eq!(
            resolve_judge_route(&settings, None, &reviewer, &reviewer, Tier::Standard)
                .unwrap()
                .id,
            "c1"
        );
    }
}
