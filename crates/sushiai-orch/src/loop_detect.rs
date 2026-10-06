//! The live loop detector (`variant.loop_detect`): rules from OpenHands'
//! stuck detector, run over a Claude implement run's stream. The stall
//! watchdog only catches silence; an agent repeating itself looks busy.

use std::collections::BTreeMap;

/// The same tool call (name + identical input) this many times in a row.
const REPEATED_CALLS: u32 = 3;
/// This many consecutive tool results that are errors.
const ERROR_STREAK: u32 = 3;
/// The same file edited this many times with no command run in between.
// ponytail: any command counts as a check -- agents run a targeted test, not the
// task's exact verify string; per-verify matching if this misses real loops.
const REPEATED_EDITS: u32 = 8;

const EDIT_TOOLS: &[&str] = &["Edit", "MultiEdit", "Write", "NotebookEdit"];

/// A rule that fired; `detail` is the failure text (rule and the repeated
/// call or file) and the next attempt's brief opens its failure with it.
#[derive(Debug, Clone, PartialEq)]
pub struct LoopHit {
    pub rule: &'static str,
    pub detail: String,
}

#[derive(Default)]
pub struct LoopDetector {
    last_call: Option<(String, String)>,
    call_run: u32,
    error_run: u32,
    edits: BTreeMap<String, u32>,
}

impl LoopDetector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Folds one stdout line of a Claude or Codex run; `Some` when a rule
    /// fires. Unparseable lines and subagent messages are ignored.
    pub fn feed(&mut self, line: &str) -> Option<LoopHit> {
        let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
        if v.get("parent_tool_use_id").is_some_and(|p| !p.is_null()) {
            return None;
        }
        let ty = v.get("type").and_then(|x| x.as_str())?;
        if ty.starts_with("item.") {
            return self.codex_item(ty, v.get("item")?);
        }
        let blocks = v.get("message")?.get("content")?.as_array()?;
        for b in blocks {
            let hit = match (ty, b.get("type").and_then(|x| x.as_str())) {
                ("assistant", Some("tool_use")) => self.tool_call(b),
                ("user", Some("tool_result")) => self.tool_result(b),
                _ => None,
            };
            if hit.is_some() {
                return hit;
            }
        }
        None
    }

    /// Codex `item.*` events: a command counts as a Bash call when it starts
    /// and as a result when it completes; a completed `file_change` edits
    /// each of its paths.
    fn codex_item(&mut self, ty: &str, item: &serde_json::Value) -> Option<LoopHit> {
        let started = ty == "item.started";
        match item.get("type").and_then(|x| x.as_str())? {
            "command_execution" if started => {
                let cmd = item.get("command").and_then(|x| x.as_str())?;
                self.call("Bash", serde_json::json!({ "command": cmd }))
            }
            "command_execution" if ty == "item.completed" => {
                let code = item.get("exit_code").and_then(|x| x.as_i64());
                let text = item
                    .get("aggregated_output")
                    .and_then(|x| x.as_str())
                    .unwrap_or("");
                self.result(
                    code.is_some_and(|c| c != 0),
                    format!("Exit code {}\n{text}", code.unwrap_or(0)),
                )
            }
            "file_change" if ty == "item.completed" => {
                let changes = item.get("changes")?.as_array()?;
                changes
                    .iter()
                    .filter_map(|c| c.get("path").and_then(|x| x.as_str()))
                    .find_map(|path| self.edit(path))
            }
            _ => None,
        }
    }

    fn tool_call(&mut self, b: &serde_json::Value) -> Option<LoopHit> {
        let name = b.get("name").and_then(|x| x.as_str()).unwrap_or("tool");
        let input = b.get("input").cloned().unwrap_or_default();
        self.call(name, input)
    }

    fn call(&mut self, name: &str, input: serde_json::Value) -> Option<LoopHit> {
        let call = (name.to_string(), input.to_string());
        if self.last_call.as_ref() == Some(&call) {
            self.call_run += 1;
        } else {
            self.call_run = 1;
            self.last_call = Some(call.clone());
        }
        if self.call_run >= REPEATED_CALLS {
            return Some(LoopHit {
                rule: "repeated_call",
                detail: format!(
                    "Loop detected (repeated_call): `{name}` was called {} times in a row with the identical input {}.",
                    self.call_run,
                    clip(&call.1, 300)
                ),
            });
        }
        if name == "Bash" {
            self.edits.clear();
        } else if EDIT_TOOLS.contains(&name) {
            let file = input
                .get("file_path")
                .or_else(|| input.get("notebook_path"))
                .and_then(|x| x.as_str())?;
            return self.edit(file);
        }
        None
    }

    fn edit(&mut self, file: &str) -> Option<LoopHit> {
        let n = self.edits.entry(file.to_string()).or_insert(0);
        *n += 1;
        (*n >= REPEATED_EDITS).then(|| LoopHit {
            rule: "repeated_edit",
            detail: format!(
                "Loop detected (repeated_edit): `{file}` was edited {n} times with no command run in between."
            ),
        })
    }

    fn tool_result(&mut self, b: &serde_json::Value) -> Option<LoopHit> {
        self.result(result_is_error(b), result_text(b))
    }

    fn result(&mut self, is_error: bool, text: String) -> Option<LoopHit> {
        if !is_error {
            self.error_run = 0;
            return None;
        }
        self.error_run += 1;
        (self.error_run >= ERROR_STREAK).then(|| LoopHit {
            rule: "error_streak",
            detail: format!(
                "Loop detected (error_streak): {} tool results in a row were errors; the last one: {}",
                self.error_run,
                clip(&text, 300)
            ),
        })
    }
}

