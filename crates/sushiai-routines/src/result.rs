//! The run result contract (`result.schema.json`, v1): validation and normalisation.

use serde_json::{json, Map, Value};

/// The embedded JSON Schema, passed to both harnesses and shipped to the UI.
pub const SCHEMA: &str = include_str!("../result.schema.json");
/// The strict variant for Codex (OpenAI structured output): every object closed with all
/// properties required, optionals nullable, `counts` as a name/value list. [`from_codex`]
/// maps an answer back to the v1 shape.
pub const SCHEMA_CODEX: &str = include_str!("../result.codex.schema.json");
/// The most bytes of `summary` the store keeps.
pub const SUMMARY_CAP: usize = 64 * 1024;
const TRUNCATION_NOTE: &str = "\n\n[summary truncated: it was longer than 64 KB]";
const STATUSES: &[&str] = &["done", "noop", "needs_you", "failed", "missing_tool"];
const TITLE_MAX: usize = 120;

/// Compact JSON of [`schema_for`], as a command-line argument.
pub fn schema_compact(harness: &str) -> String {
    let text = schema_for(harness);
    serde_json::from_str::<Value>(text)
        .map(|v| v.to_string())
        .unwrap_or_else(|_| text.to_string())
}

/// The schema text a harness gets: the strict one for Codex, v1 for everything else.
pub fn schema_for(harness: &str) -> &'static str {
    if harness == "codex" {
        SCHEMA_CODEX
    } else {
        SCHEMA
    }
}

/// Maps a strict Codex answer to the v1 shape: null optionals are dropped and a `counts`
/// list of `{name, value}` becomes an object. A v1 answer passes through unchanged.
pub fn from_codex(mut v: Value) -> Value {
    let Some(obj) = v.as_object_mut() else {
        return v;
    };
    obj.retain(|_, x| !x.is_null());
    if let Some(Value::Array(items)) = obj.get("counts") {
        let mut map = Map::new();
        for item in items {
            if let (Some(Value::String(name)), Some(value @ Value::Number(_))) =
                (item.get("name"), item.get("value"))
            {
                map.insert(name.clone(), value.clone());
            }
        }
        let counts = Value::Object(map);
        obj.insert("counts".into(), counts);
    }
    if let Some(Value::Array(actions)) = obj.get_mut("actions") {
        for action in actions {
            if let Some(a) = action.as_object_mut() {
                a.retain(|_, x| !x.is_null());
            }
        }
    }
    v
}

/// Checks `v` against the v1 schema. The error lists every problem, one per line, and is what
/// the single retry prompt carries.
pub fn validate(v: &Value) -> Result<(), String> {
    let mut problems = Vec::new();
    let Some(obj) = v.as_object() else {
        return Err("the answer must be one JSON object".into());
    };
    for key in ["status", "title", "summary", "actions"] {
        if !obj.contains_key(key) {
            problems.push(format!("missing required field `{key}`"));
        }
    }
    match obj.get("status") {
        None => {}
        Some(Value::String(s)) if STATUSES.contains(&s.as_str()) => {}
        Some(_) => problems.push(format!("`status` must be one of {}", STATUSES.join(", "))),
    }
    match obj.get("title") {
        None => {}
        Some(Value::String(s)) if s.chars().count() <= TITLE_MAX => {}
        Some(Value::String(_)) => problems.push(format!("`title` is longer than {TITLE_MAX}")),
        Some(_) => problems.push("`title` must be a string".into()),
    }
    if obj.get("summary").is_some_and(|s| !s.is_string()) {
        problems.push("`summary` must be a string".into());
    }
    match obj.get("actions") {
        None => {}
        Some(Value::Array(items)) => {
            for (i, item) in items.iter().enumerate() {
                action_problems(i, item, &mut problems);
            }
        }
        Some(_) => problems.push("`actions` must be an array".into()),
    }
    if let Some(m) = obj.get("missing") {
        let ok = m.as_array().is_some_and(|a| a.iter().all(Value::is_string));
        if !ok {
            problems.push("`missing` must be an array of strings".into());
        }
    }
    if obj.get("next").is_some_and(|n| !n.is_string()) {
        problems.push("`next` must be a string".into());
    }
    if let Some(c) = obj.get("counts") {
        let ok = c
            .as_object()
            .is_some_and(|o| o.values().all(Value::is_number));
        if !ok {
            problems.push("`counts` must be an object of numbers".into());
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("\n"))
    }
}

