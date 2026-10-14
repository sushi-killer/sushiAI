//! Replay of two first-pass cases end to end with fake harness scripts: five
//! subtasks on one file that become one task with a checklist, and a repeated
//! review finding that ends the review loop by round two.

mod common;

use common::*;
use serde_json::{json, Value};

fn decisions(task: &Value) -> Vec<String> {
    task["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d.as_str().map(String::from))
        .collect()
}

mod merged {
    use super::*;

    /// Plans the request as `$PARTS` (a ready `subtasks` JSON array body), plans
    /// a child from its `PART_` marker, and implements by writing the lines the
    /// brief asks for. `$ATTEMPT_LINES` is how many lines of `shared.txt` the
    /// first attempt writes; later attempts write five. Every run of the task's
    /// verify command appends the line count of `shared.txt` to `$VERIFY_LOG`.
    const MERGE_SCRIPT: &str = r#"#!/bin/sh
    input="$(cat)"
    emit() {
      printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
      printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"'"$1"'"}'
    }
    report='```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```'
    case "$input" in
      *sushi-plan*)
        case "$input" in
          *PART_X*) emit '```sushi-plan\n{\"title\":\"Part X\",\"goal\":\"PART_X: x.txt\",\"verify\":[\"test -f x.txt\"]}\n```' ;;
          *PART_Y*) emit '```sushi-plan\n{\"title\":\"Part Y\",\"goal\":\"PART_Y: y.txt\",\"verify\":[\"test -f y.txt\"]}\n```' ;;
          *) emit '```sushi-plan\n{\"title\":\"Whole\",\"goal\":\"WHOLE build it\",\"verify\":[\"echo $(cat shared.txt 2>/dev/null | wc -l) >> '"$VERIFY_LOG"'; true\"],\"subtasks\":'"$PARTS"'}\n```' ;;
        esac
        ;;
      *PART_X*) echo x > x.txt; emit "$report" ;;
      *PART_Y*) echo y > y.txt; emit "$report" ;;
      *)
        n=$(ls "$ATTEMPTS_DIR" 2>/dev/null | wc -l | tr -d ' ')
        touch "$ATTEMPTS_DIR/attempt.$$"
        lines=5
        if [ "$n" = "0" ]; then lines="$ATTEMPT_LINES"; fi
        i=1
        : > shared.txt
        while [ "$i" -le "$lines" ]; do echo "$i" >> shared.txt; i=$((i+1)); done
        emit "$report"
        ;;
    esac
    "#;

    const FIVE_PARTS: &str = r#"[{\"key\":\"s1\",\"title\":\"One\",\"request\":\"STEP_1 write line 1\",\"paths\":[\"shared.txt\"],\"check\":\"grep -qx 1 shared.txt\"},{\"key\":\"s2\",\"title\":\"Two\",\"request\":\"STEP_2 write line 2\",\"paths\":[\"shared.txt\"],\"check\":\"grep -qx 2 shared.txt\",\"dependsOn\":[\"s1\"]},{\"key\":\"s3\",\"title\":\"Three\",\"request\":\"STEP_3 write line 3\",\"paths\":[\"shared.txt\"],\"check\":\"grep -qx 3 shared.txt\"},{\"key\":\"s4\",\"title\":\"Four\",\"request\":\"STEP_4 write line 4\",\"paths\":[\"shared.txt\"],\"check\":\"grep -qx 4 shared.txt\"},{\"key\":\"s5\",\"title\":\"Five\",\"request\":\"STEP_5 write line 5\",\"paths\":[\"shared.txt\"],\"check\":\"grep -qx 5 shared.txt\"}]"#;

    struct Run {
        task: serde_json::Value,
        daemon: Daemon,
        _dir: tempfile::TempDir,
        _repo: tempfile::TempDir,
    }

    fn run(parts: &str, first_attempt_lines: &str) -> Run {
        let dir = tempfile::tempdir().unwrap();
        let script = fake_harness_script(dir.path(), "fake-merge.sh", MERGE_SCRIPT);
        let attempts = dir.path().join("attempts");
        std::fs::create_dir_all(&attempts).unwrap();
        let verify_log = dir.path().join("verify.log");
        let daemon = Daemon::spawn(&[
            ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
            ("PARTS", parts),
            ("ATTEMPTS_DIR", attempts.to_str().unwrap()),
            ("ATTEMPT_LINES", first_attempt_lines),
            ("VERIFY_LOG", verify_log.to_str().unwrap()),
        ]);
        let mut settings = daemon.request("settings.get", serde_json::json!({}));
        settings["review"] = serde_json::json!("");
        settings["planner"] = serde_json::json!("claude-sonnet");
        settings["tiers"]["hard"] = serde_json::json!("claude-sonnet");
        settings["tiers"]["standard"] = serde_json::json!("claude-sonnet");
        settings["tiers"]["mechanical"] = serde_json::json!("claude-sonnet");
        daemon.request("settings.set", serde_json::json!({"settings": settings}));
        let repo = init_git_repo();
        let created = daemon.request(
            "task.create",
            serde_json::json!({
                "repo": repo.path().to_str().unwrap(),
                "request": "build it in steps",
                "start": true,
            }),
        );
        let id = created["id"].as_str().unwrap().to_string();
        let task = poll_until(&daemon, &id, |s| {
            matches!(s, "done" | "failed" | "stopped" | "waiting")
        });
        Run {
            task,
            daemon,
            _dir: dir,
            _repo: repo,
        }
    }

    fn children(r: &Run) -> Vec<serde_json::Value> {
        let listed = r
            .daemon
            .request("task.list", serde_json::json!({"repo": r.task["repo"]}));
        listed
            .as_array()
            .unwrap()
            .iter()
            .filter(|t| t["parent"] == r.task["id"])
            .cloned()
            .collect()
    }

    #[test]
    fn five_same_file_subtasks_become_one_task_with_a_checklist() {
        let r = run(FIVE_PARTS, "5");
        assert_eq!(r.task["status"], "done", "task JSON: {}", r.task);
        assert!(children(&r).is_empty(), "no children expected");
        let checklist = r.task["checklist"].as_array().unwrap();
        assert_eq!(checklist.len(), 5, "{}", r.task);
        for (i, item) in checklist.iter().enumerate() {
            assert!(
                item["text"]
                    .as_str()
                    .unwrap()
                    .contains(&format!("STEP_{}", i + 1)),
                "{item}"
            );
            assert_eq!(item["passes"], true, "{item}");
        }
        assert!(
            decisions_of(&r.task)
                .iter()
                .any(|l| l == "Plan gate: merged 5 subtasks into one task with a checklist"),
            "{}",
            r.task
        );
        r.daemon.shutdown_and_wait();
    }

    fn decisions_of(task: &serde_json::Value) -> Vec<String> {
        decisions(task)
    }
}

