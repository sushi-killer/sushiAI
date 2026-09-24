//! The `system_one`-shaped classifier: redaction, HTTP backends
//! (openrouter/typesafe/openai), response parsing, and the
//! `decisions.jsonl` journal. Every call is blocking (`ureq`); callers run
//! it via `tokio::task::spawn_blocking`. A classifier error, timeout, or
//! missing key is always the caller's job to treat as "fall back to rules"
//! -- this module just reports `Err` and never panics or retries forever.

use crate::model::{ClassifierBackend, ClassifierSettings};
use serde::Serialize;
use std::collections::HashMap;
use std::time::Duration;

pub const TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug)]
pub struct ClassifyError(pub String);

impl std::fmt::Display for ClassifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for ClassifyError {}

#[derive(Debug, Clone)]
pub enum QuestionSpec {
    /// A single 0..1 probability answer, e.g. jev-belay's `claims_done`.
    Noul { name: String, prompt: String },
    /// A pick from a fixed option list, e.g. the tier or the report outcome.
    Choice {
        name: String,
        prompt: String,
        options: Vec<String>,
    },
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Answer {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub noul: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub choice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probabilities: Option<HashMap<String, f64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
}

pub type Answers = HashMap<String, Answer>;

// -- redaction ---------------------------------------------------------

const SECRET_PREFIXES: &[&str] = &[
    "sk-",
    "ghp_",
    "github_pat_",
    "AKIA",
    "xoxb-",
    "xoxa-",
    "xoxp-",
];

fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=' || c == '_' || c == '-'
}

/// Redact `-----BEGIN ... -----END ...-----` PEM-style blocks wholesale.
fn redact_pem_blocks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    loop {
        let Some(begin_rel) = text[cursor..].find("-----BEGIN") else {
            out.push_str(&text[cursor..]);
            break;
        };
        let begin = cursor + begin_rel;
        out.push_str(&text[cursor..begin]);
        out.push_str("[REDACTED]");
        match text[begin..].find("-----END") {
            Some(end_rel) => {
                let after_end = begin + end_rel + "-----END".len();
                let block_end = match text[after_end..].find("-----") {
                    Some(close_rel) => after_end + close_rel + 5,
                    None => text.len(),
                };
                cursor = block_end;
            }
            None => {
                break;
            }
        }
    }
    out
}

