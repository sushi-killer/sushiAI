//! Method names, error codes and typed params/results.

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;
pub const CAPABILITIES: &[&str] = &[
    "sessions",
    "attach",
    "resync",
    "agents",
    "catalog",
    "catalogRead",
    "hibernate",
    "schedules",
    "ownerAsk",
    "agentsCatalog",
];

pub mod method {
    pub const HELLO: &str = "hello";
    pub const SESSION_CREATE: &str = "session.create";
    pub const SESSION_LIST: &str = "session.list";
    pub const SESSION_INPUT: &str = "session.input";
    pub const SESSION_RESIZE: &str = "session.resize";
    pub const SESSION_CLOSE: &str = "session.close";
    pub const SESSION_ATTACH: &str = "session.attach";
    pub const SESSION_DETACH: &str = "session.detach";
    pub const SESSION_EXITED: &str = "session.exited";
    /// Notification sent to a subscriber that fell behind: a fresh snapshot replaces the lost bytes.
    pub const SESSION_SNAPSHOT: &str = "session.snapshot";
    /// Notification sent to a client that missed events. It has no payload: the client lists the
    /// sessions again.
    pub const SESSION_RESYNC: &str = "session.resync";
    /// Request from an agent's hook process (token required).
    pub const HOOK_EVENT: &str = "hook.event";
    /// Request: the owner's answer to an open permission ask.
    pub const ASK_RESPOND: &str = "ask.respond";
    /// Notification: the agent status of a session changed.
    pub const SESSION_STATUS: &str = "session.status";
    /// Notification: the agent's own session id or transcript path became known or changed.
    pub const SESSION_META: &str = "session.meta";
    /// Notification: an agent asks permission for a tool call.
    pub const SESSION_ASK: &str = "session.ask";
    /// Notification: an ask is closed (answered, timed out, or its agent went away).
    pub const SESSION_ASK_CLOSED: &str = "session.askClosed";
    /// Request: plain text of a session's screen, optionally with scrollback lines before it.
    pub const SESSION_READ: &str = "session.read";
    /// Request from an agent's `sushiai open` process (token required).
    pub const HOOK_OPEN: &str = "hook.open";
    /// Notification: an agent asked the desktop to open something.
    pub const SESSION_OPEN: &str = "session.open";
    pub const HOLD_ATTACH: &str = "hold.attach";
    pub const HOLD_INPUT: &str = "hold.input";
    pub const HOLD_RESIZE: &str = "hold.resize";
    pub const HOLD_CLOSE: &str = "hold.close";
    pub const HOLD_EXITED: &str = "hold.exited";

    // Catalog, lifecycle notifications and daemon control (step 3b / 5). Additive.
    /// Request: replace or merge the projects of this host (desktop is master).
    pub const PROJECTS_SYNC: &str = "projects.sync";
    /// Request: replace or merge the groups.
    pub const GROUPS_SYNC: &str = "groups.sync";
    /// Request: read the live projects and groups. Params: none. Result: `CatalogSnapshot`.
    pub const CATALOG_GET: &str = "catalog.get";
    /// Request: change a session's project, group or title.
    pub const SESSION_UPDATE: &str = "session.update";
    /// Request: forget an exited session.
    pub const SESSION_REMOVE: &str = "session.remove";
    /// Notification: a session was created. Params: `SessionInfo`.
    pub const SESSION_CREATED: &str = "session.created";
    /// Notification: a session's title or binding changed. Params: `SessionInfo`.
    pub const SESSION_UPDATED: &str = "session.updated";
    /// Notification: a session was forgotten. Params: `SessionRemoved`.
    pub const SESSION_REMOVED: &str = "session.removed";
    /// Request: liveness check for connectors. Answers `{}`, even before `hello`.
    pub const PING: &str = "$/ping";
    /// Request: stop the daemon; holders and sessions keep running and the next client starts
    /// a new daemon. Not available to a hook connection.
    /// Optional `closeSessions: true` also stops sessions and their holders.
    pub const DAEMON_SHUTDOWN: &str = "daemon.shutdown";

