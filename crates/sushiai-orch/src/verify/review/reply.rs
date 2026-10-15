use crate::prompts::reply::last_fenced_block;
use crate::task::review::severity_of;
use crate::task::review::{DeclineRuling, ReviewResult};

/// Same idea for the review's ```sushi-review fenced JSON, but more
/// forgiving, since the verdict decides whether work is committed: a reply
/// that is nothing but the JSON counts, and a finding may be an object
/// (`{"severity","file","issue"}`, as Codex writes them) instead of a string.
#[cfg_attr(not(test), allow(dead_code))]
pub fn parse_review(text: &str) -> Option<ReviewResult> {
    parse_review_with_rule(text, &[]).map(|(result, _)| result)
}

/// The review object of a reply: the first candidate with a verdict.
fn review_object(text: &str) -> Option<serde_json::Value> {
    review_candidates(text)
        .iter()
        .filter_map(|body| parse_json_object(body))
        .find(has_verdict_shape)
}

/// The ruling on each declined finding, from the review's `declineRulings`.
/// A malformed list rules on nothing, so every decline stays blocking.
pub fn parse_decline_rulings(text: &str) -> Vec<DeclineRuling> {
    review_object(text)
        .and_then(|v| serde_json::from_value(v.get("declineRulings")?.clone()).ok())
        .unwrap_or_default()
}

/// A criterion's `met` as a boolean: `true`/`false`, or the strings
/// yes/no/true/false; anything else (`null`, absent) is unchecked.
/// The criterion number a ruling gives: a JSON number.
fn criterion_number(v: &serde_json::Value) -> Option<usize> {
    v.as_u64().and_then(|n| usize::try_from(n).ok())
}