mod reviewed {
    use super::*;

    /// One fake for every Claude role. A review brief ("## Review...") takes the
    /// n-th line of `$LOG_DIR/review.plan` (the last line again when n runs
    /// past it) and is kept as `review.brief.<n>`. An advisor brief gets advice.
    /// An implement run keeps its argv as `implement.args.<pid>`, appends to
    /// CHANGED_MARKER.txt, creates `fixed` when its brief names a criterion made
    /// from a review finding, and answers `implement.reply`, or `implement.disputed`
    /// when its brief shows a failed review and that file exists.
    const FAKE_ROUNDS_SCRIPT: &str = r###"#!/bin/sh
    brief=$(cat)
    case "$brief" in
    "An implement attempt at this task failed"*)
      printf '%s\n' '{"type":"result","total_cost_usd":0.03,"usage":{"input_tokens":1,"output_tokens":1},"result":"Fix it."}' ;;
    "## Review"*)
      n=$(( $(ls "$LOG_DIR"/review.brief.* 2>/dev/null | wc -l) + 1 ))
      printf '%s' "$brief" > "$LOG_DIR/review.brief.$n"
      line=$(sed -n "${n}p" "$LOG_DIR/review.plan")
      [ -z "$line" ] && line=$(tail -n 1 "$LOG_DIR/review.plan")
      printf '%s\n' "$line" ;;
    *)
      printf '%s\n' "$@" > "$LOG_DIR/implement.args.$$"
      echo changed >> CHANGED_MARKER.txt
      case "$brief" in *"Review finding F-"*) touch "$LOG_DIR/fixed" ;; esac
      printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
      reply="$LOG_DIR/implement.reply"
      case "$brief" in
      *"The last attempt failed review"*) [ -f "$LOG_DIR/implement.disputed" ] && reply="$LOG_DIR/implement.disputed" ;;
      esac
      cat "$reply" ;;
    esac
    "###;

    const JUDGE_SCRIPT: &str = "#!/bin/sh\n[ \"$1\" = --version ] && { echo codex-cli 0.1; exit 0; }\nbrief=$(cat)\necho judge >> \"$LOG_DIR/judge.log\"\nprintf '%s' \"$brief\" > \"$LOG_DIR/judge.brief.$$\"\nprintf '%s\\n' '{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"```sushi-judge\\n{\\\"finding_valid\\\": false, \\\"why\\\": \\\"the ruling\\\"}\\n```\"}}'\n";

    fn review_line(body: Value) -> String {
        json!({"type": "result", "result": format!("```sushi-review\n{body}\n```")}).to_string()
    }

    fn fail(findings: Value) -> String {
        review_line(json!({"verdict": "FAIL", "findings": findings}))
    }

    fn report(summary: &str, disputes: Value) -> String {
        let body = json!({
            "outcome": "complete", "summary": summary, "decisions": [], "question": "",
            "disputes": disputes,
        });
        json!({
            "type": "result", "total_cost_usd": 0.01,
            "usage": {"input_tokens": 1, "output_tokens": 1},
            "result": format!("```sushi-report\n{body}\n```"),
        })
        .to_string()
    }

    struct Rig {
        daemon: Daemon,
        dir: tempfile::TempDir,
        repo: tempfile::TempDir,
    }

    impl Rig {
        fn new(plan: &[String]) -> Rig {
            let dir = tempfile::tempdir().unwrap();
            let claude = fake_harness_script(dir.path(), "fake-claude.sh", FAKE_ROUNDS_SCRIPT);
            let codex = fake_harness_script(dir.path(), "fake-judge.sh", JUDGE_SCRIPT);
            std::fs::write(dir.path().join("review.plan"), plan.join("\n") + "\n").unwrap();
            std::fs::write(
                dir.path().join("implement.reply"),
                report("done", json!([])),
            )
            .unwrap();
            let daemon = Daemon::spawn(&[
                ("ORCHD_CLAUDE_BIN", claude.to_str().unwrap()),
                ("ORCHD_CODEX_BIN", codex.to_str().unwrap()),
                ("LOG_DIR", dir.path().to_str().unwrap()),
            ]);
            let mut settings = daemon.request("settings.get", json!({}));
            settings["planReview"] = json!("");
            settings["maxAttempts"] = json!(6);
            settings["answerPolicy"] = json!(false);
            settings["review"] = json!("claude-opus");
            // Sonnet stays the implementer but is too weak to judge: Codex judges, at
            // any tier.
            for route in settings["routes"].as_array_mut().unwrap() {
                if route["id"] == "claude-sonnet" {
                    route["strength"] = json!(1);
                }
                if route["id"] == "codex" {
                    route["strength"] = json!(3);
                }
            }
            daemon.request("settings.set", json!({"settings": settings}));
            Rig {
                daemon,
                dir,
                repo: init_git_repo(),
            }
        }

        fn file(&self, name: &str, text: &str) {
            std::fs::write(self.dir.path().join(name), text).unwrap();
        }

        fn run(&self, goal: &str) -> (String, Value) {
            let task = self.daemon.request(
                "task.create",
                json!({
                    "repo": self.repo.path().to_str().unwrap(),
                    "title": "Rounds",
                    "goal": goal,
                    "verify": ["true"],
                    "start": true,
                }),
            );
            let id = task["id"].as_str().unwrap().to_string();
            let settled = poll_task_status(&self.daemon, &id);
            (id, settled)
        }

        /// Review runs recorded for one attempt (helper folders, in order).
        fn review_runs(&self, task_id: &str, attempt: u32) -> usize {
            sushiai_orch::store::Store::new(self.daemon.data_dir())
                .unwrap()
                .helper_run_dirs(task_id, "review", Some(attempt))
                .len()
        }

        fn finish(self, settled: &Value) {
            let worktree = settled["worktree"].as_str().map(str::to_string);
            self.daemon.shutdown_and_wait();
            if let Some(w) = worktree {
                let _ = std::fs::remove_dir_all(w);
            }
        }
    }

    #[test]
    fn a_repeated_review_finding_ends_the_review_loop_by_round_two() {
        // The reviewer reports the same finding on every run and the implementer
        // disputes it each time. No attempt may get a third review run, and the
        // owner is asked only when the attempt budget is spent.
        let again = json!({"finding": "P1: a.rs:1 - null check missing", "repeat": true});
        let rig = Rig::new(&[
            fail(json!(["P1: a.rs:1 - null check missing"])),
            fail(json!([again])),
        ]);
        rig.file(
            "implement.disputed",
            &report(
                "done",
                json!([{"finding": "a.rs:1 - null check missing", "rebuttal": "it is checked", "evidence": []}]),
            ),
        );
        let (id, settled) = rig.run("Write the marker");
        assert_eq!(settled["status"], "failed", "{settled}");
        let attempts = settled["attempts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|a| a["stage"] == "implement")
            .count();
        assert!(attempts >= 3, "{settled}");
        for n in 1..=attempts as u32 {
            let runs = rig.review_runs(&id, n);
            assert!(runs >= 1, "attempt {n} was reviewed");
            assert!(runs <= 2, "attempt {n} got a third run");
        }
        // Attempt 2: the judge dropped nothing (it rules once per finding), so
        // the review ran once more and ended on round two.
        assert!(rig.review_runs(&id, 2) >= 1);
        let judged = std::fs::read_to_string(rig.dir.path().join("judge.log"))
            .map(|t| t.lines().count())
            .unwrap_or(0);
        assert!(judged <= 1, "the finding is judged once: {judged}");
        rig.finish(&settled);
    }
}
