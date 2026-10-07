//! Session status state machine (spec section 8.4). Pure: the caller passes
//! the receive sequence of each hook event and the current time.
//!
//! Rules implemented:
//! - `Exited` is terminal. Every later input is ignored.
//! - A hook event whose `seq` is not newer than the last accepted one is
//!   dropped (async hooks may arrive out of order).
//! - Subagent events (`agent_id` set) change nothing, except a permission ask.
//! - `PreToolUse`/`PostToolUse` move to `Working` only when no `Stop` was seen
//!   since the last prompt. While blocked on a permission ask only
//!   `PostToolUse*`, a prompt, a stop or `PermissionClosed` unblock: an async
//!   `PreToolUse` can arrive after the ask and must not clear it.
//! - Turn-scoped events (Stop, StopFailure, tool events, PermissionRequest) of a
//!   known earlier turn are dropped. An unknown turn id (its prompt hook was
//!   lost) becomes the current turn.
//! - A permission ask is cleared only by a matching `PostToolUse*` (same tool,
//!   compatible input), `PermissionClosed { decided: true }`, a stop or a new
//!   prompt. A question block (`Input`) is cleared by any tool event.
//! - `SessionEnd` with reason `clear` or `resume` is not an exit.
//! - A `Notification` `permission_prompt` blocks only when no
//!   `PermissionRequest` hook was seen this turn (the hook is authoritative).
//! - Hook state wins over screen state: `Screen` input applies only while the
//!   source is `Heuristic`. `HookSilence` switches the source back.

use std::collections::VecDeque;

use serde_json::Value;

