use crate::clip::truncate_chars;
use crate::prompts::engine_text;
use crate::prompts::fill;
use crate::task::MissingTool;
use crate::task::Task;
use serde_json::json;

const MAX_CONFLICT_CHARS: usize = 400;

pub(super) enum Verdict {
    Clear,
    Conflict(String),
    Unusable(String),
}

pub(in crate::engine) fn build_check_brief(
    goal: &str,
    criteria: &[String],
    tools: &serde_json::Value,
) -> String {
    format!(
        "{}\n\n{}\n",
        engine_text("brief_check_check"),
        json!({"goal": goal, "criteria": criteria, "tools": tools})
    )
}

/// The criterion the reply says needs a tool the task will not have; a
/// criterion index outside the list, or a blank capability, is no claim.
pub(super) fn parse_missing_tool(
    text: &str,
    criteria: usize,
    off: &[(String, String)],
) -> Option<MissingTool> {
    let (start, end) = (text.find('{')?, text.rfind('}')?);
    let v: serde_json::Value = serde_json::from_str(&text[start..=end]).ok()?;
    let m = v.get("missingTool").filter(|m| m.is_object())?;
    let criterion = m.get("criterion")?.as_u64()? as usize;
    let capability = m
        .get("capability")?
        .as_str()?
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if criterion >= criteria || capability.is_empty() {
        return None;
    }
    let tool_id = m
        .get("toolId")
        .and_then(|t| t.as_str())
        .filter(|id| off.iter().any(|(known, _)| known == id))
        .map(str::to_string);
    Some(MissingTool {
        criterion,
        capability: truncate_chars(&capability, MAX_CONFLICT_CHARS),
        tool_id,
    })
}

/// The reply's JSON object, whatever prose or fence surrounds it. A
/// contradiction with no named conflict still counts, with a generic text.
pub(super) fn parse_verdict(text: &str) -> Verdict {
    let (Some(start), Some(end)) = (text.find('{'), text.rfind('}')) else {
        return Verdict::Unusable("the reply had no JSON".to_string());
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text[start..=end]) else {
        return Verdict::Unusable("the reply was not valid JSON".to_string());
    };
    match v.get("contradiction").and_then(|c| c.as_bool()) {
        Some(false) => Verdict::Clear,
        Some(true) => {
            let conflict = v
                .get("conflict")
                .and_then(|c| c.as_str())
                .map(|c| c.split_whitespace().collect::<Vec<_>>().join(" "))
                .unwrap_or_default();
            Verdict::Conflict(if conflict.is_empty() {
                "two requirements contradict each other".to_string()
            } else {
                truncate_chars(&conflict, MAX_CONFLICT_CHARS)
            })
        }
        None => Verdict::Unusable("the reply had no contradiction flag".to_string()),
    }
}

pub(in crate::engine) fn refresh_brief(task: &Task, changed: &[String]) -> String {
    format!(
        "{}\n\n{}\n",
        engine_text("brief_check_refresh"),
        json!({
            "changedFiles": changed,
            "paths": task.paths,
            "verify": task.verify,
            "finalVerify": task.final_verify,
        })
    )
}

/// The line a fresh redraft appends to the request so the new planner
/// session knows what the previous draft got wrong.
pub(in crate::engine) fn redraft_request_note(conflict: &str) -> String {
    fill(
        engine_text("brief_check_redraft_note"),
        &[("conflict", conflict)],
    )
}

/// A string-array member of the reply; `None` when it is absent or not a list
/// of strings.
pub(super) fn reply_list(v: &serde_json::Value, key: &str) -> Option<Vec<String>> {
    v.get(key)?
        .as_array()?
        .iter()
        .map(|x| x.as_str().map(|s| s.trim().to_string()))
        .collect::<Option<Vec<_>>>()
        .map(|l| l.into_iter().filter(|s| !s.is_empty()).collect())
}