/// Redact `KEY=...`, `TOKEN=...`, `SECRET=...`, `PASSWORD=...` (case
/// insensitive on the identifier) values.
fn redact_key_value_secrets(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '=' {
            let mut j = i;
            while j > 0 && (chars[j - 1].is_ascii_alphanumeric() || chars[j - 1] == '_') {
                j -= 1;
            }
            let ident_len = i - j;
            let ident: String = chars[j..i].iter().collect();
            let upper = ident.to_ascii_uppercase();
            let is_secret_key = ident_len > 0
                && (upper.ends_with("KEY")
                    || upper.ends_with("TOKEN")
                    || upper.ends_with("SECRET")
                    || upper.ends_with("PASSWORD"));
            if is_secret_key {
                for _ in 0..ident_len {
                    out.pop();
                }
                let mut k = i + 1;
                let quote = if k < chars.len() && (chars[k] == '"' || chars[k] == '\'') {
                    let q = chars[k];
                    k += 1;
                    Some(q)
                } else {
                    None
                };
                let val_start = k;
                while k < chars.len() {
                    let c = chars[k];
                    let stop = match quote {
                        Some(q) => c == q,
                        None => c.is_whitespace() || c == ',' || c == '&' || c == '\n',
                    };
                    if stop {
                        break;
                    }
                    k += 1;
                }
                out.push_str(&ident);
                out.push('=');
                if k > val_start {
                    if let Some(q) = quote {
                        out.push(q);
                    }
                    out.push_str("[REDACTED]");
                    if let Some(q) = quote {
                        if k < chars.len() && chars[k] == q {
                            out.push(q);
                            k += 1;
                        }
                    }
                }
                i = k;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Redact tokens with a known secret prefix, or bare runs of 40+
/// base64/hex-alphabet characters.
fn redact_tokens(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if is_token_char(chars[i]) {
            let start = i;
            while i < chars.len() && is_token_char(chars[i]) {
                i += 1;
            }
            let token: String = chars[start..i].iter().collect();
            let has_prefix = SECRET_PREFIXES.iter().any(|p| token.starts_with(p));
            if has_prefix || token.chars().count() >= 40 {
                out.push_str("[REDACTED]");
            } else {
                out.push_str(&token);
            }
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// Redact secrets from free text before it leaves the machine (spec
/// "Classifier" section patterns).
pub fn redact(text: &str) -> String {
    let text = redact_pem_blocks(text);
    let text = redact_key_value_secrets(&text);
    redact_tokens(&text)
}

/// Recursively redact every string value in a JSON document (used on the
/// `state` object before it's sent to a classifier backend).
pub fn redact_value(v: &serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::String(s) => serde_json::Value::String(redact(s)),
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(redact_value).collect())
        }
        serde_json::Value::Object(obj) => {
            let mut m = serde_json::Map::new();
            for (k, val) in obj {
                m.insert(k.clone(), redact_value(val));
            }
            serde_json::Value::Object(m)
        }
        other => other.clone(),
    }
}

// -- response parsing ----------------------------------------------------

fn parse_answer(v: &serde_json::Value) -> Answer {
    Answer {
        noul: v.get("noul").and_then(|x| x.as_f64()),
        choice: v
            .get("choice")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string()),
        probabilities: v.get("probabilities").and_then(|x| x.as_object()).map(|o| {
            o.iter()
                .filter_map(|(k, val)| val.as_f64().map(|f| (k.clone(), f)))
                .collect()
        }),
        confidence: v.get("confidence").and_then(|x| x.as_f64()),
    }
}

/// Both `openrouter` and `typesafe` return `{"answers": {"<name>": {...}}}`.
pub fn parse_system_one_response(body: &serde_json::Value) -> Result<Answers, ClassifyError> {
    let answers_obj = body
        .get("answers")
        .and_then(|v| v.as_object())
        .ok_or_else(|| ClassifyError("response missing \"answers\" object".to_string()))?;
    Ok(answers_obj
        .iter()
        .map(|(name, v)| (name.clone(), parse_answer(v)))
        .collect())
}

/// `openai` returns a normal chat-completion envelope whose
/// `choices[0].message.content` is a JSON string in the same
/// `{"answers": {...}}` shape we ask for via the response's JSON schema.
pub fn parse_openai_response(body: &serde_json::Value) -> Result<Answers, ClassifyError> {
    let content = body
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .ok_or_else(|| {
            ClassifyError("openai response missing choices[0].message.content".to_string())
        })?;
    let inner: serde_json::Value = serde_json::from_str(content)
        .map_err(|e| ClassifyError(format!("openai content is not JSON: {e}")))?;
    parse_system_one_response(&inner)
}

// -- backends ------------------------------------------------------------

const OPENROUTER_URL: &str = "https://openrouter.ai/api/alpha/decisions";
const TYPESAFE_URL: &str = "https://api.typesafe.ai/v1/systemone";

fn post_json(
    url: &str,
    key: &str,
    body: &serde_json::Value,
) -> Result<serde_json::Value, ClassifyError> {
    let agent = ureq::AgentBuilder::new().timeout(TIMEOUT).build();
    let resp = agent
        .post(url)
        .set("Authorization", &format!("Bearer {key}"))
        .set("Content-Type", "application/json")
        .send_json(body.clone())
        .map_err(|e| match e {
            ureq::Error::Status(code, resp) => {
                let text = resp.into_string().unwrap_or_default();
                let text: String = text.chars().take(300).collect();
                ClassifyError(format!("{url} returned {code}: {text}"))
            }
            other => ClassifyError(format!("request to {url} failed: {other}")),
        })?;
    resp.into_json::<serde_json::Value>()
        .map_err(|e| ClassifyError(format!("response from {url} is not JSON: {e}")))
}

/// The System One request shape: a map of question name to
/// `{type, instructions, criteria?}`; a choice's criteria map each option to
/// an optional description (TypeSafe docs, `@typesafe-ai/sdk`).
fn questions_json(questions: &[QuestionSpec]) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for q in questions {
        let (name, value) = match q {
            QuestionSpec::Noul { name, prompt } => (
                name,
                serde_json::json!({"type": "noul", "instructions": prompt}),
            ),
            QuestionSpec::Choice {
                name,
                prompt,
                options,
            } => {
                let criteria: serde_json::Map<String, serde_json::Value> = options
                    .iter()
                    .map(|o| (o.clone(), serde_json::Value::Null))
                    .collect();
                (
                    name,
                    serde_json::json!({"type": "choice", "instructions": prompt, "criteria": criteria}),
                )
            }
        };
        map.insert(name.clone(), value);
    }
    serde_json::Value::Object(map)
}

