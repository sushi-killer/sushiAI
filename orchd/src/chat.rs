//! The orchestrator agent's conversation: one thread per repository, run by
//! the daemon so a reply keeps coming (and stays readable) when the app
//! window closes or restarts. Each turn is one CLI run that only reads the
//! repository and manages tasks through `orchd mcp`; the harness session id
//! carries the conversation from turn to turn.

use super::*;
use serde::Serialize;

/// The orchestrator agent's role, handed to Claude as an appended system
/// prompt and to Codex as developer instructions. The tools it gets (read
/// only, plus the orchd MCP bridge) are what actually keep it from doing the
/// work itself; this text explains why.
const ROLE: &str = "You are the owner's task orchestrator in sushiAI. You never change files or run commands yourself: every piece of work becomes an orchd task through the sushiai-orchestrator tools, which run it in an isolated worktree, verify it and report back. Follow that server's instructions. You are not woken up between turns, so never promise to watch or report later: say the task is in the Orchestrator panel. Reply in the owner's language, briefly.";

const SERVER: &str = "sushiai-orchestrator";
const TOOLS: [&str; 8] = [
    "task_list",
    "task_get",
    "task_create",
    "task_start",
    "task_stop",
    "task_answer",
    "task_preflight",
    "settings_get",
];
const MAX_MESSAGES: usize = 200;
const MAX_TEXT: usize = 20_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub id: String,
    pub role: String,
    pub text: String,
    pub ts: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatThread {
    pub repo: String,
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
    /// True only while a turn is live in this daemon; a thread saved as busy
    /// by a daemon that died reads back as idle.
    #[serde(default)]
    pub busy: bool,
    /// The last progress line of a live turn (a tool call, "session started").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_id: Option<String>,
    /// The project's MCP launch config the app resolved; tasks this agent
    /// creates for the same repo get it (`ORCHD_TASK_MCP`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_mcp: Option<serde_json::Value>,
}

fn thread_path(app: &App, repo: &str) -> PathBuf {
    app.data_dir
        .join("chats")
        .join(format!("{}.json", simple_hash(repo)))
}

fn load(app: &App, repo: &str) -> ChatThread {
    let mut thread = std::fs::read_to_string(thread_path(app, repo))
        .ok()
        .and_then(|text| serde_json::from_str::<ChatThread>(&text).ok())
        .unwrap_or_else(|| ChatThread {
            repo: repo.to_string(),
            ..Default::default()
        });
    thread.busy = app.chat_turns.lock().unwrap().contains_key(repo);
    if !thread.busy {
        thread.note = None;
    }
    thread
}

fn save(app: &App, thread: &ChatThread) {
    let _ = store::write_json_atomic(&thread_path(app, &thread.repo), thread);
    let _ = app.events_tx.send(Event::Chat {
        thread: Box::new(serde_json::to_value(thread).unwrap_or_default()),
    });
}

fn push(thread: &mut ChatThread, role: &str, text: &str) {
    thread.messages.push(ChatMessage {
        id: uuid::Uuid::new_v4().to_string(),
        role: role.to_string(),
        text: truncate_chars(text, MAX_TEXT),
        ts: now_ms(),
    });
    let excess = thread.messages.len().saturating_sub(MAX_MESSAGES);
    thread.messages.drain(..excess);
}

fn repo_param(params: &serde_json::Value) -> Result<String, String> {
    let repo = params
        .get("repo")
        .and_then(|v| v.as_str())
        .ok_or("repo is required")?;
    if !Path::new(repo).is_absolute() || !Path::new(repo).is_dir() {
        return Err("repo must be an existing absolute directory".to_string());
    }
    Ok(repo.to_string())
}

pub async fn handle_get(app: &App, params: serde_json::Value) -> Result<serde_json::Value, String> {
    let repo = repo_param(&params)?;
    serde_json::to_value(load(app, &repo)).map_err(|e| e.to_string())
}

pub async fn handle_cancel(
    app: &App,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let repo = repo_param(&params)?;
    if let Some(cancel) = app.chat_turns.lock().unwrap().get(&repo) {
        cancel.cancel();
    }
    Ok(json!({}))
}

