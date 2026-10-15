use super::gate;
use crate::task::evolution::{ProposalForm, ProposalReply, Track};

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
