use crate::prompts::brief_text;
use crate::prompts::reply::tagged_json;
use crate::task::state::Task;

/// The review areas a change can break, as a route turn chose them. A full
/// review is briefed with these areas only; a narrow re-review never is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
pub struct Lenses {
    pub seams: bool,
    pub code: bool,
    pub tests: bool,
    pub errors: bool,
    pub docs: bool,
}

impl Lenses {
    /// Every area: the review when the route turn gave no usable answer.
    pub const ALL: Lenses = Lenses {
        seams: true,
        code: true,
        tests: true,
        errors: true,
        docs: true,
    };

    /// The names of the chosen areas, in review order.
    pub fn chosen(&self) -> Vec<&'static str> {
        [
            (self.seams, "seams"),
            (self.code, "code"),
            (self.tests, "tests"),
            (self.errors, "errors"),
            (self.docs, "docs"),
        ]
        .into_iter()
        .filter(|(on, _)| *on)
        .map(|(_, name)| name)
        .collect()
    }
}

/// The route turn's reply: the areas and one sentence of why.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct LensRouting {
    pub lenses: Lenses,
    #[serde(default)]
    pub reason: String,
}

/// A route reply with every area named. An area missing from the reply
/// refuses the whole reply.
pub fn parse_lens_routing(text: &str) -> Option<LensRouting> {
    serde_json::from_str(&tagged_json(text, "sushi-route")?).ok()
}

/// The review's section naming the chosen areas. Criteria and the scope rule
/// apply to every review; a defect outside the areas is still reported.
pub fn review_lens_block(lenses: &Lenses) -> String {
    let mut out = format!("\n{}\n\n", brief_text("review_lenses"));
    let areas = [
        (lenses.seams, "lens_seams"),
        (lenses.code, "lens_code"),
        (lenses.tests, "lens_tests"),
        (lenses.errors, "lens_errors"),
        (lenses.docs, "lens_docs"),
    ];
    let chosen: Vec<&str> = areas
        .iter()
        .filter(|(on, _)| *on)
        .map(|(_, key)| brief_text(key))
        .collect();
    if chosen.is_empty() {
        out.push_str(&format!("- {}\n", brief_text("review_lenses_none")));
    }
    for text in chosen {
        out.push_str(&format!("- {text}\n"));
    }
    out
}

/// The route turn's brief: the goal, the criteria and the changed files.
pub fn build_route_brief(task: &Task, changed: &[String]) -> String {
    let criteria = task
        .criteria
        .iter()
        .enumerate()
        .map(|(i, c)| format!("{i}. {c}\n"))
        .collect::<String>();
    let files = changed
        .iter()
        .map(|f| format!("- {f}\n"))
        .collect::<String>();
    let mut out = format!(
        "## Route the review\n\n{}\n\n## Acceptance criteria\n\n{criteria}\n## Changed files\n\n{files}\n",
        task.goal
    );
    out.push_str(brief_text("route_instructions"));
    out.push_str("\n\n");
    out.push_str(brief_text("route_report_format"));
    out.push('\n');
    out
}

#[cfg(test)]
#[path = "tests/routing.rs"]
mod tests;
