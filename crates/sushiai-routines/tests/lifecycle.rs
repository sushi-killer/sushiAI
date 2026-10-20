//! Run lifecycle with a fake daemon connector, a fake git and a fake clock. Names are
//! synthetic (my-app, build-host).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use sushiai_routines::argv::{harness_argv, skill_call, RunSpec};
use sushiai_routines::{Agent, Config, Connector, Git, Routines};

#[path = "../../test_wait.rs"]
mod test_wait;

/// The levels the agent catalog offers a harness. Claude takes all five, Codex the three a
/// routine may ask for; the same table `sushiai agents catalog` prints. The module crate
/// receives them as data, so a test stands in for the composition bin here.
fn levels(harness: &str) -> &'static [String] {
    static LEVELS: std::sync::OnceLock<(Vec<String>, Vec<String>)> = std::sync::OnceLock::new();
    let words = |list: &[&str]| list.iter().map(|level| (*level).to_string()).collect();
    let (claude, codex) = LEVELS.get_or_init(|| {
        (
            words(&["low", "medium", "high", "xhigh", "max"]),
            words(&["low", "medium", "high"]),
        )
    });
    if harness == "claude" {
        claude
    } else {
        codex
    }
}

#[derive(Default)]
struct State {
    schedules: BTreeMap<String, Value>,
    /// Every `session.create` params, in order.
    created: Vec<Value>,
    outputs: BTreeMap<String, String>,
    /// What the next created sessions print (or write for Codex), in order.
    answers: VecDeque<String>,
    next: u64,
    removed: Vec<String>,
    catalog: Value,
    /// `schedule.fire` starts a run instead of skipping.
    fire_ok: bool,
    /// The exit code `session.list` reports for ended sessions.
    exit_code: Option<i64>,
    /// Sessions the daemon closed.
    closed: Vec<String>,
    /// Sessions still reported `running` (until released).
    hold: BTreeSet<String>,
    /// Sessions the daemon has no output for (the module never captured them).
    unread: BTreeSet<String>,
    /// Sessions the module was asked to watch, in order.
    watched: Vec<String>,
    /// Every `hook.ask` the module forwarded.
    asked: Vec<Value>,
    /// What the owner answers (`null` expires the ask).
    owner_answer: Value,
}

struct FakeDaemon {
    state: Mutex<State>,
    /// A script run for each session instead of a canned answer.
    script: Mutex<Option<PathBuf>>,
}

impl FakeDaemon {
    fn new(folder: &Path) -> Arc<FakeDaemon> {
        let catalog = json!({
            "host": "build-host",
            "projects": [{
                "id": "my-app", "name": "my-app",
                "folders": [{"host": "build-host", "path": folder.to_string_lossy()}],
            }, {
                "id": "docs-site", "name": "docs-site", "purpose": "The public docs",
                "folders": [{"host": "build-host",
                             "path": folder.with_file_name("docs-site").to_string_lossy()}],
            }],
            "groups": [],
        });
        Arc::new(FakeDaemon {
            state: Mutex::new(State {
                catalog,
                ..Default::default()
            }),
            script: Mutex::new(None),
        })
    }

    fn answer(&self, text: &str) {
        self.state
            .lock()
            .unwrap()
            .answers
            .push_back(text.to_string());
    }

    fn created(&self) -> Vec<Value> {
        self.state.lock().unwrap().created.clone()
    }

    /// What the daemon's scheduler does at a slot: create the session of the entry's
    /// `create`, then report `fired` and `ended`.
    fn fire(&self, svc: &Routines, routine: &str, slot: u64) {
        let session = self.start(svc, routine, slot);
        self.end(svc, routine, &session, slot);
    }

    /// The slot starts: the session is created and `fired` reported.
    fn start(&self, svc: &Routines, routine: &str, slot: u64) -> String {
        let set = self
            .state
            .lock()
            .unwrap()
            .schedules
            .get(&format!("routine.{routine}"))
            .cloned()
            .expect("the routine has a schedule entry");
        let session = self.session_create(set["create"].clone()).unwrap();
        let id = format!("routine.{routine}");
        svc.on_schedule_event(&json!({
            "id": id, "kind": "fired", "session": session,
            "scheduledFor": slot, "startedAt": slot,
        }));
        session
    }

    fn end(&self, svc: &Routines, routine: &str, session: &str, slot: u64) {
        svc.on_schedule_event(&json!({
            "id": format!("routine.{routine}"), "kind": "ended", "session": session,
            "scheduledFor": slot, "startedAt": slot, "endedAt": slot + 1,
        }));
    }
}

impl Connector for FakeDaemon {
    fn catalog(&self) -> Result<Value, String> {
        Ok(self.state.lock().unwrap().catalog.clone())
    }
    fn schedule_set(&self, set: Value) -> Result<(), String> {
        let id = set["id"].as_str().unwrap().to_string();
        self.state.lock().unwrap().schedules.insert(id, set);
        Ok(())
    }
    fn schedule_remove(&self, id: &str) -> Result<(), String> {
        let mut s = self.state.lock().unwrap();
        s.removed.push(id.to_string());
        // Like the daemon: removing an entry that is not there is an error.
        match s.schedules.remove(id) {
            Some(_) => Ok(()),
            None => Err(format!("no schedule {id}")),
        }
    }
    fn schedule_ids(&self) -> Result<Vec<String>, String> {
        Ok(self
            .state
            .lock()
            .unwrap()
            .schedules
            .keys()
            .cloned()
            .collect())
    }
    fn schedule_fire(&self, _id: &str) -> Result<Value, String> {
        if self.state.lock().unwrap().fire_ok {
            return Ok(json!({"fired": true}));
        }
        Ok(json!({"skipped": "a run is already live"}))
    }
    fn session_create(&self, params: Value) -> Result<String, String> {
        let script = self.script.lock().unwrap().clone();
        let mut s = self.state.lock().unwrap();
        s.next += 1;
        let id = format!("s{}", s.next);
        let answer = match &script {
            Some(path) => {
                let out = std::process::Command::new("sh")
                    .arg(path)
                    .output()
                    .map_err(|e| e.to_string())?;
                String::from_utf8_lossy(&out.stdout).into_owned()
            }
            None => s.answers.pop_front().unwrap_or_default(),
        };
        // Codex writes its answer to the `-o` file; Claude prints it.
        let args: Vec<String> = params["extraArgs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        if let Some(i) = args.iter().position(|a| a == "-o") {
            std::fs::write(&args[i + 1], &answer).unwrap();
            s.outputs.insert(id.clone(), String::new());
        } else {
            s.outputs.insert(id.clone(), answer);
        }
        s.created.push(params);
        Ok(id)
    }
    fn session_info(&self, id: &str) -> Result<Value, String> {
        let s = self.state.lock().unwrap();
        let status = if s.hold.contains(id) {
            "running"
        } else {
            "exited"
        };
        let mut info = json!({"id": id, "status": status, "agentSession": format!("agent-{id}")});
        if status == "exited" {
            if let Some(code) = s.exit_code {
                info["exitCode"] = json!(code);
            }
        }
        Ok(info)
    }
    fn session_watch(&self, id: &str) {
        self.state.lock().unwrap().watched.push(id.to_string());
    }
    fn session_read(&self, id: &str) -> Result<String, String> {
        let s = self.state.lock().unwrap();
        if s.unread.contains(id) {
            return Err(format!("no output was captured for session {id}"));
        }
        Ok(s.outputs.get(id).cloned().unwrap_or_default())
    }
    fn session_close(&self, id: &str) -> Result<(), String> {
        let mut s = self.state.lock().unwrap();
        s.closed.push(id.to_string());
        s.hold.remove(id);
        Ok(())
    }
    fn hook_ask(&self, params: Value) -> Result<Value, String> {
        let mut s = self.state.lock().unwrap();
        s.asked.push(params);
        Ok(s.owner_answer.clone())
    }
}

#[derive(Default)]
struct FakeGit {
    /// Every command with the folder it ran in.
    calls: Mutex<Vec<(Vec<String>, PathBuf)>>,
    fail_clone: bool,
    /// Whether `a.txt` was in the folder when `git commit` ran.
    commit_saw_file: Mutex<Vec<bool>>,
    /// `git commit` fails while set.
    fail_commit: Mutex<bool>,
}

impl FakeGit {
    fn words(&self) -> Vec<Vec<String>> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(|(a, _)| a.clone())
            .collect()
    }
}

impl Git for FakeGit {
    fn run(&self, argv: &[String], cwd: &Path) -> Result<String, String> {
        self.calls
            .lock()
            .unwrap()
            .push((argv.to_vec(), cwd.to_path_buf()));
        let sub = argv.get(1).map(String::as_str);
        if self.fail_clone && sub == Some("clone") {
            return Err("fatal: repository not found".into());
        }
        match sub {
            // The branch does not exist yet.
            Some("rev-parse") => return Err("".into()),
            // A real `git worktree add` creates the folder it is given.
            Some("worktree") if argv.get(2).map(String::as_str) == Some("add") => {
                for a in argv.iter().skip(3).filter(|a| a.starts_with('/')) {
                    std::fs::create_dir_all(a).unwrap();
                    std::fs::write(Path::new(a).join(".git"), "gitdir: x").unwrap();
                }
            }
            Some("commit") if *self.fail_commit.lock().unwrap() => {
                return Err("fatal: unable to write the commit".into());
            }
            Some("commit") => self
                .commit_saw_file
                .lock()
                .unwrap()
                .push(cwd.join("a.txt").exists()),
            _ => {}
        }
        Ok("ok".into())
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    daemon: Arc<FakeDaemon>,
    git: Arc<FakeGit>,
    events: Arc<Mutex<Vec<Value>>>,
    clock: Arc<AtomicU64>,
    svc: Routines,
}

fn fixture_with(git: FakeGit) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("my-app");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(dir.path().join("docs-site")).unwrap();
    let daemon = FakeDaemon::new(&project);
    let git = Arc::new(git);
    let events = Arc::new(Mutex::new(Vec::new()));
    let clock = Arc::new(AtomicU64::new(1_800_000_000_000));
    let svc = Routines::new(Config {
        root: dir.path().join("routines"),
        connector: daemon.clone(),
        git: git.clone(),
        emit: {
            let events = events.clone();
            Arc::new(move |e| events.lock().unwrap().push(e))
        },
        now_ms: {
            let clock = clock.clone();
            Arc::new(move || clock.load(Ordering::SeqCst))
        },
        poll: Duration::from_millis(5),
        builder_efforts: ["claude", "codex"]
            .into_iter()
            .map(|harness| (harness.to_string(), levels(harness).to_vec()))
            .collect(),
        run_path: Some("/opt/sushiai/bin:/usr/bin".into()),
    });
    Fixture {
        _dir: dir,
        daemon,
        git,
        events,
        clock,
        svc,
    }
}

fn fixture() -> Fixture {
    fixture_with(FakeGit::default())
}

fn routine_json(id: &str) -> Value {
    json!({
        "id": id,
        "name": "Commit digest",
        "where": {"host": "ssh:build-host", "projects": ["my-app"]},
        "when": {"cron": "0 9 * * 1-5", "tz": "Europe/Berlin"},
        "agent": {"harness": "claude", "model": "sonnet", "effort": "medium",
                  "account": "default",
                  "extraArgs": ["--allowedTools", "Bash(sushiai:*) Read Grep Glob"]},
        "howItRuns": {"mode": "ask-first", "promoteAfter": 5, "accepted": 0},
        "limits": {"timeoutMin": 30},
        "enabled": false,
    })
}

fn install(f: &Fixture, id: &str) {
    f.svc
        .call(
            "save",
            json!({"routine": routine_json(id), "brief": "Summarise the last 5 commits."}),
        )
        .unwrap();
    f.svc
        .call("set_enabled", json!({"id": id, "enabled": true}))
        .unwrap();
}

fn good(status: &str) -> String {
    json!({"status": status, "title": "Digest", "summary": "5 commits.",
           "actions": [{"kind": "slack.post", "text": "post", "applied": true}]})
    .to_string()
}

fn envelope(inner: &str) -> String {
    json!({"type": "result", "subtype": "success",
           "structured_output": serde_json::from_str::<Value>(inner).unwrap()})
    .to_string()
}

fn inbox(f: &Fixture) -> Vec<Value> {
    f.events
        .lock()
        .unwrap()
        .iter()
        .filter(|e| e["inbox"] == true)
        .cloned()
        .collect()
}

fn latest(f: &Fixture, id: &str) -> Value {
    f.svc.call("result", json!({"id": id})).unwrap()
}

fn wait_for_run(f: &Fixture, id: &str) -> Value {
    test_wait::wait_until("the run to finish", || {
        f.svc
            .call("result", json!({"id": id}))
            .ok()
            .filter(|v| v["run"]["status"] != "running")
    })
}