fn met_of(criterion: &serde_json::Value) -> Option<bool> {
    match criterion.get("met") {
        Some(serde_json::Value::Bool(b)) => Some(*b),
        Some(serde_json::Value::String(s)) => match s.to_ascii_lowercase().as_str() {
            "true" | "yes" => Some(true),
            "false" | "no" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// Whether a parsed reply is a review verdict: a `verdict` of PASS/FAIL, or
/// a `criteria` array with at least one boolean-ish `met`.
fn has_verdict_shape(v: &serde_json::Value) -> bool {
    let verdict = v
        .get("verdict")
        .and_then(|x| serde_json::from_value::<crate::task::review::Verdict>(x.clone()).ok());
    verdict.is_some()
        || v.get("criteria")
            .and_then(|c| c.as_array())
            .is_some_and(|c| c.iter().any(|c| met_of(c).is_some()))
}

/// The JSON object in `body`. A body that is not valid JSON is repaired for
/// the shape reviewers have been seen to write: a first complete object
/// closed too early, then `,` plus more keys and a closing `}`; those keys
/// are spliced into the first object.
fn parse_json_object(body: &str) -> Option<serde_json::Value> {
    let body = body.trim();
    if let Ok(v @ serde_json::Value::Object(_)) = serde_json::from_str(body) {
        return Some(v);
    }
    let mut stream = serde_json::Deserializer::from_str(body).into_iter::<serde_json::Value>();
    let serde_json::Value::Object(mut first) = stream.next()?.ok()? else {
        return None;
    };
    let rest = body[stream.byte_offset()..].trim();
    let rest = rest.strip_prefix(',')?.trim();
    let rest = rest.strip_suffix('}')?;
    let more: serde_json::Value = serde_json::from_str(&format!("{{{rest}}}")).ok()?;
    first.extend(more.as_object()?.clone());
    Some(serde_json::Value::Object(first))
}

/// The fenced blocks of `text` as (label, body), in order. A fence closes on
/// a line that is just ` ``` `, or on a line ending in ` ``` ` (`}```).
fn fenced_blocks(text: &str) -> Vec<(String, String)> {
    let mut blocks = Vec::new();
    let mut open: Option<(String, Vec<&str>)> = None;
    for line in text.lines() {
        let trimmed = line.trim();
        match &mut open {
            None => {
                if let Some(label) = trimmed.strip_prefix("```") {
                    open = Some((label.trim().to_ascii_lowercase(), Vec::new()));
                }
            }
            Some((label, body)) => {
                if trimmed == "```" {
                    blocks.push((std::mem::take(label), body.join("\n")));
                    open = None;
                } else if let Some(before) = trimmed.strip_suffix("```") {
                    body.push(before);
                    blocks.push((std::mem::take(label), body.join("\n")));
                    open = None;
                } else {
                    body.push(line);
                }
            }
        }
    }
    blocks
}

/// The balanced top-level `{...}` spans of `text`, in order, skipping braces
/// inside JSON strings.
fn balanced_objects(text: &str) -> Vec<&str> {
    let mut spans = Vec::new();
    let (mut depth, mut start) = (0usize, 0usize);
    let (mut in_string, mut escaped) = (false, false);
    for (i, c) in text.char_indices() {
        if in_string {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' if depth > 0 => in_string = true,
            '{' => {
                if depth == 0 {
                    start = i;
                }
                depth += 1;
            }
            '}' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    spans.push(&text[start..=i]);
                }
            }
            _ => {}
        }
    }
    spans
}

/// Where a review reply may keep its verdict, best first: the last
/// sushi-review block, the last ```json block, the last unlabelled block,
/// the balanced `{...}` objects (last first), then the first `{` to the last
/// `}` (a premature `}` breaks the balance).
fn review_candidates(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    out.extend(last_fenced_block(text, "sushi-review"));
    let blocks = fenced_blocks(text);
    for label in ["json", ""] {
        out.extend(
            blocks
                .iter()
                .rev()
                .find(|(l, _)| l == label)
                .map(|(_, b)| b.clone()),
        );
    }
    out.extend(balanced_objects(text).iter().rev().map(|s| s.to_string()));
    if let (Some(start), Some(end)) = (text.find('{'), text.rfind('}')) {
        if start < end {
            out.push(text[start..=end].to_string());
        }
    }
    out
}

/// The `id`, `causedByDiff`, `kind` (lowercased) and `scenario` of a finding
/// object; a scenario without a `run` counts as none.
fn finding_detail(
    o: &serde_json::Map<String, serde_json::Value>,
) -> crate::task::review::FindingDetail {
    let kind = o
        .get("kind")
        .and_then(|k| k.as_str())
        .map(|k| k.trim().to_ascii_lowercase())
        .filter(|k| !k.is_empty());
    let scenario = o.get("scenario").and_then(|s| s.as_object()).and_then(|s| {
        let field = |key: &str| {
            s.get(key)
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .trim()
                .to_string()
        };
        let run = field("run");
        (!run.is_empty()).then(|| crate::task::review::FindingScenario {
            run,
            fails: field("fails"),
        })
    });
    let id = o
        .get("id")
        .and_then(|i| i.as_str())
        .map(|i| i.trim().to_string())
        .filter(|i| !i.is_empty());
    let caused_by_diff = o.get("causedByDiff").and_then(|c| c.as_bool());
    crate::task::review::FindingDetail {
        id,
        caused_by_diff,
        kind,
        scenario,
    }
}

/// The fields of a finding object that are marks about the finding, not
/// part of its text.
const FINDING_META_KEYS: [&str; 5] = ["repeat", "scenario", "kind", "id", "causedByDiff"];

/// [`parse_review`] plus whether the reviewer's FAIL was recorded as PASS:
/// it gave findings of its own, every one labelled P2/P3, and no criterion
/// is unmet. All findings are kept. A ruling names its criterion by its
/// index in `criteria`; one that names none of them rules on no criterion.
pub fn parse_review_with_rule(text: &str, criteria: &[String]) -> Option<(ReviewResult, bool)> {
    let v = review_object(text)?;
    let verdict = v
        .get("verdict")
        .and_then(|x| serde_json::from_value(x.clone()).ok())
        .unwrap_or(crate::task::review::Verdict::Pass);
    let ParsedFindings {
        findings,
        severities,
        details,
    } = parse_findings(&v);
    let own_findings_are_minor =
        !severities.is_empty() && severities.iter().all(|s| matches!(s, Some(2 | 3)));
    let mut result = ReviewResult {
        verdict,
        findings,
        repeated: Vec::new(),
        severities: severities.clone(),
        details: if details.iter().any(|d| *d != Default::default()) {
            details
        } else {
            Vec::new()
        },
        criteria: Vec::new(),
    };
    let mut any_unmet = false;
    // Any reply that rules on criteria, asked for or not: a PASS that marks
    // one unmet contradicts itself, and the ruling on the criterion wins. A
    // criterion the read-only reviewer could not check (`met: null`) is a
    // finding, not a failure -- the implementer cannot fix that.
    for c in v
        .get("criteria")
        .and_then(|c| c.as_array())
        .into_iter()
        .flatten()
    {
        let met = met_of(c);
        let named = c.get("criterion");
        let index = named
            .and_then(criterion_number)
            .filter(|i| *i < criteria.len());
        let name = match (index, named) {
            (Some(i), _) => criteria[i].clone(),
            (None, Some(serde_json::Value::String(s))) => s.clone(),
            (None, Some(other)) => format!("criterion {other}"),
            (None, None) => c.to_string(),
        };
        let evidence_text = c
            .get("evidence")
            .and_then(|x| x.as_str())
            .filter(|e| !e.trim().is_empty());
        result.criteria.push(crate::task::review::CriterionRuling {
            criterion: name.clone(),
            index,
            met,
            evidence: evidence_text.map(str::to_string),
        });
        if met == Some(true) {
            continue;
        }
        let evidence = evidence_text.map(|e| format!(" ({e})")).unwrap_or_default();
        if met == Some(false) {
            result
                .findings
                .push(format!("Unmet criterion: {name}{evidence}"));
            result.verdict = crate::task::review::Verdict::Fail;
            any_unmet = true;
        } else {
            result
                .findings
                .push(format!("Not checked by review: {name}{evidence}"));
        }
    }
    let mut recorded_as_pass = false;
    if result.verdict == crate::task::review::Verdict::Fail && own_findings_are_minor && !any_unmet
    {
        result.verdict = crate::task::review::Verdict::Pass;
        recorded_as_pass = true;
    }
    Some((result, recorded_as_pass))
}

/// The `path` of every `path:line` token in the findings: backticks, quotes
/// and a leading `./` trimmed, exact match. A token with no `:<digits>` tail
/// names no path.
pub fn finding_paths(findings: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for finding in findings {
        for token in finding.split_whitespace() {
            let token =
                token.trim_matches(|c: char| matches!(c, '`' | '"' | '\'' | '(' | ')' | ',' | ';'));
            let token = token.trim_end_matches([':', '.']);
            let Some((path, line)) = token.rsplit_once(':') else {
                continue;
            };
            let line = line.split(['-', ',']).next().unwrap_or("");
            if line.is_empty() || !line.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let path = path
                .trim_matches(|c: char| matches!(c, '`' | '"' | '\''))
                .trim_start_matches("./");
            if !path.chars().any(char::is_alphabetic) || out.iter().any(|p| p == path) {
                continue;
            }
            out.push(path.to_string());
        }
    }
    out
}

struct ParsedFindings {
    findings: Vec<String>,
    severities: Vec<Option<u8>>,
    details: Vec<crate::task::review::FindingDetail>,
}

fn parse_findings(v: &serde_json::Value) -> ParsedFindings {
    let mut severities: Vec<Option<u8>> = Vec::new();
    let mut details: Vec<crate::task::review::FindingDetail> = Vec::new();
    let findings: Vec<String> = v
        .get("findings")
        .and_then(|f| f.as_array())
        .map(|items| {
            items
                .iter()
                .inspect(|item| {
                    severities.push(match item {
                        serde_json::Value::String(s) => severity_of(s),
                        serde_json::Value::Object(o) => ["severity", "priority"]
                            .iter()
                            .find_map(|k| o.get(*k).and_then(|x| x.as_str()))
                            .and_then(severity_of),
                        _ => None,
                    });
                })
                .map(|item| match item {
                    serde_json::Value::String(s) => {
                        details.push(Default::default());
                        s.clone()
                    }
                    serde_json::Value::Object(o) => {
                        // Key order, not document order: another crate in the
                        // workspace turns on serde_json's `preserve_order`.
                        let mut fields: Vec<_> = o
                            .iter()
                            .filter(|(k, _)| !FINDING_META_KEYS.contains(&k.as_str()))
                            .collect();
                        fields.sort_by(|a, b| a.0.cmp(b.0));
                        let text = fields
                            .into_iter()
                            .map(|(_, x)| {
                                x.as_str()
                                    .map(str::to_string)
                                    .unwrap_or_else(|| x.to_string())
                            })
                            .collect::<Vec<_>>()
                            .join(": ");
                        details.push(finding_detail(o));
                        text
                    }
                    other => {
                        details.push(Default::default());
                        other.to_string()
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    ParsedFindings {
        findings,
        severities,
        details,
    }
}

#[cfg(test)]
#[path = "tests/reply.rs"]
mod tests;
