//! Schedules: cron entries that start sessions through the same path as `session.create`.
//!
//! The pure decisions live in [`Engine`]: it takes `now` as an argument, owns the entries
//! and their file, and returns [`Step`]s for the driver to carry out. The driver
//! ([`Scheduler`]) adds a clock and a [`Host`] (the daemon), so a test runs the whole
//! loop with a fake clock and a fake host.
//!
//! Rules:
//! - An entry fires at its cron slot plus a fixed per-entry jitter. Slots that passed
//!   together collapse into one run.
//! - A slot that is more than `LATE_SECS` late is a catch-up: it runs once if it is inside
//!   the entry's catch-up window and is dropped (with an event) otherwise.
//! - A slot that finds the previous session of the entry still running is skipped.
//! - A session over `timeoutSec` (not counting time spent waiting on an owner ask) is closed.
//! - A run recorded before a daemon start is ended with the reason `daemon restarted`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sushiai_core::cron::{self, Cron};
use sushiai_protocol::{
    ScheduleEvent, ScheduleEventKind, ScheduleFired, ScheduleInfo, ScheduleRun, ScheduleSet,
    SessionCreate,
};

/// A slot later than this (seconds) counts as missed, not as due now.
const LATE_SECS: u64 = 60;
/// How old a missed slot may be to run when the entry sets no window.
const DEFAULT_CATCHUP_SECS: u64 = 7 * 24 * 3600;
/// The largest jitter an entry may ask for.
const MAX_JITTER_SECS: u64 = 3600;
/// Slots older than this are never searched for.
const LOOKBACK_SECS: u64 = 366 * 24 * 3600;
/// A guard against a loop over a huge number of passed slots.
const MAX_SLOTS_PER_TICK: usize = 100_000;
const FILE_VERSION: u32 = 1;
/// Set in the environment of every session an entry starts: seconds until its next slot.
pub const NEXT_SLOT_ENV: &str = "SUSHIAI_SCHEDULE_NEXT_IN_SEC";

/// Unix seconds.
pub trait Clock: Send + Sync {
    fn now(&self) -> u64;
}

pub struct SystemClock;

/// Seconds added to the wall clock, read once from `SUSHIAI_SCHEDULE_CLOCK_OFFSET_SECS`
/// (tests only): lets an end-to-end test put a real daemon just before a cron slot.
fn clock_offset() -> i64 {
    static OFFSET: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *OFFSET.get_or_init(|| {
        std::env::var("SUSHIAI_SCHEDULE_CLOCK_OFFSET_SECS")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0)
    })
}

impl Clock for SystemClock {
    fn now(&self) -> u64 {
        let real = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        real.saturating_add_signed(clock_offset())
    }
}