#[test]
fn argv_for_claude_and_codex_is_exact() {
    let spec = RunSpec {
        schema_json: "{\"s\":1}",
        schema_file: Path::new("/r/result.schema.json"),
        last_message: Path::new("/r/last.json"),
        dry_run: false,
        granted: &[],
        read_only: &[],
        read_dirs: &[],
        skills: &[],
        efforts: levels("claude"),
    };
    let codex_spec = RunSpec {
        efforts: levels("codex"),
        ..spec
    };
    let claude = Agent {
        harness: "claude".into(),
        model: Some("sonnet".into()),
        effort: Some("high".into()),
        account: None,
        extra_args: vec!["--allowedTools".into(), "Read".into()],
    };
    assert_eq!(
        harness_argv(&claude, &spec).unwrap(),
        [
            "-p",
            "--output-format",
            "json",
            "--json-schema",
            "{\"s\":1}",
            "--model",
            "sonnet",
            "--effort",
            "high",
            "--permission-mode",
            "dontAsk",
            // The run's allowlist is the base set plus the owner's grants: the routine's own
            // `--allowedTools` ("Read" above) is not passed through.
            "--allowedTools",
            "Read,Grep,Glob,Bash(sushiai ask:*),Bash(sushiai propose:*)",
            // No declared skill: the Skill tool is denied.
            "--disallowedTools",
            "Skill"
        ]
    );
    let granted = vec![
        "Bash(gh issue list:*)".to_string(),
        "mcp__tracker__comment".to_string(),
    ];
    let with_grants = RunSpec {
        granted: &granted,
        ..RunSpec {
            schema_json: "{\"s\":1}",
            schema_file: Path::new("/r/result.schema.json"),
            last_message: Path::new("/r/last.json"),
            dry_run: false,
            granted: &[],
            read_only: &[],
            read_dirs: &[],
            skills: &[],
            efforts: levels("claude"),
        }
    };
    let argv = harness_argv(&claude, &with_grants).unwrap();
    let i = argv.iter().position(|a| a == "--allowedTools").unwrap();
    assert_eq!(
        argv[i + 1],
        "Read,Grep,Glob,Bash(sushiai ask:*),Bash(sushiai propose:*),\
         Bash(gh issue list:*),mcp__tracker__comment"
    );
    assert_eq!(argv.iter().filter(|a| *a == "--allowedTools").count(), 1);
    // A dry run uses only the granted tools whose kind is read (`read_only`), never the rest
    // of the grants.
    let dry_read = vec!["Bash(gh issue list:*)".to_string()];
    let dry_with_grants = RunSpec {
        dry_run: true,
        granted: &granted,
        read_only: &dry_read,
        ..RunSpec {
            schema_json: "{}",
            schema_file: Path::new("s"),
            last_message: Path::new("l"),
            dry_run: true,
            granted: &[],
            read_only: &[],
            read_dirs: &[],
            skills: &[],
            efforts: levels("claude"),
        }
    };
    let argv = harness_argv(&claude, &dry_with_grants).unwrap();
    let i = argv.iter().position(|a| a == "--allowedTools").unwrap();
    assert_eq!(
        argv[i + 1],
        "Read,Grep,Glob,Bash(git status:*),Bash(sushiai ask:*),Bash(sushiai propose:*),\
         Bash(gh issue list:*)"
    );
    assert!(!argv.join("|").contains("tracker__comment"));
    // Folders to read become one `--add-dir=` each, on a run and on a dry run.
    let dirs = vec!["/p/one".to_string(), "/p/two".to_string()];
    let with_dirs = RunSpec {
        read_dirs: &dirs,
        ..dry_with_grants
    };
    let argv = harness_argv(&claude, &with_dirs).unwrap();
    let added: Vec<&String> = argv.iter().filter(|a| a.starts_with("--add-dir")).collect();
    assert_eq!(added, ["--add-dir=/p/one", "--add-dir=/p/two"]);
    let codex = Agent {
        harness: "codex".into(),
        model: Some("gpt-5".into()),
        effort: Some("low".into()),
        account: None,
        extra_args: vec!["--ephemeral".into()],
    };
    assert_eq!(
        harness_argv(&codex, &codex_spec).unwrap(),
        [
            "exec",
            "--ignore-user-config",
            "--output-schema",
            "/r/result.schema.json",
            "-o",
            "/r/last.json",
            "-m",
            "gpt-5",
            "-c",
            "model_reasoning_effort=low",
            "-c",
            "mcp_servers={}",
            "-s",
            "workspace-write",
            "--skip-git-repo-check",
            "--ephemeral"
        ]
    );
    // xhigh and max are Claude only.
    let mut c = codex.clone();
    c.effort = Some("xhigh".into());
    assert!(harness_argv(&c, &codex_spec).is_err());
    // Codex has no extra readable folders to give.
    let codex_dirs = RunSpec {
        read_dirs: &dirs,
        ..codex_spec
    };
    assert!(harness_argv(&codex, &codex_dirs).is_err());
    let mut cl = claude.clone();
    cl.effort = Some("max".into());
    assert!(harness_argv(&cl, &spec).is_ok());
    // A level the harness does not offer is refused, and an empty list refuses every one.
    let mut high = claude.clone();
    high.effort = Some("high".into());
    assert!(harness_argv(
        &high,
        &RunSpec {
            efforts: &[],
            ..spec
        }
    )
    .is_err());
    // A dry run is an allowlist, not a list of known writes.
    let dry = RunSpec {
        dry_run: true,
        ..spec
    };
    let argv = harness_argv(&claude, &dry).unwrap();
    let value = |flag: &str| {
        let i = argv.iter().position(|a| a == flag).unwrap();
        argv[i + 1].clone()
    };
    let allowed: Vec<String> = value("--allowedTools")
        .split(',')
        .map(String::from)
        .collect();
    assert_eq!(
        allowed,
        [
            "Read",
            "Grep",
            "Glob",
            "Bash(git status:*)",
            "Bash(sushiai ask:*)",
            "Bash(sushiai propose:*)"
        ]
    );
    // No allowed command can write a file: `git log|diff|show` take `--output=<file>`.
    for tool in &allowed {
        if let Some(cmd) = tool.strip_prefix("Bash(") {
            assert!(
                ["git status:*)", "sushiai ask:*)", "sushiai propose:*)"].contains(&cmd),
                "{tool} may write"
            );
        }
    }
    assert_eq!(value("--disallowedTools"), "Edit,Write,NotebookEdit");
    assert_eq!(value("--tools"), "Read,Grep,Glob,Bash");
    assert_eq!(value("--permission-mode"), "dontAsk");

    // A routine's own extraArgs cannot widen a dry run; harmless ones stay.
    let mut wide = claude.clone();
    wide.extra_args = [
        "--allowedTools",
        "Bash(rm:*)",
        "Write",
        "--add-dir",
        "/etc",
        "--fallback-model",
        "haiku",
        "--max-turns=7",
    ]
    .map(String::from)
    .to_vec();
    let argv = harness_argv(&wide, &dry).unwrap();
    let text = argv.join("|");
    assert!(
        !text.contains("Bash(rm:*)") && !text.contains("/etc"),
        "{text}"
    );
    assert_eq!(argv.iter().filter(|a| *a == "--allowedTools").count(), 1);
    assert!(text.contains("--fallback-model|haiku") && text.contains("--max-turns=7"));

    // Codex: read-only sandbox, no network, no write root.
    let argv = harness_argv(&codex, &dry).unwrap();
    let text = argv.join(" ");
    assert!(text.contains("-s read-only"), "{text}");
    assert!(text.contains("-c mcp_servers={}"), "{text}");
    assert!(!text.contains("workspace-write") && !text.contains("network_access"));
    // A real Codex run never gets network access, so it cannot reach the daemon socket.
    let real = harness_argv(&codex, &spec).unwrap().join(" ");
    assert!(!real.contains("network_access"), "{real}");
    assert!(!argv.contains(&"--ephemeral".to_string()));
}

fn spec_with<'a>(dry_run: bool, skills: &'a [String]) -> RunSpec<'a> {
    RunSpec {
        schema_json: "{}",
        schema_file: Path::new("s"),
        last_message: Path::new("l"),
        dry_run,
        granted: &[],
        read_only: &[],
        read_dirs: &[],
        skills,
        efforts: levels("claude"),
    }
}

fn flag_value(argv: &[String], flag: &str) -> Option<String> {
    let i = argv.iter().position(|a| a == flag)?;
    Some(argv[i + 1].clone())
}

#[test]
fn skills_decide_the_claude_skill_tool_in_every_run_shape() {
    let agent = |harness: &str, extra: &[&str]| Agent {
        harness: harness.into(),
        model: None,
        effort: None,
        account: None,
        extra_args: extra.iter().map(|s| s.to_string()).collect(),
    };
    let none: Vec<String> = vec![];
    let some = vec!["brand".to_string()];
    let claude = agent("claude", &[]);
    // Real run, no skills: denied. With skills: no deny flag at all.
    let argv = harness_argv(&claude, &spec_with(false, &none)).unwrap();
    assert_eq!(flag_value(&argv, "--disallowedTools").unwrap(), "Skill");
    let argv = harness_argv(&claude, &spec_with(false, &some)).unwrap();
    assert_eq!(flag_value(&argv, "--disallowedTools"), None);
    // The routine's own deny list is merged into the one flag, in both spellings.
    for extra in [
        vec!["--disallowedTools", "Write", "WebFetch"],
        vec!["--disallowedTools=Write,WebFetch"],
    ] {
        let c = agent("claude", &extra);
        let argv = harness_argv(&c, &spec_with(false, &none)).unwrap();
        assert_eq!(
            argv.iter()
                .filter(|a| a.starts_with("--disallowed"))
                .count(),
            1
        );
        assert_eq!(
            flag_value(&argv, "--disallowedTools").unwrap(),
            "Write,WebFetch,Skill"
        );
        assert!(!argv.join("|").contains("--disallowedTools=Write"));
    }
    // The routine's own deny list is kept as is when skills are declared.
    let c = agent("claude", &["--disallowedTools", "Write"]);
    let argv = harness_argv(&c, &spec_with(false, &some)).unwrap();
    assert_eq!(flag_value(&argv, "--disallowedTools").unwrap(), "Write");
    // Dry run: Skill joins the fixed tool set only with skills, and nothing else changes.
    let argv = harness_argv(&claude, &spec_with(true, &none)).unwrap();
    assert_eq!(flag_value(&argv, "--tools").unwrap(), "Read,Grep,Glob,Bash");
    let argv = harness_argv(&claude, &spec_with(true, &some)).unwrap();
    assert_eq!(
        flag_value(&argv, "--tools").unwrap(),
        "Read,Grep,Glob,Bash,Skill"
    );
    assert_eq!(
        flag_value(&argv, "--disallowedTools").unwrap(),
        "Edit,Write,NotebookEdit"
    );
    assert!(!flag_value(&argv, "--allowedTools")
        .unwrap()
        .contains("Skill"));
    // Codex argv does not depend on skills.
    let codex = agent("codex", &[]);
    for dry in [false, true] {
        assert_eq!(
            harness_argv(&codex, &spec_with(dry, &none)).unwrap(),
            harness_argv(&codex, &spec_with(dry, &some)).unwrap()
        );
        assert!(!harness_argv(&codex, &spec_with(dry, &some))
            .unwrap()
            .join(" ")
            .contains("Skill"));
    }
    assert_eq!(skill_call("claude", "brand"), "/brand");
    assert_eq!(skill_call("codex", "brand"), "$brand");
}

#[test]
fn skills_are_validated_hashed_and_named_in_the_prompt() {
    let f = fixture();
    let check = |skills: Value| {
        let mut r = routine_json("r1");
        r["skills"] = skills;
        f.svc
            .call("validate", json!({"routine": r, "brief": "b"}))
            .unwrap()
    };
    assert_eq!(
        check(json!(["brand", "ponytail:ponytail-help", "a.b_c-1"]))["ok"],
        true
    );
    for bad in [
        json!(["-x"]),
        json!([""]),
        json!(["has space"]),
        json!(["../x"]),
        json!(["a/b"]),
        json!(["x".repeat(65)]),
        json!(["a", "a"]),
        json!((0..11).map(|i| format!("s{i}")).collect::<Vec<_>>()),
    ] {
        let v = check(bad.clone());
        assert_eq!(v["ok"], false, "{bad} was accepted");
        assert!(v["errors"][0]["field"]
            .as_str()
            .unwrap()
            .starts_with("skills"));
    }
    // Absent skills leave the hash as it was; present ones change it.
    let none = check(json!([]))["hash"].clone();
    let mut plain = routine_json("r1");
    let absent = f
        .svc
        .call("validate", json!({"routine": plain.clone(), "brief": "b"}))
        .unwrap()["hash"]
        .clone();
    assert_eq!(none, absent);
    plain["skills"] = json!(["brand"]);
    let with = f
        .svc
        .call("validate", json!({"routine": plain, "brief": "b"}))
        .unwrap()["hash"]
        .clone();
    assert_ne!(with, absent);
    // The prompt points at each skill, in the harness's own syntax; the brief stays intact.
    for (harness, call) in [("claude", "/brand"), ("codex", "$brand")] {
        install_with(&f, "r2", true, &|r| {
            r["agent"]["harness"] = json!(harness);
            r["skills"] = json!(["brand"]);
        });
        let create = entry(&f, "r2")["create"].clone();
        let prompt = create["prompt"].as_str().unwrap();
        assert!(
            prompt.contains(&format!("Use the skill {call} first")),
            "{prompt}"
        );
        assert!(prompt.ends_with("Summarise."));
        let shown = f.svc.call("show", json!({"id": "r2"})).unwrap();
        assert_eq!(shown["routine"]["skills"], json!(["brand"]));
    }
    // No skills: no skill line.
    install_with(&f, "r3", true, &|_| {});
    let create = entry(&f, "r3")["create"].clone();
    assert!(!create["prompt"].as_str().unwrap().contains("Use the skill"));
}

#[test]
fn skill_scripts_are_accepted_only_in_the_two_narrow_forms() {
    let f = fixture();
    let check = |tool: &str, skills: Value| {
        let mut r = routine_json("r1");
        r["skills"] = skills;
        r["access"] = json!([access(tool, "Run it", "read")]);
        f.svc
            .call("validate", json!({"routine": r, "brief": "b"}))
            .unwrap()
    };
    let brand = json!(["brand"]);
    for tool in [
        "Bash(/home/me/.claude/skills/brand/scripts/run.sh:*)",
        "Bash(python3 /home/me/.claude/skills/brand/scripts/run.py:*)",
        "Bash(python /home/me/.codex/skills/brand/scripts/run.py:*)",
        "Bash(node /home/me/.agents/skills/brand/scripts/run.mjs:*)",
        "Bash(bash /home/me/.claude/skills/brand/run.sh:*)",
        "Bash(sh /my-app/.claude/skills/brand/scripts/a/b.sh:*)",
    ] {
        let v = check(tool, brand.clone());
        assert_eq!(v["ok"], true, "{tool}: {v}");
    }
    // A plugin skill matches its own folder name.
    assert_eq!(
        check(
            "Bash(node /p/skills/ponytail-help/scripts/x.js:*)",
            json!(["ponytail:ponytail-help"])
        )["ok"],
        true
    );
    for tool in [
        "Bash(python3 -c print(1):*)",
        "Bash(python3:*)",
        "Bash(node:*)",
        "Bash(python3 /home/me/.claude/skills/other/scripts/run.py:*)",
        "Bash(python3 /home/me/scripts/run.py:*)",
        "Bash(python3 /home/me/.claude/skills/brand:*)",
        "Bash(python3 /home/me/.claude/skills/brand/:*)",
        "Bash(python3 /home/me/.claude/skills/brand/../x/run.py:*)",
        "Bash(python3 /home/me/.claude/skills/brand/scripts/../../run.py:*)",
        "Bash(python3 /home/me/.claude/skills/brand/a.py /etc/passwd:*)",
        "Bash(python3 /home/me/.claude/skills/brand/a.py;rm:*)",
        "Bash(python3 /home/me/.claude/skills/brand/$(x).py:*)",
        "Bash(python3 skills/brand/run.py:*)",
        "Bash(python3 -m http.server /home/me/.claude/skills/brand/a:*)",
        "Bash(zsh /home/me/.claude/skills/brand/scripts/run.sh:*)",
        "Bash(env /home/me/.claude/skills/brand/scripts/run.sh:*)",
        "Bash(/home/me/.claude/skills/brand/node:*)",
    ] {
        let v = check(tool, brand.clone());
        assert_eq!(v["ok"], false, "{tool} was accepted");
    }
    // A skill the routine does not list grants nothing.
    for skills in [json!([]), json!(["other"])] {
        let v = check(
            "Bash(python3 /home/me/.claude/skills/brand/scripts/run.py:*)",
            skills,
        );
        assert_eq!(v["ok"], false);
        assert_eq!(v["errors"][0]["field"], "access[0].tool");
    }
}