fn action_problems(i: usize, item: &Value, problems: &mut Vec<String>) {
    let Some(a) = item.as_object() else {
        problems.push(format!("`actions[{i}]` must be an object"));
        return;
    };
    for key in ["kind", "text"] {
        match a.get(key) {
            Some(Value::String(_)) => {}
            Some(_) => problems.push(format!("`actions[{i}].{key}` must be a string")),
            None => problems.push(format!("`actions[{i}]` misses `{key}`")),
        }
    }
    match a.get("applied") {
        Some(Value::Bool(_)) => {}
        Some(_) => problems.push(format!("`actions[{i}].applied` must be a boolean")),
        None => problems.push(format!("`actions[{i}]` misses `applied`")),
    }
    if a.get("link").is_some_and(|l| !l.is_string()) {
        problems.push(format!("`actions[{i}].link` must be a string"));
    }
}

/// Limits on what the store keeps of one answer.
const MAX_ACTIONS: usize = 50;
const KIND_MAX: usize = 64;
const TEXT_MAX: usize = 2_000;
const LINK_MAX: usize = 2_000;
const MISSING_MAX: usize = 20;
const MISSING_ITEM_MAX: usize = 500;
const NEXT_MAX: usize = 2_000;
const COUNTS_MAX: usize = 50;
const COUNT_NAME_MAX: usize = 64;

fn clip_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// What the store keeps of a valid answer: only the schema's own keys, the summary capped
/// with a note, every list and string bounded, and in a dry run every action marked as not
/// applied.
pub fn normalize(v: Value, dry_run: bool) -> Value {
    let Value::Object(mut obj) = v else {
        return v;
    };
    let mut out = Map::new();
    for key in ["status", "title"] {
        if let Some(x) = obj.remove(key) {
            out.insert(key.into(), x);
        }
    }
    if let Some(Value::String(mut summary)) = obj.remove("summary") {
        if summary.len() > SUMMARY_CAP {
            let mut end = SUMMARY_CAP;
            while !summary.is_char_boundary(end) {
                end -= 1;
            }
            summary.truncate(end);
            summary.push_str(TRUNCATION_NOTE);
        }
        out.insert("summary".into(), Value::String(summary));
    }
    if let Some(Value::Array(actions)) = obj.remove("actions") {
        let kept: Vec<Value> = actions
            .into_iter()
            .take(MAX_ACTIONS)
            .filter_map(|a| {
                let a = a.as_object()?;
                let mut o = Map::new();
                o.insert(
                    "kind".into(),
                    json!(clip_chars(a.get("kind")?.as_str()?, KIND_MAX)),
                );
                o.insert(
                    "text".into(),
                    json!(clip_chars(a.get("text")?.as_str()?, TEXT_MAX)),
                );
                if let Some(link) = a.get("link").and_then(Value::as_str) {
                    o.insert("link".into(), json!(clip_chars(link, LINK_MAX)));
                }
                let applied = a.get("applied").and_then(Value::as_bool).unwrap_or(false);
                o.insert("applied".into(), json!(applied && !dry_run));
                Some(Value::Object(o))
            })
            .collect();
        out.insert("actions".into(), Value::Array(kept));
    }
    if let Some(Value::Array(missing)) = obj.remove("missing") {
        let items: Vec<Value> = missing
            .iter()
            .filter_map(Value::as_str)
            .take(MISSING_MAX)
            .map(|m| json!(clip_chars(m, MISSING_ITEM_MAX)))
            .collect();
        out.insert("missing".into(), Value::Array(items));
    }
    if let Some(Value::String(next)) = obj.remove("next") {
        out.insert("next".into(), json!(clip_chars(&next, NEXT_MAX)));
    }
    if let Some(Value::Object(counts)) = obj.remove("counts") {
        let kept: Map<String, Value> = counts
            .into_iter()
            .take(COUNTS_MAX)
            .map(|(k, v)| (clip_chars(&k, COUNT_NAME_MAX), v))
            .collect();
        out.insert("counts".into(), Value::Object(kept));
    }
    Value::Object(out)
}