    // Session hibernation (additive).
    /// Request: wake a hibernated (or exited) agent session from its stored launch.
    pub const SESSION_WAKE: &str = "session.wake";
    /// Request: hibernate an agent session now.
    pub const SESSION_HIBERNATE: &str = "session.hibernate";
    /// Request: this connection looks at a session; a focused session never hibernates.
    pub const SESSION_FOCUS: &str = "session.focus";
    /// Request: change the daemon's settings (`hibernateAfterSecs`).
    pub const DAEMON_CONFIGURE: &str = "daemon.configure";

    // Schedules and owner asks (additive).
    /// Request: create or replace a schedule entry (`ScheduleSet`).
    pub const SCHEDULE_SET: &str = "schedule.set";
    /// Request: delete a schedule entry (`ScheduleId`).
    pub const SCHEDULE_REMOVE: &str = "schedule.remove";
    /// Request: list the entries with their state. Params: none. Result: `ScheduleInfo[]`.
    pub const SCHEDULE_LIST: &str = "schedule.list";
    /// Request: run an entry now (`ScheduleId`). Result: `ScheduleFired`.
    pub const SCHEDULE_FIRE: &str = "schedule.fire";
    /// Notification: an entry fired, was skipped, caught up, timed out or its run ended.
    /// Params: `ScheduleEvent`.
    pub const SCHEDULE_EVENT: &str = "schedule.event";
    /// Request from an agent's `sushiai ask` process (token required): ask the owner a
    /// question and wait for `ask.respond`. Params: `HookAsk`. Result: `HookResult`.
    pub const HOOK_ASK: &str = "hook.ask";

    // The agent catalog (additive). Unrelated to `catalog.get`, which is the
    // projects and groups of this host.
    /// Request: what each agent CLI on this host offers — its models, their
    /// reasoning levels, the permission modes and the defaults. No params: it
    /// reads the daemon's own Codex home. Result: one `AgentCatalog` per agent,
    /// in registry order.
    pub const AGENTS_CATALOG: &str = "agents.catalog";
}

pub mod code {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL: i64 = -32603;
    pub const NOT_INITIALIZED: i64 = 1001;
    pub const PROTOCOL_MISMATCH: i64 = 1002;
    pub const SESSION_NOT_FOUND: i64 = 1003;
    pub const SPAWN_FAILED: i64 = 1004;
    pub const SESSION_NOT_RUNNING: i64 = 1005;
    /// The input queue of a session is full; the child is not reading.
    pub const INPUT_BACKPRESSURE: i64 = 1006;
    /// A hook presented a wrong session token, or a hook connection called a non-hook method.
    pub const UNAUTHORIZED: i64 = 1007;
    pub const ASK_NOT_FOUND: i64 = 1008;
    /// The session is still running or detached; only an exited one can be removed.
    pub const SESSION_STILL_RUNNING: i64 = 1010;
    /// The daemon is stopping and takes no more changes.
    pub const SHUTTING_DOWN: i64 = 1011;
    /// The session has no stored launch (or it cannot be opened): it cannot be woken.
    pub const WAKE_NEEDS_LAUNCH: i64 = 1012;
    /// The key that opens the session's stored launch is not available (a locked or denied
    /// keychain): the session stays asleep and a later wake can work.
    pub const LAUNCH_KEY_UNAVAILABLE: i64 = 1013;
    /// No schedule entry has that id.
    pub const SCHEDULE_NOT_FOUND: i64 = 1014;
    /// A hosted module is still starting; the request waited and gave up.
    pub const MODULE_STARTING: i64 = 1100;
}

