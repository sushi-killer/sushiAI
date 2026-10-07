//! Which harness CLIs this machine (or remote host: each host runs its own
//! orchd) can actually start. Probed at daemon start and on `settings.set`,
//! cached in between; a spawn that finds no binary marks it missing too. A
//! route on a missing harness is swapped for the nearest route of an
//! available one, and with no harness at all a task waits for the owner.

use super::*;

const HARNESSES: [Harness; 2] = [Harness::Claude, Harness::Codex];

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// Whether `bin` (a path, or a name looked up on the augmented PATH) runs.
pub(super) fn binary_found(bin: &str) -> bool {
    if bin.contains('/') {
        return is_executable(Path::new(bin));
    }
    augmented_path()
        .split(':')
        .any(|dir| !dir.is_empty() && is_executable(&Path::new(dir).join(bin)))
}

/// The nearest route on an available harness to `route`: the same strength
/// first, then stronger ones (weakest of them first), then weaker ones
/// (strongest first); settings order breaks ties. `route` itself when its
/// harness is available; `None` when no route qualifies.
pub(super) fn nearest_available(
    settings: &Settings,
    has: impl Fn(Harness) -> bool,
    route: &Route,
) -> Option<Route> {
    if has(route.harness) {
        return Some(route.clone());
    }
    let want = route_strength(route);
    settings
        .routes
        .iter()
        .enumerate()
        .filter(|(_, r)| has(r.harness))
        .min_by_key(|(i, r)| {
            let s = route_strength(r);
            let rank = if s == want {
                (0, 0)
            } else if s > want {
                (1, s)
            } else {
                (2, u32::MAX - s)
            };
            (rank, *i)
        })
        .map(|(_, r)| r.clone())
}

/// The cheapest configured route on an available harness: priced routes
/// first, cheapest price, then weakest, then settings order.
pub(super) fn cheapest_available(
    settings: &Settings,
    has: impl Fn(Harness) -> bool,
) -> Option<Route> {
    settings
        .routes
        .iter()
        .enumerate()
        .filter(|(_, r)| has(r.harness))
        .min_by(|(ia, a), (ib, b)| {
            let ca = route_cost(&settings.prices, a);
            let cb = route_cost(&settings.prices, b);
            ca.is_none()
                .cmp(&cb.is_none())
                .then(ca.unwrap_or(0.0).total_cmp(&cb.unwrap_or(0.0)))
                .then(route_strength(a).cmp(&route_strength(b)))
                .then(ia.cmp(ib))
        })
        .map(|(_, r)| r.clone())
}

fn install_question() -> Question {
    let need = HARNESSES
        .iter()
        .map(|h| resolve_binary(*h))
        .collect::<Vec<_>>()
        .join(" or ");
    Question::new(
        format!(
            "No agent CLI was found on this machine: install {need} (or point ORCHD_CLAUDE_BIN / ORCHD_CODEX_BIN at one), and keep a route for it in Settings, then answer \"retry\"."
        ),
        vec!["retry".to_string()],
        QuestionKind::HarnessMissing,
        AskedBy::Implement,
    )
}

impl App {
    /// Probes both CLIs again; called at start and when settings change.
    pub(super) fn refresh_availability(&self) {
        let mut cache = self.harness_avail.lock().unwrap();
        for h in HARNESSES {
            cache.insert(h, binary_found(&resolve_binary(h)));
        }
    }

    pub(super) fn harness_available(&self, harness: Harness) -> bool {
        self.harness_avail
            .lock()
            .unwrap()
            .get(&harness)
            .copied()
            .unwrap_or(false)
    }

    /// Unit tests run against fake harnesses, not whatever CLIs this machine
    /// happens to have installed.
    #[cfg(test)]
    pub(crate) fn assume_harnesses_installed(&self) {
        let mut cache = self.harness_avail.lock().unwrap();
        for h in HARNESSES {
            cache.insert(h, true);
        }
    }

    /// A spawn found no binary although the probe did: trust the spawn.
    pub(super) fn mark_harness_missing(&self, harness: Harness) {
        self.harness_avail.lock().unwrap().insert(harness, false);
    }

    /// Whether any configured route runs on an installed harness.
    pub(super) fn any_harness_available(&self) -> bool {
        let settings = self.settings.read().unwrap();
        settings
            .routes
            .iter()
            .any(|r| self.harness_available(r.harness))
    }

    /// The orchestrator chat's route, moved to an available harness.
    pub(super) fn chat_route(&self, settings: &Settings) -> Option<Route> {
        let wanted = chat::orchestrator_route(settings)?;
        self.usable_route(settings, wanted).map(|(route, _)| route)
    }

    /// `route`, or the nearest route of an available harness with the
    /// decision line naming the swap; `None` when no route can run at all.
    pub(super) fn usable_route(
        &self,
        settings: &Settings,
        route: Route,
    ) -> Option<(Route, Option<String>)> {
        let found = nearest_available(settings, |h| self.harness_available(h), &route)?;
        let line = (found.id != route.id).then(|| {
            format!(
                "Orchestrator: {} unavailable ({} not found), using {}",
                route.id,
                resolve_binary(route.harness),
                found.id
            )
        });
        Some((found, line))
    }
}

/// Parks the task with an install question while no harness is available.
/// `true` when one is (the task is back in `resume_status` holding a slot);
/// `false` when the task was stopped meanwhile.
pub(super) async fn wait_for_harness(
    app: &Arc<App>,
    task_id: &str,
    task: &mut Task,
    resume_status: TaskStatus,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<String>>>>,
    cancel: &CancelToken,
    permit: &mut Option<tokio::sync::OwnedSemaphorePermit>,
) -> bool {
    if app.any_harness_available() {
        return true;
    }
    while !app.any_harness_available() {
        task.decisions
            .push("Orchestrator: no agent CLI found; waiting for one to be installed".to_string());
        task.question = Some(install_question());
        task.status = TaskStatus::Waiting;
        task.updated_at = now_ms();
        if wait_for_answer(app, task_id, task, pending_answer, cancel, permit)
            .await
            .is_none()
        {
            return false;
        }
        app.refresh_availability();
    }
    task.status = resume_status;
    task.updated_at = now_ms();
    let _ = app.store.save_task(task);
    app.broadcast_task(task);
    if permit.is_none() {
        let acquired = tokio::select! {
            _ = cancel.cancelled() => None,
            p = app.slots.clone().acquire_owned() => p.ok(),
        };
        match acquired {
            Some(p) => *permit = Some(p),
            None => {
                mark_stopped_if_not_already(app, task_id).await;
                return false;
            }
        }
    }
    true
}