/// A valid result for a run that has no answer: the store always holds a readable result.
pub fn synthetic(status: &str, title: &str, summary: &str, missing: &[String]) -> Value {
    let mut obj = Map::new();
    obj.insert("status".into(), json!(status));
    obj.insert("title".into(), json!(title));
    obj.insert("summary".into(), json!(summary));
    obj.insert("actions".into(), json!([]));
    if !missing.is_empty() {
        obj.insert("missing".into(), json!(missing));
    }
    Value::Object(obj)
}

/// Finds the run's answer in what a harness printed.
///
/// Claude (`--output-format json`) prints one envelope whose `structured_output` (or a JSON
/// `result` string) is the answer; Codex writes the bare answer to a file. Terminal text may
/// carry a prefix, so the text is scanned and the last object that holds an answer wins.
pub fn extract(text: &str) -> Option<Value> {
    extract_raw(text).map(from_codex)
}

fn extract_raw(text: &str) -> Option<Value> {
    objects(text)
        .into_iter()
        .filter_map(unwrap_envelope)
        .next_back()
}

/// Every top-level JSON object in `text`, in order; terminal noise around them is skipped.
fn objects(text: &str) -> Vec<Value> {
    let trimmed = text.trim();
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(rel) = trimmed[at..].find('{') {
        let start = at + rel;
        let mut stream = serde_json::Deserializer::from_str(&trimmed[start..]).into_iter::<Value>();
        match stream.next() {
            Some(Ok(v)) => {
                found.push(v);
                at = start + stream.byte_offset().max(1);
            }
            _ => at = start + 1,
        }
    }
    found
}

/// What Claude's `--output-format json` envelope says about how the run ended. These are
/// structured fields, unlike the answer text, which is the model's own words.
#[derive(Debug, Clone, PartialEq)]
pub struct Envelope {
    pub is_error: bool,
    pub subtype: Option<String>,
    /// `api_error_status`: the HTTP status of a failed API call.
    pub api_status: Option<u64>,
    /// The envelope's own `result` string (the error text when `is_error`).
    pub message: String,
    /// `permission_denials`: the tool names the harness refused to run.
    pub denied: Vec<String>,
}

/// The last `type: "result"` envelope in what the harness printed, if any.
pub fn envelope(text: &str) -> Option<Envelope> {
    objects(text)
        .into_iter()
        .filter(|o| o.get("type").and_then(Value::as_str) == Some("result"))
        .map(|o| Envelope {
            is_error: o.get("is_error").and_then(Value::as_bool).unwrap_or(false),
            subtype: o.get("subtype").and_then(Value::as_str).map(String::from),
            api_status: o.get("api_error_status").and_then(Value::as_u64),
            message: o
                .get("result")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            denied: o
                .get("permission_denials")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|d| d.get("tool_name").and_then(Value::as_str))
                .map(String::from)
                .collect(),
        })
        .next_back()
}