pub async fn handle_send(
    app: &App,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let repo = repo_param(&params)?;
    let text = params
        .get("text")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or("text is required")?
        .to_string();
    let settings = app.settings.read().unwrap().clone();
    let route =
        orchestrator_route(&settings).ok_or("no route is configured for the orchestrator")?;
    let cancel = CancelToken::new();
    {
        let mut turns = app.chat_turns.lock().unwrap();
        if turns.contains_key(&repo) {
            return Err("the orchestrator is still answering".to_string());
        }
        turns.insert(repo.clone(), cancel.clone());
    }
    let mut thread = load(app, &repo);
    if let Some(mcp) = params.get("mcp") {
        thread.task_mcp = Some(mcp.clone());
    }
    // A session belongs to one harness; switching routes starts fresh.
    if thread.route_id.as_deref() != Some(route.id.as_str()) {
        thread.session_id = None;
        thread.route_id = Some(route.id.clone());
    }
    push(&mut thread, "user", &text);
    thread.busy = true;
    thread.error = None;
    save(app, &thread);
    let app = app.arc();
    tokio::spawn(async move {
        run_turn(&app, thread, route, text, cancel).await;
    });
    Ok(json!({}))
}

/// The orchestrator agent's route: the one the owner picked, else the route
/// that handles standard tasks.
pub(super) fn orchestrator_route(settings: &Settings) -> Option<Route> {
    let find = |id: &str| settings.routes.iter().find(|r| r.id == id).cloned();
    find(&settings.orchestrator).or_else(|| find(settings.tiers.get(&Tier::Standard)?))
}

fn mcp_server(app: &App, task_mcp_file: Option<&Path>) -> serde_json::Value {
    let mut server = json!({
        "command": app.orchd_path,
        "args": [
            "mcp",
            "--data",
            app.data_dir.to_string_lossy(),
            "--socket",
            app.socket_path.to_string_lossy(),
        ],
    });
    if let Some(file) = task_mcp_file {
        server["env"] = json!({"ORCHD_TASK_MCP": file.to_string_lossy()});
    }
    server
}

