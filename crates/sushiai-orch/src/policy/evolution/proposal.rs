use crate::task::evolution::{ProposalForm, ProposalReply, Track};

/// Verb stems of "make a check weaker", matched as word prefixes.
const WEAKENING_VERBS: [&str; 9] = [
    "remov", "delet", "skip", "disabl", "weaken", "relax", "ignor", "bypass", "loosen",
];

/// What a weakening verb must be near to count: a check, gate or protected
/// path, as whole words.
const CHECK_WORDS: [&str; 19] = [
    "check",
    "checks",
    "test",
    "tests",
    "testing",
    "lint",
    "linter",
    "linting",
    "gate",
    "gates",
    "hook",
    "hooks",
    "verify",
    "verifies",
    "verification",
    "verifier",
    "protected",
    "assertion",
    "assertions",
];

/// Text that weakens a check on its face, whatever is near it.
const WEAKENING_LITERALS: [&str; 8] = [
    "--no-verify",
    "eslint-disable",
    "#[ignore",
    "@ts-ignore",
    "@ts-nocheck",
    ".skip(",
    "continue-on-error",
    "|| true",
];

/// Files a change may never target, on either track.
const PROTECTED_TARGETS: [&str; 4] = ["agents.md", "claude.md", "memory.md", "/memory/"];

/// Forms a repo-track proposal may take.
const REPO_FORMS: [ProposalForm; 6] = [
    ProposalForm::Script,
    ProposalForm::Test,
    ProposalForm::Lint,
    ProposalForm::Doc,
    ProposalForm::Command,
    ProposalForm::Skill,
];

/// Whether `text` says to weaken, skip or remove a check, gate or protected
/// path. Deliberately blunt: a false rejection costs one retry of a wording,
/// a false pass lets a proposal loosen the rules that keep the others honest.
fn weakens_a_check(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    if let Some(lit) = WEAKENING_LITERALS.iter().find(|l| lower.contains(*l)) {
        return Some(format!("`{lit}`"));
    }
    for segment in lower.split(['.', ';', '\n', '!', '?']) {
        let words: Vec<&str> = segment
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .collect();
        let is_verb = |i: usize| {
            WEAKENING_VERBS.iter().any(|v| words[i].starts_with(v))
                || (words[i] == "off" && i > 0 && words[i - 1].starts_with("turn"))
        };
        let is_object = |i: usize| CHECK_WORDS.contains(&words[i]);
        for i in 0..words.len() {
            if is_verb(i) {
                let ahead = (i + 1..words.len().min(i + 5)).find(|&j| is_object(j));
                let behind = (i.saturating_sub(3)..i).find(|&j| is_object(j));
                if let Some(j) = ahead.or(behind) {
                    return Some(format!("\"{} ... {}\"", words[i], words[j]));
                }
            }
        }
    }
    None
}

/// The reason a proposal is rejected outright, if it is.
pub(crate) fn gate(reply: &ProposalReply) -> Option<String> {
    if let Some(what) = weakens_a_check(&reply.change) {
        return Some(format!("the change weakens or removes a check: {what}"));
    }
    if reply.track == Track::Repo && !REPO_FORMS.contains(&reply.form) {
        return Some(format!(
            "a repo-track proposal may only be a script, test, lint, doc, command or skill, not {}",
            serde_json::to_value(reply.form)
                .ok()
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_default()
        ));
    }
    let lower = reply.change.to_lowercase();
    if let Some(target) = PROTECTED_TARGETS.iter().find(|t| lower.contains(*t)) {
        return Some(format!(
            "the change targets `{target}`, which proposals may not edit"
        ));
    }
    None
}

#[cfg(test)]
mod tests;