/// Serde adapter: `Vec<u8>` as a standard base64 string.
pub mod b64 {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(d)?;
        STANDARD.decode(text).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello {
    pub protocol: u32,
    pub client: String,
    /// `"hook"` limits the connection to `hook.*` methods and sends it no notifications.
    #[serde(default)]
    pub role: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelloResult {
    pub protocol: u32,
    pub capabilities: Vec<String>,
    pub daemon: String,
    /// The host name this daemon is the replica of (see `projects.sync`).
    #[serde(default)]
    pub host: String,
    /// sha256 (hex) of the daemon's own binary, measured at startup. Additive: an older daemon
    /// does not send it. The desktop compares it with the bundled manifest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCreate {
    /// Required unless `agent` is `claude` or `codex`, which build their own command line.
    #[serde(default)]
    pub cmd: Vec<String>,
    pub cwd: String,
    pub cols: u16,
    pub rows: u16,
    #[serde(default)]
    pub title: Option<String>,
    /// `claude` and `codex` get hooks; any other name only labels the session.
    #[serde(default)]
    pub agent: Option<String>,
    /// Agent session id to resume.
    #[serde(default)]
    pub resume: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub extra_args: Vec<String>,
    /// Extra variables for the child. Never logged. A claude or codex launch keeps them sealed
    /// on disk (see the daemon's launch store) so the session can wake.
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
    /// Project to bind to. Without it the daemon picks the project whose folder holds `cwd`.
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub group: Option<String>,
    /// Claude only: extra keys for the single `--settings` JSON (for example `apiKeyHelper`
    /// and `env`). A `hooks` key is rejected; the daemon's hooks always win. May hold secrets:
    /// never logged, kept only in the sealed launch store, and not part of any record clients
    /// can read back.
    #[serde(default)]
    pub claude_settings: Option<serde_json::Map<String, serde_json::Value>>,
    /// A key that matches an existing session record returns that session.
    #[serde(default)]
    pub idempotency_key: Option<String>,
    /// A session started on behalf of a job, not typed by the owner: clients keep it out of
    /// the session tree. Everything else treats it as any other session.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub background: bool,
    /// Claude or Codex run one turn without a terminal UI and exit: no folder trust dialog,
    /// no input. Claude gets `-p`; Codex runs `codex exec` and prints its events as JSON
    /// lines. A caller reads the answer from the session output.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub headless: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionId {
    pub id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    Running,
    /// The daemon lost its holder connection; the session may still run. Never persisted.
    Detached,
    Exited,
    /// No process: the record, the agent session and a saved screen tail are kept.
    Hibernated,
    /// The holder is being respawned with the agent's resume command; input is queued.
    Waking,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentStatus {
    Starting,
    Working,
    Blocked,
    Idle,
    Exited,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StatusSource {
    Hook,
    Heuristic,
}

/// An open permission ask: a tool call the agent waits on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ask {
    pub ask_id: String,
    pub session: String,
    pub tool: Option<String>,
    pub input: serde_json::Value,
}

/// Agent fields of a session record, flattened into `SessionInfo`. All stay empty for a
/// plain shell. Persisted in `state.json` except `asks`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfo {
    #[serde(default, rename = "agent", skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session: Option<String>,
    /// An agent seen in the foreground of a session that was launched without one (started by
    /// hand in a shell). Detection only: it never makes the session a hook agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreground_agent: Option<String>,
    /// The working directory of that agent's process.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreground_cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_path: Option<String>,
    /// The agent's status, separate from the process `status`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_status: Option<AgentStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_source: Option<StatusSource>,
    /// Unix milliseconds of the last status change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_since: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub asks: Vec<Ask>,
    /// SHA-256 (hex) of the session token. Only in the state file, never sent to clients.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_hash: Option<String>,
    /// Launch idempotency key of the session. Kept in the state file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    /// 0 for the first process of the session, plus one per wake.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub incarnation: u32,
    /// "Keep awake": a pinned session never hibernates. Owned by the registry.
    #[serde(default, skip_serializing_if = "is_false")]
    pub pinned: bool,
    /// Unix milliseconds the session went to sleep; present while hibernated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hibernated_at: Option<u64>,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub id: String,
    pub cmd: Vec<String>,
    pub cwd: String,
    #[serde(default)]
    pub title: Option<String>,
    pub status: SessionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Pid of the session's holder, so a dead holder can be told from a busy one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holder_pid: Option<u32>,
    pub cols: u16,
    pub rows: u16,
    /// Bound project (catalog id). Owned by the daemon's registry: `session.update` changes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Copied from `SessionCreate.background`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub background: bool,
    #[serde(flatten)]
    pub agent: AgentInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInput {
    pub id: String,
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionResize {
    pub id: String,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionClose {
    pub id: String,
    pub graceful: bool,
}

/// Params of `session.attach`. `scrollback` (default 0) puts up to that many formatted history
/// lines in front of the screen snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionAttach {
    pub id: String,
    #[serde(default)]
    pub scrollback: Option<u32>,
}

/// Params of `session.wake`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionWake {
    pub id: String,
}

/// Params of `session.focus`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionFocus {
    pub id: String,
    pub focused: bool,
}

/// Params of `daemon.configure`. 0 turns hibernation off.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonConfigure {
    pub hibernate_after_secs: u64,
}

/// Params of `session.read`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRead {
    pub id: String,
    #[serde(default)]
    pub scrollback: Option<u32>,
}