fn claude_argv(
    route: &Route,
    mcp_config: &Path,
    settings_path: &Path,
    session: Option<&str>,
) -> Vec<String> {
    let tools = TOOLS
        .iter()
        .map(|t| format!("mcp__{SERVER}__{t}"))
        .collect::<Vec<_>>()
        .join(",");
    let mut argv: Vec<String> = [
        "-p",
        "--output-format",
        "stream-json",
        "--verbose",
        "--setting-sources",
        "project,local",
        "--disable-slash-commands",
        "--tools",
        "Read,Grep,Glob",
        "--append-system-prompt",
        ROLE,
        "--strict-mcp-config",
        "--mcp-config",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    argv.push(mcp_config.to_string_lossy().to_string());
    argv.push("--settings".to_string());
    argv.push(settings_path.to_string_lossy().to_string());
    argv.push("--allowedTools".to_string());
    argv.push(tools);
    argv.push("--permission-prompts".to_string());
    argv.push("none".to_string());
    if let Some(model) = &route.model {
        argv.push("--model".to_string());
        argv.push(model.clone());
    }
    if let Some(effort) = &route.effort {
        argv.push("--effort".to_string());
        argv.push(effort.clone());
    }
    if let Some(session) = session {
        argv.push("--resume".to_string());
        argv.push(session.to_string());
    }
    argv
}

fn codex_argv(
    route: &Route,
    repo: &str,
    server: &serde_json::Value,
    session: Option<&str>,
) -> Vec<String> {
    let mut argv: Vec<String> = match session {
        // `resume` takes neither `-C` nor `--sandbox`; the cwd is set on the
        // process and the sandbox through config.
        Some(session) => vec![
            "exec".into(),
            "resume".into(),
            session.into(),
            "--json".into(),
            "--skip-git-repo-check".into(),
            "-c".into(),
            "sandbox_mode=\"read-only\"".into(),
        ],
        None => vec![
            "exec".into(),
            "--json".into(),
            "--skip-git-repo-check".into(),
            "-C".into(),
            repo.into(),
            "--sandbox".into(),
            "read-only".into(),
        ],
    };
    let toml = |v: &serde_json::Value| v.to_string();
    argv.push("-c".into());
    argv.push(format!("developer_instructions={}", toml(&json!(ROLE))));
    argv.push("-c".into());
    argv.push(format!(
        "mcp_servers.{SERVER}.command={}",
        toml(&server["command"])
    ));
    argv.push("-c".into());
    argv.push(format!(
        "mcp_servers.{SERVER}.args={}",
        toml(&server["args"])
    ));
    if let Some(env) = server.get("env").and_then(|e| e.as_object()) {
        for (key, value) in env {
            argv.push("-c".into());
            argv.push(format!("mcp_servers.{SERVER}.env.{key}={}", toml(value)));
        }
    }
    if let Some(model) = &route.model {
        argv.push("-m".into());
        argv.push(model.clone());
    }
    if let Some(effort) = &route.effort {
        argv.push("-c".into());
        argv.push(format!("model_reasoning_effort=\"{effort}\""));
    }
    argv.push("-".into());
    argv
}

async fn run_turn(
    app: &Arc<App>,
    mut thread: ChatThread,
    route: Route,
    text: String,
    cancel: CancelToken,
) {
    let repo = thread.repo.clone();
    let dir = app.data_dir.join("chats").join(simple_hash(&repo));
    let _ = std::fs::create_dir_all(&dir);
    let task_mcp_file = thread.task_mcp.as_ref().and_then(|mcp| {
        let file = dir.join("task-mcp.json");
        store::write_json_atomic(&file, &json!({"repo": repo, "mcp": mcp}))
            .ok()
            .map(|_| file)
    });
    let server = mcp_server(app, task_mcp_file.as_deref());
    let session = thread.session_id.clone();
    let key_path = dir.join("key");
    let argv = match route.harness {
        Harness::Claude => {
            let mcp_config = dir.join("mcp.json");
            let _ = store::write_json_atomic(&mcp_config, &json!({"mcpServers": {SERVER: server}}));
            let settings_path = dir.join("settings.json");
            let settings = app.settings.read().unwrap().clone();
            write_readonly_claude_settings_with_profile(
                &route,
                app,
                &key_path,
                &settings,
                &[app.data_dir.to_string_lossy().to_string()],
                &settings_path,
            );
            claude_argv(&route, &mcp_config, &settings_path, session.as_deref())
        }
        Harness::Codex => codex_argv(&route, &repo, &server, session.as_deref()),
    };

    let result = run(app, &mut thread, route.harness, &argv, &text, &dir, &cancel).await;
    let _ = std::fs::remove_file(&key_path);
    app.chat_turns.lock().unwrap().remove(&repo);

    // Messages sent meanwhile are impossible (one turn per repo), but the
    // owner may have been shown a fresher note: keep this turn's thread.
    thread.busy = false;
    thread.note = None;
    match result {
        Ok(outcome) => {
            if outcome.session_id.is_some() {
                thread.session_id = outcome.session_id.clone();
            }
            match (
                outcome.final_text.filter(|t| !t.trim().is_empty()),
                outcome.error,
            ) {
                (Some(reply), _) => push(&mut thread, "assistant", &reply),
                (None, Some(error)) => {
                    // A session the CLI no longer knows fails every resume.
                    thread.session_id = None;
                    thread.error = Some(truncate_chars(&error, 2000));
                }
                (None, None) => thread.error = Some("The orchestrator gave no reply.".into()),
            }
        }
        Err(RunError::Cancelled) => thread.error = Some("Stopped.".into()),
        Err(RunError::Io(error)) => thread.error = Some(error),
    }
    save(app, &thread);
}

async fn run(
    app: &Arc<App>,
    thread: &mut ChatThread,
    harness_kind: Harness,
    argv: &[String],
    text: &str,
    dir: &Path,
    cancel: &CancelToken,
) -> Result<harness::RunOutcome, RunError> {
    let bin = resolve_binary(harness_kind);
    let mut cmd = tokio::process::Command::new(&bin);
    cmd.args(argv)
        .current_dir(&thread.repo)
        .env("PATH", augmented_path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(unix)]
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| RunError::Io(format!("{bin}: {e}")))?;
    let pgid = child.id().map(|p| p as i32);
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        let _ = stdin.write_all(text.as_bytes()).await;
    }
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let mut out = tokio::io::AsyncBufReadExt::lines(tokio::io::BufReader::new(stdout));
    let mut err = tokio::io::AsyncBufReadExt::lines(tokio::io::BufReader::new(stderr));
    let events = dir.join("events.jsonl");
    let mut outcome = harness::RunOutcome::default();
    let mut stderr_tail = String::new();
    let (mut out_done, mut err_done) = (false, false);
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                kill_group(pgid, &mut child).await;
                return Err(RunError::Cancelled);
            }
            line = out.next_line(), if !out_done => match line {
                Ok(Some(l)) => {
                    append_line(&events, &l);
                    if let Some(note) = harness::feed_stream_line(harness_kind, &l, &mut outcome) {
                        thread.note = Some(note);
                        save(app, thread);
                    }
                }
                _ => out_done = true,
            },
            line = err.next_line(), if !err_done => match line {
                Ok(Some(l)) => {
                    stderr_tail = tail_chars(&format!("{stderr_tail}{l}\n"), 2000);
                }
                _ => err_done = true,
            },
            status = child.wait(), if out_done && err_done => {
                if let Ok(status) = status {
                    if !status.success() && outcome.error.is_none() {
                        let tail = stderr_tail.trim();
                        outcome.error = Some(if tail.is_empty() {
                            format!("{harness_kind:?} exited with {status}.")
                        } else {
                            tail.to_string()
                        });
                    }
                }
                break;
            }
        }
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(harness: Harness) -> Route {
        Route {
            id: "r".into(),
            label: "R".into(),
            harness,
            model: Some("m".into()),
            effort: None,
            profile_id: None,
        }
    }

    #[test]
    fn claude_chat_reads_only_and_reaches_orchd_through_one_allowed_tools_value() {
        let argv = claude_argv(
            &route(Harness::Claude),
            Path::new("/d/mcp.json"),
            Path::new("/d/settings.json"),
            Some("sess"),
        );
        let at = |flag: &str| argv[argv.iter().position(|a| a == flag).unwrap() + 1].clone();
        assert_eq!(at("--tools"), "Read,Grep,Glob");
        assert_eq!(at("--mcp-config"), "/d/mcp.json");
        assert_eq!(at("--resume"), "sess");
        assert_eq!(at("--allowedTools").split(',').count(), 8);
        assert!(!at("--allowedTools").contains("task_delete"));
        assert!(!argv.iter().any(|a| a == "acceptEdits"));
    }

    #[test]
    fn codex_chat_is_read_only_fresh_or_resumed() {
        let server = json!({"command": "/o", "args": ["mcp"], "env": {"ORCHD_TASK_MCP": "/f"}});
        let fresh = codex_argv(&route(Harness::Codex), "/repo", &server, None);
        assert_eq!(&fresh[..2], &["exec".to_string(), "--json".to_string()]);
        assert!(fresh
            .windows(2)
            .any(|w| w[0] == "--sandbox" && w[1] == "read-only"));
        assert!(fresh
            .contains(&"mcp_servers.sushiai-orchestrator.env.ORCHD_TASK_MCP=\"/f\"".to_string()));
        let resumed = codex_argv(&route(Harness::Codex), "/repo", &server, Some("t1"));
        assert_eq!(
            &resumed[..3],
            &["exec".to_string(), "resume".to_string(), "t1".to_string()]
        );
        assert!(resumed.contains(&"sandbox_mode=\"read-only\"".to_string()));
        assert!(!resumed.contains(&"-C".to_string()));
        assert_eq!(resumed.last().unwrap(), "-");
    }

    #[test]
    fn the_orchestrator_uses_its_own_route_else_the_standard_one() {
        let mut settings = Settings::default();
        let standard = settings.tiers.get(&Tier::Standard).unwrap().clone();
        assert_eq!(orchestrator_route(&settings).unwrap().id, standard);
        settings.orchestrator = "claude-opus".into();
        assert_eq!(orchestrator_route(&settings).unwrap().id, "claude-opus");
        settings.orchestrator = "gone".into();
        assert_eq!(orchestrator_route(&settings).unwrap().id, standard);
    }

    #[test]
    fn a_thread_keeps_its_latest_messages_only() {
        let mut thread = ChatThread::default();
        for i in 0..(MAX_MESSAGES + 5) {
            push(&mut thread, "user", &i.to_string());
        }
        assert_eq!(thread.messages.len(), MAX_MESSAGES);
        assert_eq!(thread.messages[0].text, "5");
    }
}
