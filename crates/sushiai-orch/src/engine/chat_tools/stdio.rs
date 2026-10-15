use super::catalog::KnownTool;
use crate::harness::environment::augmented_path;
use serde_json::json;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

const MCP_TIMEOUT: Duration = Duration::from_secs(30);

/// A newline-delimited JSON-RPC session with a stdio MCP server.
struct StdioSession {
    _child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    lines: tokio::io::Lines<tokio::io::BufReader<tokio::process::ChildStdout>>,
    next_id: u64,
}

impl StdioSession {
    async fn start(server: &serde_json::Value) -> Result<Self, String> {
        let command = server
            .get("command")
            .and_then(|c| c.as_str())
            .ok_or("the server has no command")?;
        let mut cmd = tokio::process::Command::new(command);
        for arg in server
            .get("args")
            .and_then(|a| a.as_array())
            .into_iter()
            .flatten()
        {
            cmd.arg(arg.as_str().unwrap_or_default());
        }
        for (key, value) in server
            .get("env")
            .and_then(|e| e.as_object())
            .into_iter()
            .flatten()
        {
            cmd.env(key, value.as_str().unwrap_or_default());
        }
        cmd.env("PATH", augmented_path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("could not start {command}: {e}"))?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let mut session = StdioSession {
            _child: child,
            stdin,
            lines: tokio::io::BufReader::new(stdout).lines(),
            next_id: 1,
        };
        session
            .request(
                "initialize",
                json!({
                    "protocolVersion": "2025-03-26",
                    "capabilities": {},
                    "clientInfo": {"name": "orchd", "version": "0"},
                }),
            )
            .await?;
        session
            .send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .await?;
        Ok(session)
    }

    async fn send(&mut self, message: &serde_json::Value) -> Result<(), String> {
        let mut line = message.to_string();
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| e.to_string())
    }

    async fn request(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await?;
        loop {
            let line = self
                .lines
                .next_line()
                .await
                .map_err(|e| e.to_string())?
                .ok_or("the server closed its output")?;
            let Ok(reply) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            if reply.get("id") != Some(&json!(id)) {
                continue;
            }
            if let Some(error) = reply.get("error") {
                let message = error.get("message").and_then(|m| m.as_str());
                return Err(message
                    .unwrap_or("the server returned an error")
                    .to_string());
            }
            return Ok(reply.get("result").cloned().unwrap_or_default());
        }
    }
}

pub(super) async fn within<T>(
    work: impl std::future::Future<Output = Result<T, String>>,
) -> Result<T, String> {
    tokio::time::timeout(MCP_TIMEOUT, work)
        .await
        .map_err(|_| "the server did not answer in time".to_string())?
}

pub(super) async fn list_stdio(server: &serde_json::Value) -> Result<Vec<KnownTool>, String> {
    within(async {
        let mut session = StdioSession::start(server).await?;
        let listed = session.request("tools/list", json!({})).await?;
        Ok(listed
            .get("tools")
            .and_then(|t| t.as_array())
            .into_iter()
            .flatten()
            .filter_map(|tool| {
                Some(KnownTool {
                    name: tool.get("name")?.as_str()?.to_string(),
                    read_only: tool
                        .pointer("/annotations/readOnlyHint")
                        .and_then(|h| h.as_bool()),
                })
            })
            .collect())
    })
    .await
}

/// The text of a `tools/call` result; an `isError` result is an error.
fn call_text(result: &serde_json::Value) -> Result<String, String> {
    let text = result
        .get("content")
        .and_then(|c| c.as_array())
        .into_iter()
        .flatten()
        .filter_map(|block| block.get("text").and_then(|t| t.as_str()))
        .collect::<Vec<_>>()
        .join("\n");
    if result.get("isError").and_then(|e| e.as_bool()) == Some(true) {
        Err(if text.is_empty() {
            "the tool reported an error".to_string()
        } else {
            text
        })
    } else {
        Ok(text)
    }
}

pub(super) async fn call_stdio(
    server: &serde_json::Value,
    tool: &str,
    args: &serde_json::Value,
) -> Result<String, String> {
    within(async {
        let mut session = StdioSession::start(server).await?;
        let result = session
            .request("tools/call", json!({"name": tool, "arguments": args}))
            .await?;
        call_text(&result)
    })
    .await
}