#[test]
fn a_scheduled_run_ends_in_a_valid_result_that_show_returns() {
    let f = fixture();
    // Fake harness: a real script that prints Claude's envelope.
    let script = f._dir.path().join("harness.sh");
    std::fs::write(
        &script,
        format!("cat <<'JSON'\n{}\nJSON\n", envelope(&good("done"))),
    )
    .unwrap();
    *f.daemon.script.lock().unwrap() = Some(script);
    install(&f, "commit-digest");

    // The entry the daemon would hold: schedule id, cron in the routine's zone, create params.
    let set = f.daemon.state.lock().unwrap().schedules["routine.commit-digest"].clone();
    assert_eq!(set["cron"], "0 9 * * 1-5");
    assert_eq!(set["tz"], "Europe/Berlin");
    assert_eq!(set["overlap"], "skip");
    assert_eq!(set["timeoutSec"], 1800);
    assert_eq!(set["create"]["background"], true);
    assert_eq!(set["create"]["agent"], "claude");
    assert!(set["create"]["prompt"]
        .as_str()
        .unwrap()
        .contains("Summarise the last 5 commits."));
    assert!(set["create"]["prompt"].as_str().unwrap().contains("DATA"));

    f.clock.store(1_800_000_100_000, Ordering::SeqCst);
    f.daemon.fire(&f.svc, "commit-digest", 1_800_000_000_000);

    let shown = f.svc.call("show", json!({"id": "commit-digest"})).unwrap();
    assert_eq!(shown["latest"]["status"], "done");
    assert_eq!(shown["latest"]["title"], "Digest");
    assert_eq!(shown["runs"][0]["status"], "done");
    assert_eq!(shown["runs"][0]["kind"], "scheduled");
    let path = f
        .svc
        .store()
        .run_dir("commit-digest", 1)
        .join("result.json");
    let text = std::fs::read_to_string(path).unwrap();
    sushiai_routines::result::validate(&serde_json::from_str(&text).unwrap()).unwrap();
    assert!(inbox(&f).is_empty());
}

#[test]
fn an_invalid_result_gets_one_retry_then_fails() {
    let f = fixture();
    install(&f, "r1");
    f.daemon.answer("this is not json at all");
    f.daemon.answer("{\"status\": \"done\"}");
    f.daemon.fire(&f.svc, "r1", 1);
    let v = latest(&f, "r1");
    assert_eq!(v["result"]["status"], "failed");
    assert!(v["run"]["reason"]
        .as_str()
        .unwrap()
        .contains("invalid result after one retry"));
    // The scheduled session plus exactly one retry.
    assert_eq!(f.daemon.created().len(), 2);
    let retry = &f.daemon.created()[1];
    // Claude resumes its own agent session and carries the validation error.
    assert_eq!(retry["resume"], "agent-s1");
    assert!(retry["prompt"].as_str().unwrap().contains("did not match"));
    assert_eq!(inbox(&f).len(), 1);
}

#[test]
fn a_retry_that_answers_validly_is_accepted() {
    let f = fixture();
    install(&f, "r1");
    f.daemon.answer("oops");
    f.daemon.answer(&envelope(&good("done")));
    f.daemon.fire(&f.svc, "r1", 1);
    let v = latest(&f, "r1");
    assert_eq!(v["result"]["status"], "done");
    assert_eq!(v["run"]["retried"], true);
    assert!(inbox(&f).is_empty());
}

#[test]
fn a_noop_run_is_archived_quietly() {
    let f = fixture();
    install(&f, "r1");
    f.daemon.answer(&envelope(&good("noop")));
    f.daemon.fire(&f.svc, "r1", 1);
    assert_eq!(latest(&f, "r1")["result"]["status"], "noop");
    assert!(inbox(&f).is_empty());
}

#[test]
fn needs_you_raises_one_inbox_event() {
    let f = fixture();
    install(&f, "r1");
    f.daemon.answer(&envelope(&good("needs_you")));
    f.daemon.fire(&f.svc, "r1", 1);
    let inbox = inbox(&f);
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0]["status"], "needs_you");
}

#[test]
fn three_failures_pause_the_routine_with_one_more_inbox_event() {
    let f = fixture();
    install(&f, "r1");
    for slot in 1..=3u64 {
        f.daemon.answer(&envelope(&good("failed")));
        f.daemon.fire(&f.svc, "r1", slot);
    }
    let routine = f.svc.store().load("r1").unwrap();
    assert!(!routine.enabled);
    assert!(f
        .daemon
        .state
        .lock()
        .unwrap()
        .removed
        .contains(&"routine.r1".to_string()));
    let events = f.events.lock().unwrap().clone();
    let paused: Vec<_> = events.iter().filter(|e| e["kind"] == "paused").collect();
    assert_eq!(paused.len(), 1);
    // Two quiet-free failures raised one inbox item each, the third only the pause.
    let inbox: Vec<_> = events.iter().filter(|e| e["inbox"] == true).collect();
    assert_eq!(inbox.len(), 3);
    assert_eq!(inbox[2]["kind"], "paused");
    // A success resets the counter.
    f.svc
        .call("set_enabled", json!({"id": "r1", "enabled": true}))
        .unwrap();
    assert_eq!(f.svc.store().failures("r1"), 0);
}

#[test]
fn a_success_between_failures_resets_the_count() {
    let f = fixture();
    install(&f, "r1");
    for (slot, status) in [(1, "failed"), (2, "failed"), (3, "done"), (4, "failed")] {
        f.daemon.answer(&envelope(&good(status)));
        f.daemon.fire(&f.svc, "r1", slot);
    }
    assert!(f.svc.store().load("r1").unwrap().enabled);
    assert_eq!(f.svc.store().failures("r1"), 1);
}

#[test]
fn a_long_summary_is_capped_with_a_note() {
    let f = fixture();
    install(&f, "r1");
    let mut v: Value = serde_json::from_str(&good("done")).unwrap();
    v["summary"] = json!("x".repeat(200 * 1024));
    f.daemon.answer(&envelope(&v.to_string()));
    f.daemon.fire(&f.svc, "r1", 1);
    let summary = latest(&f, "r1")["result"]["summary"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(summary.len() < 65 * 1024);
    assert!(summary.contains("truncated"));
}

#[test]
fn the_last_30_runs_are_kept() {
    let f = fixture();
    install(&f, "r1");
    for slot in 1..=35u64 {
        f.daemon.answer(&envelope(&good("noop")));
        f.daemon.fire(&f.svc, "r1", slot);
    }
    let runs = f.svc.call("runs", json!({"id": "r1"})).unwrap();
    assert_eq!(runs["runs"].as_array().unwrap().len(), 30);
    assert_eq!(runs["runs"][0]["n"], 35);
    assert_eq!(runs["runs"][29]["n"], 6);
}

/// Queues a proposal the way `sushiai propose` does for `session`.
fn queue(f: &Fixture, id: &str, session: &str, name: &str, proposal: Value) {
    let dir = f.svc.store().proposals_dir(id, session);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(name), proposal.to_string()).unwrap();
}