/// Result of `session.read`: plain text, scrollback lines first.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadResult {
    pub text: String,
    pub rows: u16,
    pub cols: u16,
}

/// Params of `hook.open`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookOpen {
    pub session: String,
    pub token: String,
    /// What to open with, for example `preview/files`.
    pub target: String,
    /// Absolute path or other argument for the target.
    pub arg: String,
}

/// Result of `hook.open`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookOpenResult {
    pub nonce: String,
}

/// Params of the `session.open` notification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionOpen {
    pub id: String,
    pub target: String,
    pub arg: String,
    pub nonce: String,
}

/// Result of `session.attach`; also the params of a `session.snapshot` notification (with `id`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachResult {
    #[serde(with = "b64")]
    pub snapshot: Vec<u8>,
    pub seq: u64,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub id: String,
    #[serde(flatten)]
    pub attach: AttachResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionExited {
    pub id: String,
    pub code: Option<i32>,
    /// Length of the session's output stream at the exit: every output frame with a lower
    /// offset precedes the exit, though it may reach a client after this note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HoldAttach {
    pub from_seq: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HoldAttachResult {
    pub from_seq: u64,
    pub next_seq: u64,
    #[serde(default)]
    pub pid: u32,
    /// The PTY size over the replayed bytes: the size the first replayed byte was written at,
    /// then every later resize with the offset it happened at. Empty from an older holder.
    #[serde(default)]
    pub sizes: Vec<SizeMark>,
    /// Pid of the PTY child (the shell). None from an older holder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child: Option<u32>,
}

/// The PTY had this size from stream offset `seq` on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SizeMark {
    pub seq: u64,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HoldInput {
    #[serde(with = "b64")]
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HoldResize {
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Signal {
    #[serde(rename = "TERM")]
    Term,
    #[serde(rename = "KILL")]
    Kill,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HoldClose {
    pub signal: Signal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HoldExited {
    pub code: Option<i32>,
}

/// Params of `hook.event`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookEvent {
    pub session: String,
    pub token: String,
    /// The hook argument (`session-start`, `permission`, ...).
    pub event: String,
    /// The agent that ran the hook (`claude`, `codex`); must be the session's agent.
    pub agent: String,
    pub payload: serde_json::Value,
}

/// Result of `hook.event`: for a permission ask, the JSON the hook prints; otherwise null.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookResult {
    pub answer: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Allow,
    Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AskRespond {
    pub ask_id: String,
    pub decision: Decision,
    #[serde(default)]
    pub message: Option<String>,
    /// The option id picked for an owner ask (`hook.ask`); ignored for a permission ask.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choice: Option<String>,
}

/// Params of `hook.ask`: a question for the owner from a process inside a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookAsk {
    pub session: String,
    pub token: String,
    /// The agent that owns the session (`claude`, `codex`).
    pub agent: String,
    pub text: String,
    /// Offered answers; the owner picks one id (`AskRespond.choice`). Free text goes in
    /// `AskRespond.message`.
    #[serde(default)]
    pub options: Vec<AskOption>,
    /// Seconds until the ask expires without an answer; default: the permission wait.
    #[serde(default)]
    pub ttl_sec: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskOption {
    pub id: String,
    pub label: String,
}

/// What to do when a schedule fires while its previous session still runs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Overlap {
    /// Do not start; emit a `skipped` event.
    #[default]
    Skip,
}

fn enabled_by_default() -> bool {
    true
}

/// Params of `schedule.set`: one entry; the same `id` replaces it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleSet {
    pub id: String,
    /// Five fields: minute hour day-of-month month day-of-week.
    pub cron: String,
    /// IANA zone the cron is read in, for example `Europe/Berlin`.
    pub tz: String,
    /// Upper bound of a fixed per-entry delay, derived from the id. Default 0.
    #[serde(default)]
    pub jitter_sec: Option<u64>,
    /// A fired session that runs longer is closed. Waiting on an owner ask does not count.
    #[serde(default)]
    pub timeout_sec: Option<u64>,
    #[serde(default)]
    pub overlap: Overlap,
    /// How old a missed slot may be and still run once at daemon start. Default 7 days.
    #[serde(default)]
    pub catchup_window_sec: Option<u64>,
    /// What the daemon passes to `session.create` at each fire.
    pub create: SessionCreate,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
}

/// Params of `schedule.remove` and `schedule.fire`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleId {
    pub id: String,
}

/// The run an entry waits on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleRun {
    pub session: String,
    /// Unix milliseconds.
    pub started_at: u64,
    pub scheduled_for: u64,
}