/// Call the configured classifier backend. Redacts `state` first. Returns
/// `Err` on any missing key, network error, timeout, or unparseable
/// response -- callers must treat that as "fall back to rules" (spec: "any
/// classifier error, timeout or missing key falls back to rules, never
/// blocks progress").
pub fn decide(
    settings: &ClassifierSettings,
    key: Option<&str>,
    base_url: Option<&str>,
    state: &serde_json::Value,
    questions: &[QuestionSpec],
) -> Result<Answers, ClassifyError> {
    let key = key.ok_or_else(|| ClassifyError("no classifier key configured".to_string()))?;
    let redacted_state = redact_value(state);
    let qjson = questions_json(questions);

    match settings.backend {
        ClassifierBackend::None => Err(ClassifyError("classifier backend is \"none\"".to_string())),
        ClassifierBackend::Openrouter | ClassifierBackend::Typesafe => {
            let url = if settings.backend == ClassifierBackend::Openrouter {
                OPENROUTER_URL
            } else {
                TYPESAFE_URL
            };
            let body = serde_json::json!({
                "state": redacted_state,
                "model": settings.model,
                "questions": qjson,
            });
            let resp = post_json(url, key, &body)?;
            parse_system_one_response(&resp)
        }
        ClassifierBackend::Openai => {
            let base_url = base_url
                .filter(|u| !u.is_empty())
                .ok_or_else(|| ClassifyError("classifier baseUrl is not set".to_string()))?;
            let url = format!("{}/v1/chat/completions", base_url.trim_end_matches('/'));
            let body = serde_json::json!({
                "model": settings.model,
                "messages": [
                    {
                        "role": "system",
                        "content": "Answer each question with calibrated probabilities. Reply with JSON: {\"answers\": {\"<name>\": {\"noul\"?: number, \"choice\"?: string, \"probabilities\"?: object, \"confidence\"?: number}}}."
                    },
                    {
                        "role": "user",
                        "content": serde_json::json!({"state": redacted_state, "questions": qjson}).to_string()
                    }
                ],
                "response_format": {"type": "json_object"},
            });
            let resp = post_json(&url, key, &body)?;
            parse_openai_response(&resp)
        }
    }
}

// -- decisions.jsonl -------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct DecisionLogEntry<'a> {
    pub ts: i64,
    pub point: &'a str,
    #[serde(rename = "taskId")]
    pub task_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answers: Option<&'a Answers>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<&'a str>,
    pub ms: u64,
}