/// What the host knows about a session the engine started.
#[derive(Debug, Clone, Copy, Default)]
pub struct Probe {
    /// The session exists and has not exited.
    pub alive: bool,
    /// Open asks (permission or owner): time spent there does not count to the timeout.
    pub asks: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Running {
    session: String,
    started_at: u64,
    scheduled_for: u64,
    #[serde(default)]
    paused_secs: u64,
    #[serde(default)]
    last_seen: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Item {
    #[serde(flatten)]
    set: ScheduleSet,
    /// The latest slot that was dealt with (run, skipped or dropped). New slots come after.
    anchor: u64,
    #[serde(default)]
    last_fired_at: Option<u64>,
    #[serde(default)]
    running: Option<Running>,
    /// A launch was decided and its `launched` has not come back yet.
    #[serde(skip)]
    pending: bool,
}

#[derive(Serialize, Deserialize)]
struct FileModel {
    version: u32,
    schedules: Vec<Item>,
}

/// A start the driver must carry out and report back with [`Engine::launched`].
#[derive(Debug, Clone)]
pub struct Launch {
    pub id: String,
    pub create: SessionCreate,
    pub slot: u64,
    pub kind: ScheduleEventKind,
}

#[derive(Debug, Clone)]
pub enum Step {
    Launch(Box<Launch>),
    /// Close this session.
    Close(String),
    Event(ScheduleEvent),
}

fn ms(secs: u64) -> u64 {
    secs.saturating_mul(1000)
}

fn event(id: &str, kind: ScheduleEventKind, scheduled_for: u64, started_at: u64) -> ScheduleEvent {
    ScheduleEvent {
        id: id.to_string(),
        kind,
        reason: None,
        session: None,
        scheduled_for: ms(scheduled_for),
        started_at: ms(started_at),
        ended_at: None,
    }
}

impl Item {
    fn parsed(&self) -> Option<(Cron, cron::Zone)> {
        let zone = cron::parse_zone(&self.set.tz).ok()?;
        Some((Cron::parse(&self.set.cron).ok()?, zone))
    }

    fn jitter(&self) -> u64 {
        cron::jitter_secs(&self.set.id, self.set.jitter_sec.unwrap_or(0))
    }

    /// The session parameters of one start: the entry's own, plus `NEXT_SLOT_ENV`, the seconds
    /// until the entry's next slot would start. An owner ask in the session can expire then
    /// instead of waiting for a run that has been replaced.
    fn create_for(&self, now: u64) -> SessionCreate {
        let mut create = self.set.create.clone();
        if let Some((cron, zone)) = self.parsed() {
            if let Some(next) = cron.next_after(zone, now as i64) {
                let wait = (next as u64 + self.jitter()).saturating_sub(now).max(1);
                create
                    .env
                    .insert(NEXT_SLOT_ENV.to_string(), wait.to_string());
            }
        }
        create
    }

    /// Follows the entry's current run: ended, timed out or still going. True when the
    /// stored state changed.
    fn track(
        &mut self,
        now: u64,
        first: bool,
        probe: &dyn Fn(&str) -> Probe,
        steps: &mut Vec<Step>,
    ) -> bool {
        let Some(run) = self.running.as_mut() else {
            return false;
        };
        let seen = probe(&run.session);
        let ended = |run: &Running, reason: Option<&str>, kind: ScheduleEventKind| {
            let mut e = event(&self.set.id, kind, run.scheduled_for, run.started_at);
            e.session = Some(run.session.clone());
            e.reason = reason.map(str::to_string);
            e.ended_at = Some(ms(now));
            Step::Event(e)
        };
        if first {
            // The daemon is new: whatever it ran before is over, and a survivor is closed so
            // nothing runs without an entry that tracks it.
            if seen.alive {
                steps.push(Step::Close(run.session.clone()));
            }
            steps.push(ended(
                run,
                Some("daemon restarted"),
                ScheduleEventKind::Ended,
            ));
            self.running = None;
            return true;
        }
        if !seen.alive {
            steps.push(ended(run, None, ScheduleEventKind::Ended));
            self.running = None;
            return true;
        }
        let waited = now.saturating_sub(run.last_seen.max(run.started_at));
        run.last_seen = now;
        if seen.asks > 0 {
            run.paused_secs += waited;
        }
        if let Some(limit) = self.set.timeout_sec {
            let active = now
                .saturating_sub(run.started_at)
                .saturating_sub(run.paused_secs);
            if active >= limit {
                steps.push(Step::Close(run.session.clone()));
                let reason = format!("ran longer than {limit}s");
                steps.push(ended(run, Some(&reason), ScheduleEventKind::Timeout));
                self.running = None;
                return true;
            }
        }
        false
    }

    /// Looks for a slot that is due. True when the stored state changed.
    fn due(&mut self, now: u64, steps: &mut Vec<Step>) -> bool {
        if !self.set.enabled {
            return false;
        }
        let Some((cron, zone)) = self.parsed() else {
            return false;
        };
        let jitter = self.jitter();
        let mut from = self.anchor.max(now.saturating_sub(LOOKBACK_SECS));
        let mut latest = None;
        for _ in 0..MAX_SLOTS_PER_TICK {
            let Some(slot) = cron.next_after(zone, from as i64) else {
                break;
            };
            let slot = slot as u64;
            if slot + jitter > now {
                break;
            }
            latest = Some(slot);
            from = slot;
        }
        let Some(slot) = latest else {
            return false;
        };
        self.anchor = slot;
        let id = self.set.id.clone();
        let late = now - (slot + jitter) > LATE_SECS;
        let window = self.set.catchup_window_sec.unwrap_or(DEFAULT_CATCHUP_SECS);
        let skip = |reason: &str| {
            let mut e = event(&id, ScheduleEventKind::Skipped, slot, now);
            e.reason = Some(reason.to_string());
            Step::Event(e)
        };
        if late && now - slot > window {
            steps.push(skip(
                "missed while the daemon was not running; older than the catch-up window",
            ));
            return true;
        }
        if self.running.is_some() || self.pending {
            steps.push(skip("the previous run is still running"));
            return true;
        }
        self.pending = true;
        steps.push(Step::Launch(Box::new(Launch {
            id,
            create: self.create_for(now),
            slot,
            kind: if late {
                ScheduleEventKind::CaughtUp
            } else {
                ScheduleEventKind::Fired
            },
        })));
        true
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum SetError {
    Invalid(String),
    Storage(String),
}

/// The entries and their file. No clock, no tasks.
pub struct Engine {
    path: PathBuf,
    items: BTreeMap<String, Item>,
    /// False until the first `tick`: that one treats recorded runs as interrupted.
    booted: bool,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

impl Engine {
    /// Reads the file; a missing one is an empty engine and an unreadable one is moved
    /// aside (`.bad`) so the next save does not destroy it.
    pub fn load(path: PathBuf) -> Engine {
        let mut items = BTreeMap::new();
        match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<FileModel>(&bytes) {
                Ok(file) if file.version <= FILE_VERSION => {
                    for item in file.schedules {
                        items.insert(item.set.id.clone(), item);
                    }
                }
                _ => {
                    tracing::warn!("{} is not readable; moved aside", path.display());
                    let _ = std::fs::rename(&path, path.with_extension("json.bad"));
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!("cannot read {}: {e}", path.display()),
        }
        Engine {
            path,
            items,
            booted: false,
        }
    }

    fn save(&self) -> std::io::Result<()> {
        let file = FileModel {
            version: FILE_VERSION,
            schedules: self.items.values().cloned().collect(),
        };
        let bytes = serde_json::to_vec_pretty(&file).map_err(std::io::Error::other)?;
        crate::registry::write_atomic(&self.path, &bytes, "json.tmp")
    }

    fn save_logged(&self) {
        if let Err(e) = self.save() {
            tracing::warn!("cannot save {}: {e}", self.path.display());
        }
    }

    /// Creates or replaces an entry. The same cron and zone keep the last handled slot, so
    /// a module that sets its entries on every start does not reset catch-up detection.
    pub fn set(&mut self, set: ScheduleSet, now: u64) -> Result<(), SetError> {
        let invalid = |m: String| SetError::Invalid(m);
        if !valid_id(&set.id) {
            return Err(invalid(
                "id must be 1 to 128 characters of a-z, A-Z, 0-9, '.', '_' or '-'".into(),
            ));
        }
        cron::validate(&set.cron, &set.tz).map_err(|e| invalid(e.to_string()))?;
        if set.jitter_sec.unwrap_or(0) > MAX_JITTER_SECS {
            return Err(invalid(format!(
                "jitterSec must be at most {MAX_JITTER_SECS}"
            )));
        }
        if set.timeout_sec == Some(0) || set.catchup_window_sec == Some(0) {
            return Err(invalid(
                "timeoutSec and catchupWindowSec must be above 0".into(),
            ));
        }
        crate::server::check_size(set.create.cols, set.create.rows).map_err(|(_, m)| invalid(m))?;
        if set.create.cwd.is_empty() {
            return Err(invalid("create.cwd is required".into()));
        }
        let previous = self.items.get(&set.id).cloned();
        let same_slots = previous
            .as_ref()
            .is_some_and(|p| p.set.cron == set.cron && p.set.tz == set.tz);
        let item = Item {
            anchor: match &previous {
                Some(p) if same_slots => p.anchor,
                _ => now,
            },
            last_fired_at: previous.as_ref().and_then(|p| p.last_fired_at),
            running: previous.as_ref().and_then(|p| p.running.clone()),
            pending: previous.as_ref().is_some_and(|p| p.pending),
            set,
        };
        let id = item.set.id.clone();
        self.items.insert(id.clone(), item);
        if let Err(e) = self.save() {
            match previous {
                Some(p) => self.items.insert(id, p),
                None => self.items.remove(&id),
            };
            return Err(SetError::Storage(format!("cannot save the schedules: {e}")));
        }
        Ok(())
    }

    /// Deletes an entry. A session it started keeps running.
    pub fn remove(&mut self, id: &str) -> bool {
        let found = self.items.remove(id).is_some();
        if found {
            self.save_logged();
        }
        found
    }

    pub fn list(&self, now: u64) -> Vec<ScheduleInfo> {
        self.items
            .values()
            .map(|item| {
                let mut set = item.set.clone();
                // Secrets stay in the file; clients get the shape.
                set.create.env.clear();
                set.create.claude_settings = None;
                let next_at = item
                    .set
                    .enabled
                    .then(|| item.parsed())
                    .flatten()
                    .and_then(|(c, z)| c.next_after(z, now as i64))
                    .map(|t| ms(t as u64));
                ScheduleInfo {
                    set,
                    last_fired_at: item.last_fired_at.map(ms),
                    next_at,
                    running: item.running.as_ref().map(|r| ScheduleRun {
                        session: r.session.clone(),
                        started_at: ms(r.started_at),
                        scheduled_for: ms(r.scheduled_for),
                    }),
                }
            })
            .collect()
    }

    /// One look at every entry. The first call after `load` also settles runs that were
    /// recorded before this daemon started and fires one catch-up for a missed slot.
    pub fn tick(&mut self, now: u64, probe: &dyn Fn(&str) -> Probe) -> Vec<Step> {
        let first = !std::mem::replace(&mut self.booted, true);
        let mut steps = Vec::new();
        let mut dirty = false;
        for item in self.items.values_mut() {
            dirty |= item.track(now, first, probe, &mut steps);
            dirty |= item.due(now, &mut steps);
        }
        if dirty {
            self.save_logged();
        }
        steps
    }

    /// Runs an entry now, outside its cron. `None` when there is no such entry.
    pub fn fire_now(
        &mut self,
        id: &str,
        now: u64,
        probe: &dyn Fn(&str) -> Probe,
    ) -> Option<Vec<Step>> {
        let item = self.items.get_mut(id)?;
        let mut steps = Vec::new();
        item.track(now, false, probe, &mut steps);
        if item.running.is_some() || item.pending {
            let mut e = event(id, ScheduleEventKind::Skipped, now, now);
            e.reason = Some("the previous run is still running".into());
            steps.push(Step::Event(e));
        } else {
            item.pending = true;
            steps.push(Step::Launch(Box::new(Launch {
                id: id.to_string(),
                create: item.create_for(now),
                slot: now,
                kind: ScheduleEventKind::Fired,
            })));
        }
        self.save_logged();
        Some(steps)
    }

    /// The driver's report on a [`Launch`].
    pub fn launched(
        &mut self,
        launch: &Launch,
        result: Result<String, String>,
        now: u64,
    ) -> Vec<Step> {
        let mut steps = Vec::new();
        let item = self.items.get_mut(&launch.id);
        match (item, result) {
            (Some(item), Ok(session)) => {
                item.pending = false;
                item.last_fired_at = Some(now);
                item.running = Some(Running {
                    session: session.clone(),
                    started_at: now,
                    scheduled_for: launch.slot,
                    paused_secs: 0,
                    last_seen: now,
                });
                let mut e = event(&launch.id, launch.kind, launch.slot, now);
                e.session = Some(session);
                steps.push(Step::Event(e));
            }
            (Some(item), Err(message)) => {
                item.pending = false;
                let mut e = event(&launch.id, ScheduleEventKind::Skipped, launch.slot, now);
                e.reason = Some(format!("could not start: {message}"));
                steps.push(Step::Event(e));
            }
            // Removed while it started: nothing tracks the session, so it ends.
            (None, Ok(session)) => steps.push(Step::Close(session)),
            (None, Err(_)) => {}
        }
        self.save_logged();
        steps
    }
}

/// What the scheduler needs from the daemon.
pub trait Host: Send + Sync {
    fn probe(&self, session: &str) -> Probe;
    fn create(
        &self,
        params: SessionCreate,
    ) -> impl std::future::Future<Output = Result<String, String>> + Send;
    fn close(&self, session: &str) -> impl std::future::Future<Output = ()> + Send;
    fn emit(&self, event: ScheduleEvent);
}

pub struct Scheduler {
    engine: Mutex<Engine>,
    clock: Arc<dyn Clock>,
}

impl Scheduler {
    pub fn new(path: PathBuf, clock: Arc<dyn Clock>) -> Self {
        Scheduler {
            engine: Mutex::new(Engine::load(path)),
            clock,
        }
    }

    fn engine(&self) -> MutexGuard<'_, Engine> {
        self.engine.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn set(&self, set: ScheduleSet) -> Result<(), SetError> {
        self.engine().set(set, self.clock.now())
    }

    pub fn remove(&self, id: &str) -> bool {
        self.engine().remove(id)
    }

    pub fn list(&self) -> Vec<ScheduleInfo> {
        self.engine().list(self.clock.now())
    }

    /// Carries out the steps, and the steps their launches lead to.
    async fn run<H: Host>(&self, host: &H, steps: Vec<Step>) -> Vec<ScheduleEvent> {
        let mut queue = std::collections::VecDeque::from(steps);
        let mut emitted = Vec::new();
        while let Some(step) = queue.pop_front() {
            match step {
                Step::Event(e) => {
                    emitted.push(e.clone());
                    host.emit(e);
                }
                Step::Close(session) => host.close(&session).await,
                Step::Launch(launch) => {
                    let mut create = launch.create.clone();
                    create.idempotency_key =
                        Some(format!("schedule:{}:{}", launch.id, launch.slot));
                    let result = host.create(create).await;
                    let more = self.engine().launched(&launch, result, self.clock.now());
                    queue.extend(more);
                }
            }
        }
        emitted
    }

    /// One pass of the loop; the daemon calls it every second.
    pub async fn tick<H: Host>(&self, host: &H) {
        let steps = {
            let probe = |s: &str| host.probe(s);
            self.engine().tick(self.clock.now(), &probe)
        };
        let _ = self.run(host, steps).await;
    }

    /// `schedule.fire`: starts the entry now, or says why it did not. `None`: no such entry.
    pub async fn fire<H: Host>(&self, host: &H, id: &str) -> Option<ScheduleFired> {
        let steps = {
            let probe = |s: &str| host.probe(s);
            self.engine().fire_now(id, self.clock.now(), &probe)?
        };
        let emitted = self.run(host, steps).await;
        let mut fired = ScheduleFired {
            session: None,
            skipped: None,
        };
        for e in emitted {
            match e.kind {
                ScheduleEventKind::Fired => fired.session = e.session,
                ScheduleEventKind::Skipped => fired.skipped = e.reason,
                _ => {}
            }
        }
        Some(fired)
    }
}

/// The daemon as the scheduler's host.
pub struct RegistryHost(pub Arc<crate::registry::Registry>);

/// How long a close waits for the actor of a session that is still being restored.
const CLOSE_WAIT: std::time::Duration = std::time::Duration::from_secs(12);

impl Host for RegistryHost {
    fn probe(&self, session: &str) -> Probe {
        match self.0.info(session) {
            Some(info) => Probe {
                alive: info.status != sushiai_protocol::SessionStatus::Exited,
                asks: info.agent.asks.len(),
            },
            None => Probe::default(),
        }
    }

    async fn create(&self, params: SessionCreate) -> Result<String, String> {
        match crate::server::create(&self.0, params).await {
            Ok(value) => value
                .get("id")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .ok_or_else(|| "the daemon returned no session id".to_string()),
            Err((_, message)) => Err(message),
        }
    }

    async fn close(&self, session: &str) {
        let deadline = tokio::time::Instant::now() + CLOSE_WAIT;
        loop {
            if let Some(handle) = self.0.handle(session) {
                let _ = handle.close(true).await;
                return;
            }
            if !self.0.awaiting_actor(session) || tokio::time::Instant::now() >= deadline {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    }

    fn emit(&self, event: ScheduleEvent) {
        let _ = self.0.events.send(sushiai_protocol::Notification::new(
            sushiai_protocol::method::SCHEDULE_EVENT,
            event,
        ));
    }
}

/// The scheduler loop: one pass a second until the daemon stops.
pub async fn run(registry: Arc<crate::registry::Registry>) {
    let host = RegistryHost(registry.clone());
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        if !registry.stopping() {
            registry.scheduler.tick(&host).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct FakeClock(AtomicU64);

    impl FakeClock {
        fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> Arc<FakeClock> {
            let t = chrono::Utc
                .with_ymd_and_hms(y, mo, d, h, mi, 0)
                .unwrap()
                .timestamp() as u64;
            Arc::new(FakeClock(AtomicU64::new(t)))
        }
        fn advance(&self, secs: u64) {
            self.0.fetch_add(secs, Ordering::SeqCst);
        }
    }

    impl Clock for FakeClock {
        fn now(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    #[derive(Default)]
    struct FakeHost {
        /// session id -> (alive, asks)
        sessions: Mutex<BTreeMap<String, Probe>>,
        events: Mutex<Vec<ScheduleEvent>>,
        created: Mutex<Vec<SessionCreate>>,
        closed: Mutex<Vec<String>>,
        fail_create: Mutex<Option<String>>,
    }

    impl FakeHost {
        fn kinds(&self) -> Vec<ScheduleEventKind> {
            self.events.lock().unwrap().iter().map(|e| e.kind).collect()
        }
        fn take_events(&self) -> Vec<ScheduleEvent> {
            std::mem::take(&mut *self.events.lock().unwrap())
        }
        fn end(&self, session: &str) {
            self.sessions
                .lock()
                .unwrap()
                .insert(session.into(), Probe::default());
        }
        fn asks(&self, session: &str, n: usize) {
            self.sessions.lock().unwrap().insert(
                session.into(),
                Probe {
                    alive: true,
                    asks: n,
                },
            );
        }
    }

    impl Host for FakeHost {
        fn probe(&self, session: &str) -> Probe {
            self.sessions
                .lock()
                .unwrap()
                .get(session)
                .copied()
                .unwrap_or_default()
        }
        async fn create(&self, params: SessionCreate) -> Result<String, String> {
            if let Some(message) = self.fail_create.lock().unwrap().clone() {
                return Err(message);
            }
            let mut created = self.created.lock().unwrap();
            created.push(params);
            let id = format!("s{}", created.len());
            self.sessions.lock().unwrap().insert(
                id.clone(),
                Probe {
                    alive: true,
                    asks: 0,
                },
            );
            Ok(id)
        }
        async fn close(&self, session: &str) {
            self.closed.lock().unwrap().push(session.into());
            self.end(session);
        }
        fn emit(&self, event: ScheduleEvent) {
            self.events.lock().unwrap().push(event);
        }
    }

    fn entry(id: &str, cron: &str, tz: &str) -> ScheduleSet {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "cron": cron,
            "tz": tz,
            "create": {
                "cwd": "/work/my-app", "cols": 100, "rows": 30,
                "agent": "claude", "prompt": "summarise", "background": true,
                "env": { "TOKEN": "secret" }
            },
        }))
        .unwrap()
    }

    fn scheduler(dir: &std::path::Path, clock: &Arc<FakeClock>) -> Scheduler {
        Scheduler::new(dir.join("schedules.json"), clock.clone())
    }

    #[tokio::test]
    async fn fires_on_its_cron_and_not_before() {
        let dir = tempfile::tempdir().unwrap();
        let clock = FakeClock::at(2026, 5, 4, 8, 58);
        let (s, host) = (scheduler(dir.path(), &clock), FakeHost::default());
        s.set(entry("daily", "0 9 * * *", "UTC")).unwrap();
        s.tick(&host).await; // boot tick
        clock.advance(60);
        s.tick(&host).await;
        assert!(host.kinds().is_empty());
        clock.advance(60);
        s.tick(&host).await;
        assert_eq!(host.kinds(), vec![ScheduleEventKind::Fired]);
        {
            let created = host.created.lock().unwrap();
            assert_eq!(created.len(), 1);
            assert!(created[0].background);
            assert!(created[0]
                .idempotency_key
                .as_deref()
                .is_some_and(|k| k.starts_with("schedule:daily:")));
        }
        let event = host.events.lock().unwrap()[0].clone();
        assert_eq!(event.session.as_deref(), Some("s1"));
        assert_eq!(event.scheduled_for, event.started_at);
        // The same slot does not fire again.
        s.tick(&host).await;
        assert_eq!(host.kinds().len(), 1);
    }

    #[tokio::test]
    async fn an_overlapping_slot_is_skipped_and_logged() {
        let dir = tempfile::tempdir().unwrap();
        let clock = FakeClock::at(2026, 5, 4, 9, 0);
        let (s, host) = (scheduler(dir.path(), &clock), FakeHost::default());
        s.set(entry("job", "*/10 * * * *", "UTC")).unwrap();
        s.tick(&host).await;
        clock.advance(600);
        s.tick(&host).await;
        assert_eq!(host.kinds(), vec![ScheduleEventKind::Fired]);
        // The session is still running at the next slot.
        clock.advance(600);
        s.tick(&host).await;
        let events = host.take_events();
        assert_eq!(events[1].kind, ScheduleEventKind::Skipped);
        assert!(events[1]
            .reason
            .as_deref()
            .unwrap()
            .contains("still running"));
        assert_eq!(host.created.lock().unwrap().len(), 1);
        // After it ended, the next slot runs.
        host.end("s1");
        clock.advance(600);
        s.tick(&host).await;
        let kinds = host.kinds();
        assert_eq!(
            kinds,
            vec![ScheduleEventKind::Ended, ScheduleEventKind::Fired]
        );
        assert_eq!(host.created.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn one_catch_up_after_downtime_not_one_per_slot() {
        let dir = tempfile::tempdir().unwrap();
        let clock = FakeClock::at(2026, 5, 4, 9, 0);
        let host = FakeHost::default();
        {
            let s = scheduler(dir.path(), &clock);
            s.set(entry("job", "0 * * * *", "UTC")).unwrap();
            s.tick(&host).await;
        }
        // Down for five hours: five slots passed.
        clock.advance(5 * 3600 + 120);
        let s = scheduler(dir.path(), &clock);
        s.tick(&host).await;
        assert_eq!(host.kinds(), vec![ScheduleEventKind::CaughtUp]);
        assert_eq!(host.created.lock().unwrap().len(), 1);
        let event = host.events.lock().unwrap()[0].clone();
        // The slot is the latest missed one (14:00), not the first.
        assert_eq!(event.scheduled_for % (3600 * 1000), 0);
        assert!(event.started_at - event.scheduled_for >= 120_000);
        host.end("s1");
        s.tick(&host).await;
        assert_eq!(host.created.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_slot_outside_the_catch_up_window_is_dropped_with_an_event() {
        let dir = tempfile::tempdir().unwrap();
        let clock = FakeClock::at(2026, 5, 4, 9, 0);
        let host = FakeHost::default();
        {
            let s = scheduler(dir.path(), &clock);
            let mut e = entry("job", "0 9 * * *", "UTC");
            e.catchup_window_sec = Some(3600);
            s.set(e).unwrap();
        }
        clock.advance(3 * 24 * 3600 + 7200);
        let s = scheduler(dir.path(), &clock);
        s.tick(&host).await;
        assert_eq!(host.kinds(), vec![ScheduleEventKind::Skipped]);
        assert!(host.created.lock().unwrap().is_empty());
        // Dropped slots are not retried at the next tick.
        s.tick(&host).await;
        assert_eq!(host.kinds().len(), 1);
    }

    #[tokio::test]
    async fn a_run_over_its_timeout_is_closed() {
        let dir = tempfile::tempdir().unwrap();
        let clock = FakeClock::at(2026, 5, 4, 9, 0);
        let (s, host) = (scheduler(dir.path(), &clock), FakeHost::default());
        let mut e = entry("job", "0 * * * *", "UTC");
        e.timeout_sec = Some(600);
        s.set(e).unwrap();
        s.tick(&host).await;
        clock.advance(3600);
        s.tick(&host).await;
        assert_eq!(host.kinds(), vec![ScheduleEventKind::Fired]);
        clock.advance(500);
        s.tick(&host).await;
        assert_eq!(host.kinds().len(), 1);
        clock.advance(100);
        s.tick(&host).await;
        assert_eq!(
            host.kinds(),
            vec![ScheduleEventKind::Fired, ScheduleEventKind::Timeout]
        );
        assert_eq!(*host.closed.lock().unwrap(), vec!["s1".to_string()]);
        let timeout = host.events.lock().unwrap()[1].clone();
        assert_eq!(timeout.session.as_deref(), Some("s1"));
        assert!(timeout.ended_at.is_some());
    }

    #[tokio::test]
    async fn waiting_on_an_ask_does_not_count_to_the_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let clock = FakeClock::at(2026, 5, 4, 9, 0);
        let (s, host) = (scheduler(dir.path(), &clock), FakeHost::default());
        let mut e = entry("job", "0 * * * *", "UTC");
        e.timeout_sec = Some(600);
        s.set(e).unwrap();
        s.tick(&host).await;
        clock.advance(3600);
        s.tick(&host).await;
        host.asks("s1", 1);
        for _ in 0..10 {
            clock.advance(300);
            s.tick(&host).await;
        }
        assert_eq!(host.kinds(), vec![ScheduleEventKind::Fired]);
        // The ask is answered; 600 active seconds are still needed.
        host.asks("s1", 0);
        clock.advance(300);
        s.tick(&host).await;
        assert_eq!(host.kinds().len(), 1);
        clock.advance(300);
        s.tick(&host).await;
        assert_eq!(host.kinds()[1], ScheduleEventKind::Timeout);
    }

    #[tokio::test]
    async fn entries_survive_a_reload_and_a_recorded_run_ends_as_restarted() {
        let dir = tempfile::tempdir().unwrap();
        let clock = FakeClock::at(2026, 5, 4, 9, 0);
        let host = FakeHost::default();
        {
            let s = scheduler(dir.path(), &clock);
            s.set(entry("job", "0 * * * *", "Europe/Berlin")).unwrap();
            s.tick(&host).await;
            clock.advance(3600);
            s.tick(&host).await;
            assert_eq!(host.kinds(), vec![ScheduleEventKind::Fired]);
        }
        let again = scheduler(dir.path(), &clock);
        let listed = again.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].set.tz, "Europe/Berlin");
        assert!(listed[0].set.create.env.is_empty(), "env is not listed");
        assert_eq!(listed[0].running.as_ref().unwrap().session, "s1");
        assert!(listed[0].next_at.is_some());
        let file = std::fs::read_to_string(dir.path().join("schedules.json")).unwrap();
        assert!(file.contains("secret"), "the file keeps what a run needs");
        again.tick(&host).await;
        let events = host.take_events();
        assert_eq!(events[1].kind, ScheduleEventKind::Ended);
        assert_eq!(events[1].reason.as_deref(), Some("daemon restarted"));
        assert_eq!(*host.closed.lock().unwrap(), vec!["s1".to_string()]);
        assert!(again.list()[0].running.is_none());
    }

    #[tokio::test]
    async fn setting_the_same_entry_again_keeps_catch_up_detection() {
        let dir = tempfile::tempdir().unwrap();
        let clock = FakeClock::at(2026, 5, 4, 9, 0);
        let host = FakeHost::default();
        scheduler(dir.path(), &clock)
            .set(entry("job", "0 * * * *", "UTC"))
            .unwrap();
        clock.advance(2 * 3600 + 300);
        let s = scheduler(dir.path(), &clock);
        // A module re-sends its entries at start, before the first tick.
        s.set(entry("job", "0 * * * *", "UTC")).unwrap();
        s.tick(&host).await;
        assert_eq!(host.kinds(), vec![ScheduleEventKind::CaughtUp]);
        // A changed cron starts from now: nothing is owed.
        host.take_events();
        s.set(entry("job", "30 * * * *", "UTC")).unwrap();
        s.tick(&host).await;
        assert!(host.kinds().is_empty());
    }

    #[tokio::test]
    async fn the_cron_is_read_in_its_zone_across_a_dst_change() {
        let dir = tempfile::tempdir().unwrap();
        // 08:00 Berlin on 2026-03-28 (CET) is 07:00Z; on the 29th (CEST) it is 06:00Z.
        let clock = FakeClock::at(2026, 3, 28, 6, 0);
        let (s, host) = (scheduler(dir.path(), &clock), FakeHost::default());
        s.set(entry("morning", "0 8 * * *", "Europe/Berlin"))
            .unwrap();
        s.tick(&host).await;
        clock.advance(3600);
        s.tick(&host).await;
        assert_eq!(host.kinds().len(), 1);
        host.end("s1");
        // 23 hours later it is 06:00Z on the 29th: the local 08:00 has come.
        clock.advance(23 * 3600);
        s.tick(&host).await;
        assert_eq!(
            host.kinds(),
            vec![
                ScheduleEventKind::Fired,
                ScheduleEventKind::Ended,
                ScheduleEventKind::Fired
            ]
        );
    }

    #[test]
    fn set_rejects_a_cron_below_the_minimum_interval_and_bad_input() {
        let dir = tempfile::tempdir().unwrap();
        let mut engine = Engine::load(dir.path().join("schedules.json"));
        let err = engine
            .set(entry("fast", "*/2 * * * *", "UTC"), 0)
            .unwrap_err();
        assert!(matches!(err, SetError::Invalid(m) if m.contains("minimum")));
        assert!(engine
            .set(entry("tz", "0 9 * * *", "Nowhere/Land"), 0)
            .is_err());
        assert!(engine.set(entry("bad id", "0 9 * * *", "UTC"), 0).is_err());
        let mut big = entry("j", "0 9 * * *", "UTC");
        big.jitter_sec = Some(99_999);
        assert!(engine.set(big, 0).is_err());
        assert!(engine.list(0).is_empty());
        assert!(engine.set(entry("ok", "*/5 * * * *", "UTC"), 0).is_ok());
    }

    #[tokio::test]
    async fn jitter_delays_the_fire_by_a_fixed_amount_below_the_cap() {
        let dir = tempfile::tempdir().unwrap();
        let clock = FakeClock::at(2026, 5, 4, 8, 59);
        let (s, host) = (scheduler(dir.path(), &clock), FakeHost::default());
        let mut e = entry("jittery", "0 9 * * *", "UTC");
        e.jitter_sec = Some(600);
        let jitter = cron::jitter_secs("jittery", 600);
        s.set(e).unwrap();
        s.tick(&host).await;
        clock.advance(60 + jitter.saturating_sub(1));
        s.tick(&host).await;
        let early = host.kinds().len();
        clock.advance(1);
        s.tick(&host).await;
        assert_eq!(early, if jitter == 0 { 1 } else { 0 });
        assert_eq!(host.kinds(), vec![ScheduleEventKind::Fired]);
    }

    #[tokio::test]
    async fn fire_now_runs_once_and_reports_overlap() {
        let dir = tempfile::tempdir().unwrap();
        let clock = FakeClock::at(2026, 5, 4, 9, 0);
        let (s, host) = (scheduler(dir.path(), &clock), FakeHost::default());
        s.set(entry("job", "0 3 * * *", "UTC")).unwrap();
        assert!(s.fire(&host, "nope").await.is_none());
        let first = s.fire(&host, "job").await.unwrap();
        assert_eq!(first.session.as_deref(), Some("s1"));
        let second = s.fire(&host, "job").await.unwrap();
        assert!(second.session.is_none());
        assert!(second.skipped.unwrap().contains("still running"));
        assert_eq!(
            host.kinds(),
            vec![ScheduleEventKind::Fired, ScheduleEventKind::Skipped]
        );
    }

    #[tokio::test]
    async fn a_started_session_learns_the_seconds_until_the_next_slot() {
        let dir = tempfile::tempdir().unwrap();
        // 08:59:00Z; the entry runs at 09:00 and 15:00 UTC.
        let clock = FakeClock::at(2026, 5, 4, 8, 59);
        let (s, host) = (scheduler(dir.path(), &clock), FakeHost::default());
        s.set(entry("twice", "0 9,15 * * *", "UTC")).unwrap();
        s.tick(&host).await;
        clock.advance(60);
        s.tick(&host).await;
        // Started at 09:00:00; the next slot is 15:00: 6 hours.
        let vars = host.created.lock().unwrap()[0].env.clone();
        assert_eq!(vars[NEXT_SLOT_ENV], (6 * 3600).to_string());
        // The stored entry never carries it.
        assert!(!s.engine().items["twice"]
            .set
            .create
            .env
            .contains_key(NEXT_SLOT_ENV));
        // A manual start asks for the time to the next slot from now.
        host.end("s1");
        clock.advance(3600);
        s.tick(&host).await;
        s.fire(&host, "twice").await.unwrap();
        let vars = host.created.lock().unwrap()[1].env.clone();
        assert_eq!(vars[NEXT_SLOT_ENV], (5 * 3600).to_string());
        // Other variables of the entry stay.
        assert_eq!(vars["TOKEN"], "secret");
    }

    #[tokio::test]
    async fn a_failed_start_is_a_skipped_event_and_does_not_block_the_next_slot() {
        let dir = tempfile::tempdir().unwrap();
        let clock = FakeClock::at(2026, 5, 4, 9, 0);
        let (s, host) = (scheduler(dir.path(), &clock), FakeHost::default());
        s.set(entry("job", "*/10 * * * *", "UTC")).unwrap();
        s.tick(&host).await;
        *host.fail_create.lock().unwrap() = Some("no such directory".into());
        clock.advance(600);
        s.tick(&host).await;
        let e = host.take_events();
        assert_eq!(e[0].kind, ScheduleEventKind::Skipped);
        assert!(e[0]
            .reason
            .as_deref()
            .unwrap()
            .contains("no such directory"));
        *host.fail_create.lock().unwrap() = None;
        clock.advance(600);
        s.tick(&host).await;
        assert_eq!(host.kinds(), vec![ScheduleEventKind::Fired]);
    }

    #[tokio::test]
    async fn a_disabled_entry_never_fires_and_remove_forgets_it() {
        let dir = tempfile::tempdir().unwrap();
        let clock = FakeClock::at(2026, 5, 4, 9, 0);
        let (s, host) = (scheduler(dir.path(), &clock), FakeHost::default());
        let mut e = entry("job", "*/10 * * * *", "UTC");
        e.enabled = false;
        s.set(e).unwrap();
        s.tick(&host).await;
        clock.advance(3600);
        s.tick(&host).await;
        assert!(host.kinds().is_empty());
        assert!(s.list()[0].next_at.is_none());
        assert!(s.remove("job"));
        assert!(!s.remove("job"));
        assert!(scheduler(dir.path(), &clock).list().is_empty());
    }
}