/// One entry of `schedule.list`. `create.env` and `create.claudeSettings` are left out.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleInfo {
    #[serde(flatten)]
    pub set: ScheduleSet,
    /// Unix milliseconds of the last start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_fired_at: Option<u64>,
    /// Unix milliseconds of the next slot (without jitter); none when disabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running: Option<ScheduleRun>,
}

/// Result of `schedule.fire`: the started session, or why nothing started.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleFired {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleEventKind {
    Fired,
    Skipped,
    CaughtUp,
    Timeout,
    Ended,
}

/// Params of the `schedule.event` notification. Times are unix milliseconds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleEvent {
    pub id: String,
    pub kind: ScheduleEventKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// The cron slot this belongs to (the time of the request for `schedule.fire`).
    pub scheduled_for: u64,
    /// When the run started; for `skipped`, when it was skipped.
    pub started_at: u64,
    /// For `ended` and `timeout`: when the run ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<u64>,
}

/// Params of `session.status`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStatusChanged {
    pub id: String,
    pub status: AgentStatus,
    pub status_source: StatusSource,
    pub status_since: u64,
}

/// Params of `session.meta`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMeta {
    pub id: String,
    pub agent_session: Option<String>,
    pub transcript_path: Option<String>,
    /// Additive: absent from an older daemon.
    #[serde(default)]
    pub foreground_agent: Option<String>,
    #[serde(default)]
    pub foreground_cwd: Option<String>,
}

/// Params of `session.askClosed`. `decided` is true when the owner answered; false when the
/// ask went back to the agent's terminal.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AskClosed {
    pub ask_id: String,
    pub decided: bool,
}

/// Params of `session.removed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRemoved {
    pub id: String,
}