#[test]
fn a_dry_run_writes_nothing_and_lists_actions_as_not_applied() {
    let f = fixture();
    install(&f, "r1");
    // The agent queued git writes in the session the dry run will get. One of them says
    // dryRun:false (as if the proposal was made without the switch): the run's own kind wins.
    queue(
        &f,
        "r1",
        "s1",
        "1.json",
        json!({"kind": "commit", "message": "m", "paths": ["a.txt"], "dryRun": false}),
    );
    queue(
        &f,
        "r1",
        "s1",
        "2.json",
        json!({"kind": "pr", "title": "t", "paths": ["a.txt"], "dryRun": true}),
    );
    // The agent claims it applied an action; a dry run forces applied=false.
    f.daemon.answer(&envelope(&good("done")));
    let calls_before = f.git.words().len();
    let started = f.svc.call("dry_run", json!({"id": "r1"})).unwrap();
    assert_eq!(started["run"], 1);
    let v = wait_for_run(&f, "r1");
    assert_eq!(v["run"]["kind"], "dry");
    assert_eq!(v["run"]["status"], "done");
    for a in v["result"]["actions"].as_array().unwrap() {
        assert_eq!(a["applied"], false);
    }
    let proposals = v["run"]["proposals"].as_array().unwrap();
    assert_eq!(proposals.len(), 2);
    for p in proposals {
        assert_eq!(p["status"], "would_do");
        assert_eq!(p["applied"], false);
    }
    // Nothing was committed, pushed or opened: only the worktree reset ran.
    let after: Vec<Vec<String>> = f.git.words().into_iter().skip(calls_before).collect();
    for c in &after {
        assert!(
            !matches!(c[1].as_str(), "commit" | "push" | "add" | "rm" | "pr")
                && c[0] != "gh"
                && !c.contains(&"-b".to_string()),
            "{c:?}"
        );
    }
    // The run got the dry-run switch and the read-only allowlist.
    let create = f.daemon.created().pop().unwrap();
    assert_eq!(create["env"]["SUSHIAI_DRY_RUN"], "1");
    let args: Vec<&str> = create["extraArgs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    assert!(args.contains(&"--allowedTools") && args.contains(&"--disallowedTools"));
    assert!(!args.contains(&"Bash"));
    assert!(create["prompt"].as_str().unwrap().contains("DRY RUN"));
    // A dry run never counts as a failure and raises no inbox event.
    assert!(inbox(&f).is_empty());
}

#[test]
fn a_real_run_applies_a_proposal_in_its_own_worktree_never_in_the_owners_checkout() {
    let f = fixture();
    install(&f, "r1");
    // Runs work in the routine's own worktree of the project, not in the catalog folder.
    let owners = f._dir.path().join("my-app");
    let project = f.svc.store().worktree_dir("r1");
    assert!(project.join(".git").exists());
    std::fs::write(project.join("a.txt"), "changed").unwrap();
    let before = f.git.words().len();
    let tree = f.svc.store().apply_dir("r1", 1);
    // Another session's queue is not this run's.
    let session = f.daemon.start(&f.svc, "r1", 1);
    say_yes(&f, "r1", &session, "y1", "commit");
    say_yes(&f, "r1", &session, "y2", "commit");
    queue(
        &f,
        "r1",
        &session,
        "1.json",
        json!({"kind": "commit", "message": "fix typo", "paths": ["a.txt"]}),
    );
    queue(
        &f,
        "r1",
        &session,
        "2.json",
        json!({"kind": "commit", "message": "all of it"}),
    );
    queue(
        &f,
        "r1",
        "other",
        "1.json",
        json!({"kind": "commit", "message": "x", "paths": ["b.txt"]}),
    );
    f.daemon.answer(&envelope(&good("done")));
    f.daemon.end(&f.svc, "r1", &session, 1);

    let calls: Vec<_> = f
        .git
        .calls
        .lock()
        .unwrap()
        .iter()
        .skip(before)
        .cloned()
        .collect();
    let words: Vec<Vec<String>> = calls.iter().map(|(a, _)| a.clone()).collect();
    // Nothing runs in the owner's checkout; add and commit run in the apply worktree, the
    // rest in the routine's own worktree. No add -A.
    assert!(
        !words.iter().any(|c| c.contains(&"-A".to_string())),
        "{words:?}"
    );
    for (argv, cwd) in &calls {
        assert_ne!(cwd, &owners, "{argv:?} ran in the owners checkout");
        if matches!(argv[1].as_str(), "add" | "commit" | "push" | "rm") {
            assert_eq!(cwd, &tree, "{argv:?} ran in {}", cwd.display());
        } else {
            assert_eq!(cwd, &project, "{argv:?} ran in {}", cwd.display());
        }
    }
    let add = words
        .iter()
        .find(|c| c[1] == "worktree" && c[2] == "add")
        .expect("a worktree was added");
    assert!(add.contains(&"routine/r1/1".to_string()));
    assert!(add.contains(&tree.to_string_lossy().into_owned()));
    // Only the named file is staged, and it was in the worktree at commit time.
    assert!(words
        .iter()
        .any(|c| c[1] == "add" && c[2] == "--" && c[3] == "a.txt"));
    assert_eq!(f.git.commit_saw_file.lock().unwrap().as_slice(), [true]);
    // The worktree is removed again, last.
    let last_remove = words
        .iter()
        .rposition(|c| c[1] == "worktree" && c[2] == "remove")
        .unwrap();
    let commit = words.iter().position(|c| c[1] == "commit").unwrap();
    assert!(last_remove > commit);
    assert!(!tree.exists());
    // The checkout's file is untouched.
    assert_eq!(
        std::fs::read_to_string(project.join("a.txt")).unwrap(),
        "changed"
    );
    assert!(!owners.join("a.txt").exists());

    let run = latest(&f, "r1")["run"].clone();
    assert_eq!(run["proposals"][0]["status"], "applied");
    assert_eq!(run["proposals"][0]["detail"], "routine/r1/1");
    // A commit that names no files is refused, not turned into `git add -A`.
    assert_eq!(run["proposals"][1]["status"], "failed");
    assert!(run["proposals"][1]["detail"]
        .as_str()
        .unwrap()
        .contains("--path"));
    // The other session's proposal stays queued for its own run.
    assert!(f
        .svc
        .store()
        .proposals_dir("r1", "other")
        .join("1.json")
        .exists());
}

#[test]
fn a_proposal_cannot_name_a_path_outside_the_working_folder() {
    let f = fixture();
    install(&f, "r1");
    for (i, path) in ["../secret", "/etc/passwd", ".git/config", "-rf"]
        .into_iter()
        .enumerate()
    {
        queue(
            &f,
            "r1",
            "s1",
            &format!("{i}.json"),
            json!({"kind": "commit", "message": "m", "paths": [path]}),
        );
    }
    f.daemon.answer(&envelope(&good("done")));
    f.daemon.fire(&f.svc, "r1", 1);
    let run = latest(&f, "r1")["run"].clone();
    for p in run["proposals"].as_array().unwrap() {
        assert_eq!(p["status"], "failed", "{p}");
    }
    assert!(!f
        .git
        .words()
        .iter()
        .any(|c| c[1] == "worktree" && c.contains(&"-b".to_string())));
}

#[test]
fn validate_rejects_what_the_daemon_would_stumble_on() {
    let f = fixture();
    let check = |mutate: &dyn Fn(&mut Value)| -> Value {
        let mut r = routine_json("r1");
        mutate(&mut r);
        f.svc
            .call("validate", json!({"routine": r, "brief": "b"}))
            .unwrap()
    };
    let fields = |v: &Value| -> Vec<String> {
        v["errors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["field"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(check(&|_| {})["ok"], true);
    assert!(
        fields(&check(&|r| r["when"]["cron"] = json!("*/2 * * * *")))
            .contains(&"when.cron".to_string())
    );
    assert!(
        fields(&check(&|r| r["when"]["tz"] = json!("Mars/Base"))).contains(&"when.tz".to_string())
    );
    assert!(
        fields(&check(&|r| r["where"]["host"] = json!("ssh:no-such-host")))
            .contains(&"where.host".to_string())
    );
    assert!(
        fields(&check(&|r| r["where"]["projects"] = json!(["no-such-app"])))
            .contains(&"where.projects".to_string())
    );
    assert!(fields(&check(&|r| r["agent"]["harness"] = json!("gemini")))
        .contains(&"agent.harness".to_string()));
    let v = check(&|r| {
        r["agent"]["effort"] = json!("max");
        r["agent"]["harness"] = json!("codex");
    });
    assert!(fields(&v).contains(&"agent.effort".to_string()));
    // The list is the harness's own: the same level Claude offers is refused for Codex above
    // and accepted here, and a level neither offers is refused for both.
    assert!(!fields(&check(&|r| r["agent"]["effort"] = json!("max")))
        .contains(&"agent.effort".to_string()));
    assert!(fields(&check(&|r| r["agent"]["effort"] = json!("ultra")))
        .contains(&"agent.effort".to_string()));
    for bad in [
        json!(["--dangerously-skip-permissions"]),
        json!(["--add-dir", "/etc"]),
        json!(["--settings", "{}"]),
        json!(["-c", "sandbox_mode=\"danger-full-access\""]),
        json!(["--permission-mode", "bypassPermissions"]),
        // Attached short forms, MCP and config, unbounded tools.
        json!(["-cmodel=o3"]),
        json!(["-sdanger-full-access"]),
        json!(["--config", "x=1"]),
        json!(["--mcp-config", "{}"]),
        json!(["--allowedTools", "Bash"]),
        json!(["--allowedTools", "Bash(*)"]),
        json!(["--allowedTools", "Bash(sh:*)"]),
        json!(["--allowedTools", "mcp__*__write"]),
        json!(["--allowedTools=Read,Bash(:*)"]),
        json!(["--unknown-flag"]),
        json!(["loose-word"]),
    ] {
        let v = check(&|r| r["agent"]["extraArgs"] = bad.clone());
        assert!(
            fields(&v).contains(&"agent.extraArgs".to_string()),
            "{bad} was accepted"
        );
    }
    for good_args in [
        json!(["--fallback-model", "sonnet", "--max-turns", "20"]),
        json!([
            "--allowedTools",
            "Read Grep Bash(sushiai ask:*) mcp__tracker__search"
        ]),
        json!(["--disallowedTools", "Write"]),
    ] {
        let v = check(&|r| r["agent"]["extraArgs"] = good_args.clone());
        assert_eq!(v["ok"], true, "{good_args}: {v}");
    }
    // The routine folder itself is a fine --add-dir target.
    let dir = f.svc.store().dir("r1").join("work");
    let v = check(&|r| {
        r["agent"]["extraArgs"] = json!(["--add-dir", dir.to_string_lossy()]);
    });
    assert_eq!(v["ok"], true, "{v}");
    // A model the host does not offer.
    let v = f
        .svc
        .call(
            "validate",
            json!({
                "routine": routine_json("r1"), "brief": "b",
                "catalog": {
                    "hosts": [{"id": "ssh:build-host",
                               "harnesses": {"claude": {"models": ["opus"]}}}],
                    "projects": [{"id": "my-app", "name": "my-app", "folders": []}],
                },
            }),
        )
        .unwrap();
    assert!(fields(&v).contains(&"agent.model".to_string()));
    assert!(f
        .svc
        .call(
            "validate",
            json!({"routine": routine_json("r1"), "brief": "  "})
        )
        .unwrap()["errors"]
        .to_string()
        .contains("brief"));
}

#[test]
fn changing_the_agent_changes_the_next_runs_argv() {
    let f = fixture();
    install(&f, "r1");
    let first = f.daemon.state.lock().unwrap().schedules["routine.r1"]["create"].clone();
    assert_eq!(first["agent"], "claude");
    assert!(first["extraArgs"]
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a == "sonnet"));
    let mut r = routine_json("r1");
    r["agent"] = json!({"harness": "codex", "model": "gpt-5", "effort": "high",
                        "extraArgs": []});
    // The brief is unchanged but the agent is not: the owner reviews before it runs again.
    let saved = f
        .svc
        .call(
            "save",
            json!({"routine": r, "brief": "Summarise the last 5 commits."}),
        )
        .unwrap();
    assert_eq!(saved["enabled"], false);
    assert_eq!(saved["pendingReview"], true);
    assert!(!f
        .daemon
        .state
        .lock()
        .unwrap()
        .schedules
        .contains_key("routine.r1"));
    f.svc
        .call("set_enabled", json!({"id": "r1", "enabled": true}))
        .unwrap();
    let second = f.daemon.state.lock().unwrap().schedules["routine.r1"]["create"].clone();
    assert_eq!(second["agent"], "codex");
    let args: Vec<&str> = second["extraArgs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    assert_eq!(args[0], "exec");
    assert!(args.contains(&"gpt-5"));
    assert!(args.contains(&"model_reasoning_effort=high"));
    // A Codex run starts in the routine's own folder, the only place its sandbox writes.
    assert!(second["cwd"].as_str().unwrap().ends_with("r1/work"));
}

#[test]
fn a_codex_answer_is_read_from_the_last_message_file() {
    let f = fixture();
    let mut r = routine_json("r1");
    r["agent"] = json!({"harness": "codex", "model": "gpt-5", "effort": "low", "extraArgs": []});
    f.svc
        .call("save", json!({"routine": r, "brief": "b"}))
        .unwrap();
    f.svc
        .call("set_enabled", json!({"id": "r1", "enabled": true}))
        .unwrap();
    // The strict schema is what Codex reads from the routine dir.
    let on_disk = std::fs::read_to_string(f.svc.store().schema_file("r1")).unwrap();
    assert_eq!(on_disk, sushiai_routines::result::SCHEMA_CODEX);
    // A strict answer: null optionals and counts as a list.
    f.daemon.answer(
        r#"{"status":"done","title":"t","summary":"s","actions":[],
            "missing":null,"next":null,"counts":[{"name":"n","value":3}]}"#,
    );
    f.daemon.fire(&f.svc, "r1", 1);
    let v = latest(&f, "r1");
    assert_eq!(v["result"]["status"], "done");
    assert_eq!(v["result"]["counts"], json!({"n": 3}));
    assert!(v["result"].get("next").is_none());
}

#[test]
fn an_invalid_codex_answer_gets_one_retry_then_fails() {
    let f = fixture();
    let mut r = routine_json("r1");
    r["agent"] = json!({"harness": "codex", "model": "gpt-5", "effort": "low", "extraArgs": []});
    f.svc
        .call("save", json!({"routine": r, "brief": "b"}))
        .unwrap();
    f.svc
        .call("set_enabled", json!({"id": "r1", "enabled": true}))
        .unwrap();
    f.daemon
        .answer(r#"{"status":"done","title":"t","missing":null}"#);
    f.daemon
        .answer(r#"{"status":"nope","title":"t","summary":"","actions":[],"next":null}"#);
    f.daemon.fire(&f.svc, "r1", 1);
    assert_eq!(latest(&f, "r1")["result"]["status"], "failed");
    assert_eq!(f.daemon.created().len(), 2);
}

#[test]
fn a_harness_that_is_not_logged_in_fails_with_the_fix() {
    let f = fixture();
    install(&f, "r1");
    // Claude's own envelope says the call failed and why.
    f.daemon.answer(
        &json!({"type": "result", "subtype": "success", "is_error": true,
                "api_error_status": 401, "result": "Not logged in - Please run /login"})
        .to_string(),
    );
    f.daemon.fire(&f.svc, "r1", 1);
    let v = latest(&f, "r1");
    assert_eq!(v["result"]["status"], "failed");
    assert!(v["run"]["reason"]
        .as_str()
        .unwrap()
        .contains("log in on ssh:build-host"));
    // No retry for a login problem.
    assert_eq!(f.daemon.created().len(), 1);
}

#[test]
fn a_nonzero_exit_with_login_words_and_no_envelope_is_a_login_problem() {
    let f = fixture();
    install(&f, "r1");
    f.daemon.state.lock().unwrap().exit_code = Some(1);
    f.daemon.answer("Not logged in. Please run /login");
    f.daemon.fire(&f.svc, "r1", 1);
    let v = latest(&f, "r1");
    assert!(v["run"]["reason"]
        .as_str()
        .unwrap()
        .contains("log in on ssh:build-host"));
    assert_eq!(f.daemon.created().len(), 1);
}

#[test]
fn a_valid_answer_that_mentions_logging_in_is_a_result_not_a_login_failure() {
    let f = fixture();
    install(&f, "r1");
    let mut v: Value = serde_json::from_str(&good("done")).unwrap();
    v["summary"] = json!("Tracker says: user is not logged in; invalid api key in the sample.");
    f.daemon.answer(&envelope(&v.to_string()));
    f.daemon.fire(&f.svc, "r1", 1);
    let got = latest(&f, "r1");
    assert_eq!(got["result"]["status"], "done");
    assert!(got["run"]["reason"].is_null());
}

#[test]
fn a_harness_that_exits_at_once_with_no_output_shows_the_login_fix_and_is_not_retried() {
    let f = fixture();
    install(&f, "r1");
    {
        let mut s = f.daemon.state.lock().unwrap();
        s.exit_code = Some(1);
        // The session was gone before anything could be read.
        s.unread.insert("s1".into());
    }
    f.daemon.fire(&f.svc, "r1", 1);
    let v = latest(&f, "r1");
    assert_eq!(v["result"]["status"], "failed");
    let reason = v["run"]["reason"].as_str().unwrap();
    assert!(reason.contains("log in on ssh:build-host"), "{reason}");
    assert!(reason.contains("exit code 1"));
    assert_eq!(f.daemon.created().len(), 1, "no retry for an empty run");
    // The output was asked for the moment the slot fired.
    assert_eq!(f.daemon.state.lock().unwrap().watched[0], "s1");
}

#[test]
fn a_failed_first_clone_is_cleaned_up_and_reported_with_a_fix() {
    let f = fixture_with(FakeGit {
        fail_clone: true,
        ..Default::default()
    });
    let mut r = routine_json("r1");
    r["where"]["projects"] = json!(["my-app"]);
    let catalog = json!({
        "hosts": [{"id": "ssh:build-host"}],
        "projects": [{"id": "my-app", "name": "my-app",
                      "remote": "https://example.invalid/my-app.git",
                      "folders": [{"host": "ssh:build-host", "path": "/nonexistent/my-app"}]}],
    });
    f.svc
        .call(
            "save",
            json!({"routine": r, "brief": "b", "catalog": catalog}),
        )
        .unwrap();
    let err = f
        .svc
        .call(
            "set_enabled",
            json!({"id": "r1", "enabled": true, "catalog": catalog}),
        )
        .unwrap_err();
    assert!(format!("{err:?}").contains("Fix:"));
    assert!(!f.svc.store().worktree_dir("r1").exists());
    assert!(!f.svc.store().load("r1").unwrap().enabled);
    assert_eq!(latest(&f, "r1")["result"]["status"], "failed");
}

#[test]
fn skipped_slots_are_logged_with_their_reason_and_raise_no_inbox_event() {
    let f = fixture();
    install(&f, "r1");
    f.svc.on_schedule_event(&json!({
        "id": "routine.r1", "kind": "skipped", "reason": "previous run still live",
        "scheduledFor": 1, "startedAt": 1,
    }));
    let log = std::fs::read_to_string(f.svc.store().dir("r1").join("log.jsonl")).unwrap();
    assert!(log.contains("previous run still live"));
    assert!(inbox(&f).is_empty());
    // Other entries of the daemon are none of ours.
    f.svc
        .on_schedule_event(&json!({"id": "other:1", "kind": "ended", "scheduledFor": 1,
                                    "startedAt": 1}));
}

#[test]
fn resync_restores_entries_and_drops_orphans() {
    let f = fixture();
    install(&f, "r1");
    {
        let mut s = f.daemon.state.lock().unwrap();
        s.schedules.clear();
        s.schedules
            .insert("routine.ghost".into(), json!({"id": "routine.ghost"}));
        s.schedules
            .insert("other:keep".into(), json!({"id": "other:keep"}));
    }
    f.svc.resync();
    let s = f.daemon.state.lock().unwrap();
    assert!(s.schedules.contains_key("routine.r1"));
    assert!(!s.schedules.contains_key("routine.ghost"));
    assert!(s.schedules.contains_key("other:keep"));
}

#[test]
fn delete_removes_the_entry_and_the_folder() {
    let f = fixture();
    install(&f, "r1");
    f.svc.call("delete", json!({"id": "r1"})).unwrap();
    assert!(!f.svc.store().exists("r1"));
    assert!(!f
        .daemon
        .state
        .lock()
        .unwrap()
        .schedules
        .contains_key("routine.r1"));
}

#[test]
fn list_and_unknown_methods() {
    let f = fixture();
    install(&f, "r1");
    let list = f.svc.call("list", json!({})).unwrap();
    assert_eq!(list["routines"][0]["routine"]["id"], "r1");
    assert!(f.svc.call("nope", json!({})).is_err());
}

#[test]
fn a_run_left_running_by_a_daemon_restart_is_settled_on_resync() {
    let f = fixture();
    install(&f, "r1");
    // A run record whose session the daemon no longer has.
    let n = f.svc.store().new_run("r1").unwrap();
    f.svc
        .store()
        .write_run(
            "r1",
            n,
            &json!({"n": n, "kind": "scheduled", "status": "running", "session": "gone"}),
        )
        .unwrap();
    f.svc.resync();
    // The run is concluded as failed, with the reason.
    let run = f.svc.store().run("r1", n).unwrap();
    assert_eq!(run["status"], "failed");
    assert_eq!(run["reason"], "daemon restarted");
    // A late `ended` event for the same session does not make a second run.
    let before = f.svc.store().run_numbers("r1").len();
    f.svc.on_schedule_event(
        &json!({"id": "routine.r1", "kind": "ended", "session": "gone",
                                    "scheduledFor": 1, "startedAt": 1}),
    );
    assert_eq!(f.svc.store().run_numbers("r1").len(), before);
    assert!(f.daemon.created().is_empty());
}

#[test]
fn a_run_the_daemon_ended_by_restarting_is_failed_without_a_retry() {
    let f = fixture();
    install(&f, "r1");
    // The session is known and ended, but what it printed is not a valid answer: an ordinary
    // run would be retried; one the daemon cut short is not.
    f.daemon.answer("half an answer {\"status\":");
    let session = f.daemon.start(&f.svc, "r1", 1);
    f.svc.on_schedule_event(&json!({
        "id": "routine.r1", "kind": "ended", "session": session, "reason": "daemon restarted",
        "scheduledFor": 1, "startedAt": 1, "endedAt": 2,
    }));
    let v = latest(&f, "r1");
    assert_eq!(v["run"]["status"], "failed");
    assert_eq!(v["run"]["reason"], "daemon restarted");
    assert_eq!(f.daemon.created().len(), 1, "no new session was created");

    // The same through the reconcile pass after a restart of the module.
    let n = f.svc.store().new_run("r1").unwrap();
    f.svc
        .store()
        .write_run(
            "r1",
            n,
            &json!({"n": n, "kind": "scheduled", "status": "running", "session": "s1"}),
        )
        .unwrap();
    f.svc.resync();
    let run = f.svc.store().run("r1", n).unwrap();
    assert_eq!(run["status"], "failed");
    assert_eq!(run["reason"], "daemon restarted");
    assert_eq!(f.daemon.created().len(), 1);
}

#[test]
fn a_run_is_concluded_once_however_many_paths_reach_its_end() {
    let f = fixture();
    install(&f, "r1");
    let svc = f.svc.clone();
    for round in 0..20u64 {
        let session = format!("late-{round}");
        let n = f.svc.store().new_run("r1").unwrap();
        f.svc
            .store()
            .write_run(
                "r1",
                n,
                &json!({"n": n, "kind": "scheduled", "status": "running", "session": session}),
            )
            .unwrap();
        f.daemon
            .state
            .lock()
            .unwrap()
            .outputs
            .insert(session.clone(), envelope(&good("noop")));
        let handles: Vec<_> = (0..6)
            .map(|i| {
                let svc = svc.clone();
                let session = session.clone();
                std::thread::spawn(move || {
                    if i % 2 == 0 {
                        svc.resync();
                    } else {
                        svc.on_schedule_event(&json!({
                            "id": "routine.r1", "kind": "ended", "session": session,
                            "scheduledFor": 1, "startedAt": 1}));
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
    }
    let finished = f
        .events
        .lock()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "run_finished")
        .count();
    assert_eq!(finished, 20, "one conclusion per run");
}

#[test]
fn saving_an_enabled_routine_with_new_content_switches_it_off_for_review() {
    let f = fixture();
    install(&f, "r1");
    // The same content stays enabled and keeps its entry.
    let same = f
        .svc
        .call(
            "save",
            json!({"routine": routine_json("r1"), "brief": "Summarise the last 5 commits."}),
        )
        .unwrap();
    assert_eq!(same["enabled"], true);
    assert_eq!(same["pendingReview"], false);
    assert!(f.svc.store().load("r1").unwrap().enabled);
    // A changed brief does not.
    let changed = f
        .svc
        .call(
            "save",
            json!({"routine": routine_json("r1"), "brief": "Also post it to the channel."}),
        )
        .unwrap();
    assert_eq!(changed["enabled"], false);
    assert_eq!(changed["pendingReview"], true);
    assert!(!f.svc.store().load("r1").unwrap().enabled);
    assert!(!f
        .daemon
        .state
        .lock()
        .unwrap()
        .schedules
        .contains_key("routine.r1"));
}

#[test]
fn turning_a_routine_off_succeeds_when_the_daemon_has_no_such_entry() {
    let f = fixture();
    install(&f, "r1");
    f.daemon.state.lock().unwrap().schedules.clear();
    f.svc
        .call("set_enabled", json!({"id": "r1", "enabled": false}))
        .unwrap();
    assert!(!f.svc.store().load("r1").unwrap().enabled);
}

#[test]
fn turning_off_deleting_or_suspending_closes_the_live_run_as_cancelled() {
    for how in ["disable", "delete", "suspend"] {
        let f = fixture();
        install(&f, "r1");
        let session = f.daemon.start(&f.svc, "r1", 1);
        f.daemon.state.lock().unwrap().hold.insert(session.clone());
        match how {
            "disable" => {
                f.svc
                    .call("set_enabled", json!({"id": "r1", "enabled": false}))
                    .unwrap();
            }
            "delete" => {
                // The run record is gone with the folder; the session must still be closed.
                f.svc.call("delete", json!({"id": "r1"})).unwrap();
            }
            _ => {
                f.svc.call("suspend", json!({})).unwrap();
            }
        }
        assert!(
            f.daemon.state.lock().unwrap().closed.contains(&session),
            "{how}: the session was not closed"
        );
        if how != "delete" {
            let run = f.svc.store().run("r1", 1).unwrap();
            assert_eq!(run["status"], "cancelled", "{how}");
            // A cancelled run is not a failure.
            assert_eq!(f.svc.store().failures("r1"), 0);
        }
        assert!(inbox(&f).is_empty(), "{how}");
        // The ended event the daemon sends for the closed session changes nothing.
        f.daemon.end(&f.svc, "r1", &session, 1);
        if how != "delete" {
            assert_eq!(f.svc.store().run("r1", 1).unwrap()["status"], "cancelled");
        }
    }
}

#[test]
fn events_of_one_routine_are_handled_in_arrival_order_and_make_one_run() {
    let f = fixture();
    install(&f, "r1");
    f.daemon.answer(&envelope(&good("done")));
    let session = {
        // The session exists in the daemon before its events arrive.
        let set = f.daemon.state.lock().unwrap().schedules["routine.r1"].clone();
        f.daemon.session_create(set["create"].clone()).unwrap()
    };
    // `fired` (twice, as a reconnect may repeat it) and `ended` are queued, not handled inline.
    for kind in ["fired", "fired", "ended"] {
        f.svc.enqueue_event(json!({
            "id": "routine.r1", "kind": kind, "session": session,
            "scheduledFor": 5, "startedAt": 5}));
    }
    let v = wait_for_run(&f, "r1");
    assert_eq!(v["run"]["status"], "done");
    assert_eq!(f.svc.store().run_numbers("r1"), vec![1]);
    assert_eq!(v["run"]["scheduledFor"], 5, "the fired record was kept");
}

#[test]
fn a_new_slot_beside_a_retrying_run_is_skipped_and_its_session_closed() {
    let f = fixture();
    install(&f, "r1");
    // The first answer is invalid, so the module starts a retry session (s2); that session
    // stays live until the test releases it.
    f.daemon.answer("not json");
    f.daemon.answer(&envelope(&good("done")));
    f.daemon.state.lock().unwrap().hold.insert("s2".into());
    let s1 = f.daemon.start(&f.svc, "r1", 1);
    let svc = f.svc.clone();
    let worker = std::thread::spawn(move || {
        svc.on_schedule_event(&json!({
            "id": "routine.r1", "kind": "ended", "session": s1,
            "scheduledFor": 1, "startedAt": 1}));
    });
    test_wait::wait_until("the retry session", || {
        (f.daemon.created().len() >= 2).then_some(())
    });
    // The next slot fires while the retry lives.
    f.svc.on_schedule_event(&json!({
        "id": "routine.r1", "kind": "fired", "session": "s9",
        "scheduledFor": 2, "startedAt": 2}));
    assert!(f
        .daemon
        .state
        .lock()
        .unwrap()
        .closed
        .contains(&"s9".to_string()));
    let log = std::fs::read_to_string(f.svc.store().dir("r1").join("log.jsonl")).unwrap();
    assert!(log.contains("still retrying"));
    f.daemon.state.lock().unwrap().hold.clear();
    worker.join().unwrap();
    // One run, the retried one; no record for the skipped slot.
    assert_eq!(f.svc.store().run_numbers("r1"), vec![1]);
    assert_eq!(latest(&f, "r1")["result"]["status"], "done");
}

#[test]
fn save_with_a_host_never_relabels_a_routine_for_another_host() {
    let f = fixture();
    let dir = f._dir.path().join("builder");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("routine.json"), routine_json("r1").to_string()).unwrap();
    std::fs::write(dir.join("brief.md"), "Summarise.").unwrap();
    // This daemon is `build-host`; asking it to store for another host is an error that
    // names the way out, and nothing is stored.
    let err = f
        .svc
        .call("save", json!({"dir": dir, "host": "other-host"}))
        .unwrap_err();
    let text = format!("{err:?}");
    assert!(
        text.contains("run the routine builder on other-host"),
        "{text}"
    );
    assert!(!f.svc.store().exists("r1"));
    // Its own name works, in the forms the catalog uses, and where.host is kept as written.
    for host in ["build-host", "ssh:build-host", "local"] {
        f.svc
            .call("save", json!({"dir": dir, "host": host}))
            .unwrap();
        assert_eq!(
            f.svc.store().load("r1").unwrap().place.host,
            "ssh:build-host"
        );
    }
    // A routine.json that names a third host is not rewritten either.
    let mut r = routine_json("r1");
    r["where"]["host"] = json!("ssh:elsewhere");
    std::fs::write(dir.join("routine.json"), r.to_string()).unwrap();
    let err = f
        .svc
        .call("save", json!({"dir": dir, "host": "build-host"}))
        .unwrap_err();
    assert!(format!("{err:?}").contains("where.host"));
}

#[test]
fn only_the_default_account_is_accepted() {
    let f = fixture();
    let check = |account: Value| -> Value {
        let mut r = routine_json("r1");
        r["agent"]["account"] = account;
        f.svc
            .call("validate", json!({"routine": r, "brief": "b"}))
            .unwrap()
    };
    assert_eq!(check(json!("default"))["ok"], true);
    assert_eq!(check(Value::Null)["ok"], true);
    let v = check(json!("work-account"));
    assert_eq!(v["ok"], false);
    assert_eq!(v["errors"][0]["field"], "agent.account");
}

#[test]
fn each_run_reads_its_own_last_message_file() {
    let f = fixture();
    let mut r = routine_json("r1");
    r["agent"] = json!({"harness": "codex", "model": "gpt-5", "effort": "low", "extraArgs": []});
    f.svc
        .call("save", json!({"routine": r, "brief": "b"}))
        .unwrap();
    f.svc
        .call("set_enabled", json!({"id": "r1", "enabled": true}))
        .unwrap();
    let out_of = |create: &Value| -> String {
        let args: Vec<&str> = create["extraArgs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_str().unwrap())
            .collect();
        let i = args.iter().position(|a| *a == "-o").unwrap();
        args[i + 1].to_string()
    };
    let scheduled = f.daemon.state.lock().unwrap().schedules["routine.r1"]["create"].clone();
    assert_eq!(
        Path::new(&out_of(&scheduled)),
        f.svc.store().last_message("r1")
    );
    f.daemon.answer(
        r#"{"status":"done","title":"t","summary":"s","actions":[],
            "missing":null,"next":null,"counts":null}"#,
    );
    f.svc.call("dry_run", json!({"id": "r1"})).unwrap();
    wait_for_run(&f, "r1");
    let dry = f.daemon.created().pop().unwrap();
    // A dry run writes into its own run folder, never the scheduled run's shared file.
    assert_eq!(
        Path::new(&out_of(&dry)),
        f.svc.store().run_last_message("r1", 1, false)
    );
    assert_ne!(out_of(&dry), out_of(&scheduled));
    // And a Codex proposal queue is under the folder its sandbox may write.
    assert!(f
        .svc
        .store()
        .proposals_dir("r1", "s1")
        .starts_with(f.svc.store().work_dir("r1")));
}

#[test]
fn every_run_of_a_project_routine_works_in_its_own_worktree_and_claude_gets_dont_ask() {
    let f = fixture();
    install(&f, "r1");
    let tree = f.svc.store().worktree_dir("r1");
    let create = f.daemon.state.lock().unwrap().schedules["routine.r1"]["create"].clone();
    // The run's folder is the worktree, not the catalog's project folder.
    assert_eq!(create["cwd"], tree.to_string_lossy().as_ref());
    assert!(create["prompt"]
        .as_str()
        .unwrap()
        .contains(tree.to_string_lossy().as_ref()));
    let args: Vec<&str> = create["extraArgs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    let i = args.iter().position(|a| *a == "--permission-mode").unwrap();
    assert_eq!(args[i + 1], "dontAsk");
    // The worktree was made from the project's folder, then put on the default head.
    let words = f.git.words();
    assert!(words
        .iter()
        .any(|c| c[1] == "worktree" && c.contains(&"--detach".to_string())));
    // After a run it is reset again.
    let resets = words.iter().filter(|c| c[1] == "clean").count();
    f.daemon.answer(&envelope(&good("noop")));
    f.daemon.fire(&f.svc, "r1", 1);
    let after = f.git.words().iter().filter(|c| c[1] == "clean").count();
    assert!(after > resets);
    // A retry session is launched with dontAsk as well.
    f.daemon.answer("not json");
    f.daemon.answer(&envelope(&good("done")));
    f.daemon.fire(&f.svc, "r1", 2);
    let created = f.daemon.created();
    let retry = created.last().unwrap();
    assert!(retry["extraArgs"]
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a == "dontAsk"));
    assert_eq!(retry["cwd"], tree.to_string_lossy().as_ref());
}

#[test]
fn a_pr_proposal_as_the_cli_writes_it_is_applied_with_its_paths() {
    let f = fixture();
    install(&f, "r1");
    let project = f.svc.store().worktree_dir("r1");
    std::fs::write(project.join("a.txt"), "changed").unwrap();
    let session = f.daemon.start(&f.svc, "r1", 1);
    say_yes(&f, "r1", &session, "y1", "pr");
    // The shape `sushiai propose pr --title T --path a.txt` queues (parse_propose + queue).
    queue(
        &f,
        "r1",
        &session,
        "1.json",
        json!({"kind": "pr", "title": "Fix typo", "paths": ["a.txt"], "dryRun": false,
               "at": 1}),
    );
    f.daemon.answer(&envelope(&good("done")));
    f.daemon.end(&f.svc, "r1", &session, 1);
    let run = latest(&f, "r1")["run"].clone();
    assert_eq!(run["proposals"][0]["status"], "applied", "{run}");
    let words = f.git.words();
    assert!(words
        .iter()
        .any(|c| c[1] == "add" && c[2] == "--" && c[3] == "a.txt"));
    assert!(words.iter().any(|c| c[0] == "gh" && c[1] == "pr"));
    assert!(words.iter().any(|c| c[1] == "push"));
}

// ----- access, run modes, login data -----

fn access(tool: &str, label: &str, kind: &str) -> Value {
    json!({"tool": tool, "label": label, "kind": kind, "granted": false})
}

/// Saves `routine_json` changed by `edit` and, when `enable`, turns it on.
fn install_with(f: &Fixture, id: &str, enable: bool, edit: &dyn Fn(&mut Value)) {
    let mut r = routine_json(id);
    r["agent"]["extraArgs"] = json!([]);
    edit(&mut r);
    f.svc
        .call("save", json!({"routine": r, "brief": "Summarise."}))
        .unwrap();
    if enable {
        f.svc
            .call("set_enabled", json!({"id": id, "enabled": true}))
            .unwrap();
    }
}

fn allowed_tools(create: &Value) -> Vec<String> {
    let args: Vec<&str> = create["extraArgs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    let i = args.iter().position(|a| *a == "--allowedTools").unwrap();
    args[i + 1].split(',').map(String::from).collect()
}

fn entry(f: &Fixture, id: &str) -> Value {
    f.daemon.state.lock().unwrap().schedules[&format!("routine.{id}")].clone()
}

#[test]
fn access_is_validated_to_exact_tools_and_never_granted_by_the_builder() {
    let f = fixture();
    let check = |tool: &str, kind: &str| {
        let mut r = routine_json("r1");
        r["access"] = json!([access(tool, "Do it", kind)]);
        f.svc
            .call("validate", json!({"routine": r, "brief": "b"}))
            .unwrap()
    };
    for tool in [
        "Bash(gh issue list:*)",
        "mcp__tracker__comment",
        "Edit(notes/**)",
        "Edit(docs/guide.md)",
        "Bash(sushiai ask:*)",
        "Bash(git status:*)",
        "Bash(gh issue list:*)",
    ] {
        assert_eq!(check(tool, "write")["ok"], true, "{tool}");
    }
    for tool in [
        "Bash",
        "Bash(*)",
        "Bash(sh:*)",
        "Bash(python3:*)",
        // Basename, launchers, package runners, and the git/gh escape hatches.
        "Bash(/bin/sh:*)",
        "Bash(/usr/bin/env:*)",
        "Bash(timeout 5 rm:*)",
        "Bash(timeout:*)",
        "Bash(nice:*)",
        "Bash(npx:*)",
        "Bash(osascript:*)",
        "Bash(open:*)",
        "Bash(git config:*)",
        "Bash(git -c:*)",
        "Bash(git:*)",
        "Bash(gh api:*)",
        "Bash(gh auth:*)",
        "Bash(/usr/bin/git config:*)",
        "Write",
        "Edit",
        "MultiEdit",
        "NotebookEdit",
        "Write(notes/**)",
        "Write(*)",
        "Write(**)",
        "Edit(../outside)",
        "mcp__tracker",
        "mcp__*__comment",
        "mcp__tracker__*",
        "Read*",
        "*",
        "",
    ] {
        let v = check(tool, "write");
        assert_eq!(v["ok"], false, "{tool} was accepted");
        assert_eq!(v["errors"][0]["field"], "access[0].tool");
    }
    assert_eq!(check("Read", "admin")["ok"], false);
    // The same tool twice.
    let mut r = routine_json("r1");
    r["access"] = json!([access("Read", "a", "read"), access("Read", "b", "read")]);
    let v = f
        .svc
        .call("validate", json!({"routine": r, "brief": "b"}))
        .unwrap();
    assert_eq!(v["ok"], false);

    // A save never keeps a `granted: true` the builder wrote.
    let mut r = routine_json("r1");
    r["access"] = json!([{"tool": "Edit(notes/**)", "label": "Write files", "kind": "write",
                          "granted": true}]);
    let saved = f
        .svc
        .call("save", json!({"routine": r, "brief": "b"}))
        .unwrap();
    assert_eq!(saved["access"][0]["granted"], false);
    assert_eq!(saved["needsAccess"], 1);
}

#[test]
fn grant_and_revoke_rebuild_the_allowlist_and_a_save_resets_changed_entries_only() {
    let f = fixture();
    install_with(&f, "r1", true, &|r| {
        r["access"] = json!([
            access("Bash(gh issue list:*)", "List issues", "read"),
            access("mcp__tracker__comment", "Comment", "write"),
        ]);
    });
    // Nothing is granted: the run has the base tools only.
    assert_eq!(
        allowed_tools(&entry(&f, "r1")["create"]),
        [
            "Read",
            "Grep",
            "Glob",
            "Bash(sushiai ask:*)",
            "Bash(sushiai propose:*)"
        ]
    );
    let shown = f.svc.call("show", json!({"id": "r1"})).unwrap();
    assert_eq!(shown["needsAccess"], 2);
    assert_eq!(shown["access"].as_array().unwrap().len(), 2);
    let listed = f.svc.call("list", json!({})).unwrap();
    assert_eq!(listed["routines"][0]["needsAccess"], 2);

    let granted = f
        .svc
        .call(
            "grant",
            json!({"id": "r1", "tools": ["Bash(gh issue list:*)"]}),
        )
        .unwrap();
    assert_eq!(granted["needsAccess"], 1);
    let tools = allowed_tools(&entry(&f, "r1")["create"]);
    assert!(tools.contains(&"Bash(gh issue list:*)".to_string()));
    assert!(!tools.contains(&"mcp__tracker__comment".to_string()));
    // An unknown tool is refused, not invented.
    assert!(f
        .svc
        .call("grant", json!({"id": "r1", "tools": ["Edit"]}))
        .is_err());
    assert!(f.svc.call("grant", json!({"id": "r1"})).is_err());

    f.svc
        .call("grant", json!({"id": "r1", "all": true}))
        .unwrap();
    assert_eq!(
        f.svc.call("show", json!({"id": "r1"})).unwrap()["needsAccess"],
        0
    );
    assert!(
        allowed_tools(&entry(&f, "r1")["create"]).contains(&"mcp__tracker__comment".to_string())
    );

    // Saving the same content again keeps the grants and the routine on.
    let mut r = routine_json("r1");
    r["agent"]["extraArgs"] = json!([]);
    r["access"] = json!([
        access("Bash(gh issue list:*)", "List issues", "read"),
        access("mcp__tracker__comment", "Comment", "write"),
    ]);
    let same = f
        .svc
        .call("save", json!({"routine": r.clone(), "brief": "Summarise."}))
        .unwrap();
    assert_eq!(same["pendingReview"], false);
    assert_eq!(same["needsAccess"], 0);
    // A changed kind and a new tool lose the grant; the rest keeps it, and the routine
    // goes back to draft.
    r["access"] = json!([
        access("Bash(gh issue list:*)", "List issues", "read"),
        access("mcp__tracker__comment", "Comment", "read"),
        access("Edit(notes/**)", "Write notes", "write"),
    ]);
    let changed = f
        .svc
        .call("save", json!({"routine": r, "brief": "Summarise."}))
        .unwrap();
    assert_eq!(changed["pendingReview"], true);
    assert_eq!(changed["enabled"], false);
    let granted: Vec<bool> = changed["access"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["granted"].as_bool().unwrap())
        .collect();
    assert_eq!(granted, [true, false, false]);

    let revoked = f
        .svc
        .call(
            "revoke",
            json!({"id": "r1", "tools": ["Bash(gh issue list:*)"]}),
        )
        .unwrap();
    assert_eq!(revoked["needsAccess"], 3);
}

#[test]
fn a_run_with_ungranted_access_ends_needing_it_and_counts_no_failure() {
    let f = fixture();
    install_with(&f, "r1", true, &|r| {
        r["access"] = json!([access(
            "mcp__tracker__comment",
            "Comment on issues",
            "write"
        )]);
    });
    f.daemon.answer(&envelope(&good("done")));
    f.daemon.fire(&f.svc, "r1", 1);
    let v = latest(&f, "r1");
    assert_eq!(v["result"]["status"], "needs_you");
    assert_eq!(v["run"]["status"], "needs_you");
    assert!(v["run"]["reason"]
        .as_str()
        .unwrap()
        .contains("needs access to Comment on issues"));
    assert_eq!(v["result"]["missing"], json!(["Comment on issues"]));
    // One inbox item, and not a failure: the routine stays on.
    let items = inbox(&f);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["status"], "needs_you");
    let shown = f.svc.call("show", json!({"id": "r1"})).unwrap();
    assert_eq!(shown["consecutiveFailures"], 0);
    assert_eq!(shown["routine"]["enabled"], true);
    // The prompt told the run which access it must not use.
    let prompt = f.daemon.created()[0]["prompt"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(prompt.contains("Comment on issues (mcp__tracker__comment)"));

    // Once granted, the same answer is a plain done.
    f.svc
        .call("grant", json!({"id": "r1", "all": true}))
        .unwrap();
    f.daemon.answer(&envelope(&good("done")));
    f.daemon.fire(&f.svc, "r1", 2);
    assert_eq!(latest(&f, "r1")["result"]["status"], "done");
    let prompt = f.daemon.created()[1]["prompt"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(prompt.contains("Tools the owner granted: mcp__tracker__comment"));
}

#[test]
fn a_dry_run_uses_only_granted_read_access_and_never_grants() {
    let f = fixture();
    install_with(&f, "r1", false, &|r| {
        r["access"] = json!([
            access("Bash(gh issue list:*)", "List issues", "read"),
            access("mcp__tracker__comment", "Comment", "write"),
            access("mcp__chat__read", "Read chat", "read"),
            access("mcp__chat__post", "Post to chat", "write"),
        ]);
    });
    // The owner grants one read and one write; the last read stays ungranted.
    f.svc
        .call(
            "grant",
            json!({"id": "r1", "tools": ["Bash(gh issue list:*)", "mcp__tracker__comment",
                                         "mcp__chat__post"]}),
        )
        .unwrap();
    f.daemon.answer(&envelope(&good("done")));
    f.svc.call("dry_run", json!({"id": "r1"})).unwrap();
    wait_for_run(&f, "r1");
    let create = f.daemon.created().pop().unwrap();
    let allowed = allowed_tools(&create);
    assert!(
        allowed.contains(&"Bash(gh issue list:*)".to_string()),
        "{allowed:?}"
    );
    assert!(
        !allowed.iter().any(|t| t.contains("tracker")),
        "{allowed:?}"
    );
    assert!(!allowed.iter().any(|t| t.contains("chat")), "{allowed:?}");
    let shown = f.svc.call("show", json!({"id": "r1"})).unwrap();
    let granted: Vec<bool> = shown["access"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["granted"].as_bool().unwrap())
        .collect();
    assert_eq!(granted, [true, true, false, true]);
    // A dry run is not turned into a needs_you by the missing access.
    assert_eq!(shown["latest"]["status"], "done");
}

#[test]
fn a_dry_run_names_the_ungranted_tool_it_was_refused() {
    let f = fixture();
    install_with(&f, "r1", false, &|r| {
        r["access"] = json!([access("mcp__chat__read", "Read chat", "read")]);
    });
    let failed = json!({"status": "needs_you", "title": "Cannot read the chat",
        "summary": "The tool was refused.", "actions": []});
    let denied = json!({"type": "result", "subtype": "success",
        "permission_denials": [{"tool_name": "mcp__chat__read", "tool_input": {}}],
        "structured_output": failed});
    f.daemon.answer(&denied.to_string());
    f.svc.call("dry_run", json!({"id": "r1"})).unwrap();
    let v = wait_for_run(&f, "r1");
    assert!(
        v["run"]["reason"]
            .as_str()
            .unwrap()
            .contains("mcp__chat__read"),
        "{v}"
    );
    let create = f.daemon.created().pop().unwrap();
    assert!(!allowed_tools(&create).iter().any(|t| t.contains("chat")));
}

fn extra_args(create: &Value) -> Vec<String> {
    create["extraArgs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap().to_string())
        .collect()
}

#[test]
fn one_project_keeps_its_worktree_and_gets_no_add_dir() {
    let f = fixture();
    install(&f, "r1");
    let create = entry(&f, "r1")["create"].clone();
    assert!(!extra_args(&create)
        .iter()
        .any(|a| a.starts_with("--add-dir")));
    assert_eq!(
        create["cwd"].as_str().unwrap(),
        f.svc.store().worktree_dir("r1").to_string_lossy()
    );
    assert!(f.svc.store().worktree_dir("r1").join(".git").exists());
}

#[test]
fn several_projects_run_in_work_with_one_add_dir_each_and_no_worktree() {
    let f = fixture();
    install_with(&f, "r1", true, &|r| {
        r["where"]["projects"] = json!(["my-app", "docs-site"]);
    });
    let create = entry(&f, "r1")["create"].clone();
    let dirs: Vec<String> = extra_args(&create)
        .into_iter()
        .filter(|a| a.starts_with("--add-dir="))
        .collect();
    assert_eq!(dirs.len(), 2, "{dirs:?}");
    assert!(dirs[0].ends_with("my-app"), "{dirs:?}");
    assert!(dirs[1].ends_with("docs-site"), "{dirs:?}");
    assert_eq!(
        create["cwd"].as_str().unwrap(),
        f.svc.store().work_dir("r1").to_string_lossy()
    );
    assert!(!f.svc.store().worktree_dir("r1").exists());
    assert!(
        !f.git.words().iter().any(|c| c[1] == "worktree"),
        "{:?}",
        f.git.words()
    );
    assert!(create.get("project").is_none());
    let prompt = create["prompt"].as_str().unwrap();
    assert!(prompt.contains("- docs-site | docs-site | "), "{prompt}");
    assert!(prompt.contains("The public docs"), "{prompt}");
    assert!(prompt.contains("- my-app | my-app | "), "{prompt}");
    // The dry run reads the same folders.
    f.daemon.answer(&envelope(&good("done")));
    f.svc.call("dry_run", json!({"id": "r1"})).unwrap();
    wait_for_run(&f, "r1");
    let dry = f.daemon.created().pop().unwrap();
    assert_eq!(
        extra_args(&dry)
            .iter()
            .filter(|a| a.starts_with("--add-dir="))
            .count(),
        2
    );
}

#[test]
fn several_projects_validate_unique_known_at_most_twenty_and_claude_only() {
    let f = fixture();
    let check = |edit: &dyn Fn(&mut Value)| {
        let mut r = routine_json("r1");
        edit(&mut r);
        f.svc
            .call("validate", json!({"routine": r, "brief": "b"}))
            .unwrap()
    };
    let has = |v: &Value, text: &str| {
        v["errors"].as_array().unwrap().iter().any(|e| {
            e["field"] == "where.projects" && e["message"].as_str().unwrap().contains(text)
        })
    };
    assert_eq!(
        check(&|r| r["where"]["projects"] = json!(["my-app", "docs-site"]))["ok"],
        true
    );
    assert_eq!(check(&|r| r["where"]["projects"] = json!([]))["ok"], true);
    assert!(has(
        &check(&|r| r["where"]["projects"] = json!(["my-app", "my-app"])),
        "listed twice"
    ));
    assert!(has(
        &check(&|r| r["where"]["projects"] = json!(["my-app", "nope"])),
        "does not exist"
    ));
    let many: Vec<String> = (0..21).map(|i| format!("p{i}")).collect();
    assert!(has(
        &check(&|r| r["where"]["projects"] = json!(many)),
        "at most 20"
    ));
    let codex = check(&|r| {
        r["where"]["projects"] = json!(["my-app", "docs-site"]);
        r["agent"]["harness"] = json!("codex");
        r["agent"]["model"] = json!("gpt-5");
        r["agent"]["effort"] = json!("low");
    });
    assert!(has(&codex, "claude harness"), "{codex}");
    // One project with Codex stays valid.
    let one = check(&|r| {
        r["agent"]["harness"] = json!("codex");
        r["agent"]["model"] = json!("gpt-5");
        r["agent"]["effort"] = json!("low");
    });
    assert_eq!(one["ok"], true, "{one}");
}

#[test]
fn a_proposal_of_a_routine_with_several_projects_is_refused() {
    let f = fixture();
    install_with(&f, "r1", true, &|r| {
        r["where"]["projects"] = json!(["my-app", "docs-site"]);
        r["howItRuns"]["mode"] = json!("on-its-own");
    });
    queue(
        &f,
        "r1",
        "s1",
        "1.json",
        json!({"kind": "commit", "message": "fix typo", "paths": ["a.txt"]}),
    );
    f.daemon.answer(&envelope(&good("done")));
    f.daemon.fire(&f.svc, "r1", 1);
    let run = latest(&f, "r1")["run"].clone();
    assert_eq!(run["proposals"][0]["status"], "failed");
    assert_eq!(run["proposals"][0]["applied"], false);
    assert!(run["proposals"][0]["detail"]
        .as_str()
        .unwrap()
        .contains("more than one project"));
    assert!(!f
        .git
        .words()
        .iter()
        .any(|c| matches!(c[1].as_str(), "add" | "commit" | "push")));
}

#[test]
fn an_old_routine_with_one_project_string_loads_and_migrates_on_the_next_save() {
    use sha2::{Digest, Sha256};
    let f = fixture();
    install(&f, "r1");
    let dir = f.svc.store().dir("r1");
    // Rewrite the files the way an app that wrote `where.project` left them: the recorded
    // hash is over that older form.
    let new = f.svc.store().load("r1").unwrap();
    let new_text = serde_json::to_string(&new).unwrap();
    assert!(new_text.contains(r#""projects":["my-app"]"#), "{new_text}");
    let old_text = new_text.replace(r#""projects":["my-app"]"#, r#""project":"my-app""#);
    let mut h = Sha256::new();
    h.update(old_text.as_bytes());
    h.update([0]);
    h.update(f.svc.store().brief("r1").as_bytes());
    let hash: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    std::fs::write(dir.join("routine.json"), &old_text).unwrap();
    std::fs::write(
        dir.join("state").join("saved.json"),
        json!({ "hash": hash }).to_string(),
    )
    .unwrap();
    let loaded = f.svc.store().load("r1").unwrap();
    assert_eq!(loaded.place.projects, ["my-app"]);
    let shown = f.svc.call("show", json!({"id": "r1"})).unwrap();
    assert_eq!(shown["routine"]["where"]["projects"], json!(["my-app"]));
    assert!(shown["routine"]["where"].get("project").is_none());
    // The next save writes the new shape.
    f.svc
        .call("set_enabled", json!({"id": "r1", "enabled": false}))
        .unwrap();
    let text = std::fs::read_to_string(dir.join("routine.json")).unwrap();
    assert!(text.contains("\"projects\""), "{text}");
    assert!(!text.contains("\"project\""), "{text}");
    assert!(f.svc.store().load("r1").is_ok());
    // An old file that was edited after its save is still refused.
    std::fs::write(
        dir.join("routine.json"),
        old_text.replace("my-app", "docs-site"),
    )
    .unwrap();
    std::fs::write(
        dir.join("state").join("saved.json"),
        json!({ "hash": hash }).to_string(),
    )
    .unwrap();
    assert!(f
        .svc
        .store()
        .load("r1")
        .unwrap_err()
        .contains("changed outside the app"));
}

#[test]
fn ask_every_time_asks_in_the_brief_and_applies_a_proposal_only_after_approval() {
    let f = fixture();
    install_with(&f, "r1", true, &|r| {
        r["howItRuns"]["mode"] = json!("ask-every-time");
    });
    assert!(entry(&f, "r1")["create"]["prompt"]
        .as_str()
        .unwrap()
        .contains("Ask before every write"));
    let tree = f.svc.store().worktree_dir("r1");
    std::fs::write(tree.join("a.txt"), "changed").unwrap();
    queue(
        &f,
        "r1",
        "s1",
        "1.json",
        json!({"kind": "commit", "message": "fix typo", "paths": ["a.txt"]}),
    );
    f.daemon.answer(&envelope(&good("done")));
    let before = f.git.words().len();
    f.daemon.fire(&f.svc, "r1", 1);
    // Nothing was applied: no add, no commit.
    let ran: Vec<Vec<String>> = f.git.words().into_iter().skip(before).collect();
    assert!(
        !ran.iter()
            .any(|c| matches!(c[1].as_str(), "add" | "commit" | "push")),
        "{ran:?}"
    );
    let run = latest(&f, "r1")["run"].clone();
    assert_eq!(run["proposals"][0]["status"], "pending");
    assert_eq!(run["proposals"][0]["applied"], false);
    assert_eq!(run["pendingApprovals"], 1);

    // The next run resets the worktree; the owner's answer still applies the stashed file.
    std::fs::remove_file(tree.join("a.txt")).unwrap();
    let done = f
        .svc
        .call("approve", json!({"id": "r1", "run": 1, "proposal": 0}))
        .unwrap();
    assert_eq!(done["status"], "applied");
    assert_eq!(f.git.commit_saw_file.lock().unwrap().as_slice(), [true]);
    let run = latest(&f, "r1")["run"].clone();
    assert_eq!(run["proposals"][0]["status"], "applied");
    assert_eq!(run["pendingApprovals"], 0);
    // Once is enough.
    assert!(f
        .svc
        .call("approve", json!({"id": "r1", "run": 1, "proposal": 0}))
        .is_err());
    assert!(f
        .svc
        .call("approve", json!({"id": "r1", "run": 1, "proposal": 5}))
        .is_err());
}

#[test]
fn an_answered_ask_of_the_same_kind_applies_a_proposal_in_ask_every_time_mode() {
    let f = fixture();
    install_with(&f, "r1", true, &|r| {
        r["howItRuns"]["mode"] = json!("ask-every-time");
    });
    let tree = f.svc.store().worktree_dir("r1");
    std::fs::write(tree.join("a.txt"), "x").unwrap();
    // `sushiai ask --kind commit` was answered yes; the pr was not asked about.
    let session = f.daemon.start(&f.svc, "r1", 1);
    say_yes(&f, "r1", &session, "y1", "commit");
    let asks = f.svc.store().asks_dir("r1", &session);
    assert!(asks.starts_with(f.svc.store().state_dir("r1")));
    queue(
        &f,
        "r1",
        &session,
        "1.json",
        json!({"kind": "commit", "message": "m", "paths": ["a.txt"]}),
    );
    queue(
        &f,
        "r1",
        &session,
        "2.json",
        json!({"kind": "pr", "title": "t", "paths": ["a.txt"]}),
    );
    f.daemon.answer(&envelope(&good("done")));
    f.daemon.end(&f.svc, "r1", &session, 1);
    let run = latest(&f, "r1")["run"].clone();
    assert_eq!(run["proposals"][0]["status"], "applied");
    assert_eq!(run["proposals"][1]["status"], "pending");
    // The ask record is consumed.
    assert_eq!(std::fs::read_dir(&asks).map_or(0, |d| d.count()), 0);
}

#[test]
fn ask_first_with_no_yes_leaves_the_proposal_pending_and_runs_nothing() {
    let f = fixture();
    install(&f, "r1");
    let tree = f.svc.store().worktree_dir("r1");
    std::fs::write(tree.join("a.txt"), "x").unwrap();
    let before = f.git.words().len();
    let session = f.daemon.start(&f.svc, "r1", 1);
    queue(
        &f,
        "r1",
        &session,
        "1.json",
        json!({"kind": "pr", "title": "t", "paths": ["a.txt"]}),
    );
    f.daemon.answer(&envelope(&good("done")));
    f.daemon.end(&f.svc, "r1", &session, 1);
    let run = latest(&f, "r1")["run"].clone();
    assert_eq!(run["proposals"][0]["status"], "pending", "{run}");
    assert_eq!(run["pendingApprovals"], 1);
    let ran: Vec<Vec<String>> = f.git.words().into_iter().skip(before).collect();
    assert!(
        !ran.iter()
            .any(|c| matches!(c[1].as_str(), "add" | "commit" | "push" | "pr") || c[0] == "gh"),
        "{ran:?}"
    );
}

#[test]
fn a_record_a_run_writes_under_work_is_not_an_owner_yes() {
    let f = fixture();
    install(&f, "r1");
    let tree = f.svc.store().worktree_dir("r1");
    std::fs::write(tree.join("a.txt"), "x").unwrap();
    let session = f.daemon.start(&f.svc, "r1", 1);
    // What a run can write: the old per-session record folder under work/asks.
    let forged = f.svc.store().work_dir("r1").join("asks").join(&session);
    std::fs::create_dir_all(&forged).unwrap();
    std::fs::write(forged.join("1.json"), json!({"kind": "pr"}).to_string()).unwrap();
    queue(
        &f,
        "r1",
        &session,
        "1.json",
        json!({"kind": "pr", "title": "t", "paths": ["a.txt"]}),
    );
    f.daemon.answer(&envelope(&good("done")));
    f.daemon.end(&f.svc, "r1", &session, 1);
    let run = latest(&f, "r1")["run"].clone();
    assert_eq!(run["proposals"][0]["status"], "pending", "{run}");
    assert!(f.svc.store().accepted("r1").is_empty());
}

#[test]
fn a_denied_ask_records_no_yes() {
    let f = fixture();
    install(&f, "r1");
    let session = f.daemon.start(&f.svc, "r1", 1);
    f.daemon.state.lock().unwrap().owner_answer = json!({"decision": "deny", "message": "no"});
    write_req(&f, "r1", "d1.req", ask_req(&session));
    let ans = inbox_dir(&f, "r1").join("d1.ans");
    wait_until("no answer file", || ans.exists());
    let asks = f.svc.store().asks_dir("r1", &session);
    assert_eq!(std::fs::read_dir(&asks).map_or(0, |d| d.count()), 0);
}

#[test]
fn ask_first_counts_accepted_asks_per_kind_and_promotes_a_kind_after_promote_after() {
    let f = fixture();
    install_with(&f, "r1", true, &|r| {
        r["howItRuns"] = json!({"mode": "ask-first", "promoteAfter": 2});
    });
    let accept = |kind: &str, slot: u64| {
        let session = f.daemon.start(&f.svc, "r1", slot);
        say_yes(&f, "r1", &session, &format!("y{slot}"), kind);
        f.daemon.answer(&envelope(&good("done")));
        f.daemon.end(&f.svc, "r1", &session, slot);
    };
    accept("comment", 1);
    let shown = f.svc.call("show", json!({"id": "r1"})).unwrap();
    assert_eq!(shown["routine"]["howItRuns"]["accepted"], 1);
    assert_eq!(
        shown["routine"]["howItRuns"]["acceptedByKind"]["comment"],
        1
    );
    assert_eq!(shown["routine"]["howItRuns"]["onItsOwn"], json!([]));
    assert!(!entry(&f, "r1")["create"]["prompt"]
        .as_str()
        .unwrap()
        .contains("no longer need an ask"));

    accept("comment", 2);
    accept("message", 3);
    let shown = f.svc.call("show", json!({"id": "r1"})).unwrap();
    let how = &shown["routine"]["howItRuns"];
    assert_eq!(how["mode"], "ask-first");
    assert_eq!(how["onItsOwn"], json!(["comment"]));
    assert_eq!(how["acceptedByKind"]["comment"], 2);
    assert_eq!(how["acceptedByKind"]["message"], 1);
    // The kind that is not yet promoted is the progress the app shows.
    assert_eq!(how["accepted"], 1);
    assert_eq!(how["promoteAfter"], 2);
    // The schedule entry says which kind no longer asks.
    assert!(entry(&f, "r1")["create"]["prompt"]
        .as_str()
        .unwrap()
        .contains("no longer need an ask: comment"));
    // Counting never changes what the owner reviewed: the same save keeps the routine on.
    let mut r = routine_json("r1");
    r["agent"]["extraArgs"] = json!([]);
    r["howItRuns"] = json!({"mode": "ask-first", "promoteAfter": 2});
    let saved = f
        .svc
        .call("save", json!({"routine": r, "brief": "Summarise."}))
        .unwrap();
    assert_eq!(saved["pendingReview"], false);
    assert_eq!(saved["enabled"], true);
}

#[test]
fn on_its_own_never_asks_and_a_dry_run_asks_count_nothing() {
    let f = fixture();
    install_with(&f, "r1", true, &|r| {
        r["howItRuns"]["mode"] = json!("on-its-own");
    });
    let prompt = entry(&f, "r1")["create"]["prompt"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(!prompt.contains("sushiai ask") && !prompt.contains("Ask before"));
    // A proposal is applied without an ask.
    let tree = f.svc.store().worktree_dir("r1");
    std::fs::write(tree.join("a.txt"), "x").unwrap();
    queue(
        &f,
        "r1",
        "s1",
        "1.json",
        json!({"kind": "commit", "message": "m", "paths": ["a.txt"]}),
    );
    f.daemon.answer(&envelope(&good("done")));
    f.daemon.fire(&f.svc, "r1", 1);
    assert_eq!(latest(&f, "r1")["run"]["proposals"][0]["status"], "applied");
    assert!(f.svc.store().accepted("r1").is_empty());
}

#[test]
fn a_login_failure_records_where_and_with_which_harness() {
    let f = fixture();
    install(&f, "r1");
    f.daemon.answer(
        &json!({"type": "result", "subtype": "success", "is_error": true,
                "api_error_status": 401, "result": "Not logged in - Please run /login"})
        .to_string(),
    );
    f.daemon.fire(&f.svc, "r1", 1);
    let run = latest(&f, "r1")["run"].clone();
    assert_eq!(run["status"], "failed");
    assert_eq!(run["loginHost"], "ssh:build-host");
    assert_eq!(run["loginHarness"], "claude");
    // Any other failure carries neither.
    f.daemon.answer("not json");
    f.daemon.answer("still not json");
    f.daemon.fire(&f.svc, "r1", 2);
    let run = latest(&f, "r1")["run"].clone();
    assert_eq!(run["status"], "failed");
    assert!(run["loginHost"].is_null() && run["loginHarness"].is_null());
}

#[test]
fn a_codex_401_with_a_missing_bearer_is_a_login_problem() {
    let f = fixture();
    install(&f, "r1");
    f.daemon.state.lock().unwrap().exit_code = Some(1);
    f.daemon.answer(
        "ERROR: unexpected status 401 Unauthorized: Missing bearer or basic authentication in header",
    );
    f.daemon.fire(&f.svc, "r1", 1);
    let reason = latest(&f, "r1")["run"]["reason"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(reason, "not logged in: log in on ssh:build-host");
    assert_eq!(f.daemon.created().len(), 1);
}

#[test]
fn a_no_answer_reason_keeps_the_last_output_line_without_secrets() {
    let f = fixture();
    install(&f, "r1");
    f.daemon.state.lock().unwrap().exit_code = Some(2);
    f.daemon.answer(
        "starting\n\u{1b}[31merror: model does-not-exist not found\u{1b}[0m sk-abcdefghijklmnop SUSHIAI_TOKEN=hunter2\n\n",
    );
    f.daemon.fire(&f.svc, "r1", 1);
    let v = latest(&f, "r1");
    let reason = v["run"]["reason"].as_str().unwrap();
    assert_eq!(
        reason,
        "the harness gave no answer (exit code 2): error: model does-not-exist not found [redacted]"
    );
    assert!(!reason.contains("hunter2") && !reason.contains('\u{1b}'));
    assert_eq!(
        f.daemon.created().len(),
        1,
        "no retry for a crashed harness"
    );
}

#[test]
fn the_run_record_keeps_the_spawn_argv_and_env_key_names_only() {
    let f = fixture();
    install(&f, "r1");
    f.daemon.answer(&envelope(&good("done")));
    f.daemon.fire(&f.svc, "r1", 1);
    let run = latest(&f, "r1")["run"].clone();
    let argv: Vec<&str> = run["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    assert_eq!(argv[0], "claude");
    assert!(argv.contains(&"--permission-mode"));
    assert_eq!(
        run["envKeys"],
        json!([
            "PATH",
            "SUSHIAI_ROUTINE_DIR",
            "SUSHIAI_ROUTINE_ID",
            "SUSHIAI_ROUTINE_PROJECTS"
        ])
    );
    assert!(!run.to_string().contains("\"env\""));
}

#[test]
fn a_run_started_by_run_now_is_manual_and_a_scheduled_one_is_not() {
    let f = fixture();
    install(&f, "r1");
    f.daemon.state.lock().unwrap().fire_ok = true;
    f.daemon.answer(&envelope(&good("done")));
    f.svc.call("run_now", json!({"id": "r1"})).unwrap();
    f.daemon.fire(&f.svc, "r1", 1);
    assert_eq!(latest(&f, "r1")["run"]["kind"], "manual");
    f.daemon.answer(&envelope(&good("done")));
    f.daemon.fire(&f.svc, "r1", 2);
    assert_eq!(latest(&f, "r1")["run"]["kind"], "scheduled");
}

fn inbox_dir(f: &Fixture, id: &str) -> PathBuf {
    sushiai_routines::store::ask_inbox(&f.svc.store().dir(id))
}

fn write_req(f: &Fixture, id: &str, name: &str, req: Value) -> PathBuf {
    let dir = inbox_dir(f, id);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, req.to_string()).unwrap();
    path
}

fn wait_until(what: &str, check: impl Fn() -> bool) {
    test_wait::wait_until(what, || check().then_some(()));
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn ask_req(session: &str) -> Value {
    json!({"session": session, "token": "tok", "agent": "claude", "text": "Post it?",
           "options": [], "kind": "comment", "ttlSec": 60})
}

/// The owner allows an ask of `kind` of a live run's `session`, the only way a "yes" is
/// recorded: the request file is forwarded by the module, which records it after the allow.
fn say_yes(f: &Fixture, id: &str, session: &str, name: &str, kind: &str) {
    f.daemon.state.lock().unwrap().owner_answer = json!({"decision": "allow", "choice": "yes"});
    let mut req = ask_req(session);
    req["kind"] = json!(kind);
    write_req(f, id, &format!("{name}.req"), req);
    let ans = inbox_dir(f, id).join(format!("{name}.ans"));
    wait_until("no answer file", || ans.exists());
}

#[test]
fn a_request_file_of_a_live_run_reaches_the_owner_and_gets_an_answer_file() {
    let f = fixture();
    install(&f, "r1");
    f.daemon.state.lock().unwrap().owner_answer =
        json!({"decision": "allow", "choice": "yes", "extra": 1});
    let session = f.daemon.start(&f.svc, "r1", 1);
    let req = write_req(&f, "r1", "a1.req", ask_req(&session));
    // A request of a session this routine's run does not own stays where it is.
    let other = write_req(&f, "r1", "b1.req", ask_req("s99"));
    let ans = inbox_dir(&f, "r1").join("a1.ans");
    wait_until("no answer file", || ans.exists());
    assert_eq!(
        read_json(&ans),
        json!({"decision": "allow", "choice": "yes", "message": null})
    );
    let asked = f.daemon.state.lock().unwrap().asked.clone();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0]["session"], json!(session));
    assert_eq!(asked[0]["token"], "tok");
    assert_eq!(asked[0]["text"], "Post it?");
    assert_eq!(asked[0]["ttlSec"], 60);
    assert!(!req.exists() && other.exists());
    // The yes is counted in the per-session records, next to the request files.
    let counted = std::fs::read_dir(f.svc.store().asks_dir("r1", &session))
        .unwrap()
        .flatten()
        .count();
    assert_eq!(counted, 1);
    f.daemon.answer(&envelope(&good("done")));
    f.daemon.end(&f.svc, "r1", &session, 1);
    let shown = f.svc.call("show", json!({"id": "r1"})).unwrap();
    assert_eq!(
        shown["routine"]["howItRuns"]["acceptedByKind"]["comment"],
        1
    );
}

#[test]
fn an_expired_or_overdue_request_gets_expired_and_overdue_is_not_asked() {
    let f = fixture();
    install(&f, "r1");
    f.daemon.state.lock().unwrap().owner_answer = Value::Null;
    let session = f.daemon.start(&f.svc, "r1", 1);
    write_req(&f, "r1", "a1.req", ask_req(&session));
    let ans = inbox_dir(&f, "r1").join("a1.ans");
    wait_until("no answer file", || ans.exists());
    assert_eq!(read_json(&ans), json!({"expired": true}));
    assert_eq!(f.daemon.state.lock().unwrap().asked.len(), 1);
    // Left unanswered past its ttl before it was picked up: expired without asking.
    let mut old = ask_req(&session);
    old["ttlSec"] = json!(1);
    let path = write_req(&f, "r1", "a2.req", old);
    let file = std::fs::File::options().write(true).open(&path).unwrap();
    file.set_modified(std::time::SystemTime::now() - Duration::from_secs(30))
        .unwrap();
    drop(file);
    let ans = inbox_dir(&f, "r1").join("a2.ans");
    wait_until("no answer file for the overdue request", || ans.exists());
    assert_eq!(read_json(&ans), json!({"expired": true}));
    assert_eq!(f.daemon.state.lock().unwrap().asked.len(), 1);
}

#[test]
fn request_files_of_a_finished_run_are_removed() {
    let f = fixture();
    install(&f, "r1");
    let session = f.daemon.start(&f.svc, "r1", 1);
    // A request in flight, a stale answer, and a file of a session this run does not own.
    let work = write_req(&f, "r1", "c1.work", ask_req(&session));
    let stale = inbox_dir(&f, "r1").join("c1.ans");
    std::fs::write(&stale, "{}").unwrap();
    let foreign = write_req(&f, "r1", "d1.req", ask_req("s99"));
    f.daemon.answer(&envelope(&good("done")));
    f.daemon.end(&f.svc, "r1", &session, 1);
    wait_until("the files of the finished run stayed", || {
        !work.exists() && !stale.exists()
    });
    assert!(foreign.exists());
}

#[test]
fn a_run_gets_the_modules_path_so_sushiai_resolves() {
    let f = fixture();
    install(&f, "r1");
    f.daemon.answer(&envelope(&good("done")));
    f.daemon.fire(&f.svc, "r1", 1);
    let create = f.daemon.created().pop().unwrap();
    assert_eq!(create["env"]["PATH"], "/opt/sushiai/bin:/usr/bin");
}

#[test]
fn a_failed_result_the_agent_reports_itself_becomes_the_runs_reason() {
    let f = fixture();
    install(&f, "r1");
    let failed = json!({"status": "failed", "title": "Cannot read the repo",
        "summary": "No access.", "missing": ["git read access", "a token"], "actions": []});
    f.daemon.answer(&envelope(&failed.to_string()));
    f.daemon.fire(&f.svc, "r1", 1);
    let v = wait_for_run(&f, "r1");
    assert_eq!(v["run"]["status"], "failed");
    assert_eq!(v["run"]["reason"], "Cannot read the repo: git read access");
}

#[test]
fn a_dry_run_that_was_denied_a_tool_names_it_and_says_what_a_dry_run_may_use() {
    let f = fixture();
    install(&f, "r1");
    let failed = json!({"status": "failed", "title": "Not a git repository",
        "summary": "git log failed.", "actions": []});
    let denied = json!({"type": "result", "subtype": "success", "is_error": false,
        "permission_denials": [{"tool_name": "Bash", "tool_input": {"command": "git log"}}],
        "structured_output": failed});
    f.daemon.answer(&denied.to_string());
    f.svc.call("dry_run", json!({"id": "r1"})).unwrap();
    let v = wait_for_run(&f, "r1");
    assert_eq!(
        v["run"]["reason"],
        "a dry run uses only tools the owner granted that are read-only: Bash was refused in a dry run \
         (not granted, or not read-only)"
    );
    // Without denials and without such words the agent's own title stays the reason.
    f.daemon.answer(&envelope(&failed.to_string()));
    f.svc.call("dry_run", json!({"id": "r1"})).unwrap();
    let v = test_wait::wait_until("the second run to finish", || {
        let v = f.svc.call("result", json!({"id": "r1"})).unwrap();
        (v["run"]["n"] == 2 && v["run"]["status"] != "running").then_some(v)
    });
    assert_eq!(v["run"]["reason"], "Not a git repository");
}

// ----- trust boundary: what a run or an outside writer can change -----

fn rewrite_routine(f: &Fixture, id: &str, edit: &dyn Fn(&mut Value)) {
    let path = f.svc.store().dir(id).join("routine.json");
    let mut v: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    edit(&mut v);
    std::fs::write(path, v.to_string()).unwrap();
}

#[test]
fn a_routine_file_changed_outside_the_app_is_unusable_and_its_run_is_refused() {
    let f = fixture();
    install(&f, "r1");
    assert!(f.svc.store().load("r1").is_ok());
    // A run (or anything else that can write the folder) widens the routine.
    rewrite_routine(&f, "r1", &|v| {
        v["access"] = json!([{"tool": "Bash(gh issue list:*)", "label": "x", "kind": "read",
                              "granted": true}]);
    });
    let err = f.svc.store().load("r1").unwrap_err();
    assert!(err.contains("changed outside the app; re-save it"), "{err}");
    assert!(f.svc.call("show", json!({"id": "r1"})).is_err());
    assert!(f
        .svc
        .call("grant", json!({"id": "r1", "all": true}))
        .is_err());
    assert!(f
        .svc
        .call("set_enabled", json!({"id": "r1", "enabled": true}))
        .is_err());

    // The daemon's scheduler still starts the session it holds; the module closes it and
    // pauses the routine instead of recording a run from the file.
    let session = f.daemon.start(&f.svc, "r1", 1);
    assert!(f.daemon.state.lock().unwrap().closed.contains(&session));
    assert!(!f
        .daemon
        .state
        .lock()
        .unwrap()
        .schedules
        .contains_key("routine.r1"));
    assert!(f.svc.store().run_numbers("r1").is_empty());
    assert!(inbox(&f).iter().any(|e| e["kind"] == "paused"));

    // Only a save from the app makes it usable again, as a draft with no grant.
    let mut r = routine_json("r1");
    r["agent"]["extraArgs"] = json!([]);
    f.svc
        .call("save", json!({"routine": r, "brief": "Summarise."}))
        .unwrap();
    assert!(f.svc.store().load("r1").is_ok());
}

#[test]
fn a_routine_file_that_is_not_valid_is_unusable_on_every_load() {
    let f = fixture();
    install(&f, "r1");
    let dir = f.svc.store().dir("r1");
    let original = std::fs::read(dir.join("routine.json")).unwrap();
    type Edit = Box<dyn Fn(&mut Value)>;
    let cases: Vec<(&str, Edit)> = vec![
        ("id", Box::new(|v| v["id"] = json!("other"))),
        (
            "mode",
            Box::new(|v| v["howItRuns"]["mode"] = json!("whenever")),
        ),
        (
            "tool",
            Box::new(|v| {
                v["access"] = json!([{"tool": "Bash(sh:*)", "label": "x", "kind": "write"}])
            }),
        ),
        (
            "add-dir",
            Box::new({
                let root = dir.to_string_lossy().into_owned();
                move |v| v["agent"]["extraArgs"] = json!(["--add-dir", root])
            }),
        ),
    ];
    for (name, edit) in cases {
        rewrite_routine(&f, "r1", &|v| edit(v));
        assert!(f.svc.store().load("r1").is_err(), "{name} was accepted");
        std::fs::write(dir.join("routine.json"), &original).unwrap();
        assert!(f.svc.store().load("r1").is_ok(), "{name}: restore");
    }
}

#[test]
fn add_dir_may_name_only_a_folder_under_the_routines_work_folder() {
    let f = fixture();
    let root = f.svc.store().dir("r1");
    let check = |dir: &Path| {
        let mut r = routine_json("r1");
        r["agent"]["extraArgs"] = json!(["--add-dir", dir.to_string_lossy()]);
        f.svc
            .call("validate", json!({"routine": r, "brief": "b"}))
            .unwrap()["ok"]
            == true
    };
    assert!(check(&root.join("work").join("notes")));
    assert!(!check(&root));
    assert!(!check(&root.join("state")));
    assert!(!check(&root.join("work").join("..").join("state")));
}

#[test]
fn a_codex_routine_cannot_be_granted_mcp_tools() {
    let f = fixture();
    let check = |harness: &str, tool: &str| {
        let mut r = routine_json("r1");
        r["agent"]["harness"] = json!(harness);
        r["agent"]["extraArgs"] = json!([]);
        r["access"] = json!([access(tool, "Do it", "write")]);
        f.svc
            .call("validate", json!({"routine": r, "brief": "b"}))
            .unwrap()
    };
    let v = check("codex", "mcp__tracker__comment");
    assert_eq!(v["ok"], false);
    assert!(v["errors"][0]["message"]
        .as_str()
        .unwrap()
        .contains("Codex runs cannot be granted MCP tools yet; use Claude"));
    assert_eq!(check("claude", "mcp__tracker__comment")["ok"], true);
    assert_eq!(check("codex", "Bash(gh issue list:*)")["ok"], true);
}

#[test]
fn a_failed_approve_keeps_the_proposal_pending_with_its_files() {
    let f = fixture();
    install_with(&f, "r1", true, &|r| {
        r["howItRuns"]["mode"] = json!("ask-every-time");
    });
    let tree = f.svc.store().worktree_dir("r1");
    std::fs::write(tree.join("a.txt"), "changed").unwrap();
    queue(
        &f,
        "r1",
        "s1",
        "1.json",
        json!({"kind": "commit", "message": "fix", "paths": ["a.txt"]}),
    );
    f.daemon.answer(&envelope(&good("done")));
    f.daemon.fire(&f.svc, "r1", 1);
    assert_eq!(latest(&f, "r1")["run"]["proposals"][0]["status"], "pending");
    let stash = f.svc.store().pending_dir("r1", 1, 0);
    assert!(stash.join("a.txt").is_file());

    *f.git.fail_commit.lock().unwrap() = true;
    let err = f
        .svc
        .call("approve", json!({"id": "r1", "run": 1, "proposal": 0}));
    assert!(err.is_err());
    let run = latest(&f, "r1")["run"].clone();
    assert_eq!(run["proposals"][0]["status"], "pending", "{run}");
    assert!(run["proposals"][0]["lastError"]
        .as_str()
        .unwrap()
        .contains("unable to write the commit"));
    assert_eq!(run["pendingApprovals"], 1);
    assert!(stash.join("a.txt").is_file(), "the stash was deleted");

    *f.git.fail_commit.lock().unwrap() = false;
    let done = f
        .svc
        .call("approve", json!({"id": "r1", "run": 1, "proposal": 0}))
        .unwrap();
    assert_eq!(done["status"], "applied");
    assert!(!stash.exists());
}