/// Why a dry run that reports a failure or a need probably could not read what it wanted: a
/// dry run only reads files, so a refused tool is not the repository's fault. The harness's
/// own `permission_denials` decide; without them the words of the answer do.
pub fn dry_denial(envelope: Option<&Envelope>, result: &Value) -> Option<String> {
    if !matches!(result["status"].as_str(), Some("failed" | "needs_you")) {
        return None;
    }
    let tool = match envelope.and_then(|e| e.denied.first()) {
        Some(tool) => tool.clone(),
        None => {
            let said = format!(
                "{} {}",
                result["title"].as_str().unwrap_or_default(),
                result["summary"].as_str().unwrap_or_default()
            )
            .to_lowercase();
            let refused = ["permission", "not allowed", "denied", "not permitted"];
            if !refused.iter().any(|w| said.contains(w)) {
                return None;
            }
            "a tool".to_string()
        }
    };
    Some(format!(
        "a dry run only reads files: {tool} is not allowed in a dry run"
    ))
}

fn unwrap_envelope(v: Value) -> Option<Value> {
    let obj = v.as_object()?;
    if let Some(s @ Value::Object(_)) = obj.get("structured_output") {
        return Some(s.clone());
    }
    if obj.get("type").and_then(Value::as_str) == Some("result") {
        if let Some(Value::String(r)) = obj.get("result") {
            if let Ok(inner @ Value::Object(_)) = serde_json::from_str::<Value>(r.trim()) {
                return Some(inner);
            }
        }
        return None;
    }
    if obj.contains_key("status") && obj.contains_key("actions") {
        return Some(v);
    }
    None
}

