use super::*;
use crate::engine::evolution::test_support::*;

#[test]
fn evolution_owner_answers_cluster_by_their_first_word() {
    let answers = [
        ("a", "continue: attempt 7 failed only because of a restart"),
        ("b", "Continue once more, with a new brief"),
        ("c", "continue - change no code"),
        ("d", "drop this check (the parent runs it)"),
    ];
    let signals: Vec<StoredSignal> = answers
        .iter()
        .map(|(t, d)| waste(signal(SignalKind::OwnerQuestion, t, "/r", d), 0, 0.0))
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
        waste(
            signal(SignalKind::Loop, "a", "/r1", "reads /tmp/x1/foo 3 times"),
            1,
            0.0,
        ),
        waste(
            signal(SignalKind::Loop, "b", "/r2", "reads /tmp/y2/foo 9 times"),
            1,
            0.0,
        ),
        waste(
            signal(SignalKind::Loop, "c", "/r2", "reads /tmp/z/foo 5 times"),
            1,
            0.0,
        ),
        waste(
            signal(SignalKind::Discovery, "a", "/r1", "reads foo times"),
            1,
            0.0,
        ),
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