/// Serialize one journal line (no trailing newline; the store appends it).
pub fn journal_line(entry: &DecisionLogEntry) -> String {
    serde_json::to_string(entry).unwrap_or_else(|_| "{}".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_known_prefixes() {
        let text = "token sk-abcdefghijklmno and ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ012345 end";
        let redacted = redact(text);
        assert!(!redacted.contains("sk-abcdefghijklmno"));
        assert!(!redacted.contains("ghp_ABCDEF"));
        assert!(redacted.contains("[REDACTED]"));
        assert!(redacted.contains("token"));
        assert!(redacted.contains("end"));
    }

    #[test]
    fn redacts_aws_and_slack_and_github_pat() {
        let text = "AKIAABCD1234EFGH5678 xoxb-1234-5678-abcdefghij github_pat_11ABCDEFG0123456789";
        let redacted = redact(text);
        assert!(!redacted.contains("AKIAABCD1234EFGH5678"));
        assert!(!redacted.contains("xoxb-1234"));
        assert!(!redacted.contains("github_pat_11ABCDEFG"));
    }

    #[test]
    fn redacts_pem_blocks() {
        let text = "before\n-----BEGIN PRIVATE KEY-----\nMIIBogIBAAKCAQ==\n-----END PRIVATE KEY-----\nafter";
        let redacted = redact(text);
        assert!(redacted.contains("before"));
        assert!(redacted.contains("after"));
        assert!(!redacted.contains("MIIBogIBAAKCAQ"));
    }

    #[test]
    fn redacts_key_value_secrets() {
        let text = "API_KEY=abcd1234 OTHER=fine PASSWORD=\"hunter2 secret\" ok";
        let redacted = redact(text);
        assert!(redacted.contains("API_KEY=[REDACTED]"));
        assert!(redacted.contains("OTHER=fine"));
        assert!(redacted.contains("PASSWORD=\"[REDACTED]\""));
        assert!(redacted.contains("ok"));
    }

    #[test]
    fn redacts_long_base64_or_hex_runs() {
        let long_hex = "a".repeat(45);
        let text = format!("value={long_hex} short=abc");
        let redacted = redact(&text);
        // caught either by key=value (ends with nothing matching) or bare-token rule
        assert!(!redacted.contains(&long_hex));
        assert!(redacted.contains("short=abc"));
    }

    #[test]
    fn leaves_ordinary_text_alone() {
        let text = "Add a Save button to the settings dialog, then run npm test.";
        assert_eq!(redact(text), text);
    }

    #[test]
    fn redact_value_recurses_into_json() {
        let v = serde_json::json!({
            "note": "key sk-abcdefghijklmno here",
            "nested": {"list": ["fine", "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ012345"]}
        });
        let redacted = redact_value(&v);
        assert!(!redacted.to_string().contains("sk-abcdefghijklmno"));
        assert!(!redacted.to_string().contains("ghp_ABCDEF"));
        assert!(redacted.to_string().contains("fine"));
    }

    #[test]
    fn parses_system_one_style_answers_noul_and_choice() {
        let body = serde_json::json!({
            "answers": {
                "claims_done": {"noul": 0.9},
                "tier": {"choice": "standard", "probabilities": {"mechanical": 0.1, "standard": 0.7, "hard": 0.2}, "confidence": 0.8}
            }
        });
        let answers = parse_system_one_response(&body).unwrap();
        assert_eq!(answers["claims_done"].noul, Some(0.9));
        assert_eq!(answers["tier"].choice.as_deref(), Some("standard"));
        assert_eq!(
            answers["tier"].probabilities.as_ref().unwrap()["standard"],
            0.7
        );
    }

    #[test]
    fn system_one_response_missing_answers_is_an_error() {
        let body = serde_json::json!({"oops": true});
        assert!(parse_system_one_response(&body).is_err());
    }

    #[test]
    fn parses_openai_chat_completion_shape() {
        let inner = serde_json::json!({
            "answers": {"answerable": {"noul": 0.85}}
        })
        .to_string();
        let body = serde_json::json!({
            "choices": [
                {"message": {"content": inner}}
            ]
        });
        let answers = parse_openai_response(&body).unwrap();
        assert_eq!(answers["answerable"].noul, Some(0.85));
    }

    #[test]
    fn openai_response_missing_content_is_an_error() {
        let body = serde_json::json!({"choices": []});
        assert!(parse_openai_response(&body).is_err());
    }

    #[test]
    fn decide_without_key_is_an_error_not_a_panic() {
        let settings = crate::model::ClassifierSettings {
            backend: ClassifierBackend::Openrouter,
            model: "typesafe/jev-1.13".to_string(),
            provider_id: String::new(),
        };
        let result = decide(&settings, None, None, &serde_json::json!({}), &[]);
        assert!(result.is_err());
    }

    #[test]
    fn decide_with_backend_none_is_an_error() {
        let settings = crate::model::ClassifierSettings {
            backend: ClassifierBackend::None,
            model: String::new(),
            provider_id: String::new(),
        };
        let result = decide(&settings, Some("key"), None, &serde_json::json!({}), &[]);
        assert!(result.is_err());
    }

    #[test]
    fn decide_openai_without_base_url_is_an_error() {
        let settings = crate::model::ClassifierSettings {
            backend: ClassifierBackend::Openai,
            model: "gpt-4o-mini".to_string(),
            provider_id: String::new(),
        };
        let result = decide(&settings, Some("key"), None, &serde_json::json!({}), &[]);
        assert!(result.is_err());
    }

    #[test]
    fn journal_line_is_compact_json_with_camel_case_task_id() {
        let mut answers: Answers = HashMap::new();
        answers.insert(
            "claims_done".to_string(),
            Answer {
                noul: Some(0.5),
                ..Default::default()
            },
        );
        let entry = DecisionLogEntry {
            ts: 123,
            point: "stop_gate",
            task_id: "t1",
            answers: Some(&answers),
            error: None,
            ms: 42,
        };
        let line = journal_line(&entry);
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["taskId"], "t1");
        assert_eq!(v["ms"], 42);
        assert!(v.get("error").is_none());
    }

    #[test]
    fn questions_serialize_as_the_system_one_name_map() {
        let q = questions_json(&[
            QuestionSpec::Noul {
                name: "claims_done".into(),
                prompt: "Does it claim done?".into(),
            },
            QuestionSpec::Choice {
                name: "tier".into(),
                prompt: "How hard?".into(),
                options: vec!["mechanical".into(), "hard".into()],
            },
        ]);
        assert_eq!(
            q,
            serde_json::json!({
                "claims_done": {"type": "noul", "instructions": "Does it claim done?"},
                "tier": {"type": "choice", "instructions": "How hard?",
                         "criteria": {"mechanical": null, "hard": null}}
            })
        );
    }
}