/// Terminal output as plain text: escape sequences removed, `\r\n` as `\n`.
pub fn plain(bytes: &[u8]) -> String {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            0x1b => {
                i += 1;
                match bytes.get(i) {
                    // CSI: parameters, then a final byte 0x40-0x7e.
                    Some(b'[') => {
                        i += 1;
                        while i < bytes.len() && !(0x40..=0x7e).contains(&bytes[i]) {
                            i += 1;
                        }
                        i += 1;
                    }
                    // OSC: up to BEL or ESC \.
                    Some(b']') => {
                        i += 1;
                        while i < bytes.len() && bytes[i] != 0x07 && bytes[i] != 0x1b {
                            i += 1;
                        }
                        i += if bytes.get(i) == Some(&0x1b) { 2 } else { 1 };
                    }
                    _ => i += 1,
                }
            }
            b'\r' => i += 1,
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_reads_the_claude_wrapper_and_terminal_noise() {
        let answer = r#"{"status":"done","title":"t","summary":"s","actions":[]}"#;
        let wrapped = format!(r#"{{"type":"result","structured_output":{answer}}}"#);
        assert_eq!(extract(&wrapped).unwrap()["status"], "done");
        // The answer as a JSON string in `result`.
        let quoted = serde_json::json!({"type": "result", "result": answer}).to_string();
        assert_eq!(extract(&quoted).unwrap()["title"], "t");
        // Prefix, escapes and a trailing prompt around the object.
        let noisy = format!("\u{1b}[2Jstarting\r\n{wrapped}\r\n$ ");
        assert_eq!(extract(&plain(noisy.as_bytes())).unwrap()["summary"], "s");
        assert!(extract("no json here").is_none());
        assert!(extract(r#"{"type":"result","result":"plain words"}"#).is_none());
    }

    fn walk(v: &Value, at: &str, seen: &mut usize) {
        match v {
            Value::Object(o) => {
                if o.get("type") == Some(&json!("object")) {
                    *seen += 1;
                    assert_eq!(o.get("additionalProperties"), Some(&json!(false)), "{at}");
                    let props = o["properties"].as_object().unwrap();
                    let mut req: Vec<_> = o["required"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|k| k.as_str().unwrap().to_string())
                        .collect();
                    let mut keys: Vec<_> = props.keys().cloned().collect();
                    req.sort();
                    keys.sort();
                    assert_eq!(req, keys, "{at}");
                }
                for (k, x) in o {
                    walk(x, &format!("{at}/{k}"), seen);
                }
            }
            Value::Array(a) => a.iter().for_each(|x| walk(x, at, seen)),
            _ => {}
        }
    }

    #[test]
    fn the_codex_schema_is_strict_at_every_object_level() {
        let v: Value = serde_json::from_str(SCHEMA_CODEX).unwrap();
        let mut seen = 0;
        walk(&v, "", &mut seen);
        assert_eq!(seen, 3);
        assert_eq!(schema_for("codex"), SCHEMA_CODEX);
        assert_eq!(schema_for("claude"), SCHEMA);
    }

    #[test]
    fn a_codex_answer_is_normalised_to_v1() {
        let raw = json!({"status":"done","title":"t","summary":"s",
            "actions":[{"kind":"k","text":"x","link":null,"applied":true}],
            "missing":null,"next":null,
            "counts":[{"name":"a","value":2},{"name":"b","value":1.5}]});
        let v = extract(&raw.to_string()).unwrap();
        assert_eq!(
            v,
            json!({"status":"done","title":"t","summary":"s",
                "actions":[{"kind":"k","text":"x","applied":true}],
                "counts":{"a":2,"b":1.5}})
        );
        assert!(validate(&v).is_ok());
        let none = json!({"status":"noop","title":"t","summary":"s","actions":[],
            "missing":null,"next":null,"counts":null});
        assert!(validate(&extract(&none.to_string()).unwrap()).is_ok());
    }

    #[test]
    fn normalize_drops_unknown_keys_and_bounds_every_list_and_string() {
        let actions: Vec<Value> = (0..500)
            .map(|i| {
                json!({"kind": "k", "text": "t".repeat(10_000), "applied": true,
                            "link": "l".repeat(10_000), "extra": i})
            })
            .collect();
        let v = json!({"status":"done","title":"t","summary":"s","actions":actions,
                       "evil":{"x":1},"next":"n".repeat(10_000),
                       "missing":(0..100).map(|i| i.to_string()).collect::<Vec<_>>()});
        let out = normalize(v, false);
        assert!(out.get("evil").is_none());
        let acts = out["actions"].as_array().unwrap();
        assert_eq!(acts.len(), 50);
        assert_eq!(acts[0]["text"].as_str().unwrap().len(), 2_000);
        assert_eq!(acts[0]["link"].as_str().unwrap().len(), 2_000);
        assert!(acts[0].get("extra").is_none());
        assert_eq!(out["next"].as_str().unwrap().len(), 2_000);
        assert_eq!(out["missing"].as_array().unwrap().len(), 20);
        assert!(validate(&out).is_ok());
    }

    #[test]
    fn the_envelope_reports_structured_error_fields() {
        let e = envelope(
            r#"x {"type":"result","subtype":"success","is_error":true,"api_error_status":401,
                 "result":"Not logged in"} y"#,
        )
        .unwrap();
        assert!(e.is_error);
        assert_eq!(e.api_status, Some(401));
        assert_eq!(e.message, "Not logged in");
        assert!(envelope(r#"{"status":"done"}"#).is_none());
    }

    #[test]
    fn the_last_answer_wins() {
        let a = r#"{"status":"failed","title":"a","summary":"","actions":[]}"#;
        let b = r#"{"status":"done","title":"b","summary":"","actions":[]}"#;
        assert_eq!(extract(&format!("{a}\n{b}")).unwrap()["title"], "b");
    }

    #[test]
    fn validation_lists_every_problem() {
        let bad = serde_json::json!({"status": "great", "title": 4, "actions": [{"kind": "x"}]});
        let e = validate(&bad).unwrap_err();
        assert!(e.contains("`status` must be one of"));
        assert!(e.contains("missing required field `summary`"));
        assert!(e.contains("`title` must be a string"));
        assert!(e.contains("misses `applied`"));
        let long = serde_json::json!({"status":"done","title":"x".repeat(121),
                                      "summary":"","actions":[]});
        assert!(validate(&long).is_err());
    }
}
