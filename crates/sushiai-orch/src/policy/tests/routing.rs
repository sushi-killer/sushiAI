use crate::policy::routing::Tier;
use crate::policy::settings::Settings;

#[test]
fn pr_draft_route_prefers_pr_route_then_mechanical_then_planner() {
    let mut s = Settings::default();
    s.tiers.remove(&Tier::Mechanical);
    let ids: Vec<String> = s.routes.iter().map(|r| r.id.clone()).collect();
    assert!(ids.len() >= 2);
    assert_eq!(
        s.pr_draft_route().map(|r| r.id.as_str()),
        Some(s.planner.as_str())
    );
    s.tiers.insert(Tier::Mechanical, ids[1].clone());
    assert_eq!(s.pr_draft_route().unwrap().id, ids[1]);
    s.pr_route = ids[0].clone();
    assert_eq!(s.pr_draft_route().unwrap().id, ids[0]);
}

#[test]
fn tier_up_caps_at_hard() {
    assert_eq!(Tier::Mechanical.up(), Tier::Standard);
    assert_eq!(Tier::Standard.up(), Tier::Hard);
    assert_eq!(Tier::Hard.up(), Tier::Hard);
}
