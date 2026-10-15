use crate::engine::attempts::report::Report;
use crate::harness::RunOutcome;
use crate::prompts::text::tail_chars;
use crate::task::attempts::{self as observations, AttemptReport};
use crate::task::Task;

pub(in crate::engine) struct ImplementerReport {
    pub(in crate::engine) final_text: String,
    pub(in crate::engine) report: Option<Report>,
    pub(in crate::engine) note: String,
    pub(in crate::engine) decisions: Vec<String>,
}

pub(in crate::engine) fn record(
    task: &mut Task,
    idx: usize,
    outcome: &RunOutcome,
) -> ImplementerReport {
    let final_text = outcome.final_text.clone().unwrap_or_default();
    let report = crate::engine::attempts::report::parse_report(&final_text);
    // Not a gate: the reviewer weighs it. A `partial` or missing report,
    // or a harness that errored after editing files, is exactly what a
    // reviewer should look at harder.
    let implementer_note = format!(
        "Implementer report: {}{}",
        match report.as_ref().map(|r| &r.outcome) {
            Some(crate::engine::attempts::report::Outcome::Complete) => "complete",
            Some(crate::engine::attempts::report::Outcome::Partial) => "partial",
            Some(crate::engine::attempts::report::Outcome::Blocked) => "blocked",
            None => "missing (no parseable sushi-report)",
        },
        outcome
            .error
            .as_deref()
            .map(|e| format!("; the harness ended with an error: {}", tail_chars(e, 400)))
            .unwrap_or_default()
    );
    // What the implementer says it did and decided (e.g. "no lesson
    // this time", "ran the desktop smoke"): the reviewer weighs these
    // claims against the evidence instead of never hearing them.
    let agent_decisions = report
        .as_ref()
        .map(|r| crate::engine::attempts::report::agent_decision_lines(&r.decisions))
        .unwrap_or_default();
    let implementer_note = match report.as_ref() {
        Some(r) if !r.summary.trim().is_empty() => {
            let account = r.summary.trim().to_string();
            format!(
                    "{implementer_note}\n\n{}",
                    crate::prompts::text::untrusted_block(
                        "The implementer's own account, a claim to check against the diff and verify results",
                        &account
                    )
                )
        }
        _ => implementer_note,
    };
    observations::record_report(
        task,
        idx,
        AttemptReport {
            summary: report.as_ref().map(|r| r.summary.clone()),
            declines: report
                .as_ref()
                .map(|r| r.declines.clone())
                .unwrap_or_default(),
            handoff: report
                .as_ref()
                .map(|r| r.handoff.trim().to_string())
                .filter(|h| !h.is_empty()),
        },
    );
    // Agent-reported decisions are trusted less than the owner's: strip
    // any leading "Owner:" the agent might have echoed back, prefix
    // with "Agent:", and never duplicate an identical entry.
    for entry in &agent_decisions {
        if !task.decisions.contains(entry) {
            task.record_decision(entry.clone());
        }
    }

    ImplementerReport {
        final_text,
        report,
        note: implementer_note,
        decisions: agent_decisions,
    }
}
