use crate::prompts::brief_text;
use crate::prompts::reply::tagged_json;
use crate::task::attempt::VerifyOutcome;
use crate::task::state::Task;
use crate::verify::evidence::is_held_out;

/// A candidate of a best-of attempt, as the pick run sees it.
pub struct PickCandidate<'a> {
    pub verify: &'a [VerifyOutcome],
    pub diff: &'a str,
}

/// The read-only run that chooses between two candidates that both passed
/// verify and the checks. It starts with `## Pick`, never `## Review`.
pub fn build_pick_brief(task: &Task, a: &PickCandidate, b: &PickCandidate) -> String {
    let mut out = format!("{}\n\n", brief_text("pick"));
    out.push_str(&task.goal);
    out.push_str("\n\n## Acceptance criteria\n\n");
    for c in &task.criteria {
        out.push_str("- ");
        out.push_str(c);
        out.push('\n');
    }
    for (name, cand) in [("a", a), ("b", b)] {
        out.push_str(&format!("\n## Candidate {name}\n\n"));
        for v in cand.verify.iter().filter(|v| !is_held_out(v)) {
            out.push_str(&format!("- `{}` -> exit {:?}\n", v.command, v.code));
        }
        out.push_str("\n```diff\n");
        out.push_str(cand.diff);
        out.push_str("\n```\n");
    }
    out.push_str(&format!("\n{}\n", brief_text("pick_report_format")));
    out
}

/// The `sushi-pick` block: `(true, why)` for candidate `b`, `(false, why)`
/// for `a`.
pub fn parse_pick(text: &str) -> Option<(bool, String)> {
    let body = tagged_json(text, "sushi-pick")?;
    let v: serde_json::Value = serde_json::from_str(&body).ok()?;
    let pick = v.get("pick")?.as_str()?.trim().to_ascii_lowercase();
    let why = v
        .get("why")
        .and_then(|w| w.as_str())
        .unwrap_or_default()
        .trim()
        .to_string();
    match pick.as_str() {
        "a" => Some((false, why)),
        "b" => Some((true, why)),
        _ => None,
    }
}

#[cfg(test)]
#[path = "tests/pick.rs"]
mod tests;