fn result_text(b: &serde_json::Value) -> String {
    match b.get("content") {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .filter_map(|i| i.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// `is_error`, or a Bash result reporting a non-zero `Exit code N`.
fn result_is_error(b: &serde_json::Value) -> bool {
    if b.get("is_error").and_then(|x| x.as_bool()) == Some(true) {
        return true;
    }
    result_text(b)
        .strip_prefix("Exit code ")
        .and_then(|rest| rest.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|n| n.parse::<i32>().ok())
        .is_some_and(|n| n != 0)
}

fn clip(text: &str, max: usize) -> String {
    let line = text.trim();
    if line.chars().count() > max {
        format!("{}…", line.chars().take(max).collect::<String>())
    } else {
        line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(name: &str, input: serde_json::Value) -> String {
        json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"t","name":name,"input":input}]}}).to_string()
    }

    fn result(text: &str, is_error: bool) -> String {
        json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t","content":text,"is_error":is_error}]}}).to_string()
    }

    fn run(d: &mut LoopDetector, lines: &[String]) -> Option<LoopHit> {
        lines.iter().find_map(|l| d.feed(l))
    }

    #[test]
    fn the_same_call_three_times_in_a_row_fires_and_names_the_call() {
        let mut d = LoopDetector::new();
        let l = call("Read", json!({"file_path":"a.rs"}));
        assert!(d.feed(&l).is_none());
        assert!(d.feed(&l).is_none());
        let hit = d.feed(&l).unwrap();
        assert_eq!(hit.rule, "repeated_call");
        assert!(hit.detail.contains("`Read`") && hit.detail.contains("a.rs"));
    }

    #[test]
    fn a_different_call_in_between_resets_the_repeat_count() {
        let mut d = LoopDetector::new();
        let a = call("Read", json!({"file_path":"a.rs"}));
        let b = call("Read", json!({"file_path":"b.rs"}));
        assert!(run(&mut d, &[a.clone(), a.clone(), b, a.clone(), a]).is_none());
    }

    #[test]
    fn three_error_results_in_a_row_fire_but_a_success_resets() {
        let mut d = LoopDetector::new();
        let bad = result("nope", true);
        assert!(run(
            &mut d,
            &[
                bad.clone(),
                bad.clone(),
                result("ok", false),
                bad.clone(),
                bad.clone()
            ]
        )
        .is_none());
        assert_eq!(d.feed(&bad).unwrap().rule, "error_streak");
    }

    #[test]
    fn a_nonzero_bash_exit_counts_as_an_error() {
        let mut d = LoopDetector::new();
        let bad = result("Exit code 2\nboom", false);
        assert_eq!(
            run(&mut d, &[bad.clone(), bad.clone(), bad]).unwrap().rule,
            "error_streak"
        );
        let zero = result("Exit code 0\nfine", false);
        let mut d = LoopDetector::new();
        assert!(run(&mut d, &[zero.clone(), zero.clone(), zero]).is_none());
    }

    #[test]
    fn eight_edits_of_one_file_fire_unless_a_command_runs_between() {
        let edit = |n: u32| {
            call(
                "Edit",
                json!({"file_path":"src/a.rs","new_string":n.to_string()}),
            )
        };
        let mut d = LoopDetector::new();
        let seven: Vec<String> = (0..7).map(edit).collect();
        assert!(run(&mut d, &seven).is_none());
        let hit = d.feed(&edit(7)).unwrap();
        assert_eq!(hit.rule, "repeated_edit");
        assert!(hit.detail.contains("src/a.rs"));

        let mut d = LoopDetector::new();
        let mut lines: Vec<String> = (0..7).map(edit).collect();
        lines.push(call("Bash", json!({"command":"cargo test -p x one_test"})));
        lines.extend((7..14).map(edit));
        assert!(run(&mut d, &lines).is_none());
    }

    #[test]
    fn a_normal_run_does_not_fire() {
        let mut d = LoopDetector::new();
        let mut lines = vec![];
        for i in 0..4 {
            lines.push(call("Read", json!({"file_path": format!("f{i}.rs")})));
            lines.push(result("contents", false));
            lines.push(call(
                "Edit",
                json!({"file_path": format!("f{i}.rs"), "new_string": "x"}),
            ));
            lines.push(result("ok", false));
        }
        lines.push(call("Bash", json!({"command":"cargo test"})));
        lines.push(result("Exit code 1\nfailed", true));
        lines.push(call("Edit", json!({"file_path":"f0.rs","new_string":"y"})));
        lines.push(result("ok", false));
        assert!(run(&mut d, &lines).is_none());
    }

    #[test]
    fn codex_events_fire_the_same_rules() {
        let started = |c: &str| {
            json!({"type":"item.started","item":{"type":"command_execution","command":c}})
                .to_string()
        };
        let done = |code: i64| {
            json!({"type":"item.completed","item":{"type":"command_execution","command":"x","exit_code":code,"aggregated_output":"boom"}}).to_string()
        };
        let change = json!({"type":"item.completed","item":{"type":"file_change","changes":[{"path":"a.rs","kind":"update"}]}}).to_string();
        let mut d = LoopDetector::new();
        let l = started("ls");
        assert_eq!(
            run(&mut d, &[l.clone(), l.clone(), l]).unwrap().rule,
            "repeated_call"
        );
        let mut d = LoopDetector::new();
        let lines: Vec<String> = (0..3)
            .flat_map(|i| [started(&format!("c{i}")), done(1)])
            .collect();
        assert_eq!(run(&mut d, &lines).unwrap().rule, "error_streak");
        let mut d = LoopDetector::new();
        let lines = vec![change; 8];
        assert_eq!(run(&mut d, &lines).unwrap().rule, "repeated_edit");
        let mut d = LoopDetector::new();
        let lines: Vec<String> = (0..4)
            .flat_map(|i| [started(&format!("c{i}")), done(0)])
            .collect();
        assert!(run(&mut d, &lines).is_none());
    }
}