use crate::hooks::{HookEvent, HookPayload};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Starting,
    Working,
    Blocked,
    Idle,
    Exited,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusSource {
    Hook,
    Heuristic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockedKind {
    Permission,
    Input,
}

/// What a screen heuristic saw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detected {
    Working,
    Blocked(BlockedKind),
    Idle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitInfo {
    pub code: Option<i32>,
    pub signal: Option<i32>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    Hook {
        seq: u64,
        payload: Box<HookPayload>,
    },
    /// The PTY child exited. Authoritative.
    Exit(ExitInfo),
    /// The owner answered, the ask timed out, or the ask was cancelled.
    /// The ask is closed. `decided: true` means the owner answered or it timed
    /// out (Claude moves on); `false` means it was handed back to the terminal,
    /// so the dialog is still open.
    PermissionClosed {
        decided: bool,
    },
    /// A `SessionEnd` hook was seen and the PTY did not exit within the grace.
    EndingTimeout,
    /// No hook event for the silence window while the screen shows activity.
    HookSilence,
    Screen(Detected),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionStatus {
    pub status: Status,
    pub source: StatusSource,
    pub blocked_kind: Option<BlockedKind>,
    /// Time of the last change of `status` or `blocked_kind`.
    pub since: u64,
    pub agent_session_id: Option<String>,
    pub transcript_path: Option<String>,
    /// The last turn ended with an API error (`StopFailure`).
    pub error: bool,
    pub exit: Option<ExitInfo>,
    /// A `SessionEnd` hook arrived; waiting for the PTY exit.
    pub ending: bool,
    stop_seen: bool,
    permission_seen_this_turn: bool,
    last_seq: Option<u64>,
    current_turn: Option<String>,
    past_turns: VecDeque<String>,
    pending_ask: Option<(Option<String>, Option<Value>)>,
}

impl SessionStatus {
    pub fn new(now: u64) -> Self {
        SessionStatus {
            status: Status::Starting,
            source: StatusSource::Heuristic,
            blocked_kind: None,
            since: now,
            agent_session_id: None,
            transcript_path: None,
            error: false,
            exit: None,
            ending: false,
            stop_seen: false,
            permission_seen_this_turn: false,
            last_seq: None,
            current_turn: None,
            past_turns: VecDeque::new(),
            pending_ask: None,
        }
    }

    /// The process is up and has sent no hook yet (a resumed Codex): idle, so it can sleep
    /// again. Any other status stays.
    pub fn ready(&mut self, now: u64) {
        if self.status == Status::Starting {
            self.set(Status::Idle, None, now);
        }
    }

    fn set(&mut self, status: Status, kind: Option<BlockedKind>, now: u64) {
        if self.status != status || self.blocked_kind != kind {
            self.status = status;
            self.blocked_kind = kind;
            self.since = now;
        }
        if !(status == Status::Blocked && kind == Some(BlockedKind::Permission)) {
            self.pending_ask = None;
        }
    }

    fn work(&mut self, now: u64) {
        self.set(Status::Working, None, now);
    }

    /// Apply one input. Returns true when the visible state changed.
    pub fn apply(&mut self, input: Input, now: u64) -> bool {
        let before = (self.status, self.blocked_kind, self.source, self.error);
        let ids = (self.agent_session_id.clone(), self.transcript_path.clone());
        if self.status != Status::Exited {
            match input {
                Input::Exit(info) => {
                    self.exit = Some(info);
                    self.set(Status::Exited, None, now);
                }
                Input::EndingTimeout => {
                    if self.ending {
                        self.set(Status::Exited, None, now);
                    }
                }
                Input::PermissionClosed { decided } => {
                    if self.blocked_on_ask() {
                        if decided {
                            self.work(now);
                        } else {
                            // Handed back: the dialog is open, a later
                            // permission_prompt notification may confirm it.
                            self.permission_seen_this_turn = false;
                        }
                    }
                }
                Input::HookSilence => self.source = StatusSource::Heuristic,
                Input::Screen(d) => self.apply_screen(d, now),
                Input::Hook { seq, payload } => self.apply_hook(seq, *payload, now),
            }
        }
        before != (self.status, self.blocked_kind, self.source, self.error)
            || ids != (self.agent_session_id.clone(), self.transcript_path.clone())
    }

    fn apply_screen(&mut self, d: Detected, now: u64) {
        if self.source == StatusSource::Hook {
            return;
        }
        match d {
            Detected::Working => self.work(now),
            Detected::Blocked(k) => self.set(Status::Blocked, Some(k), now),
            Detected::Idle => self.set(Status::Idle, None, now),
        }
    }

    fn blocked_on_ask(&self) -> bool {
        self.status == Status::Blocked && self.blocked_kind == Some(BlockedKind::Permission)
    }

    /// Does this tool event belong to the pending ask?
    fn matches_ask(&self, tool: Option<&str>, input: Option<&Value>) -> bool {
        // No stored ask (hook failed, ask came from a notification or the
        // screen): any finished tool is taken as the answer.
        let Some((ask_tool, ask_input)) = &self.pending_ask else {
            return true;
        };
        if ask_tool.as_deref() != tool {
            return false;
        }
        match (ask_input, input) {
            (Some(a), Some(b)) => compatible(a, b),
            _ => true,
        }
    }

    /// Make `turn` current; remember the previous one (bounded).
    fn enter_turn(&mut self, turn: &str) {
        if let Some(old) = self.current_turn.replace(turn.to_owned()) {
            if old != turn {
                self.past_turns.push_back(old);
                if self.past_turns.len() > 16 {
                    self.past_turns.pop_front();
                }
            }
        }
    }

    fn apply_hook(&mut self, seq: u64, p: HookPayload, now: u64) {
        if self.last_seq.is_some_and(|last| seq <= last) {
            return;
        }
        self.last_seq = Some(seq);
        let in_subagent = p.common.agent_id.is_some();
        if matches!(p.event, HookEvent::Unknown(_)) {
            return;
        }
        let ask_pending = self.pending_ask.is_some();
        let passes = matches!(p.event, HookEvent::PermissionRequest(_))
            || (ask_pending
                && matches!(
                    p.event,
                    HookEvent::PostToolUse { .. } | HookEvent::PostToolUseFailure { .. }
                ));
        if in_subagent && !passes {
            return;
        }
        let turn_scoped = matches!(
            p.event,
            HookEvent::Stop
                | HookEvent::StopFailure { .. }
                | HookEvent::PreToolUse { .. }
                | HookEvent::PostToolUse { .. }
                | HookEvent::PostToolUseFailure { .. }
                | HookEvent::PermissionRequest(_)
        );
        if turn_scoped {
            if let Some(ev) = p.common.turn() {
                if self.current_turn.as_deref() != Some(ev) {
                    if self.past_turns.iter().any(|t| t == ev) {
                        return; // a late event of an earlier turn
                    }
                    // A new turn whose UserPromptSubmit was lost.
                    self.enter_turn(ev);
                    self.stop_seen = false;
                    self.permission_seen_this_turn = false;
                }
            }
        }
        let heuristic_dialog = self.source == StatusSource::Heuristic
            && self.status == Status::Blocked
            && self.blocked_kind == Some(BlockedKind::Input);
        self.source = StatusSource::Hook;
        self.ending = false;
        match p.event {
            HookEvent::SessionStart { .. } => {
                if p.common.session_id.is_some() {
                    self.agent_session_id = p.common.session_id;
                }
                if p.common.transcript_path.is_some() {
                    self.transcript_path = p.common.transcript_path;
                }
                // A dialog read from the screen before any hook (Codex asking to trust the
                // folder) is over once the session starts.
                if self.status == Status::Starting || heuristic_dialog {
                    self.set(Status::Idle, None, now);
                }
            }
            HookEvent::UserPromptSubmit => {
                self.stop_seen = false;
                self.permission_seen_this_turn = false;
                self.error = false;
                if let Some(ev) = p.common.turn() {
                    self.enter_turn(ev);
                }
                self.work(now);
            }
            HookEvent::PreToolUse { .. } | HookEvent::SubagentStart => self.tool_started(now),
            HookEvent::PostToolUse { tool_name } | HookEvent::PostToolUseFailure { tool_name } => {
                if self.blocked_on_ask() {
                    if self.matches_ask(tool_name.as_deref(), p.common.tool_input.as_ref()) {
                        self.work(now);
                    }
                } else {
                    self.tool_started(now);
                }
            }
            HookEvent::PermissionRequest(r) => {
                self.permission_seen_this_turn = true;
                self.set(Status::Blocked, Some(BlockedKind::Permission), now);
                let input = (!r.tool_input.is_null()).then_some(r.tool_input);
                self.pending_ask = Some((r.tool_name, input));
            }
            HookEvent::Notification {
                notification_type, ..
            } => self.notification(notification_type.as_deref(), now),
            HookEvent::Stop | HookEvent::Interrupt => self.finish_turn(false, now),
            HookEvent::StopFailure { .. } => self.finish_turn(true, now),
            HookEvent::SessionEnd { reason } => {
                // `/clear` and `/resume` end a session but not the process.
                if !matches!(reason.as_deref(), Some("clear" | "resume")) {
                    self.ending = true;
                }
            }
            HookEvent::SubagentStop | HookEvent::Unknown(_) => {}
        }
    }

    /// A tool started or finished outside a permission block.
    fn tool_started(&mut self, now: u64) {
        if self.blocked_on_ask() {
            return;
        }
        if self.is_blocked() || !self.stop_seen {
            self.work(now);
        }
    }

    fn is_blocked(&self) -> bool {
        self.status == Status::Blocked
    }

    fn finish_turn(&mut self, error: bool, now: u64) {
        self.stop_seen = true;
        self.error = error;
        self.set(Status::Idle, None, now);
    }

    fn notification(&mut self, kind: Option<&str>, now: u64) {
        match kind {
            Some("permission_prompt") => {
                if !self.permission_seen_this_turn && !self.is_blocked() {
                    self.set(Status::Blocked, Some(BlockedKind::Permission), now);
                }
            }
            Some("elicitation_dialog" | "elicitation_url_dialog" | "agent_needs_input") => {
                if !self.is_blocked() {
                    self.set(Status::Blocked, Some(BlockedKind::Input), now);
                }
            }
            Some("idle_prompt") => {
                if self.status == Status::Working {
                    self.set(Status::Idle, None, now);
                }
            }
            _ => {}
        }
    }
}

/// One tool input is the other with extra keys (the ask carries a
/// description, the later tool event may not).
fn compatible(a: &Value, b: &Value) -> bool {
    match (a.as_object(), b.as_object()) {
        (Some(x), Some(y)) => {
            let (small, big) = if x.len() <= y.len() { (x, y) } else { (y, x) };
            small.iter().all(|(k, v)| big.get(k) == Some(v))
        }
        _ => a == b,
    }
}
